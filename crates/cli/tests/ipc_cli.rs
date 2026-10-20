//! CLI 换轨 IPC 后的命令层端到端(进程内 Daemon + 真实 `DaemonClient`)。
//!
//! 语义大盘由 daemon_e2e 与 nat-matrix 接管;本文件只盖 CLI 命令层自身:
//! 未登录守门、login→devices→status 全链、recv→set_recv_dir、守护进程
//! 发现(`MTDSSD_BIN` 覆盖 / 在场复用不拉起)。env 类用例持互斥锁串行
//! ——环境变量是进程级全局,并发用例读写会互相踩。

use mtdss_cli::client::{self, CliError, DaemonClient};
use mtdss_daemon::{Daemon, DaemonOpts};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::Path;
use std::time::{Duration, Instant};

/// env 类测试互斥(见文件头注释)。tokio 锁:守卫跨 await 持有是测试语义
/// 本身(env 覆盖须贯穿「设值 → connect → 断言 → 还原」全程)。
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 进程内 daemon 的启动选项(独立 home → 独立命名管道,不碰真实 mtdss_home)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
    }
}

/// 未登录且命令不在豁免集 → 守门拒绝:退出码 2 + 文案指到 `mtdss login`
/// (devices 与 send 走同一守门入口,各过一遍;status 豁免放行)。
#[tokio::test(flavor = "multi_thread")]
async fn not_logged_in_commands_refuse_with_hint() {
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();

    for cmd in ["devices", "send"] {
        let err: CliError =
            client::ensure_logged_in(&c, false).expect_err("未登录必须被守门拒绝");
        assert_eq!(err.code, 2, "守门退出码必须是 2({cmd}): {err}");
        assert!(err.message.contains("未登录"), "文案须含「未登录」({cmd}): {err}");
        assert!(
            err.message.contains("mtdss login"),
            "文案须指到 `mtdss login`({cmd}): {err}"
        );
    }
    // 豁免命令(status)不受拦截
    client::ensure_logged_in(&c, true).expect("status 豁免登录守门");

    daemon.shutdown().await.unwrap();
}

