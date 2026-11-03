//! 审批状态机集成面(V3-1 Task 4,spec §3.1/§3.10):
//! - verify 三分支:老终端直入 / 新终端+在线受信端→审批 / 新终端+快照空→自动受信直入
//! - 会话先到先裁(approval-respond 同意 / approve-code 输码两 settle 路)、拒绝一票否决
//! - 300s TTL(expire 注入)、单次有效(ok 带票恰一次,再轮询=终态不重发票)
//! - 快照 fail-closed:verify 后受信端全掉线仍须裁决,绝不降级直入
//! - 换绑守卫:已属他账号的 pubkey 恒走审批;approved 后 upsert 迁移、旧账号失去
//! - approve-code 错 5 次作废;approve-code 30/min、login-status 120/min per-IP 限流

use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message;
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::sync::Arc;
use std::time::Duration;

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn post(addr: &std::net::SocketAddr, path: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let resp = client()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let st = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (st, v)
}

async fn post_full(
    addr: &std::net::SocketAddr,
    path: &str,
    body: Value,
) -> (reqwest::StatusCode, reqwest::header::HeaderMap, Value) {
    let resp = client()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let st = resp.status();
    let headers = resp.headers().clone();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (st, headers, v)
}

async fn post_bearer(
    addr: &std::net::SocketAddr,
    path: &str,
    body: Value,
    bearer: &str,
) -> (reqwest::StatusCode, Value) {
    let resp = client()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap();
    let st = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (st, v)
}

/// 常量字节多半不是合法 Ed25519 曲线点(服务端 decompress 会拒),seed 派生。
fn pubkey_hex(seed: [u8; 32]) -> String {
    hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .as_bytes(),
    )
}

/// dev 回显领码(测试基建恒 echo_code=on)。
async fn request_code(addr: &std::net::SocketAddr, email: &str) -> String {
    let (_, v) = post(addr, "/auth/request-code", json!({ "email": email })).await;
    v["code"].as_str().unwrap().to_string()
}

/// 走完整 verify;返回原始响应(三分支形态各异,断言侧自取)。
async fn verify_raw(
    addr: &std::net::SocketAddr,
    email: &str,
    code: &str,
    pk: &str,
    name: &str,
) -> (reqwest::StatusCode, Value) {
    post(
        addr,
        "/auth/verify",
        json!({ "email": email, "code": code, "pubkey": pk, "name": name, "platform": "test" }),
    )
    .await
}

/// 直入形态注册(预置账号;无 WS 在场时恒 ok):返回 verify ok 响应。
async fn register(
    state: &Arc<signaling::AppState>,
    addr: &std::net::SocketAddr,
    email: &str,
    seed: [u8; 32],
    name: &str,
) -> Value {
    signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
    let code = request_code(addr, email).await;
    let (st, v) = verify_raw(addr, email, &code, &pubkey_hex(seed), name).await;
    assert_eq!(st, reqwest::StatusCode::OK, "直入注册应 ok: {v}");
    assert_eq!(v["status"], "ok", "直入注册应 ok 形态: {v}");
    v
}

fn account_id_of(state: &Arc<signaling::AppState>, email: &str) -> String {
    state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
        .unwrap()
}

fn device_count(state: &Arc<signaling::AppState>, email: &str) -> i64 {
    let id = account_id_of(state, email);
    state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM devices WHERE account_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap()
}

/// 连 WS + hello,返回流(在线受信终端模拟)。
async fn connect_trusted(addr: &std::net::SocketAddr, jwt: &str, dev: &str) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let f = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: dev.into(),
            jwt: jwt.into(),
        })),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(f.encode_to_vec()))
        .await
        .unwrap();
    let f = recv_frame(&mut ws).await;
    assert!(
        matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))),
        "hello 应得 HelloOk: {f:?}"
    );
    ws
}

