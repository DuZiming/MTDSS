//! V3-1 登录面端到端:真实信令服务器(dev 门回显)+ 守护进程 + IPC 客户端。
//!
//! 覆盖登录状态机全链(spec §3.1/§3.2/§3.7):
//! - 新终端审批全链两 settle 路——受信终端 `approval.respond` 同意(A 端轮询
//!   `login.status` 领票)与新终端 `login.submit_code` 直接输码;
//! - 配额超额全链 limit→evict→ok(verify 拿单次 enroll_token 与设备清单,
//!   踢选腾位后 enroll 收口登录);
//! - 扫码全链(qr_begin 铸 token → 受信端 IPC 透传 qr.claim/qr.approve
//!   [Ruling R8]→ PC 轮询 qr_status pending→ok 领票;拒绝路与死码/重放面);
//! - 被踢端:临别帧/code=9 清会话(identity.seed 保留)+ admin unbind 后
//!   重连拒的「连续两次鉴权拒」收口;
//! - AuthNotice 路由:join_request→event.approval、devices_changed→新事件
//!   帧 `event.devices_changed {}` + 快照刷新(admin-unbind 懒踢隔离锚:
//!   已知设备删行,Presence 回退覆盖不到)、data_changed→`event.data_changed`
//!   {key} 事件面(V3-3 点亮);event.device_evicted 逐设备到场;
//! - login.cancel 作废本地续接态;kv.get/put 透传(404 typed not_found)。

use mtdss_core::identity::DeviceIdentity;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::{spawn_test_server, spawn_test_server_with};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 常规调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// 代理豁免(一次性):e2e 全程打回环,而 reqwest 按 http_proxy 环境变量
/// 把请求改道本地代理进程——代理不可达/规则不直连回环时 502 误伤测试。
/// reqwest 在 client 构建时读 NO_PROXY;daemon 的 REST 客户端按请求现建,
/// 本测试进程启动即免疫,in-process daemon 与测试台 reqwest 一并豁免
/// (edition 2021,set_var 安全;二进制内所有用例同值幂等)。
fn bypass_proxy_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
    });
}

/// 临时 home + 显式 recv_dir 的启动选项(同 daemon_basic,不碰真实 mtdss_home)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        // 桌面形态:无收件箱发布面(v2-P5 T5b,哨兵回退默认收件目录)
        inbox_publish: false,
        photo_serve: false,
    }
}

/// 限时 IPC 调用;超时视为测试失败(暴露悬挂),RpcError 原样上抛供断言。
async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    match tokio::time::timeout(CALL_TIMEOUT, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("call {method} 超时({CALL_TIMEOUT:?})"),
    }
}

/// IPC 完整登录:login.request_code(dev 回显取码)→ login.verify →
/// status:"ok"。返回 ok 投影(`{"status":"ok","device_id","name"}`)。
async fn ipc_login(
    client: &IpcClient,
    server: &str,
    email: &str,
    name: &str,
) -> Value {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("result 必含 code 字符串");
    let r = call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
    .expect("login.verify 应成功(预置老终端直入)");
    assert_eq!(r["status"], "ok", "预置老终端 verify 必须直入: {r}");
    r
}

/// V3-1 审批状态机:同账号已有在线受信终端时,新设备 verify 转审批面。
/// 测试台在任何上线发生前先 REST 预置设备行(预生成身份落 seed,daemon IPC
/// 登录幂等收敛同一 device_id),登录恒走老终端直入。
async fn preregister_identity(
    addr: std::net::SocketAddr,
    home: &Path,
    email: &str,
    name: &str,
) -> String {
    let id = DeviceIdentity::generate();
    id.save_seed(&home.join("identity.seed")).unwrap();
    register_rest(addr, email, &hex::encode(id.public_key().as_bytes()), name).await
}