/// login → devices → status(→ logout)全链:全部走 main 同款命令层 helper。
#[tokio::test(flavor = "multi_thread")]
async fn login_devices_status_flow() {
    let (addr, _state) = spawn_test_server().await;
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();

    // login 豁免守门;request_code 拿 dev 码 → verify(名字显式传入)
    client::ensure_logged_in(&c, true).unwrap();
    let code = client::login_request_code(&c, &server, "cli@x.y", None).await.unwrap();
    let r = client::login_verify(&c, "cli@x.y", &code, Some("cli-a")).await.unwrap();
    let dev = r["device_id"].as_str().expect("device_id").to_string();
    assert_eq!(r["name"].as_str(), Some("cli-a"));
    assert!(home.path().join("session.json").is_file(), "会话必须落盘");

    // 登录后守门放行(devices/send 面)。守门读连接时的 hello 快照——
    // 每条命令都是新进程新连接,此处换新客户端即等价新进程。
    drop(c);
    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();
    client::ensure_logged_in(&c, false).unwrap();

    // devices:等在线循环建连后,本机在目录且 online=true、名字回填一致
    let deadline = Instant::now() + Duration::from_secs(30);
    let me = loop {
        let devs = client::devices_cmd(&c).await.unwrap();
        let me = devs["devices"]
            .as_array()
            .and_then(|l| {
                l.iter()
                    .find(|d| d["device_id"].as_str() == Some(dev.as_str()))
                    .cloned()
            });
        if let Some(me) = me {
            assert_eq!(me["name"].as_str(), Some("cli-a"), "目录名字须回填: {me}");
            if me["online"].as_bool() == Some(true) {
                break me;
            }
        }
        assert!(Instant::now() < deadline, "devices 未见到本机在线: {devs}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(me["platform"].is_string(), "目录须含 platform 字段: {me}");

    // status:登录态/在线/服务器一致
    let st = client::status_cmd(&c).await.unwrap();
    assert_eq!(st["logged_in"].as_bool(), Some(true));
    assert_eq!(st["online"].as_bool(), Some(true), "status 须反映连接态: {st}");
    assert_eq!(st["server"].as_str(), Some(server.as_str()));
    assert_eq!(st["device"]["device_id"].as_str(), Some(dev.as_str()));

    // logout 幂等收尾:状态归零
    client::login_logout(&c).await.unwrap();
    let st = client::status_cmd(&c).await.unwrap();
    assert_eq!(st["logged_in"].as_bool(), Some(false));

    daemon.shutdown().await.unwrap();
}

/// `mtdss recv --out` → set_recv_dir:status 即时反映 + settings.json 持久化
/// (recv 不在守门豁免集,先登录;重启同口径解析由 daemon_e2e 覆盖)。
#[tokio::test(flavor = "multi_thread")]
async fn set_recv_dir_via_recv_cmd() {
    let (addr, _state) = spawn_test_server().await;
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();

    let code = client::login_request_code(&c, &server, "rd@x.y", None).await.unwrap();
    client::login_verify(&c, "rd@x.y", &code, None).await.unwrap();

    let dir = home.path().join("收件 目录");
    client::set_recv_dir_cmd(&c, &dir).await.unwrap();
    let st = client::status_cmd(&c).await.unwrap();
    assert_eq!(
        st["recv_dir"].as_str(),
        Some(dir.to_str().unwrap()),
        "status 必须即时反映新目录: {st}"
    );
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved["recv_dir"].as_str(),
        Some(dir.to_str().unwrap()),
        "settings.json 必须持久化新目录"
    );

    daemon.shutdown().await.unwrap();
}

/// `MTDSSD_BIN` 覆盖:指向什么返回什么(原样采用);清掉后 None 或 dev
/// 回退均合法,不 panic。
#[tokio::test(flavor = "multi_thread")]
async fn find_daemon_bin_env_override() {
    let _g = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-mtdssd.exe");
    std::fs::write(&fake, b"not a binary").unwrap();

    std::env::set_var("MTDSSD_BIN", &fake);
    assert_eq!(
        mtdss_ipc::find_daemon_bin().as_deref(),
        Some(fake.as_path()),
        "MTDSSD_BIN 必须原样采用"
    );

    std::env::remove_var("MTDSSD_BIN");
    match mtdss_ipc::find_daemon_bin() {
        None => {}
        Some(p) => assert!(p.is_file(), "dev 回退必须指向真实文件: {}", p.display()),
    }
}

/// 在场 daemon 直接复用:connect_or_spawn 不进拉起分支(`MTDSSD_BIN` 指向
/// 不可执行文件——误入 spawn 分支会立即显形)。
#[tokio::test(flavor = "multi_thread")]
async fn connect_or_spawn_uses_running_daemon() {
    let _g = ENV_LOCK.lock().await;
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();

    let saved = std::env::var("MTDSSD_BIN").ok();
    let bogus = home.path().join("not-mtdssd");
    std::fs::write(&bogus, b"").unwrap();
    std::env::set_var("MTDSSD_BIN", &bogus);

    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();
    assert!(c.hello["version"].is_string(), "hello 必须握上: {:?}", c.hello);

    match saved {
        Some(v) => std::env::set_var("MTDSSD_BIN", v),
        None => std::env::remove_var("MTDSSD_BIN"),
    }
    daemon.shutdown().await.unwrap();
}

/// unix:`MTDSSD_BIN` 指向立即退出的脚本 → connect_or_spawn 快速失败且文案
/// 可行动(须指明「daemon 提前退出」并指到手动运行 mtdssd);耗时有界
/// (<10s)——防回退成轮询满 10s/60s 才报错。Windows 等价测试脆弱(临时
/// 脚本语义不同),人工验证由 Task 11 冒烟承担。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn spawn_fails_fast_when_child_exits() {
    let _g = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake-mtdssd");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let home = tempfile::tempdir().unwrap();
    let saved = std::env::var("MTDSSD_BIN").ok();
    std::env::set_var("MTDSSD_BIN", &script);

    let started = Instant::now();
    // 不用 unwrap_err:Err 侧(CliError)有 Debug,但 Ok 侧 DaemonClient 没有
    // (IpcClient 字段无 Debug),unwrap_err 会要求 T: Debug 而 Unix 上编译失败。
    let err: CliError = match DaemonClient::connect_or_spawn(home.path()).await {
        Err(e) => e,
        Ok(_) => panic!("守护进程本应启动失败(指向立即退出的假 MTDSSD_BIN)"),
    };
    let elapsed = started.elapsed();

    match saved {
        Some(v) => std::env::set_var("MTDSSD_BIN", v),
        None => std::env::remove_var("MTDSSD_BIN"),
    }

    assert!(
        elapsed < Duration::from_secs(10),
        "子进程退出须快速失败,不得拖满轮询窗: {elapsed:?}"
    );
    assert_eq!(err.code, 1, "拉起失败是环境故障 code 1: {err}");
    assert!(err.message.contains("提前退出"), "文案须指明提前退出: {err}");
    assert!(
        err.message.contains("mtdssd"),
        "文案须指到手动运行 mtdssd 排查: {err}"
    );
}