/// 下一二进制帧(跳 Ping)。
async fn recv_frame(ws: &mut Ws) -> WsServerFrame {
    loop {
        match ws.next().await.unwrap().unwrap() {
            tokio_tungstenite::tungstenite::Message::Binary(b) => {
                return WsServerFrame::decode(b.as_ref()).unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// 下一 AuthNotice(跳过 presence 等 interleaved 帧)。
async fn recv_notice(ws: &mut Ws) -> AuthNotice {
    loop {
        match recv_frame(ws).await.frame {
            Some(ws_server_frame::Frame::Notice(n)) => return n,
            Some(_) => continue,
            None => panic!("stream closed before notice"),
        }
    }
}

async fn poll(addr: &std::net::SocketAddr, session_id: &str) -> (reqwest::StatusCode, Value) {
    post(addr, "/auth/login-status", json!({ "session_id": session_id })).await
}

// ---------- 三分支 ----------

/// ①老终端(device 已在本账号 devices)直入:同 pubkey 重走验证码恒 ok。
#[tokio::test]
async fn old_terminal_relogin_direct_ok() {
    let (addr, _state) = spawn_test_server().await;
    let v1 = register(&_state, &addr, "old@x.y", [1u8; 32], "d1").await;
    let code = request_code(&addr, "old@x.y").await;
    let (st, v2) = verify_raw(&addr, "old@x.y", &code, &pubkey_hex([1u8; 32]), "d1").await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v2["status"], "ok", "老终端重登必须直入 ok: {v2}");
    assert_eq!(v2["device_id"], v1["device_id"], "同 pubkey 同 device_id");
    assert!(v2["jwt"].as_str().is_some() && v2["refresh"].as_str().is_some());
    assert!(v2["account_id"].as_str().is_some());
}

/// ③新终端 + 快照空(有受信设备行但全不在线)→ 自动受信直插 ok。
#[tokio::test]
async fn new_terminal_no_online_peer_auto_trusts() {
    let (addr, state) = spawn_test_server().await;
    register(&state, &addr, "a@b.c", [1u8; 32], "d1").await;
    // 第二台新设备注册:无任何 WS 在线 → 快照空 → 直入
    let b = register(&state, &addr, "a@b.c", [2u8; 32], "d2").await;
    assert_eq!(b["status"], "ok", "快照空的新终端必须自动受信直入: {b}");
    assert!(b["jwt"].as_str().is_some());
    assert_eq!(device_count(&state, "a@b.c"), 2);
}

/// ②新终端 + 在线受信端在场 → approval 形态 + 全端 AuthNotice join_request。
#[tokio::test]
async fn new_terminal_with_online_peer_requires_approval_and_pushes_notice() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "desktop-a").await;
    let c = register(&state, &addr, "fam@x.y", [2u8; 32], "desktop-c").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let mut ws_c =
        connect_trusted(&addr, c["jwt"].as_str().unwrap(), c["device_id"].as_str().unwrap()).await;

    // 新终端 B verify:在线受信端在场 → approval
    let code = request_code(&addr, "fam@x.y").await;
    let (st, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "新来的").await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "approval", "在线受信端在场必须转审批: {v}");
    let sid = v["session_id"].as_str().unwrap().to_string();
    assert!(!sid.is_empty());
    let exp = v["expires_at"].as_i64().unwrap();
    assert!(exp > signaling::db::now(), "expires_at 须在未来: {exp}");

    // 全端各收一条 join_request(尽力推,两端都到)
    for ws in [&mut ws_a, &mut ws_c] {
        let n = recv_notice(ws).await;
        assert_eq!(n.kind, "join_request");
        assert_eq!(n.session_id, sid);
        assert_eq!(n.code.len(), 6, "审批码 6 位: {}", n.code);
        assert!(n.code.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(n.candidate_name, "新来的");
        assert_eq!(n.candidate_platform, "test");
        assert_eq!(n.expires_at, exp);
    }
}

// ---------- settle 两路 + 拒绝 + 单次 ----------

/// settle 路 1:approval-respond 同意 → 轮询得 ok 带票;单次有效(再轮询=终态
/// 不重发票);已 settle 会话再 respond/approve-code 一律 409。
#[tokio::test]
async fn respond_allow_issues_tokens_once() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let _ = recv_notice(&mut ws_a).await; // 弹窗面(内容断言归专门测试)

    // 未 settle 前轮询 = approval
    let (st, v) = poll(&addr, &sid).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "approval", "settle 前轮询须 approval: {v}");

    // 受信端同意
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    // 新终端轮询:ok 带票
    let (st, v) = poll(&addr, &sid).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "ok", "approved 后轮询须 ok 带票: {v}");
    assert!(v["jwt"].as_str().is_some() && v["refresh"].as_str().is_some());
    assert!(v["account_id"].as_str().is_some());
    assert_eq!(device_count(&state, "fam@x.y"), 2, "approved 须插入设备行");

    // 单次有效:再轮询 = 终态,不重发票
    let (st, v) = poll(&addr, &sid).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "expired", "已消费会话再轮询须终态: {v}");
    assert!(v["jwt"].as_str().is_none(), "终态不得重发票: {v}");

    // 已 settle:再 respond / approve-code 一律 409
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT);
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": "000000" })).await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT);
}

