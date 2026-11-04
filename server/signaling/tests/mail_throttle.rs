//! mail/throttle 面(V3-1 Task 2,spec §3.5/§3.6/§3.10):
//! - dev 回显门:开=响应含 code 且零发信;关=响应 {} 且先发信后落库
//! - 先发后落无半态:发信失败 500 且 codes 无行
//! - 未配置(DM env 缺失且非 dev 门):500 fail-closed
//! - request-code 限速:per-email 1/min 第 2 次 429 + Retry-After
//! - 码错 5 次作废:原码再对也 401;重新领码重置计数
//! - DM RPC v1 签名纯函数(pop_encode/base64/待签串)与中文模板逐字

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use signaling::build_router;
use std::sync::{Arc, Mutex};
use tower::ServiceExt; // dev-dep tower with util

async fn post(
    state: &Arc<signaling::AppState>,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let (code, _, v) = post_full(state, path, body).await;
    (code, v)
}

/// 带响应头的变体(429 Retry-After 断言用)。
async fn post_full(
    state: &Arc<signaling::AppState>,
    path: &str,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let (code, headers, _, v) = post_raw(state, path, body).await;
    (code, headers, v)
}

/// 原始文本变体(500 体泛化断言用,final review M1)。
async fn post_raw(
    state: &Arc<signaling::AppState>,
    path: &str,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, String, Value) {
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
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (code, headers, text, v)
}

/// 计数 mock:记录 (email, code);`fail=true` 时 send 恒 Err(投递失败)。
/// 克隆共享同一计数(Arc 在字段内,整只 Box 进 state 后 test 侧仍可读)。
#[derive(Clone)]
struct MockMailer {
    sends: Arc<Mutex<Vec<(String, String)>>>,
    fail: bool,
}

impl MockMailer {
    fn ok() -> Self {
        Self { sends: Arc::new(Mutex::new(Vec::new())), fail: false }
    }
    fn failing() -> Self {
        Self { sends: Arc::new(Mutex::new(Vec::new())), fail: true }
    }

    fn sends(&self) -> Vec<(String, String)> {
        self.sends.lock().unwrap().clone()
    }
}

impl signaling::mail::CodeMailer for MockMailer {
    fn send(&self, email: &str, code: &str) -> anyhow::Result<()> {
        if self.fail {
            anyhow::bail!("smtp down");
        }
        self.sends.lock().unwrap().push((email.to_owned(), code.to_owned()));
        Ok(())
    }
}

/// 自装配测试态(db=:memory:;echo_code 显式,绕开 spawn_test_server 的 dev 缺省)。
fn st(
    mailer: Box<dyn signaling::mail::CodeMailer>,
    echo_code: bool,
) -> Arc<signaling::AppState> {
    Arc::new(signaling::AppState {
        db: Mutex::new(signaling::db::open(":memory:").unwrap()),
        jwt_secret: rand::random(),
        presence: Arc::new(signaling::PresenceRegistry::new()),
        keepalive: signaling::ws::Keepalive::default(),
        stun_addrs: Vec::new(),
        relay: std::sync::OnceLock::new(),
        mailer,
        echo_code,
        throttle: signaling::throttle::Throttle::new(),
    })
}

fn codes_rows(state: &signaling::AppState) -> i64 {
    state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM codes", [], |r| r.get(0))
        .unwrap()
}

fn sends(mock: &MockMailer) -> Vec<(String, String)> {
    mock.sends()
}

// ---------- dev 回显门 + 先发后落(spec §3.5/§3.6) ----------

#[tokio::test]
async fn mock_send_success_persists_and_response_has_no_code() {
    let mock = MockMailer::ok();
    let state = st(Box::new(mock.clone()), false);
    signaling::admin::user_add(&state.db.lock().unwrap(), "a@b.c").unwrap();
    let (code, v) = post(&state, "/auth/request-code", json!({"email": "a@b.c"})).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(v, json!({}), "真实投递模式响应必须为空对象(无 code)");
    assert_eq!(sends(&mock).len(), 1, "应恰好发信一次");
    assert_eq!(sends(&mock)[0].0, "a@b.c");
    assert_eq!(codes_rows(&state), 1, "发信成功后必须落库");
    // mock 记录的码与库里哈希一致(同一码先发后落)
    let stored: String = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT code_hash FROM codes", [], |r| r.get(0))
        .unwrap();
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(sends(&mock)[0].1.as_bytes());
    assert_eq!(stored, hex::encode(h.finalize()), "落库哈希须对应发出去的码");
}