// ---------- resolve_device / format_ls(纯函数) ----------

/// 纯函数测试样本:id 恒唯一,name 可重名(alpha/gamma 同名)。
fn sample_devices() -> Vec<Value> {
    vec![
        json!({ "device_id": "id-alpha", "name": "手机", "platform": "test", "online": true }),
        json!({ "device_id": "id-beta", "name": "笔记本", "platform": "test", "online": false }),
        json!({ "device_id": "id-gamma", "name": "手机", "platform": "test", "online": true }),
    ]
}

/// id 精确命中直接返回该 id;且 id 命中优先于 name(某设备名恰为另一台
/// 的 id 时,按 id 解析)。
#[test]
fn resolve_device_id_exact() {
    let devs = sample_devices();
    let got = client::resolve_device(&devs, "id-beta").expect("id 精确命中");
    assert_eq!(got, "id-beta");

    // id 优先:名字空间里也有「id-beta」这样的名字时,id 命中仍胜出
    let tricky = vec![
        json!({ "device_id": "id-x", "name": "id-beta" }),
        json!({ "device_id": "id-beta", "name": "其他" }),
    ];
    let got = client::resolve_device(&tricky, "id-beta").expect("id 优先于名");
    assert_eq!(got, "id-beta", "id 精确命中必须优先于 name 匹配");
}

/// name 精确匹配恰 1 台 → 其 device_id。
#[test]
fn resolve_device_unique_name() {
    let devs = sample_devices();
    let got = client::resolve_device(&devs, "笔记本").expect("唯一名命中");
    assert_eq!(got, "id-beta");
}

/// name 命中多台 → Err 提示歧义并列出全部匹配的 device_id。
#[test]
fn resolve_device_ambiguous_lists_ids() {
    let devs = sample_devices();
    let err: CliError = client::resolve_device(&devs, "手机").expect_err("重名必须报歧义");
    assert_eq!(err.code, 1, "歧义是业务失败 code 1: {err}");
    assert!(err.message.contains("id-alpha"), "歧义须列 id-alpha: {err}");
    assert!(err.message.contains("id-gamma"), "歧义须列 id-gamma: {err}");
}

/// name 与 id 都无命中 → Err 列出全部可用设备(id+name 每行一台)。
#[test]
fn resolve_device_none_lists_available() {
    let devs = sample_devices();
    let err: CliError = client::resolve_device(&devs, "平板").expect_err("无命中必须报错");
    assert_eq!(err.code, 1, "无命中是业务失败 code 1: {err}");
    for (id, name) in [
        ("id-alpha", "手机"),
        ("id-beta", "笔记本"),
        ("id-gamma", "手机"),
    ] {
        assert!(err.message.contains(id), "可用列表须含 {id}: {err}");
        assert!(err.message.contains(name), "可用列表须含 {name}: {err}");
    }
}

/// 行格式 `{d|-}\t{name}\t{size}\t{mtime_ms}`;truncated=true 追加末行提示。
#[test]
fn format_ls_rows_and_truncated() {
    let v = json!({
        "entries": [
            { "name": "子目录", "path": "C:\\t\\子目录", "size": 0,
              "mtime_ms": 1700000000000u64, "is_dir": true },
            { "name": "a.txt", "path": "C:\\t\\a.txt", "size": 5,
              "mtime_ms": 1700000001000u64, "is_dir": false },
        ],
        "truncated": false,
    });
    let out = client::format_ls(&v);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        vec!["d\t子目录\t0\t1700000000000", "-\ta.txt\t5\t1700000001000"],
        "行格式必须稳定可 grep: {out:?}"
    );

    // truncated=true:末行追加截断提示
    let vt = json!({ "entries": [], "truncated": true });
    let out = client::format_ls(&vt);
    assert!(
        out.ends_with("…(结果已截断,仅显示前 10000 项)\n"),
        "截断提示必须是末行: {out:?}"
    );
}

