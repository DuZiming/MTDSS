//! 登录链路端到端:真实信令服务器(dev 模式验证码回显)+ 守护进程 + IPC 客户端。
//!
//! 覆盖五条主路径:请求验证码、错误验证码、成功登录落盘、登出清理、同邮箱
//! 重登。每次测试独立 tempdir home(命名管道按 home 派生,互不串扰)与
//! 独立随机端口服务器。

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::Path;
use std::time::Duration;

/// 调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂。
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// 临时 home + 显式 recv_dir 的启动选项(同 daemon_basic,不碰真实 mtdss_home)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
    }
}

/// 限时 IPC 调用;超时视为测试失败(暴露悬挂),RpcError 原样上抛供断言。
async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    match tokio::time::timeout(CALL_TIMEOUT, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("call {method} 超时({CALL_TIMEOUT:?})"),
    }
}

/// 完整登录走一遍:login.request_code(dev 回显取码)→ login.verify。
/// 返回 verify 的 result(`{"device_id","name"}`)。
async fn login(client: &IpcClient, server: &str, email: &str, name: &str) -> Result<Value, RpcError> {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("result 必含 code 字符串");
    call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
}

#[tokio::test]
async fn request_code_returns_six_digits() {
    let (addr, state) = spawn_test_server().await;
    // 认证收口后仅预置账号可领码:登录前先预置(幂等,下同)
    signaling::admin::user_add(&state.db.lock().unwrap(), "a@x.y").unwrap();
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    let r = call(
        &client,
        "login.request_code",
        json!({ "server": format!("http://{addr}"), "email": "a@x.y" }),
    )
    .await
    .unwrap();
    let code = r["code"].as_str().expect("result 必含 code 字符串");
    assert_eq!(code.len(), 6, "dev 验证码应为六位: {code}");
    assert!(
        code.chars().all(|c| c.is_ascii_digit()),
        "dev 验证码应全为数字: {code}"
    );

    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn verify_wrong_code_is_rpc_error() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "bad@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    // 先拿真码,再造一个必然不同的错码(000000 巧合命中真码时换 000001)
    let r = call(
        &client,
        "login.request_code",
        json!({ "server": server, "email": "bad@x.y" }),
    )
    .await
    .unwrap();
    let real = r["code"].as_str().unwrap();
    let wrong = if real == "000000" { "000001" } else { "000000" };

    let err = call(
        &client,
        "login.verify",
        json!({ "email": "bad@x.y", "code": wrong }),
    )
    .await
    .expect_err("错误验证码必须返回 RPC error");
    assert_eq!(err.code, 1, "业务失败(验证码错)用 code=1: {err:?}");
    assert!(!err.message.is_empty());

    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn verify_ok_saves_session_and_hello_shows_device() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "ok@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    let r = login(&client, &server, "ok@x.y", "测试设备").await.unwrap();
    let device_id = r["device_id"].as_str().expect("result 必含 device_id");
    assert_eq!(r["name"].as_str(), Some("测试设备"));

    // hello 反映登录态:logged_in=true、device_id 回显
    let hello = call(&client, "hello", json!({})).await.unwrap();
    assert_eq!(hello["logged_in"], true);
    assert_eq!(hello["device"]["device_id"].as_str(), Some(device_id));

    // status 同步反映:logged_in 与 server
    let status = call(&client, "status", json!({})).await.unwrap();
    assert_eq!(status["logged_in"], true);
    assert_eq!(status["server"].as_str(), Some(server.as_str()));

    // session.json 已落盘且内容可读回(同一 device_id)
    assert!(home.path().join("session.json").exists(), "session.json 必须存在");
    let sess = mtdss_daemon::session::Session::load(home.path()).unwrap();
    assert_eq!(sess.device_id, device_id);
    assert_eq!(sess.server, server);

    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn logout_clears_session() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "bye@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    login(&client, &server, "bye@x.y", "d").await.unwrap();
    assert!(home.path().join("session.json").exists());

    let r = call(&client, "login.logout", json!({})).await.unwrap();
    assert_eq!(r, json!({}), "logout 返回空对象");

    // 文件已删 + 登录态翻转回未登录
    assert!(!home.path().join("session.json").exists(), "session.json 必须已删除");
    let hello = call(&client, "hello", json!({})).await.unwrap();
    assert_eq!(hello["logged_in"], false);
    assert_eq!(hello["device"], Value::Null);

    daemon.shutdown().await.unwrap();
}

