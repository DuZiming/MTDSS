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
