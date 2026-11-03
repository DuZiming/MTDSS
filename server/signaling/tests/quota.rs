//! 配额门 + evict + 懒踢 + last_seen + 移除刷新推送(V3-1 Task 5,spec §3.3/§3.7):
//! - enroll_device 三路径同门一致(email 直入 / 审批 approved / 占位扫码=直呼同函数),
//!   各跑 limit→evict→ok 全链
//! - evict:即时踢断在线 WS(被踢端先 device_evicted 后 code9 再断)、token 单次、
//!   过期、自踢不可(404)、离线踢照常 ok、被踢设备 refresh→401、重连→code1
//! - 懒踢:admin unbind / user_remove 删行后 ≤2 压缩 tick 内 Error code=9 断
//! - last_seen:presence 注册写、keepalive tick 滚动
//! - devices_changed:evict 后 / 审批插入后广播至同账号在线他端
//! - 配额满时老终端重登不受门(无净增行)

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

/// 走完整 verify;返回原始响应。
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

/// 直入形态注册(预置账号;无 WS 在场时恒 ok)。
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

fn device_ids(state: &Arc<signaling::AppState>, email: &str) -> Vec<String> {
    let id = account_id_of(state, email);
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT device_id FROM devices WHERE account_id=?1 ORDER BY device_id")
        .unwrap();
    let rows = stmt
        .query_map([&id], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows
}

fn last_seen_of(state: &Arc<signaling::AppState>, device_id: &str) -> i64 {
    state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT last_seen FROM devices WHERE device_id=?1",
            [device_id],
            |r| r.get(0),
        )
        .unwrap()
}

/// 连 WS + hello,断言 HelloOk,返回流。
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

/// 连 WS + hello(不断言结果),返回首个服务器帧。
async fn hello_first_frame(addr: &std::net::SocketAddr, jwt: &str, dev: &str) -> WsServerFrame {
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
    recv_frame(&mut ws).await
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

/// 断言流已收口(Close 帧 / None / 传输错皆算)。
async fn assert_stream_closed(ws: &mut Ws) {
    for _ in 0..50 {
        match ws.next().await {
            None => return,
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => return,
            Some(Err(_)) => return,
            Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_))) => continue,
            Some(Ok(other)) => panic!("kick 后流应收口,却来了 {other:?}"),
        }
    }
    panic!("kick 后流未收口");
}

async fn poll(addr: &std::net::SocketAddr, session_id: &str) -> (reqwest::StatusCode, Value) {
    post(addr, "/auth/login-status", json!({ "session_id": session_id })).await
}

// ---------- 配额三路径同门一致(limit→evict→ok 全链参数化) ----------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Path {
    /// email 直入(快照空自动受信,verify 响应即 limit 形态)
    DirectEmail,
    /// 审批 approved(受信端输码 settle 后轮询得 limit 形态)
    AfterApproval,
    /// 占位扫码路径:T6 将直呼同一 enroll_device,这里直呼同函数证同门
    PlaceholderQr,
}

