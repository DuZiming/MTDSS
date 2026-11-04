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
async fn refresh_with_orphaned_device_rejected() {
    let state = signaling::test_state().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "orphan@x.y").unwrap();
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "orphan@x.y"})).await;
    let (_, ok) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "orphan@x.y", "code": v["code"].as_str().unwrap(),
            "pubkey": "00".repeat(32), "name": "d", "platform": "test"
        }),
    )
    .await;
    let refresh = ok["refresh"].as_str().unwrap().to_string();
    // 只删 accounts 行、保留 devices 行(绕过 user_remove 的设备级联):
    // devices 查询先过,rotate_refresh 的"设备在而账号已删"兜底分支才可达
    state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM accounts WHERE email=?1", ["orphan@x.y"])
        .unwrap();
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

// ===== 字段形收口(final review I1) =====

/// 带自定义头的变体(XFF 限流键面用)。
async fn post_with_headers(
    state: &std::sync::Arc<signaling::AppState>,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let resp = app
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// email >254 字节 → 400(request-code/verify 双面;先于限流与查库)。
#[tokio::test]
async fn oversized_email_rejected_400() {
    let state = signaling::test_state().await;
    let long_email = "a".repeat(255) + "@x.y";
    let (code, _) = post(&state, "/auth/request-code", json!({"email": long_email})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "request-code 超长 email 须 400");
    let (code, _) = post(
        &state,
        "/auth/verify",
        json!({
            "email": long_email, "code": "000000",
            "pubkey": "00".repeat(32), "name": "d", "platform": "test"
        }),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "verify 超长 email 须 400");
}

/// name/platform 超长截断入库(verify 面;字符边界不劈多字节 UTF-8)。
#[tokio::test]
async fn verify_truncates_name_and_platform() {
    let state = signaling::test_state().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "t@x.y").unwrap();
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "t@x.y"})).await;
    let long_name = "甲".repeat(40); // 120 字节
    let long_platform = "linux-".repeat(10); // 60 字节
    let (code, v) = post(
        &state,
        "/auth/verify",
        json!({
            "email": "t@x.y", "code": v["code"].as_str().unwrap(),
            "pubkey": "00".repeat(32), "name": long_name, "platform": long_platform
        }),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let device_id = v["device_id"].as_str().unwrap();
    let (name, platform): (String, String) = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT name, platform FROM devices WHERE device_id=?1",
            [device_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    // 64 字节截在字符边界:「甲」3 字节 → 21 字(63 字节)
    assert_eq!(name, "甲".repeat(21), "name 须截到 64 字节内(字符边界)");
    assert_eq!(platform, &long_platform[..32], "platform 须截到 32 字节");
}

/// XFF 末段为限流键(final review I1):首段轮换不绕过;键=反代写入的
/// 真实地址。走恒限速面 approve-code(30/min;echo 门只豁免 request-code):
/// 30 次同末段(任意首段)→ 第 31 次 429;换末段 → 不受累。
#[tokio::test]
async fn xff_last_segment_is_the_throttle_key() {
    let state = signaling::test_state().await;
    for i in 0..30 {
        let xff = format!("10.0.0.{i}, 5.5.5.5");
        let (code, _) = post_with_headers(
            &state,
            "/auth/approve-code",
            json!({"session_id": "s", "code": "123456"}),
            &[("x-forwarded-for", xff.as_str())],
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND, "第 {} 次未知会话 404(未限速)", i + 1);
    }
    // 首段继续轮换、末段不变 → 撞同一限流键 429
    let (code, _) = post_with_headers(
        &state,
        "/auth/approve-code",
        json!({"session_id": "s", "code": "123456"}),
        &[("x-forwarded-for", "10.9.9.9, 5.5.5.5")],
    )
    .await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "末段同键须限速");
    // 换末段(新键)→ 不受累,照常 404
    let (code, _) = post_with_headers(
        &state,
        "/auth/approve-code",
        json!({"session_id": "s", "code": "123456"}),
        &[("x-forwarded-for", "5.5.5.5, 6.6.6.6")],
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "新末段是新键,不得误伤");
}

// ===== 终态行 GC(final review I1) =====

/// 过期超 24h 的 approvals/qr_tokens/enroll_tokens 行摘除,新鲜行与临界
/// 行(恰 24h)保留;节流闸关窗后重复调用零成本。
#[tokio::test]
async fn gc_removes_expired_terminal_rows_keeps_fresh() {
    let state = signaling::test_state().await;
    let now = signaling::db::now();
    {
        let conn = state.db.lock().unwrap();
        // 老(过期 25h,三表各一)→ 摘
        conn.execute(
            "INSERT INTO approvals(id, account_id, pubkey, name, platform, code_hash, attempts, state, expires_at)
             VALUES('old-a','acct','pk','n','p','h',0,'expired',?1)",
            [now - 25 * 3600],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO qr_tokens(token, account_id, claimed_by, candidate_pubkey, candidate_name, candidate_platform, state, expires_at)
             VALUES('old-q','acct','dev','pk','n','p','expired',?1)",
            [now - 25 * 3600],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO enroll_tokens(token, account_id, pubkey, name, platform, expires_at)
             VALUES('old-e','acct','pk','n','p',?1)",
            [now - 25 * 3600],
        )
        .unwrap();
        // 新鲜(未过期)→ 留
        conn.execute(
            "INSERT INTO approvals(id, account_id, pubkey, name, platform, code_hash, attempts, state, expires_at)
             VALUES('fresh-a','acct','pk','n','p','h',0,'pending',?1)",
            [now + 600],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO enroll_tokens(token, account_id, pubkey, name, platform, expires_at)
             VALUES('fresh-e','acct','pk','n','p',?1)",
            [now + 600],
        )
        .unwrap();
        // 临界(恰 -24h,不早于 horizon)→ 留(严格小于才摘)
        conn.execute(
            "INSERT INTO qr_tokens(token, account_id, claimed_by, candidate_pubkey, candidate_name, candidate_platform, state, expires_at)
             VALUES('edge-q','acct','dev','pk','n','p','expired',?1)",
            [now - 24 * 3600],
        )
        .unwrap();
    }
    // 无节流核心:老三行全摘,新鲜/临界全留
    let removed = signaling::api::_gc_expired_terminal_rows_now_for_test(&state);
    assert_eq!(removed, 3, "恰摘三张老行: {removed}");
    let conn = state.db.lock().unwrap();
    for keep in ["fresh-a", "fresh-e", "edge-q"] {
        let (table, col) = if keep.ends_with("-q") { ("qr_tokens", "token") } else if keep.ends_with("-a") { ("approvals", "id") } else { ("enroll_tokens", "token") };
        let n: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {col}=?1"),
                [keep],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{keep} 须保留");
    }
    for gone in ["old-a", "old-q", "old-e"] {
        let (table, col) = if gone.ends_with("-q") { ("qr_tokens", "token") } else if gone.ends_with("-a") { ("approvals", "id") } else { ("enroll_tokens", "token") };
        let n: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {col}=?1"),
                [gone],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "{gone} 须被摘除");
    }
    drop(conn);
    // 节流闸:紧随其后的生产入口调用必 0(60s 窗内;进程级单例,时序无关)
    assert_eq!(signaling::api::gc_expired_terminal_rows(&state), 0, "节流窗内不得重复清扫");
}
