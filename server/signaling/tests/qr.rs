//! 扫码登录集成面(V3-1 Task 6,spec §3.2/§3.10):
//! - 四端点全链:qr-begin(128bit token+120s)→ qr-claim(首认领者钉死/同认领者
//!   幂等/他者 409)→ qr-approve(须=claimant;allow→enroll 收口;拒→denied)→
//!   GET qr-status(pending|ok|denied|expired|limit)
//! - token 单次:status 领票消费后重放恒终态不重票;approve 后重放=终态 409
//! - 401 面:claim/approve 须 access 型 Bearer;403 面:非认领者 approve
//! - 过期:status=expired、claim=404、approve=409;未知 token:status 按
//!   expired 终态呈现(轮询面无 404 通道)、claim/approve=404
//! - limit 路径经同一 enroll_device 收口 → PC 以 T5 evict 面完成踢选
//! - 限流:qr-begin 10/h、claim/approve 30/min、qr-status 120/min(per-IP)

use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::sync::Arc;

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

async fn post_bearer_full(
    addr: &std::net::SocketAddr,
    path: &str,
    body: Value,
    bearer: &str,
) -> (reqwest::StatusCode, reqwest::header::HeaderMap, Value) {
    let resp = client()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap();
    let st = resp.status();
    let headers = resp.headers().clone();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (st, headers, v)
}

async fn get_status(addr: &std::net::SocketAddr, token: &str) -> (reqwest::StatusCode, Value) {
    let resp = client()
        .get(format!("http://{addr}/auth/qr-status"))
        .query(&[("t", token)])
        .send()
        .await
        .unwrap();
    let st = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (st, v)
}

async fn get_status_full(
    addr: &std::net::SocketAddr,
    token: &str,
) -> (reqwest::StatusCode, reqwest::header::HeaderMap, Value) {
    let resp = client()
        .get(format!("http://{addr}/auth/qr-status"))
        .query(&[("t", token)])
        .send()
        .await
        .unwrap();
    let st = resp.status();
    let headers = resp.headers().clone();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    (st, headers, v)
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

/// qr-begin 便捷封装。
async fn qr_begin(
    addr: &std::net::SocketAddr,
    seed: [u8; 32],
    name: &str,
    platform: &str,
) -> (reqwest::StatusCode, Value) {
    post(
        addr,
        "/auth/qr-begin",
        json!({ "pubkey": pubkey_hex(seed), "name": name, "platform": platform }),
    )
    .await
}

// ---------- 全链:begin → claim → approve → status ok 带票(单次) ----------

/// 全链:pending 轮询 → claim 确认弹层数据 → approve → status ok 带票入
/// claimant 账号;单次:再轮询=终态不重票,token 重放(再 approve)=409 终态。
#[tokio::test]
async fn qr_full_chain_claim_approve_status_ok_once() {
    let (addr, state) = spawn_test_server().await;
    let phone = register(&state, &addr, "fam@x.y", [1u8; 32], "phone").await;

    // begin:128bit=32 hex token + expires_at≈now+120
    let (st, b) = qr_begin(&addr, [3u8; 32], "win-pc", "windows").await;
    assert_eq!(st, reqwest::StatusCode::OK, "{b}");
    let token = b["qr_token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 32, "128bit token=32 hex: {token}");
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    let exp = b["expires_at"].as_i64().unwrap();
    let now = signaling::db::now();
    assert!(exp > now && exp <= now + 121, "expires_at≈now+120: {exp}");

    // 未认领轮询 = pending
    let (st, v) = get_status(&addr, &token).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "pending", "{v}");

    // claim:确认弹层数据(candidate name/platform)+ token 时限
    let (st, v) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "{v}");
    assert_eq!(v["candidate"]["name"], "win-pc", "{v}");
    assert_eq!(v["candidate"]["platform"], "windows", "{v}");
    assert_eq!(v["expires_at"], exp, "claim 回同一 token 时限: {v}");

    // 认领后未裁决仍 = pending
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "pending", "{v}");

    // approve allow=true
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    // status ok 带票,入 claimant 账号
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "ok", "{v}");
    assert!(v["jwt"].as_str().is_some() && v["refresh"].as_str().is_some(), "{v}");
    assert!(v["device_id"].as_str().is_some(), "{v}");
    assert_eq!(v["account_id"], account_id_of(&state, "fam@x.y"), "{v}");
    assert_eq!(device_count(&state, "fam@x.y"), 2, "扫码获批须插入设备行");

    // 单次:再轮询=终态不重票;token 重放(再 approve)=终态 409;终态稳定
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "expired", "已消费 token 再轮询须终态: {v}");
    assert!(v["jwt"].is_null(), "终态不得重发票: {v}");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "done 后再 approve 须终态 409");
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "expired", "终态稳定");
}