/// dev-mode 验证码直接回带,REST 直注注册(预置受信终端行)。
async fn register_rest(
    addr: std::net::SocketAddr,
    email: &str,
    pubkey_hex: &str,
    name: &str,
) -> String {
    let client = reqwest::Client::new();
    let c: Value = client
        .post(format!("http://{addr}/auth/request-code"))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: Value = client
        .post(format!("http://{addr}/auth/verify"))
        .json(&json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": pubkey_hex,
            "name": name,
            "platform": "test"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["device_id"].as_str().unwrap().to_string()
}

/// 轮询 status.online 直到等于 want(在线循环异步起连,不能同步断言)。
async fn wait_online(client: &IpcClient, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = call(client, "status", json!({})).await.unwrap();
        if s["online"].as_bool() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "status.online 未在 {secs}s 内到 {want}: {s}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 轮询事件流直至谓词命中;超时 panic(点名在等什么,便于排障)。
async fn wait_event(
    evs: &mut tokio::sync::broadcast::Receiver<EventFrame>,
    secs: u64,
    what: &str,
    pred: impl Fn(&EventFrame) -> bool,
) -> EventFrame {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, evs.recv()).await {
            Ok(Ok(f)) => {
                if pred(&f) {
                    return f;
                }
                // 非目标帧:继续等
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                panic!("事件流已关闭,等不到 {what}")
            }
            Err(_) => panic!("等 {what} 超时({secs}s)"),
        }
    }
}

/// 新终端 verify 撞审批面:返回 (session_id, expires_at)。
async fn verify_expect_approval(client: &IpcClient, server: &str, email: &str, name: &str) -> (String, i64) {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("code 回显").to_string();
    let r = call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
    .expect("verify 请求应成功(审批面不是 RPC 错)");
    assert_eq!(r["status"], "approval", "新终端(受信端在线)必须走审批: {r}");
    assert!(r["expires_at"].as_i64().unwrap_or(0) > 0, "expires_at 必须在场: {r}");
    (r["session_id"].as_str().unwrap().to_string(), r["expires_at"].as_i64().unwrap())
}

/// 受信终端(A)上线 + 事件订阅,供审批链复用。返回
/// (home, daemon, client, device_id, session.json 路径)。
async fn trusted_online(
    addr: std::net::SocketAddr,
    email: &str,
    name: &str,
) -> (tempfile::TempDir, Daemon, IpcClient, String, PathBuf) {
    bypass_proxy_once();
    let home = tempfile::tempdir().unwrap();
    let dev = preregister_identity(addr, home.path(), email, name).await;
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    let server = format!("http://{addr}");
    ipc_login(&client, &server, email, name).await;
    wait_online(&client, true, 30).await;
    let sess_path = home.path().join("session.json");
    (home, daemon, client, dev, sess_path)
}

// ---------- 审批全链(B 端 respond 同意 / A 端输码两路) ----------