/// 三路径共用全链:limit 形态 → evict 踢旧 → ok 带票 → 旧端 401/重连拒 → token 单次。
async fn limit_evict_ok_chain(kind: Path) {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 1).unwrap();
    // dev1 占满唯一名额
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let account_id = account_id_of(&state, "q@x.y");
    let dev1_id = dev1["device_id"].as_str().unwrap().to_string();

    // dev2 撞配额门
    let token = match kind {
        Path::DirectEmail => {
            let code = request_code(&addr, "q@x.y").await;
            let (st, v) =
                verify_raw(&addr, "q@x.y", &code, &pubkey_hex([2u8; 32]), "newcomer").await;
            assert_eq!(st, reqwest::StatusCode::OK);
            assert_eq!(v["status"], "limit", "[{kind:?}] 直入撞门须 limit 形态: {v}");
            assert!(v["jwt"].is_null(), "[{kind:?}] limit 形态不带票: {v}");
            // 清单=现有 dev1(离线),不插 dev2 行
            let devices = v["devices"].as_array().unwrap();
            assert_eq!(devices.len(), 1, "[{kind:?}] 清单须只含现有设备: {v}");
            assert_eq!(devices[0]["device_id"], dev1["device_id"]);
            assert_eq!(devices[0]["name"], "victim");
            assert_eq!(devices[0]["online"], false);
            let ids = device_ids(&state, "q@x.y");
            assert_eq!(ids, vec![dev1_id.clone()], "[{kind:?}] 撞门不得插行");
            v["enroll_token"].as_str().unwrap().to_string()
        }
        Path::AfterApproval => {
            let ws1 = connect_trusted(&addr, dev1["jwt"].as_str().unwrap(), &dev1_id).await;
            let code = request_code(&addr, "q@x.y").await;
            let (_, v) =
                verify_raw(&addr, "q@x.y", &code, &pubkey_hex([2u8; 32]), "newcomer").await;
            assert_eq!(v["status"], "approval", "[{kind:?}] 在线受信端在场须审批: {v}");
            let sid = v["session_id"].as_str().unwrap().to_string();
            let mut ws1 = ws1;
            let n = recv_notice(&mut ws1).await;
            let (st, _) = post(
                &addr,
                "/auth/approve-code",
                json!({ "session_id": sid, "code": n.code }),
            )
            .await;
            assert_eq!(st, reqwest::StatusCode::OK);
            let (_, v) = poll(&addr, &sid).await;
            assert_eq!(v["status"], "limit", "[{kind:?}] approved 后撞门须 limit: {v}");
            let ids = device_ids(&state, "q@x.y");
            assert_eq!(ids, vec![dev1_id.clone()], "[{kind:?}] 撞门不得插行");
            v["enroll_token"].as_str().unwrap().to_string()
        }
        Path::PlaceholderQr => {
            let outcome = signaling::auth::enroll_device(
                &state,
                &account_id,
                &pubkey_hex([2u8; 32]),
                "newcomer",
                "test",
            )
            .unwrap();
            match outcome {
                signaling::auth::EnrollOutcome::Limit { enroll_token, devices } => {
                    assert_eq!(devices.len(), 1, "[{kind:?}] 清单须只含现有设备");
                    assert_eq!(devices[0].device_id, dev1_id);
                    assert!(!devices[0].online);
                    enroll_token
                }
                other => panic!("[{kind:?}] 占位扫码路径须 Limit: {other:?}"),
            }
        }
    };
    assert!(!token.is_empty(), "[{kind:?}] enroll_token 须非空");

    // evict:删 dev1 行 + 铸 dev2 票 → ok 形态
    let (st, v) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token, "device_id": dev1_id }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "[{kind:?}] evict 应 ok: {v}");
    assert_eq!(v["status"], "ok", "[{kind:?}] evict 应 ok 形态: {v}");
    assert!(v["jwt"].is_string() && v["refresh"].is_string(), "[{kind:?}] evict 须带票: {v}");
    assert_eq!(v["account_id"], account_id, "[{kind:?}] evict 票须归本账号");
    let dev2_id = v["device_id"].as_str().unwrap().to_string();
    assert_ne!(dev2_id, dev1_id, "[{kind:?}] 新票须是新设备身份");
    assert_eq!(
        device_ids(&state, "q@x.y"),
        vec![dev2_id.clone()],
        "[{kind:?}] evict 后目录只有新设备"
    );

    // 被踢设备 refresh → 401
    let (st, _) = post(&addr, "/auth/refresh", json!({ "refresh": dev1["refresh"] })).await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "[{kind:?}] 被踢设备 refresh 须 401");

    // 新设备用票上线 → HelloOk
    let mut _ws2 = connect_trusted(&addr, v["jwt"].as_str().unwrap(), &dev2_id).await;

    // 被踢设备重连 → code 1 device not bound
    let f = hello_first_frame(&addr, dev1["jwt"].as_str().unwrap(), &dev1_id).await;
    assert!(
        matches!(&f.frame, Some(ws_server_frame::Frame::Error(e))
            if e.code == 1 && e.message == "device not bound to account"),
        "[{kind:?}] 被踢设备重连须 code1 device not bound: {f:?}"
    );

    // token 单次:重放 401
    let (st, _) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token, "device_id": dev1_id }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "[{kind:?}] token 重放须 401");
}

#[tokio::test]
async fn quota_direct_email_path_limit_evict_ok() {
    limit_evict_ok_chain(Path::DirectEmail).await;
}