// ---------- claim 钉死/幂等/他者 409 + approve 须=claimant ----------

/// 首认领者钉死:同认领者重 claim 幂等;他者(同账号他端/跨账号)claim 409;
/// 未认领与非认领者 approve 403;认领者裁决照常放行。
#[tokio::test]
async fn qr_claim_first_wins_idempotent_and_approve_needs_claimant() {
    let (addr, state) = spawn_test_server().await;
    let a = register(&state, &addr, "fam@x.y", [1u8; 32], "phone-a").await;
    let b = register(&state, &addr, "fam@x.y", [2u8; 32], "phone-b").await;
    let x = register(&state, &addr, "x@y.z", [9u8; 32], "other-acct").await;

    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();

    // 未认领时 approve:即便合法 Bearer 也非认领者 → 403
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::FORBIDDEN, "未认领 approve 须 403(无认领者)");

    // 首认领者钉死
    let (st, v) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "{v}");
    assert_eq!(v["candidate"]["name"], "pc", "{v}");

    // 同认领者幂等:同一候选面
    let (st, v2) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "同认领者重 claim 须幂等: {v2}");
    assert_eq!(v2["candidate"], v["candidate"], "幂等回同一候选面");

    // 他者 claim(同账号他端 / 跨账号)→ 409
    for other in [b["jwt"].as_str().unwrap(), x["jwt"].as_str().unwrap()] {
        let (st, _) = post_bearer(&addr, "/auth/qr-claim", json!({ "qr_token": token }), other).await;
        assert_eq!(st, reqwest::StatusCode::CONFLICT, "他者认领须 409");
    }

    // 非认领者 approve → 403(同账号他端与跨账号同判),不得入会
    for other in [b["jwt"].as_str().unwrap(), x["jwt"].as_str().unwrap()] {
        let (st, _) = post_bearer(
            &addr,
            "/auth/qr-approve",
            json!({ "qr_token": token, "allow": true }),
            other,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::FORBIDDEN, "非认领者 approve 须 403");
    }
    assert_eq!(device_count(&state, "fam@x.y"), 2, "被拒裁决不得入会");

    // 认领者裁决放行 → ok(默认上限 3 内:a+b+pc)
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        a["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "ok", "{v}");
    assert_eq!(device_count(&state, "fam@x.y"), 3);
}

// ---------- 401 面:claim/approve 须 access 型 Bearer ----------

#[tokio::test]
async fn qr_claim_and_approve_require_access_bearer() {
    let (addr, state) = spawn_test_server().await;
    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();
    let refresh = signaling::auth::mint_refresh_jwt(&state.jwt_secret, "a", "d");

    // claim:无 Bearer / 坏票 / refresh 型票一律 401
    let (st, _) = post(&addr, "/auth/qr-claim", json!({ "qr_token": token })).await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "无 Bearer claim 须 401");
    let (st, _) = post_bearer(&addr, "/auth/qr-claim", json!({ "qr_token": token }), "bogus").await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "坏票 claim 须 401");
    let (st, _) = post_bearer(&addr, "/auth/qr-claim", json!({ "qr_token": token }), &refresh).await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "refresh 型票不得认领");

    // approve 同面
    let (st, _) = post(&addr, "/auth/qr-approve", json!({ "qr_token": token, "allow": true })).await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "无 Bearer approve 须 401");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        "bogus",
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "坏票 approve 须 401");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        &refresh,
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED, "refresh 型票不得裁决");
}

// ---------- allow=false → denied 终态 ----------

