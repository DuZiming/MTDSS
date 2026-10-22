use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use signaling::build_router;
use tower::ServiceExt;

async fn post(state: &std::sync::Arc<signaling::AppState>, path: &str, body: Value) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(Request::builder().method("POST").uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap())
        .await.unwrap();
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(state: &std::sync::Arc<signaling::AppState>, path: &str, jwt: &str) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(Request::builder().method("GET").uri(path)
            .header("authorization", format!("Bearer {jwt}"))
            .body(Body::empty()).unwrap())
        .await.unwrap();
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// 确定性测试公钥（hex）：由 SigningKey 派生，保证是合法 Ed25519 曲线点
/// ——服务端 verify 会 decompress 校验，任意 32 字节（如 [2u8;32]）会被拒。
fn pubkey_hex(seed_byte: u8) -> String {
    hex::encode(SigningKey::from_bytes(&[seed_byte; 32]).verifying_key().as_bytes())
}

async fn register_device(state: &std::sync::Arc<signaling::AppState>, email: &str, name: &str, seed_byte: u8) -> Value {
    // 认证收口后仅预置账号可领码:helper 统一预置(幂等,一处覆盖全部登录点)
    signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
    let (_, v) = post(state, "/auth/request-code", json!({"email": email})).await;
    post(state, "/auth/verify", json!({
        "email": email, "code": v["code"].as_str().unwrap(),
        "pubkey": pubkey_hex(seed_byte), "name": name, "platform": "test"
    })).await.1
}

#[tokio::test]
async fn devices_scoped_to_account_and_shape() {
    let state = signaling::test_state().await;
    let a = register_device(&state, "a@b.c", "dev-a", 0).await;
    let (code, v) = get(&state, "/devices", a["jwt"].as_str().unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    let list = v["devices"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"].as_str().unwrap(), "dev-a");
    assert!(!list[0]["online"].as_bool().unwrap());
    assert_eq!(list[0]["pubkey"].as_str().unwrap(), pubkey_hex(0));
    assert_eq!(list[0]["device_id"].as_str().unwrap(), a["device_id"].as_str().unwrap());
}

#[tokio::test]
async fn same_account_second_device_reauth_lists_both() {
    let state = signaling::test_state().await;
    let a1 = register_device(&state, "a@b.c", "dev-a", 0).await;
    // 同邮箱二次验证码流程注册第二台设备（不同 pubkey）
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "a@b.c"})).await;
    let a2 = post(&state, "/auth/verify", json!({
        "email": "a@b.c", "code": v["code"].as_str().unwrap(),
        "pubkey": pubkey_hex(1), "name": "dev-b", "platform": "test"
    })).await.1;
    assert_ne!(a1["device_id"], a2["device_id"]);
    let (_, v) = get(&state, "/devices", a1["jwt"].as_str().unwrap()).await;
    assert_eq!(v["devices"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn other_account_isolated() {
    // 两台设备（不同 pubkey => 不同 device_id）分属两个账号；同一 pubkey
    // 重注册会触发设备整体迁移账号（见 reregister_... 测试），不能当隔离
    // 用例的夹具。
    let state = signaling::test_state().await;
    let a = register_device(&state, "a@b.c", "dev-a", 0).await;
    let _b = register_device(&state, "x@y.z", "dev-x", 1).await;
    let (_, v) = get(&state, "/devices", a["jwt"].as_str().unwrap()).await;
    assert_eq!(v["devices"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn bad_jwt_rejected() {
    let state = signaling::test_state().await;
    let (code, _) = get(&state, "/devices", "not-a-jwt").await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn reregister_same_pubkey_under_new_email_moves_device() {
    // 同一身份（pubkey）先用邮箱 A 注册，再用邮箱 B 重走验证码流程：
    // 设备必须整体迁移到 B 的账号。回归场景：upsert 曾漏更新 account_id，
    // 导致新 JWT 的设备列表为空、WS hello 恒 code 1。
    let state = signaling::test_state().await;
    let pubkey = pubkey_hex(2);

    // 该测试绕过 register_device 直连,预置点在调用处
    signaling::admin::user_add(&state.db.lock().unwrap(), "old@x.y").unwrap();
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "old@x.y"})).await;
    let (fcode, first) = post(&state, "/auth/verify", json!({
        "email": "old@x.y", "code": v["code"].as_str().unwrap(),
        "pubkey": pubkey, "name": "dev-x", "platform": "test"
    })).await;
    assert_eq!(fcode, StatusCode::OK, "first verify failed: {first}");
    assert_eq!(first["device_id"].as_str().unwrap().len(), 32);
    // 迁移前：旧账号下能看到这台设备
    let (_, old_list) = get(&state, "/devices", first["jwt"].as_str().unwrap()).await;
    assert_eq!(old_list["devices"].as_array().unwrap().len(), 1);

    // 同一 pubkey 用邮箱 B 重走完整验证码流程
    signaling::admin::user_add(&state.db.lock().unwrap(), "new@x.y").unwrap();
    let (_, v2) = post(&state, "/auth/request-code", json!({"email": "new@x.y"})).await;
    let again = post(&state, "/auth/verify", json!({
        "email": "new@x.y", "code": v2["code"].as_str().unwrap(),
        "pubkey": pubkey, "name": "dev-x", "platform": "test"
    })).await.1;
    // 同一设备身份（device_id 由 pubkey 唯一决定）
    assert_eq!(again["device_id"], first["device_id"]);

    // 新 JWT 的账号必须列出这台设备（账号已干净迁移）
    let (code, moved) = get(&state, "/devices", again["jwt"].as_str().unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    let list = moved["devices"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["device_id"].as_str().unwrap(), first["device_id"].as_str().unwrap());
    assert_eq!(list[0]["pubkey"].as_str().unwrap(), pubkey);

    // 旧 JWT 的账号下不再有设备
    let (_, stale) = get(&state, "/devices", first["jwt"].as_str().unwrap()).await;
    assert_eq!(stale["devices"].as_array().unwrap().len(), 0);
}