#[tokio::test]
async fn relogin_same_email_ok() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "again@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    let first = login(&client, &server, "again@x.y", "again").await.unwrap();
    call(&client, "login.logout", json!({})).await.unwrap();

    // 同邮箱重登:重新 request_code + verify;身份种子已存在,同一身份
    // → 服务器按公钥派生,device_id 必然一致
    let second = login(&client, &server, "again@x.y", "again").await.unwrap();
    assert_eq!(
        first["device_id"].as_str(),
        second["device_id"].as_str(),
        "同身份重登 device_id 必须一致"
    );

    let hello = call(&client, "hello", json!({})).await.unwrap();
    assert_eq!(hello["logged_in"], true);
    assert_eq!(
        hello["device"]["device_id"].as_str(),
        second["device_id"].as_str()
    );

    daemon.shutdown().await.unwrap();
}

// multi_thread:验证并发竞速必须让两个连接的分派任务真并行(current_thread
// 下读半工任务轮流推进,同步段不可能重叠,赛不出身份生成竞速;与
// cli/flow.rs tests 同款约束)。
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_verify_yields_single_identity() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "race@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    // 两个独立连接:服务端按连接各起读半工,单连接的请求会被串行处理,
    // 赛不出并发 verify。
    let ca = IpcClient::connect(home.path()).await.unwrap();
    let cb = IpcClient::connect(home.path()).await.unwrap();

    let r = call(
        &ca,
        "login.request_code",
        json!({ "server": server, "email": "race@x.y" }),
    )
    .await
    .unwrap();
    let code = r["code"].as_str().expect("result 必含 code 字符串").to_string();

    // 同邮箱同码并发 verify:pending 窥视同步先行(两侧都拿到快照),登录
    // 全程互斥串行;服务器码单次有效 → 必须恰好一个成功。
    let params = json!({ "email": "race@x.y", "code": code, "name": "race" });
    let (ra, rb) = tokio::join!(
        call(&ca, "login.verify", params.clone()),
        call(&cb, "login.verify", params.clone()),
    );
    let oks = [&ra, &rb].into_iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        oks, 1,
        "码单次有效:并发 verify 必须恰好一个成功 a={ra:?} b={rb:?}"
    );
    let winner = if ra.is_ok() { ra.as_ref().unwrap() } else { rb.as_ref().unwrap() };
    let device_id = winner["device_id"].as_str().expect("成功侧必含 device_id");
    for r in [&ra, &rb] {
        if let Err(e) = r {
            assert_eq!(
                e.code, 1,
                "失败侧必须是业务错(吃不到已用码,带 401 上下文): {e:?}"
            );
        }
    }

    // 磁盘身份与会话永不分歧:identity.seed 的 device_id == session.json 的
    let id = mtdss_core::identity::DeviceIdentity::from_seed_file(
        &home.path().join("identity.seed"),
    )
    .unwrap();
    let sess = mtdss_daemon::session::Session::load(home.path()).unwrap();
    assert_eq!(id.device_id, sess.device_id, "seed 身份与会话设备必须一致");
    assert_eq!(sess.device_id, device_id);

    // hello 一致:logged_in 且 device_id 回显同一身份
    let hello = call(&ca, "hello", json!({})).await.unwrap();
    assert_eq!(hello["logged_in"], true);
    assert_eq!(hello["device"]["device_id"].as_str(), Some(device_id));

    daemon.shutdown().await.unwrap();
}