/// settle 路 2:approve-code 输码(免认证)→ approved → 轮询 ok 带票(单次)。
#[tokio::test]
async fn approve_code_path_issues_tokens_once() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n = recv_notice(&mut ws_a).await;

    // 新终端直接输码(免认证)
    let (st, v) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": n.code })).await;
    assert_eq!(st, reqwest::StatusCode::OK, "对上审批码须 approved: {v}");

    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "ok", "输码 approved 后轮询须 ok: {v}");
    assert!(v["jwt"].as_str().is_some());
    assert_eq!(device_count(&state, "fam@x.y"), 2);
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "expired", "单次有效: 再轮询终态");
    assert!(v["jwt"].as_str().is_none());
}

/// 拒绝一票否决:allow=false → denied 终态,设备行不得插入,输码也救不回。
#[tokio::test]
async fn deny_veto_is_terminal() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n = recv_notice(&mut ws_a).await;

    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": false }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "denied", "拒绝后轮询须 denied: {v}");
    assert_eq!(device_count(&state, "fam@x.y"), 1, "denied 不得插设备行");
    // denied 终态:正码输码也拒,轮询恒 denied
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": n.code })).await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT);
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "denied");
}

/// 300s TTL:expire 注入后轮询=expired;正码输码与受信端同意都不再 settle。
#[tokio::test]
async fn approval_expires_after_ttl() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n = recv_notice(&mut ws_a).await;

    // expire 注入(沿 _expire_all_codes_for_test 先例:直接置 expires_at=0,
    // 自然 TTL 判定路径被真实走到)
    signaling::auth::_expire_all_approvals_for_test(&state);

    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "expired", "TTL 过后轮询须 expired: {v}");
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": n.code })).await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "过期会话正码也不得 settle");
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "过期会话不得再裁决");
    assert_eq!(device_count(&state, "fam@x.y"), 1);
}

// ---------- 快照 fail-closed / 换绑 ----------

/// 快照 fail-closed(review focus #1):verify 时刻有在线受信端,随后全掉线,
/// 会话仍只认 approvals 行——不降级直入,输码裁决仍然必要且有效。
#[tokio::test]
async fn snapshot_fail_closed_after_trusted_disconnect() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n = recv_notice(&mut ws_a).await;

    // 受信端全掉线
    drop(ws_a);
    let acct = account_id_of(&state, "fam@x.y");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !state.presence.account_peers(&acct).is_empty() {
        assert!(std::time::Instant::now() < deadline, "受信端未按预期掉线");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 不降级:快照空也不会自动直入,会话仍须裁决
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "approval", "快照后全掉线不得降级,仍须裁决: {v}");
    assert_eq!(device_count(&state, "fam@x.y"), 1, "掉线不降级=不得直插设备行");
    // 输码裁决照常有效
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": n.code })).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "ok", "掉线后输码裁决仍放行: {v}");
}