#[tokio::test(flavor = "multi_thread")]
async fn approval_full_chain_via_respond() {
    let (addr, _state) = spawn_test_server().await;
    signaling::admin::user_add(&_state.db.lock().unwrap(), "fam@x.y").unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, _dev_a, sess_a) = trusted_online(addr, "fam@x.y", "终端A").await;
    let mut evs_a = ca.events();

    // 新终端 B:verify → approval(受信端 A 在线)
    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let (session_id, _) = verify_expect_approval(&cb, &server, "fam@x.y", "终端B").await;

    // A 端收 event.approval(join_request 路由):候选面与审批码齐全
    let ev = wait_event(&mut evs_a, 15, "event.approval", |f| f.method == "event.approval").await;
    assert_eq!(ev.params["session_id"], session_id, "会话号一致: {ev:?}");
    assert_eq!(ev.params["candidate_name"], "终端B");
    assert_eq!(
        ev.params["candidate_platform"],
        std::env::consts::OS,
        "候选平台 = 新终端自报(daemon 发本机 OS)"
    );
    let approval_code = ev.params["code"].as_str().expect("审批码在场").to_string();
    assert_eq!(approval_code.len(), 6, "审批码六位: {approval_code}");
    assert!(ev.params["expires_at"].as_i64().unwrap_or(0) > 0);

    // B 端先轮询:仍 pending(approval 形态,非 RPC 错)
    let st = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect("轮询是正常投影");
    assert_eq!(st["status"], "approval", "未裁决前轮询仍 approval: {st}");

    // A 端弹窗同意
    let r = call(&ca, "approval.respond", json!({ "session_id": session_id, "allow": true }))
        .await
        .expect("受信端应答应成功");
    assert_eq!(r, json!({}));

    // B 端轮询领票:ok → 会话落盘 → 上线面翻转
    let st = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect("轮询领票应成功");
    assert_eq!(st["status"], "ok", "裁决后轮询必须 ok: {st}");
    assert_eq!(st["name"], "终端B", "ok 投影回显候选名(槽内候选)");
    assert!(st["jwt"].is_null(), "jwt 恒不出 IPC: {st}");
    assert!(home_b.path().join("session.json").exists(), "会话必须落盘");
    let sess_b = mtdss_daemon::session::Session::load(home_b.path()).unwrap();
    assert_eq!(sess_b.device_id, st["device_id"].as_str().unwrap());
    wait_online(&cb, true, 30).await;

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_b);
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_full_chain_via_submit_code() {
    let (addr, _state) = spawn_test_server().await;
    signaling::admin::user_add(&_state.db.lock().unwrap(), "sub@x.y").unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, _dev_a, sess_a) = trusted_online(addr, "sub@x.y", "终端A").await;
    let mut evs_a = ca.events();

    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let (session_id, _) = verify_expect_approval(&cb, &server, "sub@x.y", "终端B").await;

    // A 端收弹窗(取审批码)→ B 端直接输码 settle(不轮询,一步收口)
    let ev = wait_event(&mut evs_a, 15, "event.approval", |f| f.method == "event.approval").await;
    let approval_code = ev.params["code"].as_str().expect("审批码在场").to_string();
    let r = call(
        &cb,
        "login.submit_code",
        json!({ "session_id": session_id, "code": approval_code }),
    )
    .await
    .expect("输码收口应成功");
    assert_eq!(r["status"], "ok", "输码成功即领票登录: {r}");
    assert_eq!(r["name"], "终端B");
    assert!(home_b.path().join("session.json").exists());
    let sess_b = mtdss_daemon::session::Session::load(home_b.path()).unwrap();
    assert_eq!(sess_b.device_id, r["device_id"].as_str().unwrap());
    wait_online(&cb, true, 30).await;

    // 码错路径:错码 → 业务错(带 401 上下文),会话不动
    // (另起一条审批会话验证,原会话已被上面消费)
    let home_c = tempfile::tempdir().unwrap();
    let daemon_c = Daemon::start(home_c.path(), opts(home_c.path())).await.unwrap();
    let cc = IpcClient::connect(home_c.path()).await.unwrap();
    let (sid_c, _) = verify_expect_approval(&cc, &server, "sub@x.y", "终端C").await;
    let wrong = if approval_code == "000000" { "000001" } else { "000000" };
    let err = call(&cc, "login.submit_code", json!({ "session_id": sid_c, "code": wrong }))
        .await
        .expect_err("错码必须业务错");
    assert_eq!(err.code, 1, "错码是 code 1: {err:?}");
    assert!(!home_c.path().join("session.json").exists(), "错码不得落会话");

    drop(cc);
    daemon_c.shutdown().await.unwrap();
    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(sess_b);
    drop(home_a);
    drop(home_b);
    drop(home_c);
}

// ---------- limit → evict → ok(两条到达路径) ----------

