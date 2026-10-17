//! 守护进程基础行为测试:hello/status 语义、同 home 单例、关停即断连。

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::IpcClient;
use std::path::Path;
use std::time::Duration;

/// 调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂。
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// 「连不上」断言助手:限时连接,失败/拒绝/不存在均返回 true;
/// 超时返回 false——宁可测试失败暴露悬挂,不容忍 BUSY 自旋混过断言。
async fn connect_fails(home: &Path) -> bool {
    match tokio::time::timeout(CALL_TIMEOUT, IpcClient::connect(home)).await {
        Ok(r) => r.is_err(),
        Err(_) => false,
    }
}

/// 临时 home + 显式 recv_dir 的启动选项
/// (DaemonOpts::default 指向真实 mtdss_home,测试不可用)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
    }
}

#[tokio::test]
async fn hello_before_login() {
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    let r = tokio::time::timeout(CALL_TIMEOUT, client.call("hello", serde_json::json!({})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["logged_in"], false);
    assert_eq!(r["device"], serde_json::Value::Null);
    assert!(r["version"].as_str().is_some(), "version 必须有值");
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn status_reflects_opts() {
    let home = tempfile::tempdir().unwrap();
    let recv_dir = home.path().join("custom-inbox");
    let daemon = Daemon::start(
        home.path(),
        DaemonOpts {
            recv_dir: recv_dir.clone(),
            tray: false,
        },
    )
    .await
    .unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    let r = tokio::time::timeout(CALL_TIMEOUT, client.call("status", serde_json::json!({})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        r["recv_dir"].as_str().unwrap(),
        recv_dir.to_string_lossy().into_owned()
    );
    assert_eq!(r["online"], false);
    assert_eq!(r["logged_in"], false);
    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn second_daemon_same_home_fails() {
    let home = tempfile::tempdir().unwrap();
    let d1 = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    assert!(
        Daemon::start(home.path(), opts(home.path())).await.is_err(),
        "同 home 第二个 Daemon::start 必须失败(socket 已被占用)"
    );
    d1.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_socket() {
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    daemon.shutdown().await.unwrap();
    assert!(
        connect_fails(home.path()).await,
        "shutdown 后必须连不上"
    );
}