#[tokio::test]
async fn qr_allow_false_denies_terminal() {
    let (addr, state) = spawn_test_server().await;
    let phone = register(&state, &addr, "fam@x.y", [1u8; 32], "phone").await;
    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": false }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "denied", "{v}");
    assert_eq!(device_count(&state, "fam@x.y"), 1, "denied 不得插设备行");

    // denied 终态:再 approve 409、轮询恒 denied、claim 通道关闭
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "denied 后再 approve 须终态 409");
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "denied");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "settled 后认领须 409");
}

// ---------- 过期 / 未知 token ----------

/// 过期(注入 expires_at=0,T4/T5 同款先例):status=expired、claim=404、
/// approve=409(与审批机 approve-code/respond 过期同判),不得入会。
#[tokio::test]
async fn qr_expired_token_terminal_everywhere() {
    let (addr, state) = spawn_test_server().await;
    let phone = register(&state, &addr, "fam@x.y", [1u8; 32], "phone").await;
    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();

    signaling::auth::_expire_all_qr_tokens_for_test(&state);

    let (st, v) = get_status(&addr, &token).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "expired", "TTL 过后轮询须 expired: {v}");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::NOT_FOUND, "过期 token 认领须 404(QR 已死)");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::CONFLICT, "过期 token approve 须 409");
    assert_eq!(device_count(&state, "fam@x.y"), 1);
    // 惰性落库后稳定:再轮询仍 expired
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "expired");
}

/// 未知 token:status 按终态 expired 呈现(轮询面无 404 通道,login-status
/// 同款);claim/approve 404。
#[tokio::test]
async fn qr_unknown_token_handling() {
    let (addr, state) = spawn_test_server().await;
    let phone = register(&state, &addr, "fam@x.y", [1u8; 32], "phone").await;
    let ghost = "ffffffffffffffffffffffffffffffff";
    let (st, v) = get_status(&addr, ghost).await;
    assert_eq!(st, reqwest::StatusCode::OK);
    assert_eq!(v["status"], "expired", "未知 token 按终态 expired 呈现: {v}");
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": ghost }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::NOT_FOUND);
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": ghost, "allow": true }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::NOT_FOUND);
}

/// qr-begin 候选面先验形:非法 pubkey(fail-closed 提前到入口)→ 400。
#[tokio::test]
async fn qr_begin_rejects_malformed_pubkey() {
    let (addr, _state) = spawn_test_server().await;
    let (st, _) = post(
        &addr,
        "/auth/qr-begin",
        json!({ "pubkey": "deadbeef", "name": "x", "platform": "y" }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::BAD_REQUEST, "非 32 字节/非法曲线点须 400");
}

// ---------- limit 路径经同一 enroll 收口 → evict 踢选完成 ----------

/// 上限 2 账号(victim+phone 占满)扫码 approve → status limit(单次
/// enroll_token+现有清单,不插行)→ PC 走 T5 evict 面删 victim → ok 带票;
/// status 再轮询恒终态不重票。
#[tokio::test]
async fn qr_limit_path_through_enroll_gate_then_evict_ok() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "q@x.y").unwrap();
    signaling::admin::user_set_device_limit(&state.db.lock().unwrap(), "q@x.y", 2).unwrap();
    let dev1 = register(&state, &addr, "q@x.y", [1u8; 32], "victim").await;
    let dev1_id = dev1["device_id"].as_str().unwrap().to_string();
    let phone = register(&state, &addr, "q@x.y", [2u8; 32], "phone").await;
    let account_id = account_id_of(&state, "q@x.y");

    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-claim",
        json!({ "qr_token": token }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);
    let (st, _) = post_bearer(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        phone["jwt"].as_str().unwrap(),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK);

    // status limit:同一 enroll 收口回单次踢选凭据+现有清单,不插行不带票
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "limit", "{v}");
    assert!(v["jwt"].is_null(), "limit 形态不带票: {v}");
    let etoken = v["enroll_token"].as_str().unwrap().to_string();
    assert!(!etoken.is_empty(), "enroll_token 须非空");
    let devices = v["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2, "清单=现有两台: {v}");
    assert_eq!(devices[0]["device_id"], dev1["device_id"], "{v}");
    assert_eq!(devices[0]["online"], false, "{v}");
    assert_eq!(device_count(&state, "q@x.y"), 2, "撞门不得插行");

    // PC 踢选完成(T5 既有 evict 面):删 victim + 铸票 → ok
    let (st, v) = post(
        &addr,
        "/auth/evict",
        json!({ "enroll_token": etoken, "device_id": dev1_id }),
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::OK, "{v}");
    assert_eq!(v["status"], "ok", "{v}");
    assert!(v["jwt"].as_str().is_some() && v["refresh"].as_str().is_some(), "{v}");
    assert_eq!(v["account_id"], account_id, "{v}");
    assert_eq!(device_count(&state, "q@x.y"), 2, "victim 换 pc");

    // 单次:status 再轮询恒终态不重票
    let (_, v) = get_status(&addr, &token).await;
    assert_eq!(v["status"], "expired", "{v}");
    assert!(v["enroll_token"].is_null(), "终态不得重发 enroll_token: {v}");
}