/// verify 直达 limit:受信端行在场但**全部离线**(presence 快照空 = 自动
/// 受信直入),配额门在 enroll 收口拦下 → limit 形态(单次凭据+清单)→
/// 踢选腾位 → ok 收口登录。
#[tokio::test(flavor = "multi_thread")]
async fn limit_then_evict_then_ok() {
    bypass_proxy_once();
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "cap@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "cap@x.y", 1).unwrap();
    let server = format!("http://{addr}");
    // A:只预置行,不起 daemon(离线受信端,快照空)
    let home_a = tempfile::tempdir().unwrap();
    let dev_a = preregister_identity(addr, home_a.path(), "cap@x.y", "终端A").await;

    // B 新终端:直入 → 配额满 → limit 形态
    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let code = call(&cb, "login.request_code", json!({ "server": server, "email": "cap@x.y" }))
        .await
        .unwrap();
    let r = call(
        &cb,
        "login.verify",
        json!({ "email": "cap@x.y", "code": code["code"].as_str().unwrap(), "name": "终端B" }),
    )
    .await
    .expect("verify 请求应成功(limit 不是 RPC 错)");
    assert_eq!(r["status"], "limit", "配额满必须回 limit: {r}");
    assert!(!r["enroll_token"].as_str().unwrap_or_default().is_empty());
    let devices = r["devices"].as_array().expect("devices 清单在场");
    assert_eq!(devices.len(), 1, "现有清单恰 A 一台: {r}");
    assert_eq!(devices[0]["device_id"], dev_a.as_str());
    assert_eq!(devices[0]["name"], "终端A");
    assert_eq!(devices[0]["online"], false, "A 离线(踢选屏在线列)");
    assert_eq!(devices[0]["platform"], "test");

    // 踢选 A → 本机 enroll 收口登录
    let r = call(
        &cb,
        "login.evict",
        json!({ "enroll_token": r["enroll_token"].as_str().unwrap(), "device_id": dev_a }),
    )
    .await
    .expect("踢选应成功");
    assert_eq!(r["status"], "ok", "踢选后直入: {r}");
    assert_eq!(r["name"], "终端B", "ok 投影回显候选名");
    assert!(home_b.path().join("session.json").exists());
    wait_online(&cb, true, 30).await;

    // B 的目录:只剩 B(A 行已被 evict 删除)
    let devs = call(&cb, "devices", json!({})).await.unwrap();
    let list = devs["devices"].as_array().unwrap();
    assert_eq!(list.len(), 1, "evict 后账号只剩 B: {devs}");
    assert_eq!(
        list[0]["device_id"].as_str(),
        Some(sess_b_dev(home_b.path()).as_str())
    );

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(home_a);
    drop(home_b);
}

/// 审批 settle 撞配额:A 在线,B verify→approval→A 同意→轮询时 enroll 收口
/// 才撞门 → login.status 回 **limit 形态**(凭据记槽)→ 踢选续接 evict → ok。
/// 钉死「status 的 limit 转场凭据可续接」(update_pending_session 路径)。
#[tokio::test(flavor = "multi_thread")]
async fn settle_hits_limit_then_evict_continues() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "cap2@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "cap2@x.y", 1).unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, dev_a, sess_a) = trusted_online(addr, "cap2@x.y", "终端A").await;

    // B verify → approval(A 在线)→ A 同意
    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let (session_id, _) = verify_expect_approval(&cb, &server, "cap2@x.y", "终端B").await;
    call(&ca, "approval.respond", json!({ "session_id": session_id, "allow": true }))
        .await
        .expect("受信端应答应成功");

    // B 轮询:settle 领票撞配额 → limit 形态(enroll_token + 清单)
    let st = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect("轮询应成功");
    assert_eq!(st["status"], "limit", "settle 撞门回 limit: {st}");
    let enroll_token = st["enroll_token"].as_str().expect("凭据在场").to_string();
    assert_eq!(st["devices"][0]["device_id"], dev_a.as_str());

    // 转场凭据已记槽:踢选续接成功(settle-login-status 一条龙)
    let r = call(&cb, "login.evict", json!({ "enroll_token": enroll_token, "device_id": dev_a }))
        .await
        .expect("踢选续接应成功");
    assert_eq!(r["status"], "ok", "踢选后直入: {r}");
    assert!(home_b.path().join("session.json").exists());
    wait_online(&cb, true, 30).await;

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_b);
}

/// 从落盘会话读本机 device_id(目录断言用)。
fn sess_b_dev(home: &Path) -> String {
    mtdss_daemon::session::Session::load(home).unwrap().device_id
}

// ---------- 被踢端:清会话 + identity.seed 保留 ----------