#[tokio::test]
async fn mailer_failure_is_500_and_no_row() {
    let mock = MockMailer::failing();
    let state = st(Box::new(mock.clone()), false);
    signaling::admin::user_add(&state.db.lock().unwrap(), "a@b.c").unwrap();
    let (code, _, body, _) = post_raw(&state, "/auth/request-code", json!({"email": "a@b.c"})).await;
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR, "发信失败必须 500");
    assert_eq!(codes_rows(&state), 0, "发信失败不得落库(无半态)");
    // 体泛化(final review M1):完整 anyhow 链(含 DM 上游响应等内网细节)
    // 只进日志;响应体恒泛化短句,不外泄底层错误文本
    assert_eq!(body, "send mail failed", "500 体须为泛化短句: {body}");
    assert!(!body.contains("smtp down"), "500 体不得含底层错误细节: {body}");
}

#[tokio::test]
async fn dev_gate_echoes_code_and_never_sends() {
    let mock = MockMailer::ok();
    let state = st(Box::new(mock.clone()), true);
    signaling::admin::user_add(&state.db.lock().unwrap(), "d@e.f").unwrap();
    let (code, v) = post(&state, "/auth/request-code", json!({"email": "d@e.f"})).await;
    assert_eq!(code, StatusCode::OK);
    let echoed = v["code"].as_str().expect("dev 门开响应必须含 code").to_string();
    assert_eq!(echoed.len(), 6);
    assert!(
        echoed.chars().all(|c| c.is_ascii_digit()),
        "回显码应为六位数字: {echoed}"
    );
    assert_eq!(sends(&mock).len(), 0, "dev 门开必须零发信");
    assert_eq!(codes_rows(&state), 1, "dev 门仍须落库(verify 可验)");
}

#[tokio::test]
async fn unconfigured_mailer_is_500() {
    let state = st(Box::new(signaling::mail::Unconfigured), false);
    signaling::admin::user_add(&state.db.lock().unwrap(), "u@v.w").unwrap();
    let (code, _) = post(&state, "/auth/request-code", json!({"email": "u@v.w"})).await;
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR, "未配置必须 500 fail-closed");
    assert_eq!(codes_rows(&state), 0);
}

#[test]
fn assembly_rules() {
    // dev 门优先;否则 env 四键齐→AliyunDm,缺→Unconfigured
    assert!(matches!(
        signaling::mail::assemble(true),
        signaling::mail::Mailer::Dev(_)
    ));
    let keys = [
        "MTDSS_DM_ACCESS_KEY_ID",
        "MTDSS_DM_ACCESS_KEY_SECRET",
        "MTDSS_DM_ACCOUNT",
        "MTDSS_DM_ALIAS",
    ];
    let saved: Vec<(String, Option<String>)> = keys
        .iter()
        .map(|k| (k.to_string(), std::env::var(k).ok()))
        .collect();
    for k in keys {
        std::env::remove_var(k);
    }
    assert!(matches!(
        signaling::mail::assemble(false),
        signaling::mail::Mailer::Unconfigured(_)
    ));
    for (k, v) in saved {
        if let Some(v) = v {
            std::env::set_var(k, v);
        }
    }
}

// ---------- 限速(spec §3.10) ----------

#[tokio::test]
async fn throttle_second_request_429_with_retry_after() {
    // echo 关 + DevEcho(发送恒成功)→ 第 2 次 per-email 1/min 撞 429
    let state = st(Box::new(signaling::mail::DevEcho), false);
    signaling::admin::user_add(&state.db.lock().unwrap(), "t@u.v").unwrap();
    let (code, _, v) = post_full(&state, "/auth/request-code", json!({"email": "t@u.v"})).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(v, json!({}), "真实投递模式响应无 code");
    let (code, headers, _) = post_full(&state, "/auth/request-code", json!({"email": "t@u.v"})).await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "1/min 第 2 次必须 429");
    let ra = headers
        .get("retry-after")
        .expect("429 必须带 Retry-After")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .expect("Retry-After 必须是秒数");
    assert!(ra >= 1, "Retry-After 至少 1 秒: {ra}");
}

#[test]
fn throttle_unit_sliding_window() {
    let t = signaling::throttle::Throttle::new();
    assert!(t.allow("k", 2, std::time::Duration::from_millis(120)));
    assert!(t.allow("k", 2, std::time::Duration::from_millis(120)));
    assert!(!t.allow("k", 2, std::time::Duration::from_millis(120)), "满 2 次须拒绝");
    assert!(
        t.retry_after_secs("k", std::time::Duration::from_millis(120)) >= 1,
        "拒绝后须报正的等待秒数"
    );
    // 窗口滑出后恢复
    std::thread::sleep(std::time::Duration::from_millis(140));
    assert!(t.allow("k", 2, std::time::Duration::from_millis(120)));
    // 不同键互不影响
    assert!(t.allow("other", 2, std::time::Duration::from_millis(120)));
}