/// 换绑守卫:pubkey 已属账号 A,B 账号(无任何在线受信端)verify → 恒走审批,
/// 绝不直入直迁(直入分支「row 不存在 OR 已属本账号」守卫)。
#[tokio::test]
async fn rebind_requires_approval_even_without_online_peer() {
    let (addr, state) = spawn_test_server().await;
    let first = register(&state, &addr, "old@x.y", [1u8; 32], "d1").await;
    let dev_id = first["device_id"].as_str().unwrap().to_string();

    // 同一 pubkey 以 B 账号邮箱 verify:B 快照空也必须 approval(不直迁)
    signaling::admin::user_add(&state.db.lock().unwrap(), "b@x.y").unwrap();
    let code = request_code(&addr, "b@x.y").await;
    let (st, v) = verify_raw(&addr, "b@x.y", &code, &pubkey_hex([1u8; 32]), "d1").await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "approval", "已属他账号的 pubkey 恒走审批: {v}");
    assert!(v["jwt"].as_str().is_none(), "审批形态不带票");
    assert_eq!(v["device_id"], Value::Null, "审批形态无 device_id 字段");
    // 设备行未迁移:仍挂旧账号
    assert_eq!(device_count(&state, "old@x.y"), 1);
    assert_eq!(device_count(&state, "b@x.y"), 0);
    let _ = dev_id;
}

/// 换绑 approved 全链:B 账号受信端同意 → 设备 upsert 迁移 A→B,旧账号失去。
#[tokio::test]
async fn rebind_approved_migrates_device_between_accounts() {
    let (addr, state) = spawn_test_server().await;
    let old = register(&state, &addr, "old@x.y", [1u8; 32], "d1").await;
    // B 账号预置一台在线受信端 C
    let c = register(&state, &addr, "new@x.y", [2u8; 32], "tc").await;
    let mut ws_c =
        connect_trusted(&addr, c["jwt"].as_str().unwrap(), c["device_id"].as_str().unwrap()).await;

    // 旧 pubkey 以 B 邮箱 verify → approval(B 的受信端 C 在场)
    let code = request_code(&addr, "new@x.y").await;
    let (_, v) = verify_raw(&addr, "new@x.y", &code, &pubkey_hex([1u8; 32]), "迁绑机").await;
    assert_eq!(v["status"], "approval");
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n = recv_notice(&mut ws_c).await;
    assert_eq!(n.candidate_name, "迁绑机");
    assert_eq!(n.session_id, sid);

    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        c["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "ok", "approved 迁移后轮询须 ok: {v}");
    assert_eq!(v["device_id"], old["device_id"], "迁移不换设备身份");
    assert_eq!(v["account_id"], account_id_of(&state, "new@x.y"), "JWT 账号须为新账号");

    // 新账号列出该设备;旧账号失去(单行迁移,无吊销残留)
    assert_eq!(device_count(&state, "new@x.y"), 2, "C + 迁入设备");
    assert_eq!(device_count(&state, "old@x.y"), 0, "旧账号失去该设备");
}

// ---------- 码错作废 / 重复 verify / 限流 / respond 授权面 ----------

/// approve-code 错满 5 次作废:原码再对也不放行,轮询=expired。
#[tokio::test]
async fn approval_code_five_wrong_attempts_invalidate() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n = recv_notice(&mut ws_a).await;
    let wrong = if n.code == "000000" { "000001" } else { "000000" };

    for i in 0..5 {
        let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": wrong })).await;
        assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "第 {} 次错码应 401", i + 1);
    }
    // 作废:原码再对也不放行
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": n.code })).await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "错满 5 次后原码必须作废");
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "expired", "作废会话轮询须 expired: {v}");
    assert_eq!(device_count(&state, "fam@x.y"), 1);
}