#[tokio::test(flavor = "multi_thread")]
async fn evicted_device_clears_session_keeps_seed() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "kick1@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "kick1@x.y", 1).unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, dev_a, sess_a) = trusted_online(addr, "kick1@x.y", "终端A").await;
    let mut evs_a = ca.events();

    // B 审批链入会撞配额:verify→approval → A 同意 → 轮询回 limit → 踢选 A
    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let (session_id, _) = verify_expect_approval(&cb, &server, "kick1@x.y", "终端B").await;
    call(&ca, "approval.respond", json!({ "session_id": session_id, "allow": true }))
        .await
        .unwrap();
    let st = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect("轮询应成功");
    assert_eq!(st["status"], "limit", "settle 撞门回 limit: {st}");
    call(
        &cb,
        "login.evict",
        json!({ "enroll_token": st["enroll_token"].as_str().unwrap(), "device_id": dev_a }),
    )
    .await
    .expect("踢选应成功");

    // A 端:临别帧路由成 event.device_evicted(空参数面)
    let ev = wait_event(&mut evs_a, 15, "event.device_evicted", |f| {
        f.method == "event.device_evicted"
    })
    .await;
    assert_eq!(ev.params, json!({}), "被踢事件空参数面: {ev:?}");

    // A 端会话已清(identity.seed 保留,设备身份不变),UI 回登录态
    assert!(!sess_a.exists(), "session.json 必须已删除");
    assert!(home_a.path().join("identity.seed").exists(), "identity.seed 必须保留");
    let hello = call(&ca, "hello", json!({})).await.unwrap();
    assert_eq!(hello["logged_in"], false, "被踢即回登录态: {hello}");

    // 不复活:会话已清,在线循环不再带旧凭据重连(观察窗内恒 offline)
    wait_online(&ca, false, 5).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let s = call(&ca, "status", json!({})).await.unwrap();
    assert_eq!(s["online"], false, "无会话不得复活在线: {s}");
    assert_eq!(s["logged_in"], false);

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(home_a);
    drop(home_b);
}

/// admin unbind(懒踢路径,无临别帧):被unbind设备此后重连被拒(code=1,
/// refresh 同样 401)——daemon 连续两次鉴权拒后按本机已移除收口,清会话
/// (identity.seed 保留)并发 event.device_evicted,不再无限退避空转。
#[tokio::test(flavor = "multi_thread")]
async fn reconnect_after_unbind_clears_session() {
    bypass_proxy_once();
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "unbind@x.y").unwrap();
    let server = format!("http://{addr}");
    let home_a = tempfile::tempdir().unwrap();
    let dev_a = preregister_identity(addr, home_a.path(), "unbind@x.y", "终端A").await;
    let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
    let ca = IpcClient::connect(home_a.path()).await.unwrap();
    ipc_login(&ca, &server, "unbind@x.y", "终端A").await;
    wait_online(&ca, true, 30).await;

    // 摘掉 IPC 客户端与在线循环(进程级下线,session.json 保留在盘);
    // 随后 admin 删行——设备在离线期间被移除,daemon 无从得知。
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    assert!(
        signaling::admin::device_unbind(&state.db.lock().unwrap(), "unbind@x.y", &dev_a).unwrap(),
        "unbind 必须命中在册设备"
    );

    // 重启 daemon:重连恒 code=1(device not bound),refresh 恒 401(行已
    // 删)→ 连续两次鉴权拒 → 清会话回登录态
    let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
    let mut evs = daemon_a.events.subscribe();
    let f = wait_event(&mut evs, 30, "event.device_evicted(重连拒收口)", |f| {
        f.method == "event.device_evicted"
    })
    .await;
    assert_eq!(f.params, json!({}));

    let ca = IpcClient::connect(home_a.path()).await.unwrap();
    let hello = call(&ca, "hello", json!({})).await.unwrap();
    assert_eq!(hello["logged_in"], false, "重连拒必须清会话: {hello}");
    assert!(!home_a.path().join("session.json").exists(), "session.json 必须已删除");
    assert!(home_a.path().join("identity.seed").exists(), "identity.seed 必须保留");

    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(home_a);
}

// ---------- 扫码全链(IPC 透传 op 驱动;Ruling R8) ----------

