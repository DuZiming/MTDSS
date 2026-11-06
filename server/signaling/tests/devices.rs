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
    // §13.4(V3-3 返工 R1):快照携 created_at(Unix 秒,服务端既有列)
    assert_eq!(list[0]["created_at"].as_i64().unwrap(), {
        state.db.lock().unwrap()
            .query_row("SELECT created_at FROM devices WHERE device_id=?1",
                [&a["device_id"].as_str().unwrap()], |r| r.get::<_, i64>(0)).unwrap()
    });
}

#[tokio::test]
async fn devices_ordered_by_created_at_asc_device_id_tiebreak() {
    // V3-3 返工 R1(spec §13.4):响应默认序 = created_at 升序(终端加入
    // 顺序 = 64 色色板取色序);同秒并列按 device_id 稳定次序兜底。
    let state = signaling::test_state().await;
    let a = register_device(&state, "a@b.c", "dev-a", 0).await;
    let account_id: String = state.db.lock().unwrap()
        .query_row("SELECT account_id FROM devices WHERE device_id=?1",
            [&a["device_id"].as_str().unwrap()], |r| r.get(0)).unwrap();
    // 乱序直插三行:late(3000)在 early 两行(同秒 1000)之后入库
    {
        let conn = state.db.lock().unwrap();
        for (did, name, ca) in [("dev-late", "dev-late", 3000i64), ("dev-early-2", "dev-e2", 1000), ("dev-early-1", "dev-e1", 1000)] {
            conn.execute(
                "INSERT INTO devices(account_id, device_id, pubkey, name, platform, created_at, last_seen)
                 VALUES(?1,?2,?3,?4,'test',?5,0)",
                rusqlite::params![account_id, did, pubkey_hex(9), name, ca],
            ).unwrap();
        }
    }
    let (_, v) = get(&state, "/devices", a["jwt"].as_str().unwrap()).await;
    let list = v["devices"].as_array().unwrap();
    assert_eq!(list.len(), 4);
    // created_at 升序;同秒(1000)两行按 device_id 字典序兜底
    let ids: Vec<&str> = list.iter().map(|d| d["device_id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["dev-early-1", "dev-early-2", "dev-late", a["device_id"].as_str().unwrap()]);
    let cas: Vec<i64> = list.iter().map(|d| d["created_at"].as_i64().unwrap()).collect();
    assert_eq!(cas, [1000, 1000, 3000, cas[3]], "created_at 升序且末位为本机(now)");
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
async fn reregister_same_pubkey_under_new_email_requires_approval() {
    // 同一身份（pubkey）先用邮箱 A 注册，再用邮箱 B 重走验证码流程：
    // v3p1 换绑守卫(spec §3.1)——已属他账号的 pubkey 恒走审批面,直入分支
    // (「row 不存在 OR 已属本账号」)不放行,设备不随 verify 即时迁移。
    // approved 后的 upsert 迁移全链归 tests/approval.rs
    // (rebind_approved_migrates_device_between_accounts)。
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
    assert_eq!(first["status"], "ok");
    assert_eq!(first["device_id"].as_str().unwrap().len(), 32);
    // 迁移前：旧账号下能看到这台设备
    let (_, old_list) = get(&state, "/devices", first["jwt"].as_str().unwrap()).await;
    assert_eq!(old_list["devices"].as_array().unwrap().len(), 1);

    // 同一 pubkey 用邮箱 B 重走完整验证码流程:B 无任何设备/在线端(快照空)
    // 也必须转审批——归属守卫优先于快照
    signaling::admin::user_add(&state.db.lock().unwrap(), "new@x.y").unwrap();
    let (_, v2) = post(&state, "/auth/request-code", json!({"email": "new@x.y"})).await;
    let (acode, again) = post(&state, "/auth/verify", json!({
        "email": "new@x.y", "code": v2["code"].as_str().unwrap(),
        "pubkey": pubkey, "name": "dev-x", "platform": "test"
    })).await;
    assert_eq!(acode, StatusCode::OK);
    assert_eq!(again["status"], "approval", "已属他账号的 pubkey 恒走审批: {again}");
    assert!(again["session_id"].as_str().is_some());
    assert!(again["jwt"].is_null(), "审批形态不带票");

    // 设备行未迁移:仍挂旧账号,B 账号 0 台
    let (_, old_list) = get(&state, "/devices", first["jwt"].as_str().unwrap()).await;
    assert_eq!(old_list["devices"].as_array().unwrap().len(), 1, "verify 不迁移设备行");
    let new_count: i64 = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM devices WHERE account_id=(SELECT id FROM accounts WHERE email='new@x.y')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_count, 0, "审批 settle 前不得迁移设备行");
}
