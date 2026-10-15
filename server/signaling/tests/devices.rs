use axum::body::Body;
use axum::http::{Request, StatusCode};
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

async fn register_device(state: &std::sync::Arc<signaling::AppState>, email: &str, name: &str) -> Value {
    let (_, v) = post(state, "/auth/request-code", json!({"email": email})).await;
    post(state, "/auth/verify", json!({
        "email": email, "code": v["code"].as_str().unwrap(),
        "pubkey": hex::encode([0u8; 32]), "name": name, "platform": "test"
    })).await.1
}

#[tokio::test]
async fn devices_scoped_to_account_and_shape() {
    let state = signaling::test_state().await;
    let a = register_device(&state, "a@b.c", "dev-a").await;
    let (code, v) = get(&state, "/devices", a["jwt"].as_str().unwrap()).await;
    assert_eq!(code, StatusCode::OK);
    let list = v["devices"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"].as_str().unwrap(), "dev-a");
    assert_eq!(list[0]["online"].as_bool().unwrap(), false);
    assert_eq!(list[0]["pubkey"].as_str().unwrap(), hex::encode([0u8; 32]));
    assert_eq!(list[0]["device_id"].as_str().unwrap(), a["device_id"].as_str().unwrap());
}

#[tokio::test]
async fn same_account_second_device_reauth_lists_both() {
    let state = signaling::test_state().await;
    let a1 = register_device(&state, "a@b.c", "dev-a").await;
    // 同邮箱二次验证码流程注册第二台设备（不同 pubkey）
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "a@b.c"})).await;
    let a2 = post(&state, "/auth/verify", json!({
        "email": "a@b.c", "code": v["code"].as_str().unwrap(),
        "pubkey": hex::encode([1u8; 32]), "name": "dev-b", "platform": "test"
    })).await.1;
    assert_ne!(a1["device_id"], a2["device_id"]);
    let (_, v) = get(&state, "/devices", a1["jwt"].as_str().unwrap()).await;
    assert_eq!(v["devices"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn other_account_isolated() {
    let state = signaling::test_state().await;
    let a = register_device(&state, "a@b.c", "dev-a").await;
    let _b = register_device(&state, "x@y.z", "dev-x").await;
    let (_, v) = get(&state, "/devices", a["jwt"].as_str().unwrap()).await;
    assert_eq!(v["devices"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn bad_jwt_rejected() {
    let state = signaling::test_state().await;
    let (code, _) = get(&state, "/devices", "not-a-jwt").await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}