#[tokio::test]
async fn quota_after_approval_path_limit_evict_ok() {
    limit_evict_ok_chain(Path::AfterApproval).await;
}

#[tokio::test]
async fn quota_placeholder_qr_path_same_gate() {
    limit_evict_ok_chain(Path::PlaceholderQr).await;
}

/// 配额满时老终端重登不受门:同 pubkey 重 verify 是 upsert(无净增行),恒 ok。
#[tokio::test]
async fn old_terminal_relogin_at_full_quota_still_ok() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 1).unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let code = request_code(&addr, "q@x.y").await;
    let (st, v) = verify_raw(&addr, "q@x.y", &code, &pubkey_hex([1u8; 32]), "victim").await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "ok", "满员老终端重登须直入 ok: {v}");
    assert_eq!(v["device_id"], dev1["device_id"], "同 pubkey 同设备身份");
    assert_eq!(device_ids(&state, "q@x.y").len(), 1, "重登不得增行");
}

// ---------- evict 错误面 / 离线踢 / 在线踢 ----------

/// token 未知/过期一律 401(expire 注入沿既有 _expire_all_*_for_test 先例)。
#[tokio::test]
async fn evict_rejects_unknown_and_expired_tokens() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 1).unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let (st, _) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": "ffffffff", "device_id": dev1["device_id"] }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "未知 token 须 401");

    let (_, v) = post(
        &addr,
        "/auth/verify",
        json!({
            "email": "q@x.y", "code": request_code(&addr, "q@x.y").await,
            "pubkey": pubkey_hex([2u8; 32]), "name": "newcomer", "platform": "test"
        }),
    )
    .await;
    assert_eq!(v["status"], "limit");
    let token = v["enroll_token"].as_str().unwrap().to_string();
    signaling::auth::_expire_all_enroll_tokens_for_test(&state);
    let (st, _) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token, "device_id": dev1["device_id"] }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "过期 token 须 401");
}

/// 自踢不可(limit 清单里没有自己——尚无行)与未知目标同判 404;离线目标照常
/// ok(删行铸票,无 WS 可踢)。token 先消费后验目标:坏目标烧票(fail-closed)。
#[tokio::test]
async fn evict_unknown_target_404_and_offline_target_ok() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 1).unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let account_id = account_id_of(&state, "q@x.y");

    // token1:未知目标(dev1 之外)→ 404
    let outcome = signaling::auth::enroll_device(
        &state,
        &account_id,
        &pubkey_hex([2u8; 32]),
        "newcomer",
        "test",
    )
    .unwrap();
    let token1 = match outcome {
        signaling::auth::EnrollOutcome::Limit { enroll_token, .. } => enroll_token,
        other => panic!("须 Limit: {other:?}"),
    };
    let (st, v) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token1, "device_id": "f".repeat(32) }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::NOT_FOUND, "未知目标须 404: {v}");
    assert_eq!(device_ids(&state, "q@x.y").len(), 1, "404 不得动目录");
    // 烧掉的 token1 不可再用
    let (st, _) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token1, "device_id": dev1["device_id"] }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "坏目标烧票(fail-closed)");

    // token2:离线 dev1 → ok(无 WS 可踢,链路照常走完)
    let outcome = signaling::auth::enroll_device(
        &state,
        &account_id,
        &pubkey_hex([3u8; 32]),
        "third",
        "test",
    )
    .unwrap();
    let token2 = match outcome {
        signaling::auth::EnrollOutcome::Limit { enroll_token, .. } => enroll_token,
        other => panic!("须 Limit: {other:?}"),
    };
    let (st, v) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token2, "device_id": dev1["device_id"] }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "离线踢须 ok: {v}");
    let dev3_id = v["device_id"].as_str().unwrap().to_string();
    assert_eq!(device_ids(&state, "q@x.y"), vec![dev3_id]);
}