/// 同 pubkey 重复 verify = 作废旧会话建新会话(重推 join_request)。
#[tokio::test]
async fn reverify_supersedes_session() {
    let (addr, _state) = spawn_test_server().await;
    let a = register(&_state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;

    let code = request_code(&addr, "fam@x.y").await;
    let (_, v1) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid1 = v1["session_id"].as_str().unwrap().to_string();
    let n1 = recv_notice(&mut ws_a).await;
    assert_eq!(n1.session_id, sid1);

    // 重试 verify(新码)→ 新会话 + 重推
    let code2 = request_code(&addr, "fam@x.y").await;
    let (_, v2) = verify_raw(&addr, "fam@x.y", &code2, &pubkey_hex([3u8; 32]), "tb").await;
    assert_eq!(v2["status"], "approval");
    let sid2 = v2["session_id"].as_str().unwrap().to_string();
    assert_ne!(sid2, sid1, "重复 verify 须建新会话");
    let n2 = recv_notice(&mut ws_a).await;
    assert_eq!(n2.session_id, sid2, "须重推 join_request");

    // 旧会话作废:轮询终态、正码输码也拒
    let (_, v) = poll(&addr, &sid1).await;
    assert_eq!(v["status"], "expired", "旧会话须已作废: {v}");
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid1, "code": n1.code })).await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT);
    // 新会话照常 pending
    let (_, v) = poll(&addr, &sid2).await;
    assert_eq!(v["status"], "approval");
}

/// approve-code per-IP 30/min:第 31 次 429 + Retry-After(spec §3.10)。
#[tokio::test]
async fn approve_code_throttled_30_per_min() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let _ = recv_notice(&mut ws_a).await;
    let _ = a;

    for i in 0..30 {
        let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": "000000" })).await;
        assert_ne!(
            st,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "第 {} 次不应 429(前 30 次额度内)",
            i + 1
        );
    }
    let (st, headers, _) = post_full(&addr, "/auth/approve-code", json!({ "session_id": sid, "code": "000000" })).await;
    assert_eq!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "第 31 次必须 429");
    let ra = headers
        .get("retry-after")
        .expect("429 必须带 Retry-After")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(ra >= 1);
}

/// login-status per-IP 120/min:第 121 次 429(轮询兜底限速,spec §3.10)。
#[tokio::test]
async fn login_status_throttled_120_per_min() {
    let (addr, _state) = spawn_test_server().await;
    let a = register(&_state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let _ = recv_notice(&mut ws_a).await;

    for i in 0..120 {
        let (st, _) = poll(&addr, &sid).await;
        assert_ne!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "第 {} 次不应 429", i + 1);
    }
    let (st, _, _) = post_full(&addr, "/auth/login-status", json!({ "session_id": sid })).await;
    assert_eq!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "第 121 次必须 429");
}

/// approval-respond 授权面:Bearer 必须、须为该账号的在线受信设备;跨账号/
/// 离线受信端一律 403;合法受信端照常放行。
#[tokio::test]
async fn approval_respond_requires_online_trusted_device() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    // D(受信但离线)须在本机上线前注册:受信端在场时新设备会正确转审批面
    let d = register(&state, &addr, "fam@x.y", [4u8; 32], "td").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let _ = recv_notice(&mut ws_a).await;

    // 无 Bearer → 401
    let (st, _) = post(&addr, "/auth/approval-respond", json!({ "session_id": sid, "allow": true })).await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED);
    // 坏 token → 401
    let (st, _) = post_bearer(&addr, "/auth/approval-respond", json!({ "session_id": sid, "allow": true }), "bogus").await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED);

    // 跨账号在线受信端 → 403
    let x = register(&state, &addr, "x@y.z", [9u8; 32], "tx").await;
    let ws_x = connect_trusted(&addr, x["jwt"].as_str().unwrap(), x["device_id"].as_str().unwrap()).await;
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        x["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::FORBIDDEN, "跨账号 respond 必须 403");
    drop(ws_x);

    // 本账号受信但离线 → 403
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        d["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::FORBIDDEN, "离线受信端 respond 必须 403");

    // 本账号在线受信端(A)→ 200,照常 settle
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "ok", "合法受信端 respond 后轮询须 ok: {v}");
}