#[tokio::test(flavor = "multi_thread")]
async fn qr_full_chain_claim_then_approve() {
    let (addr, _state) = spawn_test_server().await;
    signaling::admin::user_add(&_state.db.lock().unwrap(), "qr@x.y").unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, _dev_a, sess_a) = trusted_online(addr, "qr@x.y", "手机A").await;

    // PC:qr_begin → 二维码内容由 daemon 拼(daemon 无登录态也可发起)
    let home_pc = tempfile::tempdir().unwrap();
    let daemon_pc = Daemon::start(home_pc.path(), opts(home_pc.path())).await.unwrap();
    let cpc = IpcClient::connect(home_pc.path()).await.unwrap();
    let r = call(
        &cpc,
        "login.qr_begin",
        json!({ "server": server, "name": "PC端" }),
    )
    .await
    .expect("qr_begin 应成功");
    let token = r["token"].as_str().expect("token 在场").to_string();
    assert_eq!(
        r["qr_url"].as_str().unwrap(),
        format!("https://www.mtdss.top/qr?t={token}"),
        "二维码内容 = 固定前缀 + token: {r}"
    );
    assert!(r["expires_at"].as_i64().unwrap_or(0) > 0);

    // 轮询:未认领 = pending(正常投影,非 RPC 错)
    let st = call(&cpc, "login.qr_status", json!({ "token": token })).await.unwrap();
    assert_eq!(st["status"], "pending", "未认领恒 pending: {st}");

    // 未登录端(daemon_pc)认领 → code 1(Bearer 面须已登录;T8 令牌卫生)
    let err = call(&cpc, "qr.claim", json!({ "token": token }))
        .await
        .expect_err("未登录不可认领");
    assert_eq!(err.code, 1, "未登录认领是业务错: {err:?}");
    assert!(err.message.contains("未登录"), "未登录文案: {}", err.message);

    // 手机扫码认领(IPC 透传 op,Bearer=手机 daemon 会话 JWT 代打):
    // 候选对象拍平投影
    let c = call(&ca, "qr.claim", json!({ "token": token }))
        .await
        .expect("qr.claim 应成功");
    assert_eq!(c["candidate_name"], "PC端", "确认弹层带候选名: {c}");
    assert_eq!(c["candidate_platform"], std::env::consts::OS, "候选平台 = PC 自报");
    assert!(c["expires_at"].as_i64().unwrap_or(0) > 0, "候选时限在场: {c}");

    // 同认领者幂等:重扫回同一候选面(先到先裁,不 409)
    let c2 = call(&ca, "qr.claim", json!({ "token": token })).await.unwrap();
    assert_eq!(c2, c, "同认领者 claim 幂等回同面: {c2}");

    // 认领后 PC 轮询仍 pending(claimed 对 PC 透明,继续等)
    let st = call(&cpc, "login.qr_status", json!({ "token": token })).await.unwrap();
    assert_eq!(st["status"], "pending", "已认领未裁决仍 pending: {st}");

    // 死码认领:typed not_found(不存在/已过期同 404 面,服务端惰性落库)
    let err = call(&ca, "qr.claim", json!({ "token": "qrt-nonexistent" }))
        .await
        .expect_err("死码认领必须 typed not_found");
    assert_eq!(err.code, 1);
    assert_eq!(err.data.as_ref().unwrap()["kind"], "not_found", "typed kind: {err:?}");
    assert!(err.message.contains("二维码已失效"), "死码文案: {}", err.message);

    // 手机确认(IPC 透传 allow=true)→ settle 投影 ok → PC 轮询领票 ok →
    // 登录落盘
    let r = call(&ca, "qr.approve", json!({ "token": token, "allow": true }))
        .await
        .expect("qr.approve 应成功");
    assert_eq!(r["status"], "ok", "裁决受理投影: {r}");
    let st = call(&cpc, "login.qr_status", json!({ "token": token })).await.unwrap();
    assert_eq!(st["status"], "ok", "裁决后轮询必须 ok: {st}");
    assert_eq!(st["name"], "PC端", "ok 投影回显 qr_begin 的候选名");
    assert!(home_pc.path().join("session.json").exists());
    wait_online(&cpc, true, 30).await;

    // token 单次性由服务端 qr.rs 钉死;daemon 侧收口即清槽(settle 后续接
    // 态不复用),原 token 再轮询走「无待登录记录」参数面。
    let err = call(&cpc, "login.qr_status", json!({ "token": token }))
        .await
        .expect_err("收口后槽已清,重放不再受理");
    assert_eq!(err.code, -32602, "槽已清 → -32602: {err:?}");

    // 第二轮:拒绝路(IPC 全链)——claim → approve allow=false → PC 轮询
    // denied 终态,不落会话
    let r2 = call(
        &cpc,
        "login.qr_begin",
        json!({ "server": server, "name": "PC端" }),
    )
    .await
    .expect("第二轮 qr_begin 应成功");
    let token2 = r2["token"].as_str().expect("token2 在场").to_string();
    call(&ca, "qr.claim", json!({ "token": token2 })).await.expect("二次认领应成功");
    let r2 = call(&ca, "qr.approve", json!({ "token": token2, "allow": false }))
        .await
        .expect("拒绝裁决应成功");
    assert_eq!(r2["status"], "denied", "拒绝投影: {r2}");
    let st = call(&cpc, "login.qr_status", json!({ "token": token2 })).await.unwrap();
    assert_eq!(st["status"], "denied", "拒绝轮询终态: {st}");
    // denied 不落新会话(首轮 ok 的会话保持原样,轮询终态已证未走 settle)

    // 已 settle 的 token 再裁决(单次):expired 投影(正常投影非 RPC 错)
    let r2 = call(&ca, "qr.approve", json!({ "token": token2, "allow": true }))
        .await
        .unwrap();
    assert_eq!(r2["status"], "expired", "settle 后重放 → expired: {r2}");

    drop(cpc);
    daemon_pc.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_pc);
}