// ---------- 键存储有界(review Important #1:免认证面可任意铸键,破窗键不得永驻) ----------

#[test]
fn sweep_removes_drained_keys_keeps_live() {
    // 注入清扫 horizon=60ms;先铸 2000 个独立键(窗口 1h,事件在 horizon 内=活)
    let t = signaling::throttle::Throttle::_with_sweep_window_for_test(
        std::time::Duration::from_millis(60),
    );
    for i in 0..2000 {
        assert!(t.allow(&format!("k{i}"), 1, std::time::Duration::from_secs(3600)));
    }
    assert_eq!(t.len(), 2000, "活跃键必须保留(清扫不得破坏限流语义)");
    // 时间流逝过 horizon:2000 键全破窗;再铸一键(活)后清扫
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(t.allow("fresh", 1, std::time::Duration::from_secs(3600)));
    t._sweep_for_test();
    assert_eq!(
        t.len(),
        1,
        "破窗的 2000 键必须被摘除,只留 horizon 内的活键"
    );
    // 清扫后同键重新记账照常(限流语义无损)
    assert!(t.allow("k0", 1, std::time::Duration::from_secs(3600)));
    assert_eq!(t.len(), 2);
    // 连续清扫不再误摘活键
    t._sweep_for_test();
    assert_eq!(t.len(), 2, "horizon 内的键重复清扫不得误摘");
}

#[test]
fn allow_triggers_amortized_sweep() {
    // 第 SWEEP_EVERY 次 allow 必须自动触发清扫:4095 破窗键 + 1 活键
    let t = signaling::throttle::Throttle::_with_sweep_window_for_test(
        std::time::Duration::from_millis(80),
    );
    for _ in 0..signaling::throttle::SWEEP_EVERY - 1 {
        let n = t.len();
        assert!(t.allow(&format!("k{n}"), 1, std::time::Duration::from_secs(3600)));
    }
    assert_eq!(
        t.len(),
        (signaling::throttle::SWEEP_EVERY - 1) as usize,
        "触发前不清扫,键全数保留"
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(t.allow("trigger", 1, std::time::Duration::from_secs(3600)));
    assert_eq!(
        t.len(),
        1,
        "第 SWEEP_EVERY 次 allow 须摊销清扫:破窗键摘光只留活键"
    );
}

// ---------- 码错 5 次作废(spec §3.10) ----------

#[tokio::test]
async fn five_wrong_attempts_invalidate_code() {
    let state = st(Box::new(signaling::mail::DevEcho), true);
    signaling::admin::user_add(&state.db.lock().unwrap(), "w@x.y").unwrap();
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "w@x.y"})).await;
    let real = v["code"].as_str().unwrap().to_string();
    let wrong = if real == "000000" { "000001" } else { "000000" };
    for i in 0..5 {
        let (code, _) = post(
            &state,
            "/auth/verify",
            json!({"email": "w@x.y", "code": wrong, "pubkey": "00".repeat(32), "name": "d", "platform": "test"}),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED, "第 {} 次错码应 401", i + 1);
    }
    // 作废:原码再对也 401
    let (code, _) = post(
        &state,
        "/auth/verify",
        json!({"email": "w@x.y", "code": real, "pubkey": "00".repeat(32), "name": "d", "platform": "test"}),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED, "错满 5 次后原码必须作废");
    assert_eq!(codes_rows(&state), 0, "作废即删行");
}

#[tokio::test]
async fn re_request_resets_attempts() {
    let state = st(Box::new(signaling::mail::DevEcho), true);
    signaling::admin::user_add(&state.db.lock().unwrap(), "r@s.t").unwrap();
    // 第一轮:错 3 次(未作废)
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "r@s.t"})).await;
    let old = v["code"].as_str().unwrap().to_string();
    let wrong = if old == "000000" { "000001" } else { "000000" };
    for _ in 0..3 {
        let (code, _) = post(
            &state,
            "/auth/verify",
            json!({"email": "r@s.t", "code": wrong, "pubkey": "00".repeat(32), "name": "d", "platform": "test"}),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
    }
    // 重新领码:计数重置,新码直接可用
    let (_, v) = post(&state, "/auth/request-code", json!({"email": "r@s.t"})).await;
    let fresh = v["code"].as_str().unwrap().to_string();
    let (code, val) = post(
        &state,
        "/auth/verify",
        json!({"email": "r@s.t", "code": fresh, "pubkey": "00".repeat(32), "name": "d", "platform": "test"}),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "重新领码后计数须重置,新码可用");
    assert!(val["jwt"].as_str().is_some());
}