/// 即时踢:在线被踢端先收 device_evicted 临别帧、再 Error code=9,随后流收口。
#[tokio::test]
async fn evict_kicks_online_device_with_notice_then_code9() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 1).unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let dev1_id = dev1["device_id"].as_str().unwrap().to_string();
    let mut ws1 = connect_trusted(&addr, dev1["jwt"].as_str().unwrap(), &dev1_id).await;
    assert!(state.presence.is_online(&dev1_id));

    let account_id = account_id_of(&state, "q@x.y");
    let outcome = signaling::auth::enroll_device(
        &state,
        &account_id,
        &pubkey_hex([2u8; 32]),
        "newcomer",
        "test",
    )
    .unwrap();
    let token = match outcome {
        signaling::auth::EnrollOutcome::Limit { enroll_token, .. } => enroll_token,
        other => panic!("须 Limit: {other:?}"),
    };
    let (st, v) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token, "device_id": dev1_id }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "evict 应 ok: {v}");

    // 临别帧次序:device_evicted → Error code=9 → 流收口
    let n = recv_notice(&mut ws1).await;
    assert_eq!(n.kind, "device_evicted", "被踢端先收临别通告: {n:?}");
    let f = recv_frame(&mut ws1).await;
    assert!(
        matches!(&f.frame, Some(ws_server_frame::Frame::Error(e))
            if e.code == 9 && e.message == "device unbound"),
        "临别通告后须 Error code=9: {f:?}"
    );
    assert_stream_closed(&mut ws1).await;
    // 注册表即摘(不等 keepalive)
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while state.presence.is_online(&dev1_id) {
        assert!(std::time::Instant::now() < deadline, "被踢设备注册未摘");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// devices_changed 在 evict 后广播至同账号在线他端(kick 摘注册不发 offline,
/// 他端恰好只收一条 devices_changed)。
#[tokio::test]
async fn devices_changed_broadcast_after_evict_reaches_other_online_device() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 2).unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let dev2 = register(&state, &addr, "q@x.y", [2u8; 32], "watcher").await;
    let dev1_id = dev1["device_id"].as_str().unwrap().to_string();
    let mut ws1 = connect_trusted(&addr, dev1["jwt"].as_str().unwrap(), &dev1_id).await;
    let mut ws2 = connect_trusted(&addr, dev2["jwt"].as_str().unwrap(), dev2["device_id"].as_str().unwrap()).await;
    // ws1 收到 ws2 上线 presence(清掉,免混入后续断言面)
    let _ = recv_frame(&mut ws1).await;

    let account_id = account_id_of(&state, "q@x.y");
    let outcome = signaling::auth::enroll_device(
        &state,
        &account_id,
        &pubkey_hex([3u8; 32]),
        "newcomer",
        "test",
    )
    .unwrap();
    let token = match outcome {
        signaling::auth::EnrollOutcome::Limit { enroll_token, devices } => {
            // 清单两台都在线
            assert_eq!(devices.len(), 2);
            assert!(devices.iter().all(|d| d.online), "清单在线态须实时: {devices:?}");
            enroll_token
        }
        other => panic!("须 Limit: {other:?}"),
    };
    let (st, v) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": token, "device_id": dev1_id }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "evict 应 ok: {v}");

    let n = recv_notice(&mut ws2).await;
    assert_eq!(n.kind, "devices_changed", "他端须收 devices_changed: {n:?}");
    assert_eq!(n.session_id, "", "devices_changed 无载荷");
}

/// 审批插入也广播:dev2 获批入会后 dev1 收 join_request + devices_changed 两条。
#[tokio::test]
async fn devices_changed_broadcast_on_approval_insert() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 5).unwrap();
    let a = register(&state, &addr, "q@x.y", [1u8; 32], "ta").await;
    let mut ws1 =
        connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let code = request_code(&addr, "q@x.y").await;
    let (_, v) = verify_raw(&addr, "q@x.y", &code, &pubkey_hex([2u8; 32]), "tb").await;
    assert_eq!(v["status"], "approval");
    let sid = v["session_id"].as_str().unwrap().to_string();
    let n1 = recv_notice(&mut ws1).await;
    assert_eq!(n1.kind, "join_request");
    let (st, _) = post(
        &addr,
        "/auth/approve-code",
        json!({ "session_id": sid, "code": n1.code }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);
    let (_, v) = poll(&addr, &sid).await;
    assert_eq!(v["status"], "ok", "获批轮询须 ok: {v}");
    let n2 = recv_notice(&mut ws1).await;
    assert_eq!(n2.kind, "devices_changed", "插入后他端须收 devices_changed: {n2:?}");
}

// ---------- 懒踢(admin unbind / user_remove 级联) ----------