// ---------- devices_changed 快照刷新 + data_changed 事件面 + KV 透传 ----------

#[tokio::test(flavor = "multi_thread")]
async fn devices_changed_refreshes_snapshot_and_kv_roundtrip() {
    let (addr, _state) = spawn_test_server().await;
    signaling::admin::user_add(&_state.db.lock().unwrap(), "chg@x.y").unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, _dev_a, sess_a) = trusted_online(addr, "chg@x.y", "终端A").await;
    let mut evs_a = ca.events();

    // B 经审批链入会(新终端 → A 弹窗同意 → B 轮询领票)
    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let (session_id, _) = verify_expect_approval(&cb, &server, "chg@x.y", "终端B").await;
    call(&ca, "approval.respond", json!({ "session_id": session_id, "allow": true }))
        .await
        .unwrap();
    call(&cb, "login.status", json!({ "session_id": session_id })).await.unwrap();
    wait_online(&cb, true, 30).await;
    let dev_b = sess_b_dev(home_b.path());

    // devices 表增删通告:任一在线端(A)的快照刷新(event.devices)必须
    // 带上新成员 B(devices_changed 与 presence-未知刷新双路,殊途同归)
    wait_event(&mut evs_a, 15, "event.devices(含 B)", |f| {
        f.method == "event.devices"
            && f.params["devices"]
                .as_array()
                .map(|l| l.iter().any(|d| d["device_id"] == dev_b.as_str()))
                .unwrap_or(false)
    })
    .await;

    // KV 透传:B 写 favorites(V3-3 spec §3.1 键名)→ A 读同值
    let put = call(
        &cb,
        "kv.put",
        json!({ "key": "favorites", "value": "{\"dirs\":[\"/工作\"]}" }),
    )
    .await
    .expect("kv.put 应成功");
    assert_eq!(put, json!({ "ok": true }), "kv.put 投影: {put}");
    // data_changed 通告点亮为 `event.data_changed {key}` 事件面(V3-3 spec §7):
    // A 的事件流收帧,params.key 投影写端 KV 键(通告后连接保持不变)
    wait_event(&mut evs_a, 15, "event.data_changed(favorites)", |f| {
        f.method == "event.data_changed" && f.params["key"] == "favorites"
    })
    .await;
    let got = call(&ca, "kv.get", json!({ "key": "favorites" })).await.unwrap();
    assert_eq!(got["value"], "{\"dirs\":[\"/工作\"]}", "跨设备同值: {got}");

    // 404 = 合法未命 → typed not_found(code 1 + data.kind)
    let err = call(&ca, "kv.get", json!({ "key": "无此键" }))
        .await
        .expect_err("未命是 typed 错");
    assert_eq!(err.code, 1, "未命 code 1: {err:?}");
    assert_eq!(err.kind(), Some("not_found"), "typed kind 必须在场: {err:?}");

    // data_changed 通告送达后 A 的连接保持(容忍面)
    tokio::time::sleep(Duration::from_millis(800)).await;
    let s = call(&ca, "status", json!({})).await.unwrap();
    assert_eq!(s["online"], true, "data_changed 不得断开 A 的连接: {s}");

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_b);
}

