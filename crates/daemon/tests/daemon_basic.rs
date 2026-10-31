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
        authorized_roots: None,
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
            authorized_roots: None,
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
async fn concurrent_start_exactly_one_wins() {
    let home = tempfile::tempdir().unwrap();
    let (a, b) = tokio::join!(
        Daemon::start(home.path(), opts(home.path())),
        Daemon::start(home.path(), opts(home.path())),
    );
    let wins = [a.is_ok(), b.is_ok()].into_iter().filter(|w| *w).count();
    assert_eq!(
        wins, 1,
        "并发启动必须恰好一个成功(启动锁排他): a_ok={} b_ok={}",
        a.is_ok(),
        b.is_ok()
    );
    // 只对胜者收尾;败者已是 Err,无资源需回收
    if let Ok(d) = a {
        d.shutdown().await.unwrap();
    }
    if let Ok(d) = b {
        d.shutdown().await.unwrap();
    }
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

/// 优雅退出必须先广播 `event.quit` 再收摊(fix-wave Defect D):托盘「退出」
/// 等用户主动收尾走 shutdown 入口,已连客户端(同理 UI)必须在端点死亡前
/// 从事件流收到该帧——帧经收摊前的活连接送达即钉死「先广播后关摊」的顺序;
/// 若广播缺失或晚于收摊,客户端事件流只等来连接关闭,本测试以超时/通道关闭
/// 失败。崩溃/被杀不经过 shutdown,不发此帧(UI 保持 M3b 离线态)——该边界
/// 由「只在本入口发」的结构保证,此处断言在场帧的形态与顺序。
#[tokio::test]
async fn shutdown_broadcasts_quit_frame_before_teardown() {
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    let mut evs = client.events();
    // 管道预热(确定性辅助):先走一次完整 RPC + 事件往返(幂等登出恒广播
    // event.login),把客户端读半工的 BufReader 唤到稳态——否则 Windows 命名
    // 管道 flush 会等到对端消费才返回,首帧时序在高负载机上抖动,把「送达
    // 断言」变成对调度时序的猜测。预热后事件管道已验通,随后的 event.quit
    // 断言才是对广播本身的判定。
    let _ = tokio::time::timeout(
        CALL_TIMEOUT,
        client.call("login.logout", serde_json::json!({})),
    )
    .await
    .unwrap();
    let primed = tokio::time::timeout(Duration::from_secs(2), evs.recv()).await;
    assert!(
        matches!(&primed, Ok(Ok(f)) if f.method == "event.login"),
        "预热往返未收到 event.login: {primed:?}"
    );
    daemon.shutdown().await.unwrap();
    // 收摊已启动:客户端事件流里必须能等到 event.quit(先广播后收摊)。
    // 客户端本地事件广播的发送端随 IpcClient 常驻,通道永不 Closed;帧没
    // 过网(缺失/晚于收摊)即 recv 悬等 → 超时失败暴露。
    let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
    let quit = loop {
        match tokio::time::timeout_at(deadline, evs.recv()).await {
            Ok(Ok(f)) if f.method == "event.quit" => break f,
            Ok(Ok(_)) => continue, // 其他帧:跳过继续等(防御性)
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                panic!("事件通道滞后 {n} 帧:单客户端单事件不该滞后");
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                panic!("事件通道已关闭:未收到 event.quit 即收摊");
            }
            Err(_) => panic!("等待 event.quit 超时({CALL_TIMEOUT:?}):优雅退出帧缺失或晚于收摊"),
        }
    };
    assert_eq!(
        quit.params,
        serde_json::json!({}),
        "event.quit 目前无载荷面,空对象即契约"
    );
    // 收摊后:端点必须不可再连(既有语义回归锚点)。先释放客户端(真实
    // 场景里 UI 收到 event.quit 即退出、连接随之消失):客户端持有的管道
    // 实例会让管道名保持占用(interprocess 对 ERROR_PIPE_BUSY 自旋重试),
    // 不先放手,「连不上」断言量到的是自己连接的残影而非守护进程的监听面。
    drop(client);
    assert!(connect_fails(home.path()).await, "shutdown 后必须连不上");
}