// ---------- ls 全链(devices 快照 → 定位 → list → 行格式) ----------

/// ls 全链:登录 → devices 快照回填名字 → resolve_device(按名,唯一)→
/// ls_cmd 列本机 tempdir(短路,不经网络)→ format_ls 含预期行。
#[tokio::test(flavor = "multi_thread")]
async fn ls_end_to_end() {
    let (addr, _state) = spawn_test_server().await;
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();

    client::ensure_logged_in(&c, true).unwrap();
    let code = client::login_request_code(&c, &server, "ls@x.y", None).await.unwrap();
    client::login_verify(&c, "ls@x.y", &code, Some("ls-cli")).await.unwrap();

    // 等快照回填名字(名字权威在服务器目录,在线循环连接建立时全量刷新)
    let deadline = Instant::now() + Duration::from_secs(30);
    let devs = loop {
        let devs = client::devices_cmd(&c).await.unwrap();
        let named = devs["devices"]
            .as_array()
            .map(|l| l.iter().any(|d| d["name"].as_str() == Some("ls-cli")))
            .unwrap_or(false);
        if named {
            break devs;
        }
        assert!(Instant::now() < deadline, "devices 快照未回填名字: {devs}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // 按名定位(唯一)→ id;快照数组直传,签名与 main 同口径
    let arr = devs["devices"].as_array().cloned().unwrap_or_default();
    let dev_id = client::resolve_device(&arr, "ls-cli").expect("按名定位本机");
    assert!(!dev_id.is_empty(), "定位结果必须是本机 id");

    // 列本机 tempdir(短路路径,不依赖对端在线)
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("b.txt"), b"bb").unwrap();
    std::fs::create_dir_all(src.path().join("d1")).unwrap();
    let listing = client::ls_cmd(&c, &dev_id, &src.path().to_string_lossy())
        .await
        .expect("ls 本机 tempdir 应成功");
    let out = client::format_ls(&listing);
    assert!(out.contains("d\td1\t0\t"), "目录行: {out}");
    assert!(out.contains("-\tb.txt\t2\t"), "文件行: {out}");
    assert!(!out.contains("已截断"), "小目录不得截断: {out}");

    daemon.shutdown().await.unwrap();
}

// ---------- parse_scp_target / conflict_prompt_data(纯函数,Task 7) ----------

/// scp 形目标解析:`dev_b:D:\x` → ("dev_b", `D:\x`)(路径段保留盘符冒号,
/// 首冒号切分)。
#[test]
fn parse_scp_target_splits_first_colon() {
    let (dev, path) = client::parse_scp_target(r"dev_b:D:\x").expect("合法目标");
    assert_eq!(dev, "dev_b");
    assert_eq!(path, r"D:\x");

    // 正斜杠路径与无盘符形态同样按首冒号切分
    let (dev, path) = client::parse_scp_target("alpha:/srv/data").expect("unix 形目标");
    assert_eq!(dev, "alpha");
    assert_eq!(path, "/srv/data");
}

/// 裸路径(无冒号/Windows 裸盘符路径)与多冒号(第二冒号不在盘符位)报错。
#[test]
fn parse_scp_target_rejects_bare_and_multi_colon() {
    // 无冒号 = 裸路径(注意:输入必须真的无冒号——`D:\foo` 形在 Windows
    // 靠单字母盘符守卫碰巧被拒、在 Unix 是合法 设备:路径,不得当无冒号用例)
    let err: CliError = client::parse_scp_target(r"D\foo\bar").expect_err("无冒号必须报错");
    assert!(err.message.contains("设备:路径"), "文案须说明目标形态: {err}");

    // 多冒号且第二冒号不在盘符位 → 报错
    let err: CliError = client::parse_scp_target("dev:D:\\a:b").expect_err("多冒号必须报错");
    assert!(err.message.contains("设备:路径"), "文案须说明目标形态: {err}");

    // 空设备段 / 空路径段 → 报错
    assert!(client::parse_scp_target(r":D:\x").is_err(), "空设备段必须报错");
    assert!(client::parse_scp_target("dev:").is_err(), "空路径段必须报错");

    // Windows 裸盘符路径(`C:\foo`)形似 设备:路径:单冒号 + 单字母设备段
    // 按裸路径拒(设备名恰为单字母的场景改用完整 id)
    #[cfg(windows)]
    {
        let err: CliError = client::parse_scp_target(r"C:\foo").expect_err("裸盘符路径必须报错");
        assert!(err.message.contains("设备:路径"), "文案须说明目标形态: {err}");
    }

    // 盘符位冒号合法:`dev:D:\x` 两枚冒号,第二枚在路径段第 2 字符(盘符)
    let (dev, path) = client::parse_scp_target(r"dev:D:\x").expect("盘符位冒号合法");
    assert_eq!(dev, "dev");
    assert_eq!(path, r"D:\x");
}

// ---------- parse_cp_target(纯函数,Task 8 cp 四态分发) ----------

/// cp 源/目标解析:远端 `dev:path` → Some(dev);本机裸路径(无冒号)与
/// Windows 盘符形态(`C:\x`)→ None;非法 scp 形(空段/多冒号)原样报错
/// 不静默按本机解释。
#[test]
fn parse_cp_target_routes_remote_local_and_rejects() {
    // 远端形态:设备段非空 → Some(dev)
    let (dev, path) = client::parse_cp_target("dev_a:/srv/dst").expect("合法远端目标");
    assert_eq!(dev.as_deref(), Some("dev_a"));
    assert_eq!(path, "/srv/dst");

    // 本机裸路径(无冒号)→ None
    let (dev, path) = client::parse_cp_target("./local/dir").expect("本机相对路径");
    assert_eq!(dev, None);
    assert_eq!(path, "./local/dir");

    // 非法 scp 形不吞:空设备段 / 多冒号(第二枚不在盘符位)原样报错
    assert!(client::parse_cp_target(":D:\\x").is_err(), "空设备段必须报错");
    assert!(client::parse_cp_target("dev:D:\\a:b").is_err(), "多冒号必须报错");
    assert!(client::parse_cp_target("dev:").is_err(), "空路径段必须报错");

    // Windows 盘符形态(`C:\x`)按本机路径(与 parse_scp_target 的裸盘符
    // 守卫同口径);Unix 无此歧义,单字母设备段照常按远端解析
    #[cfg(windows)]
    {
        let (dev, path) = client::parse_cp_target(r"C:\Users\x").expect("盘符形态=本机");
        assert_eq!(dev, None, "盘符形态必须按本机路径");
        assert_eq!(path, r"C:\Users\x");
    }
}

/// 冲突问询纯数据:文案列出冲突条目与三个选项;应答解析 s/o/c(大小写
/// 不敏感),非法输入 None(交互段重问)。
#[test]
fn conflict_prompt_data_lists_items_and_parses_answers() {
    let conflicts = vec!["报告.docx".to_string(), "照片".to_string()];
    let prompt = client::conflict_prompt_data(&conflicts);
    assert!(prompt.text.contains("报告.docx"), "文案须列冲突条目: {}", prompt.text);
    assert!(prompt.text.contains("照片"), "文案须列冲突条目: {}", prompt.text);
    for opt in ["s", "o", "c"] {
        assert!(prompt.text.contains(opt), "文案须含选项 {opt}: {}", prompt.text);
    }

    use client::ConflictAction;
    assert_eq!(client::parse_conflict_answer("s"), Some(ConflictAction::SkipAll));
    assert_eq!(client::parse_conflict_answer("O"), Some(ConflictAction::OverwriteAll));
    assert_eq!(client::parse_conflict_answer(" c \n"), Some(ConflictAction::Cancel));
    assert_eq!(client::parse_conflict_answer("x"), None, "非法应答必须 None(重问)");
    assert_eq!(client::parse_conflict_answer(""), None, "空应答必须 None(重问)");
}

// ---------- need_force_confirm / parse_same_device_targets / move_pairs(纯函数,Task 9 rm/mv) ----------

/// 永久删除升级判定:daemon 报文含「回收站不可用」标记串 → 需操作端确认
/// 后 force 重发;其余错误(缺失/离线/普通业务失败)一律 false 不触发。
#[test]
fn need_force_confirm_matches_marker() {
    assert!(
        client::need_force_confirm("回收站不可用: 无法初始化回收设施"),
        "标记串开头必须命中"
    );
    assert!(
        client::need_force_confirm("delete 失败: 回收站不可用: 原因详情"),
        "标记串在报文中段同样命中(子串判定,daemon 无结构化错误码面的已知债)"
    );
    assert!(
        !client::need_force_confirm("路径不存在: D:\\x"),
        "NOT_FOUND 等普通错误不得触发确认梯"
    );
    assert!(!client::need_force_confirm(""), "空报文不触发");
}

/// rm/mv 参数解析纯函数:多目标同设备归并(混设备/裸路径/空表报错);
/// move_pairs 目录形 `--to` 落位名 = 源末段名(分隔符随目录形态、尾分隔
/// 符不重复、平台无关——目标设备平台未知),卷根等无名源拒绝。
#[test]
fn rm_mv_arg_shapes() {
    // 同设备多目标 → (设备, 路径表)
    let (dev, paths) = client::parse_same_device_targets(&[
        "dev_a:/srv/a".to_string(),
        "dev_a:/srv/b".to_string(),
    ])
    .expect("同设备多目标合法");
    assert_eq!(dev, "dev_a");
    assert_eq!(paths, vec!["/srv/a", "/srv/b"]);

    // 混设备 → code 1 报错并点名冲突双方
    let err: CliError = client::parse_same_device_targets(&[
        "dev_a:/srv/a".to_string(),
        "dev_b:/srv/b".to_string(),
    ])
    .expect_err("混设备必须报错");
    assert_eq!(err.code, 1, "混设备是业务失败 code 1: {err}");
    assert!(err.message.contains("dev_a"), "须点名冲突方: {err}");
    assert!(err.message.contains("dev_b"), "须点名冲突方: {err}");

    // 裸路径(无设备段,scp 形非法)原样报错
    assert!(
        client::parse_same_device_targets(&["/srv/bare".to_string()]).is_err(),
        "无设备段必须报错"
    );
    // 空表防御拒绝(clap required 之外的兜底)
    assert!(client::parse_same_device_targets(&[]).is_err(), "空目标必须报错");

    // move_pairs:unix 形目录,from 原样、to = dir/源末段名
    let pairs = client::move_pairs(
        &["/srv/data/a.txt".to_string(), "/srv/树".to_string()],
        "/dst",
    )
    .expect("合法源表");
    assert_eq!(
        pairs,
        vec![
            ("/srv/data/a.txt".to_string(), "/dst/a.txt".to_string()),
            ("/srv/树".to_string(), "/dst/树".to_string()),
        ],
        "落位名必须是源末段名: {pairs:?}"
    );

    // Windows 形目录:分隔符随目录形态(`\`),目录自带尾分隔符不重复
    let pairs = client::move_pairs(&[r"D:\data\a.txt".to_string()], r"D:\dst\")
        .expect("Windows 形目录");
    assert_eq!(pairs[0].1, r"D:\dst\a.txt", "Windows 分隔符须随目录: {pairs:?}");

    // 卷根等无名源拒绝(批前整体短路,不发起任何 RPC)
    let err: CliError =
        client::move_pairs(&["D:".to_string()], "/dst").expect_err("卷根源必须拒绝");
    assert_eq!(err.code, 1, "卷根是无名源,业务失败 code 1: {err}");
}

// ---------- cp ③ 同机糖冲突策略(M4a-3) ----------

/// cp 同机糖 ask 流程 + copy RPC 带 skip/overwrite 的落盘效果:预置应答
/// 序列经注入面直驱(管道面;终端问询循环留 main,库层纯决策——与
/// conflict_prompt_data/parse_conflict_answer 同款纯数据测试形态)。
/// s → 全表 skip(命中项目标原样、响应回带 skipped);o → 全表 overwrite
/// (原位翻新无 ` (1)`);c → code 0 干净取消;非法应答重问后生效。
#[tokio::test(flavor = "multi_thread")]
async fn cp_same_device_conflict_ask_flow_applies_policy() {
    let (addr, _state) = spawn_test_server().await;
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let c = DaemonClient::connect_or_spawn(home.path()).await.unwrap();

    client::ensure_logged_in(&c, true).unwrap();
    let code = client::login_request_code(&c, &server, "cp3@x.y", None).await.unwrap();
    client::login_verify(&c, "cp3@x.y", &code, Some("cp3-cli")).await.unwrap();
    // 同机糖走本机短路:device = 本机 id(status 直取,不依赖名字回填)
    let st = client::status_cmd(&c).await.unwrap();
    let dev_id = st["device"]["device_id"].as_str().expect("device_id").to_string();

    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let pa = src.path().join("a.txt");
    let pb = src.path().join("b.txt");
    std::fs::write(&pa, b"new-a").unwrap();
    std::fs::write(&pb, b"new-b").unwrap();
    // 仅 a.txt 撞名;b.txt 无冲突(s 应答下走 legacy 原名落位)
    std::fs::write(dst.path().join("a.txt"), b"old-a").unwrap();
    let paths = [
        pa.to_string_lossy().into_owned(),
        pb.to_string_lossy().into_owned(),
    ];
    let dir = dst.path().to_string_lossy().into_owned();

    // 冲突预扫(同机糖共用 scan_cross_conflicts,顶层粒度):仅 a.txt
    let conflicts = client::scan_cross_conflicts(&c, &dev_id, &paths, &dev_id, &dir)
        .await
        .unwrap();
    assert_eq!(conflicts, vec!["a.txt"], "预扫须仅报撞名条目: {conflicts:?}");

    // ask 流程,预置应答「x(非法,重问)→ s」:全表 skip
    let mut queue: std::collections::VecDeque<&str> = ["x", "s"].into_iter().collect();
    let (skip, overwrite) =
        client::resolve_conflicts_ask(&conflicts, || Ok(queue.pop_front().map(str::to_string)))
            .unwrap();
    assert!(overwrite.is_empty(), "s 应答必须清空 overwrite 表: {skip:?}");
    assert_eq!(skip, conflicts, "s 应答必须全表进 skip: {skip:?}");

    // 名 → 引擎判定键(目标路径,拼名与操作端同规则)→ copy RPC 带 skip
    let skip: Vec<String> = skip.iter().map(|n| client::join_remote(&dir, n)).collect();
    let r = client::copy_remote_cmd(&c, &dev_id, &paths, &dir, &skip, &overwrite)
        .await
        .unwrap();
    assert_eq!(r["copied"].as_u64(), Some(1), "仅未命中项落盘: {r}");
    assert_eq!(
        r["skipped"].as_array().map(|s| s.len()),
        Some(1),
        "skipped 响应须回带跳过项: {r}"
    );
    assert_eq!(
        std::fs::read(dst.path().join("a.txt")).unwrap(),
        b"old-a",
        "skip 命中目标原样"
    );
    assert_eq!(
        std::fs::read(dst.path().join("b.txt")).unwrap(),
        b"new-b",
        "未命中项沿 legacy 落位"
    );
    assert!(!dst.path().join("b (1).txt").exists(), "无冲突不触发改名");

    // ask 流程,预置应答「o」:全表 overwrite → 原位翻新、无改名
    let mut queue: std::collections::VecDeque<&str> = ["o"].into_iter().collect();
    let (skip2, overwrite2) =
        client::resolve_conflicts_ask(&conflicts, || Ok(queue.pop_front().map(str::to_string)))
            .unwrap();
    assert!(skip2.is_empty(), "o 应答必须清空 skip 表: {overwrite2:?}");
    let ow: Vec<String> = overwrite2.iter().map(|n| client::join_remote(&dir, n)).collect();
    let r = client::copy_remote_cmd(&c, &dev_id, &paths, &dir, &skip2, &ow)
        .await
        .unwrap();
    assert_eq!(r["copied"].as_u64(), Some(2), "overwrite 全落盘: {r}");
    assert!(
        r["skipped"].as_array().unwrap().is_empty(),
        "overwrite 无跳过: {r}"
    );
    assert_eq!(
        std::fs::read(dst.path().join("a.txt")).unwrap(),
        b"new-a",
        "overwrite 内容翻新"
    );
    assert_eq!(std::fs::read(dst.path().join("b.txt")).unwrap(), b"new-b");
    assert!(
        !dst.path().join("a (1).txt").exists(),
        "覆盖落位不得触发冲突改名"
    );

    // ask 流程,c 应答 → code 0 干净取消(空表,不发起传输)
    let err: CliError = client::resolve_conflicts_ask(&conflicts, || Ok(Some("c".into())))
        .expect_err("c 应答必须取消");
    assert_eq!(err.code, 0, "取消是 code 0 干净退出: {err}");

    // 无冲突直通:空表双返,不应答源不被消费
    let mut untouched = true;
    let (skip3, ow3) = client::resolve_conflicts_ask(&[], || {
        untouched = false;
        Ok(None)
    })
    .unwrap();
    assert!(skip3.is_empty() && ow3.is_empty() && untouched, "无冲突必须直通空表");

    daemon.shutdown().await.unwrap();
}