/// 压缩 ping 间隔的测试服务器(懒踢 ≤2 tick 断)。
async fn spawn_fast_tick_server() -> (std::net::SocketAddr, Arc<signaling::AppState>) {
    signaling::spawn_test_server_with(
        signaling::ws::Keepalive {
            ping_interval: Duration::from_millis(50),
            silent_limit: Duration::from_secs(10),
        },
        Vec::new(),
    )
    .await
}

/// admin unbind(纯删行,跨进程无直踢)→ keepalive 绑定复核懒踢:
/// ≤2 压缩 tick 内 Error code=9 且流收口。
#[tokio::test]
async fn admin_unbind_lazy_kicks_within_two_ticks() {
    let (addr, state) = spawn_fast_tick_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let dev1_id = dev1["device_id"].as_str().unwrap().to_string();
    let mut ws1 = connect_trusted(&addr, dev1["jwt"].as_str().unwrap(), &dev1_id).await;

    assert!(signaling::admin::device_unbind(&state.db.lock().unwrap(), "q@x.y", &dev1_id).unwrap());

    let f = recv_frame(&mut ws1).await;
    assert!(
        matches!(&f.frame, Some(ws_server_frame::Frame::Error(e))
            if e.code == 9 && e.message == "device unbound"),
        "懒踢须 Error code=9: {f:?}"
    );
    assert_stream_closed(&mut ws1).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while state.presence.is_online(&dev1_id) {
        assert!(std::time::Instant::now() < deadline, "懒踢后注册未清");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// user_remove 级联删全部设备行 → 全账号在线端逐一懒踢。
#[tokio::test]
async fn user_remove_cascade_lazy_kicks_all_online() {
    let (addr, state) = spawn_fast_tick_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 3).unwrap();
    let a = register(&state, &addr, "q@x.y", [1u8; 32], "ta").await;
    let b = register(&state, &addr, "q@x.y", [2u8; 32], "tb").await;
    let mut ws_a = connect_trusted(&addr, a["jwt"].as_str().unwrap(), a["device_id"].as_str().unwrap()).await;
    let mut ws_b = connect_trusted(&addr, b["jwt"].as_str().unwrap(), b["device_id"].as_str().unwrap()).await;
    // ws_a 收到 b 上线 presence(清掉,免混入懒踢断言面)
    let f = recv_frame(&mut ws_a).await;
    assert!(matches!(&f.frame, Some(ws_server_frame::Frame::Presence(p)) if p.online));

    assert!(signaling::admin::user_remove(&state.db.lock().unwrap(), "q@x.y").unwrap());

    for (ws, tag) in [(&mut ws_a, "a"), (&mut ws_b, "b")] {
        // 各端 tick 懒踢前可能先收到他端清理的 offline presence 广播(先断的
        // 那端 fallthrough 清理照常广播),跳过非目标帧直到本端 code=9
        let f = loop {
            let f = recv_frame(ws).await;
            if matches!(&f.frame,
                Some(ws_server_frame::Frame::Error(e)) if e.code == 9)
            {
                break f;
            }
        };
        assert!(
            matches!(&f.frame, Some(ws_server_frame::Frame::Error(e))
                if e.code == 9 && e.message == "device unbound"),
            "user_remove 级联懒踢 {tag} 须 code=9: {f:?}"
        );
        assert_stream_closed(ws).await;
    }
}

// ---------- last_seen 滚动 ----------

/// presence 注册写 last_seen(注册前 0),keepalive tick 滚动续写(过秒界递增)。
#[tokio::test]
async fn last_seen_written_on_presence_and_rolled_by_tick() {
    let (addr, state) = spawn_fast_tick_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "d1").await;
    let dev1_id = dev1["device_id"].as_str().unwrap().to_string();
    assert_eq!(last_seen_of(&state, &dev1_id), 0, "注册前 last_seen=0");

    let _ws1 = connect_trusted(&addr, dev1["jwt"].as_str().unwrap(), &dev1_id).await;
    let at_connect = last_seen_of(&state, &dev1_id);
    assert!(at_connect > 0, "presence 注册须写 last_seen");

    // 压缩 tick 滚动:now() 秒级精度,跨过 1 秒界后必见递增
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let rolled = last_seen_of(&state, &dev1_id);
    assert!(rolled > at_connect, "tick 须滚动 last_seen: {at_connect} -> {rolled}");
}