/// T4 评审遗留修复①:approved 未领票会话同样受 300s TTL 约束——过期后
/// consume 守卫(`expires_at>?`)拒绝领票,不再是无 TTL 的常驻登录凭据。
#[tokio::test]
async fn approved_unclaimed_session_expires_with_ttl() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid = v["session_id"].as_str().unwrap().to_string();
    let _ = recv_notice(&mut ws_a).await;

    // settle 为 approved 但尚未轮询领票
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    // TTL 注入(expires_at=0,自然判定路径被真实走到)
    signaling::auth::_expire_all_approvals_for_test(&state);

    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "expired", "approved 未领票过 TTL 后轮询须 expired: {v}");
    assert!(v["jwt"].is_null(), "过期会话不得发票: {v}");
    assert_eq!(device_count(&state, "fam@x.y"), 1, "过期领票失败不得插入设备行");

    // 再轮询仍终态(恒 expired,不复活)
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "expired");
}

/// T4 评审遗留修复②:重复 verify 的作废面须覆盖 approved 未领票会话
/// (原只清 state='pending',approved 会话成了绕过 supersede 的常驻凭据)。
#[tokio::test]
async fn reverify_supersedes_approved_unclaimed_session() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "ta").await;
    let mut ws_a =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "fam@x.y").await;
    let (_, v1) = verify_raw(&addr, "fam@x.y", &code, &pubkey_hex([3u8; 32]), "tb").await;
    let sid1 = v1["session_id"].as_str().unwrap().to_string();
    let n1 = recv_notice(&mut ws_a).await;

    // settle 为 approved 但尚未领票
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": sid1, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    // 同 pubkey 重新 verify:旧 approved 会话必须一并作废
    let code2 = request_code(&addr, "fam@x.y").await;
    let (_, v2) = verify_raw(&addr, "fam@x.y", &code2, &pubkey_hex([3u8; 32]), "tb").await;
    assert_eq!(v2["status"], "approval", "重 verify 须建新会话: {v2}");
    let sid2 = v2["session_id"].as_str().unwrap().to_string();
    assert_ne!(sid2, sid1);
    let _ = recv_notice(&mut ws_a).await;

    // 旧会话:轮询终态,正码输码也拒(approved 不得残留为可领凭据)
    let (_, v) = poll(&addr, &sid1).await;
    assert_eq!(v["status"], "expired", "approved 未领票会话被重 verify 作废: {v}");
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": sid1, "code": n1.code })).await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "被作废会话输正码也须 409");

    // 新会话照常 pending
    let (_, v) = poll(&addr, &sid2).await;
    assert_eq!(v["status"], "approval");
}

/// 未知会话:login-status→expired(合法终态,轮询面无 404 通道)、
/// approve-code→404、approval-respond(合法 Bearer)→404。
#[tokio::test]
async fn unknown_session_handling() {
    let (addr, state) = spawn_test_server().await;
    let ghost = "ffffffffffffffffffffffffffffffff";
    let (st, v) = poll(&addr, ghost).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "expired", "未知会话按终态 expired 呈现: {v}");
    let (st, _) = post(&addr, "/auth/approve-code", json!({ "session_id": ghost, "code": "000000" })).await;
    assert_eq!(st, reqwest::StatusCode::NOT_FOUND);
    let bearer = signaling::auth::mint_access_jwt(&state.jwt_secret, "a", "d");
    let (st, _) = post_bearer(
        &addr,
        "/auth/approval-respond",
        json!({ "session_id": ghost, "allow": true }),
        &bearer,
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::NOT_FOUND);
}