// ---------- 限流(per-IP,spec §3.10) ----------

/// qr-begin per-IP 10/h:第 11 次 429 + Retry-After。
#[tokio::test]
async fn qr_begin_throttled_10_per_hour() {
    let (addr, _state) = spawn_test_server().await;
    let body = json!({ "pubkey": pubkey_hex([3u8; 32]), "name": "pc", "platform": "windows" });
    for i in 0..10 {
        let (st, _) = post(&addr, "/auth/qr-begin", body.clone()).await;
        assert_ne!(
            st,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "第 {} 次不应 429(前 10 次额度内)",
            i + 1
        );
    }
    let (st, headers, _) = post_full(&addr, "/auth/qr-begin", body).await;
    assert_eq!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "第 11 次必须 429");
    let ra = headers
        .get("retry-after")
        .expect("429 必须带 Retry-After")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(ra >= 1);
}

/// claim/approve per-IP 30/min:第 31 次 429(claim 同认领者幂等面上计数,
/// approve 首 200 后余 409 终态,均不 429)。
#[tokio::test]
async fn qr_claim_and_approve_throttled_30_per_min() {
    let (addr, state) = spawn_test_server().await;
    let phone = register(&state, &addr, "fam@x.y", [1u8; 32], "phone").await;
    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();
    let jwt = phone["jwt"].as_str().unwrap();

    for i in 0..30 {
        let (st, _) = post_bearer(&addr, "/auth/qr-claim", json!({ "qr_token": token }), jwt).await;
        assert_ne!(
            st,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "claim 第 {} 次不应 429",
            i + 1
        );
    }
    let (st, _, _) = post_bearer_full(&addr, "/auth/qr-claim", json!({ "qr_token": token }), jwt).await;
    assert_eq!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "claim 第 31 次必须 429");

    for i in 0..30 {
        let (st, _) = post_bearer(
            &addr,
            "/auth/qr-approve",
            json!({ "qr_token": token, "allow": true }),
            jwt,
        )
        .await;
        assert_ne!(
            st,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "approve 第 {} 次不应 429",
            i + 1
        );
    }
    let (st, _, _) = post_bearer_full(
        &addr,
        "/auth/qr-approve",
        json!({ "qr_token": token, "allow": true }),
        jwt,
    )
    .await;
    assert_eq!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "approve 第 31 次必须 429");
}

/// qr-status per-IP 120/min:第 121 次 429(轮询兜底限速)。
#[tokio::test]
async fn qr_status_throttled_120_per_min() {
    let (addr, _state) = spawn_test_server().await;
    let (_, t) = qr_begin(&addr, [3u8; 32], "pc", "windows").await;
    let token = t["qr_token"].as_str().unwrap().to_string();
    for i in 0..120 {
        let (st, _) = get_status(&addr, &token).await;
        assert_ne!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "第 {} 次不应 429", i + 1);
    }
    let (st, _, _) = get_status_full(&addr, &token).await;
    assert_eq!(st, reqwest::StatusCode::TOO_MANY_REQUESTS, "第 121 次必须 429");
}