/// devices_changed 通告的**隔离锚**:admin unbind 的懒踢 tick 对**已在快照
/// 内**的设备删行——B 的 Presence false 走已知设备就地翻转(不触发未知设备
/// 的全量刷新),故「快照里 B 行消失」只能来自 devices_changed 通告触发的
/// 刷新;同时钉死新事件帧 `event.devices_changed {}` 的真实发射(hub.rs
/// devices_changed 臂——review Important #1:此前 golden 有帧而 daemon 无
/// 发射点的幽灵契约)。
#[tokio::test(flavor = "multi_thread")]
async fn unbind_notices_devices_changed_and_refreshes_snapshot() {
    bypass_proxy_once();
    // 压缩 ping 间隔:绑定复核随 keepalive tick,懒踢 ≤2 tick 内生效
    // (spawn_test_server_with 注入,server 套件 quota.rs 同款手法)
    let (addr, state) = spawn_test_server_with(
        signaling::ws::Keepalive {
            ping_interval: Duration::from_millis(300),
            silent_limit: Duration::from_secs(10),
        },
        Vec::new(),
    )
    .await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "chg2@x.y").unwrap();
    let server = format!("http://{addr}");
    // B 行先预置(任何上线发生前):A 在线后再 REST 注册会撞审批面
    let home_b = tempfile::tempdir().unwrap();
    let dev_b = preregister_identity(addr, home_b.path(), "chg2@x.y", "终端B").await;
    let (home_a, daemon_a, ca, _dev_a, sess_a) = trusted_online(addr, "chg2@x.y", "终端A").await;
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    ipc_login(&cb, &server, "chg2@x.y", "终端B").await;
    wait_online(&cb, true, 30).await;
    let mut evs_a = ca.events();

    // admin 删 B 行(懒踢:绑定复核失守 → 全账号通告 + code=9 逐 B)
    assert!(
        signaling::admin::device_unbind(&state.db.lock().unwrap(), "chg2@x.y", &dev_b).unwrap(),
        "unbind 必须命中在册设备"
    );

    // 新事件帧:event.devices_changed {}(空参数面,真实发射)
    let f = wait_event(&mut evs_a, 20, "event.devices_changed", |fr| {
        fr.method == "event.devices_changed"
    })
    .await;
    assert_eq!(f.params, json!({}), "devices_changed 空参数面: {f:?}");

    // 快照刷新(event.devices)里 B 行消失——Presence 路径只会翻 online 位,
    // 「行消失」必然来自通告触发的全量刷新(REST 为权威)
    wait_event(&mut evs_a, 15, "event.devices(无 B)", |fr| {
        fr.method == "event.devices"
            && fr.params["devices"]
                .as_array()
                .map(|l| !l.iter().any(|d| d["device_id"] == dev_b.as_str()))
                .unwrap_or(false)
    })
    .await;

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_b);
}

// ---------- login.cancel 作废本地续接态 ----------

#[tokio::test(flavor = "multi_thread")]
async fn cancel_voids_local_pending() {
    let (addr, _state) = spawn_test_server().await;
    signaling::admin::user_add(&_state.db.lock().unwrap(), "cancel@x.y").unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, _dev_a, sess_a) = trusted_online(addr, "cancel@x.y", "终端A").await;

    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let (session_id, _) = verify_expect_approval(&cb, &server, "cancel@x.y", "终端B").await;

    let r = call(&cb, "login.cancel", json!({})).await.unwrap();
    assert_eq!(r, json!({}), "cancel 返回空对象");

    // 续接槽已作废:同会话号的轮询不再受理(-32602 参数面,非轮询投影)
    let err = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect_err("作废后轮询必须被拒");
    assert_eq!(err.code, -32602, "续接槽已清,参数面拒绝: {err:?}");
    assert!(!home_b.path().join("session.json").exists(), "cancel 不落会话");

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_b);
}
