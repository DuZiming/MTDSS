//! CLI 换轨 IPC 后的命令层端到端(进程内 Daemon + 真实 `DaemonClient`)。
//!
//! 语义大盘由 daemon_e2e 与 nat-matrix 接管;本文件只盖 CLI 命令层自身:
//! 未登录守门、login→devices→status 全链、recv→set_recv_dir、守护进程
//! 发现(`MTDSSD_BIN` 覆盖 / 在场复用不拉起)。env 类用例持互斥锁串行
//! ——环境变量是进程级全局,并发用例读写会互相踩。

use mtdss_cli::client::{self, CliError, DaemonClient};
use mtdss_daemon::{Daemon, DaemonOpts};
use serde_json::Value;
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