// ---------- DM 签名纯函数 + 模板 ----------

#[test]
fn pop_encode_rfc3986_and_aliyun_star() {
    let e = signaling::mail::pop_encode;
    assert_eq!(e("abc"), "abc");
    assert_eq!(e("a b"), "a%20b");
    assert_eq!(e("a+b"), "a%2Bb");
    assert_eq!(e("a~_-.z09"), "a~_-.z09");
    assert_eq!(e("a/b"), "a%2Fb");
    assert_eq!(e("a*b"), "a%2Ab", "阿里云特例:* 也须编码");
    assert_eq!(e("星"), "%E6%98%9F", "中文按 UTF-8 编字节");
}

#[test]
fn base64_rfc4648_vectors() {
    let b = signaling::mail::base64;
    assert_eq!(b(b""), "");
    assert_eq!(b(b"f"), "Zg==");
    assert_eq!(b(b"fo"), "Zm8=");
    assert_eq!(b(b"foobar"), "Zm9vYmFy");
}

#[test]
fn directmail_signature_golden() {
    let cfg = signaling::mail::DmConfig {
        access_key_id: "AKID-test".into(),
        access_key_secret: "secret-test".into(),
        account: "noreply@mtdss.top".into(),
        alias: "星枢/MTDSS".into(),
    };
    let body = signaling::mail::signed_single_send_mail(
        &cfg,
        "u@d.e",
        "正文",
        "nonce-1",
        "2026-01-01T00:00:00Z",
    );
    // 请求体 = 按参数名排序的 query + &Signature=
    let (query, sig) = body
        .split_once("&Signature=")
        .expect("请求体必含 &Signature=");
    assert!(query.contains("Action=SingleSendMail"));
    assert!(query.contains("AddressType=1"));
    assert!(query.contains("Version=2015-11-23"));
    assert!(query.contains("ReplyToAddress=false"));
    assert!(query.contains("ToAddress=u%40d.e"), "ToAddress 须百分号编码");
    assert!(
        query.contains("FromAlias=%E6%98%9F%E6%9E%A2%2FMTDSS"),
        "中文别名须逐字节百分号编码(星=%E6%98%9F 枢=%E6%9E%A2 /=2F)"
    );
    // 排序(字节序):AccessKeyId < AccountName < Action < AddressType < Format
    // < FromAlias < ReplyToAddress < SignatureMethod < SignatureNonce
    // < SignatureVersion < Subject < TextBody < Timestamp < ToAddress < Version
    // 首参恒为 AccessKeyId(最小键);其余以 &前缀锚定(防 ReplyToAddress 值域
    // 内的 "ToAddress=" 子串误配)。
    let pos = |k: &str| {
        if k == "AccessKeyId" {
            0
        } else {
            query.find(&format!("&{k}=")).expect(k) + 1
        }
    };
    let order = [
        "AccessKeyId",
        "AccountName",
        "Action",
        "AddressType",
        "Format",
        "FromAlias",
        "ReplyToAddress",
        "SignatureMethod",
        "SignatureNonce",
        "SignatureVersion",
        "Subject",
        "TextBody",
        "Timestamp",
        "ToAddress",
        "Version",
    ];
    for w in order.windows(2) {
        assert!(pos(w[0]) < pos(w[1]), "参数须按名排序: {} < {}", w[0], w[1]);
    }
    // 签名 = base64(HMAC-SHA1(key=secret&, data="POST&%2F&"+pop_encode(query)));
    // 请求体内嵌的 Signature 值还要再过一层 pop_encode(form 编码)
    use hmac::Mac;
    let sts = format!("POST&%2F&{}", signaling::mail::pop_encode(query));
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(b"secret-test&").unwrap();
    mac.update(sts.as_bytes());
    let expected = signaling::mail::base64(&mac.finalize().into_bytes());
    assert_eq!(
        sig,
        signaling::mail::pop_encode(&expected),
        "POP v1 签名须与待签公式逐位一致(内嵌前已 form 编码)"
    );
}

#[test]
fn mail_template_verbatim() {
    // spec §3.5 中文模板逐字
    assert_eq!(
        signaling::mail::code_mail_body("012345"),
        "你的验证码是 012345,10 分钟内有效。若非你本人操作,请忽略。"
    );
}
