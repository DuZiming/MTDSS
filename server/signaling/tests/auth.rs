use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use signaling::build_router;
use tower::ServiceExt; // dev-dep tower with util

async fn post(
    state: &std::sync::Arc<signaling::AppState>,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn request_and_verify_flow() {
    let state = signaling::test_state().await;
    let (code, v) = post(&state, "/auth/request-code", json!({"email": "a@b.c"})).await;
    assert_eq!(code, StatusCode::OK);
    let dev_code = v["code"].as_str().unwrap().to_string();

    let (code, v) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "a@b.c", "code": dev_code,
            "pubkey": "00".repeat(32), "name": "dev1", "platform": "windows"
        }),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert!(v["jwt"].as_str().is_some());
    assert!(v["refresh"].as_str().is_some());
    assert_eq!(v["device_id"].as_str().unwrap().len(), 32);
}

#[tokio::test]
async fn wrong_code_rejected() {
    let state = signaling::test_state().await;
    let (_, _) = post(&state, "/auth/request-code", json!({"email": "x@y.z"})).await;
    let (code, _) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "x@y.z", "code": "000000",
            "pubkey": "00".repeat(32), "name": "d", "platform": "linux"
        }),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_code_rejected() {
    let state = signaling::test_state().await;
    signaling::auth::_expire_all_codes_for_test(&state);
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "e@f.g"})).await;
    let dev_code = v["code"].as_str().unwrap().to_string();
    signaling::auth::_expire_all_codes_for_test(&state);
    let (code, _) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "e@f.g", "code": dev_code,
            "pubkey": "00".repeat(32), "name": "d", "platform": "macos"
        }),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn jwt_roundtrip_and_refresh() {
    let state = signaling::test_state().await;
    let token = signaling::auth::mint_access_jwt(&state.jwt_secret, "acct1", "dev1");
    let claims = signaling::auth::verify_jwt(&state.jwt_secret, &token).unwrap();
    assert_eq!(claims.sub, "acct1");
    assert_eq!(claims.device, "dev1");
    let (code, _) = post(&state, "/auth/refresh", json!({"refresh": "bogus"})).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}
