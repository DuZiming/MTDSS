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
    // 认证收口后仅预置账号可领码(下同,各登录点先预置)
    signaling::admin::user_add(&state.db.lock().unwrap(), "a@b.c").unwrap();
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
    signaling::admin::user_add(&state.db.lock().unwrap(), "x@y.z").unwrap();
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
    signaling::admin::user_add(&state.db.lock().unwrap(), "e@f.g").unwrap();
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

// ===== 认证收口(v2-P1a):未预置/已删账号的登录路径全部拒绝 =====

#[tokio::test]
async fn unknown_email_gets_404() {
    let state = signaling::test_state().await;
    // 未预置邮箱:request-code 必须 404,不得发码
    let (code, _) = post(&state, "/auth/request-code", json!({"email": "ghost@x.y"})).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn verify_never_autocreates() {
    let state = signaling::test_state().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "vanish@x.y").unwrap();
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "vanish@x.y"})).await;
    let dev_code = v["code"].as_str().unwrap().to_string();
    // user_remove 会级联清掉 codes;手工补回一条有效验证码,精确命中
    // "码有效但账号已删"的窗口——否则码校验先挂,测不到建号分支
    assert!(signaling::admin::user_remove(&state.db.lock().unwrap(), "vanish@x.y").unwrap());
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(dev_code.as_bytes());
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO codes(email, code_hash, expires_at) VALUES(?1,?2,?3)",
            rusqlite::params![
                "vanish@x.y",
                hex::encode(h.finalize()),
                signaling::db::now() + 600
            ],
        )
        .unwrap();
    let (code, _) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "vanish@x.y", "code": dev_code,
            "pubkey": "00".repeat(32), "name": "d", "platform": "test"
        }),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    // 库里不得出现该账号(验证码流程不得重建已删账号)
    let recreated = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT 1 FROM accounts WHERE email=?1",
            ["vanish@x.y"],
            |r| r.get::<_, i64>(0),
        )
        .is_ok();
    assert!(!recreated);
}

#[tokio::test]
async fn refresh_after_removal_rejected() {
    let state = signaling::test_state().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "rm@x.y").unwrap();
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "rm@x.y"})).await;
    let (_, ok) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "rm@x.y", "code": v["code"].as_str().unwrap(),
            "pubkey": "00".repeat(32), "name": "d", "platform": "test"
        }),
    )
    .await;
    let refresh = ok["refresh"].as_str().unwrap().to_string();
    // 账号被删:长期 refresh token 不得再换新凭据
    assert!(signaling::admin::user_remove(&state.db.lock().unwrap(), "rm@x.y").unwrap());
    let (code, _) = post(&state, "/auth/refresh", json!({"refresh": refresh})).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn removed_account_cannot_get_new_code() {
    let state = signaling::test_state().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "dead@x.y").unwrap();
    assert!(signaling::admin::user_remove(&state.db.lock().unwrap(), "dead@x.y").unwrap());
    // 已删账号再领码:同未知邮箱,404
    let (code, _) = post(&state, "/auth/request-code", json!({"email": "dead@x.y"})).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}
