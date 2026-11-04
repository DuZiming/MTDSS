use crate::auth;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize)]
pub struct RequestCodeReq {
    pub email: String,
}

/// 客户端 IP(XFF 首项优先 → ConnectInfo → "unknown"):per-IP 限流键。
/// 产线恒经反代(Caddy 设 XFF,socket 侧只见回环);直连 TLS 形态 XFF 可
/// 伪造轮换——过渡形态接受,ConnectInfo/unknown 兜底。
pub struct ClientIp(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        let ip = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_owned())
            .or_else(|| {
                parts
                    .extensions
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|c| c.0.ip().to_string())
            })
            .unwrap_or_else(|| "unknown".to_owned());
        Ok(Self(ip))
    }
}

/// request-code 错误面:404 未知账号 / 429 限速(Retry-After)/ 500 发信失败。
pub enum ReqCodeErr {
    UnknownAccount,
    Throttled { retry_after_secs: u64 },
    Mail(anyhow::Error),
}

impl axum::response::IntoResponse for ReqCodeErr {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::UnknownAccount => (StatusCode::NOT_FOUND, "unknown account").into_response(),
            Self::Throttled { retry_after_secs } => {
                let mut resp = (StatusCode::TOO_MANY_REQUESTS, "稍后再试").into_response();
                resp.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    retry_after_secs
                        .to_string()
                        .parse()
                        .expect("retry-after is ascii digits"),
                );
                resp
            }
            Self::Mail(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("send mail failed: {e:#}"),
            )
                .into_response(),
        }
    }
}

pub async fn request_code(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    Json(req): Json<RequestCodeReq>,
) -> Result<Json<serde_json::Value>, ReqCodeErr> {
    // 限速先于一切(spec §3.10):per-email 1/min + 5/h、per-IP 10/h,
    // 超限 429+Retry-After。dev 回显门开(本地/e2e/冒烟)不计费不限速。
    if !state.echo_code {
        for (key, max, window) in [
            (
                format!("reqc:email-min:{}", req.email),
                1u32,
                Duration::from_secs(60),
            ),
            (
                format!("reqc:email-hour:{}", req.email),
                5,
                Duration::from_secs(3600),
            ),
            (format!("reqc:ip:{ip}"), 10, Duration::from_secs(3600)),
        ] {
            if !state.throttle.allow(&key, max, window) {
                return Err(ReqCodeErr::Throttled {
                    retry_after_secs: state.throttle.retry_after_secs(&key, window),
                });
            }
        }
    }
    // 认证收口:仅已预置账号可领验证码,未知邮箱一律 404
    // (v2 起不再"任意邮箱发码即注册",入口先查 accounts 防未预置邮箱领码)
    let known: bool = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM accounts WHERE email=?1",
            [&req.email],
            |r| r.get::<_, i64>(0),
        )
        .is_ok()
    };
    if !known {
        return Err(ReqCodeErr::UnknownAccount);
    }
    let code = auth::gen_code();
    if state.echo_code {
        // dev 门开:不发信,落库后回显(本地/e2e/冒烟;spec §3.6)
        auth::store_code(&state, &req.email, &code).map_err(ReqCodeErr::Mail)?;
        tracing::info!(email = %req.email, "verification code issued (dev echo)");
        return Ok(Json(serde_json::json!({ "code": code })));
    }
    // 真实投递:先发后落,发信失败=500 且不产生 codes 行(spec §3.5 无半态);
    // send 为阻塞网络调用,丢 blocking 池避免占用异步执行器。
    let st = state.clone();
    let email = req.email.clone();
    let code_for_send = code.clone();
    tokio::task::spawn_blocking(move || st.mailer.send(&email, &code_for_send))
        .await
        .map_err(|e| ReqCodeErr::Mail(anyhow::anyhow!("mail worker: {e}")))?
        .map_err(ReqCodeErr::Mail)?;
    auth::store_code(&state, &req.email, &code).map_err(ReqCodeErr::Mail)?;
    tracing::info!(email = %req.email, "verification code issued (real mail)");
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
pub struct VerifyReq {
    pub email: String,
    pub code: String,
    pub pubkey: String,
    pub name: String,
    pub platform: String,
}

/// verify 响应三形态(spec §3.1;`status` 判别字段,旧客户端缺字段按 ok 解析
/// 的容错在客户端侧)。limit 形态:配额超额,不插行、带单次 enroll_token 与
/// 现有设备清单供踢选(spec §3.3)。
#[derive(Serialize)]
#[serde(tag = "status")]
pub enum VerifyResp {
    #[serde(rename = "ok")]
    Ok {
        jwt: String,
        refresh: String,
        device_id: String,
        account_id: String,
    },
    #[serde(rename = "approval")]
    Approval {
        session_id: String,
        expires_at: i64,
    },
    #[serde(rename = "limit")]
    Limit {
        enroll_token: String,
        devices: Vec<crate::auth::DeviceInfo>,
    },
}

pub async fn verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyReq>,
) -> Result<Json<VerifyResp>, (StatusCode, String)> {
    match auth::verify_code(&state, &req.email, &req.code, &req.pubkey, &req.name, &req.platform) {
        Ok(auth::VerifyOutcome::Ok(ok)) => Ok(Json(VerifyResp::Ok {
            jwt: ok.jwt,
            refresh: ok.refresh,
            device_id: ok.device_id,
            account_id: ok.account_id,
        })),
        Ok(auth::VerifyOutcome::Approval(p)) => Ok(Json(VerifyResp::Approval {
            session_id: p.session_id,
            expires_at: p.expires_at,
        })),
        Ok(auth::VerifyOutcome::Limit { enroll_token, devices }) => {
            Ok(Json(VerifyResp::Limit { enroll_token, devices }))
        }
        Err(_) => Err((StatusCode::UNAUTHORIZED, "invalid code".into())),
    }
}

#[derive(Deserialize)]
pub struct RefreshReq {
    pub refresh: String,
}

/// refresh 响应(旧形态原样,无 status 判别——判别面只属登录状态机)。
#[derive(Serialize)]
pub struct RefreshResp {
    pub jwt: String,
    pub refresh: String,
    pub device_id: String,
    pub account_id: String,
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RefreshReq>,
) -> Result<Json<RefreshResp>, (StatusCode, String)> {
    auth::rotate_refresh(&state, &req.refresh)
        .map(|ok| {
            Json(RefreshResp {
                jwt: ok.jwt,
                refresh: ok.refresh,
                device_id: ok.device_id,
                account_id: ok.account_id,
            })
        })
        .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid refresh".into()))
}

// ---------- 审批会话端点(spec §3.1 settling 面) ----------

/// 免认证面错误统一形态:429 限速(Retry-After)+ 其余 (code, 文案)。
pub enum AuthErr {
    Throttled { retry_after_secs: u64 },
    Status(StatusCode, String),
}

impl axum::response::IntoResponse for AuthErr {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::Throttled { retry_after_secs } => {
                let mut resp = (StatusCode::TOO_MANY_REQUESTS, "稍后再试").into_response();
                resp.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    retry_after_secs
                        .to_string()
                        .parse()
                        .expect("retry-after is ascii digits"),
                );
                resp
            }
            Self::Status(code, msg) => (code, msg).into_response(),
        }
    }
}

fn throttled(state: &AppState, key: &str, window: Duration) -> AuthErr {
    AuthErr::Throttled {
        retry_after_secs: state.throttle.retry_after_secs(key, window),
    }
}

/// Bearer access-JWT 提取(qr-claim / qr-approve 同款;approval-respond 先例):
/// 缺头/坏票/typ 不对一律 401。
fn bearer_claims(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<auth::Claims, AuthErr> {
    let jwt = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| AuthErr::Status(StatusCode::UNAUTHORIZED, "missing bearer".into()))?;
    let claims = auth::verify_jwt(&state.jwt_secret, jwt)
        .map_err(|_| AuthErr::Status(StatusCode::UNAUTHORIZED, "bad token".into()))?;
    if claims.typ != "access" {
        return Err(AuthErr::Status(StatusCode::UNAUTHORIZED, "wrong token type".into()));
    }
    Ok(claims)
}

#[derive(Deserialize)]
pub struct ApproveCodeReq {
    pub session_id: String,
    pub code: String,
}

/// POST /auth/approve-code {session_id, code}(免认证;per-IP 30/min):
/// 新终端直接输审批码 settle。404 未知会话 / 401 码错(错满 5 次会话作废)/
/// 409 已裁决或已过期 / 429 限速。
pub async fn approve_code(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    Json(req): Json<ApproveCodeReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("approvec:ip:{ip}");
    if !state.throttle.allow(&key, 30, Duration::from_secs(60)) {
        return Err(throttled(&state, &key, Duration::from_secs(60)));
    }
    match auth::approve_with_code(&state, &req.session_id, &req.code) {
        Ok(()) => Ok(Json(serde_json::json!({ "status": "approved" }))),
        Err(auth::ApproveCodeErr::UnknownSession) => {
            Err(AuthErr::Status(StatusCode::NOT_FOUND, "unknown session".into()))
        }
        Err(auth::ApproveCodeErr::NotPending) => Err(AuthErr::Status(
            StatusCode::CONFLICT,
            "session already settled or expired".into(),
        )),
        Err(auth::ApproveCodeErr::WrongCode) => {
            Err(AuthErr::Status(StatusCode::UNAUTHORIZED, "invalid code".into()))
        }
        Err(auth::ApproveCodeErr::Db(e)) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct ApprovalRespondReq {
    pub session_id: String,
    pub allow: bool,
}

/// POST /auth/approval-respond {session_id, allow}(Bearer + 须为该账号在线
/// 受信设备):受信终端裁决。401 鉴权失败 / 403 非在线受信设备或跨账号 /
/// 404 未知会话 / 409 已裁决或已过期。allow=false 一票否决。
pub async fn approval_respond(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ApprovalRespondReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let jwt = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| AuthErr::Status(StatusCode::UNAUTHORIZED, "missing bearer".into()))?;
    let claims = auth::verify_jwt(&state.jwt_secret, jwt)
        .map_err(|_| AuthErr::Status(StatusCode::UNAUTHORIZED, "bad token".into()))?;
    if claims.typ != "access" {
        return Err(AuthErr::Status(StatusCode::UNAUTHORIZED, "wrong token type".into()));
    }
    match auth::approval_respond(&state, &claims.sub, &claims.device, &req.session_id, req.allow) {
        Ok(()) => Ok(Json(serde_json::json!({}))),
        Err(auth::RespondErr::UnknownSession) => {
            Err(AuthErr::Status(StatusCode::NOT_FOUND, "unknown session".into()))
        }
        Err(auth::RespondErr::NotPending) => Err(AuthErr::Status(
            StatusCode::CONFLICT,
            "session already settled or expired".into(),
        )),
        Err(auth::RespondErr::Forbidden) => Err(AuthErr::Status(
            StatusCode::FORBIDDEN,
            "responder must be an online trusted device of the session's account".into(),
        )),
        Err(auth::RespondErr::Db(e)) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct LoginStatusReq {
    pub session_id: String,
}

/// POST /auth/login-status {session_id}(免认证轮询;per-IP 120/min):
/// `{status:"ok|approval|denied|expired|limit"}`,ok 带票且单次有效。未知/已消费
/// 会话一律按终态 expired 呈现(轮询面无 404 通道)。
pub async fn login_status(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    Json(req): Json<LoginStatusReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("loginstatus:ip:{ip}");
    if !state.throttle.allow(&key, 120, Duration::from_secs(60)) {
        return Err(throttled(&state, &key, Duration::from_secs(60)));
    }
    match auth::login_status(&state, &req.session_id) {
        Ok(auth::LoginStatus::Approval { expires_at }) => Ok(Json(serde_json::json!({
            "status": "approval",
            "expires_at": expires_at,
        }))),
        Ok(auth::LoginStatus::Denied) => Ok(Json(serde_json::json!({ "status": "denied" }))),
        Ok(auth::LoginStatus::Expired) => Ok(Json(serde_json::json!({ "status": "expired" }))),
        Ok(auth::LoginStatus::Limit { enroll_token, devices }) => Ok(Json(serde_json::json!({
            "status": "limit",
            "enroll_token": enroll_token,
            "devices": devices,
        }))),
        Ok(auth::LoginStatus::Ok(ok)) => Ok(Json(serde_json::json!({
            "status": "ok",
            "jwt": ok.jwt,
            "refresh": ok.refresh,
            "device_id": ok.device_id,
            "account_id": ok.account_id,
        }))),
        Err(e) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e:#}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct EvictReq {
    pub enroll_token: String,
    pub device_id: String,
}

/// POST /auth/evict {enroll_token, device_id}(免认证踢选;per-IP 30/min,
/// spec §3.3):单次消费 enroll_token → 删目标 devices 行(共享删除路径,归属
/// 守卫)→ 即时踢(device_evicted 临别帧 → code 9 → 断,摘注册)→ 插新行铸票
/// (配额已由删行腾出,不再过门)→ ok 形态。401 token 未知/过期/已用、404
/// 目标不属此账号(自踢不可——新设备尚无行;坏目标烧票 fail-closed)。
pub async fn evict(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    Json(req): Json<EvictReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("evict:ip:{ip}");
    if !state.throttle.allow(&key, 30, Duration::from_secs(60)) {
        return Err(throttled(&state, &key, Duration::from_secs(60)));
    }
    // 单次消费(RETURNING 一语句裁定唯一赢家;未知/过期/已用同判 401)
    let claimed: Option<(String, String, String, String)> = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "DELETE FROM enroll_tokens WHERE token=?1 AND expires_at>?2
             RETURNING account_id, pubkey, name, platform",
            rusqlite::params![req.enroll_token, crate::db::now()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?
    };
    let Some((account_id, pubkey, name, platform)) = claimed else {
        return Err(AuthErr::Status(
            StatusCode::UNAUTHORIZED,
            "invalid or expired enroll token".into(),
        ));
    };
    // 先派生新设备身份(同 verify 口径;token 行在铸出时已验过形,此处兜底)
    let device_id = auth::device_id_of_pubkey(&pubkey)
        .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    // 候选身份若已属他账号,插行即迁出 → 旧账号在线端也须刷新(review Important #1)
    let departed: Option<String> = {
        let conn = state.db.lock().unwrap();
        let owner: Option<String> = conn
            .query_row(
                "SELECT account_id FROM devices WHERE device_id=?1",
                [&device_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
        owner.filter(|a| a.as_str() != account_id.as_str())
    };
    // 删目标行(唯一删除收口;归属守卫)
    let removed = {
        let conn = state.db.lock().unwrap();
        crate::admin::device_remove(&conn, &account_id, &req.device_id)
            .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?
    };
    if !removed {
        return Err(AuthErr::Status(StatusCode::NOT_FOUND, "no such device".into()));
    }
    // 即时踢(先临别 notice 后断)
    crate::ws::kick_device(&state, &req.device_id).await;
    // 插新行铸票(token 承载的候选身份;配额已由删行腾出,不再过门)
    let ok = auth::enroll_and_mint(&state, &account_id, &device_id, &pubkey, &name, &platform)
        .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e:#}")))?;
    // 删+插同批完成 → 全账号在线端刷新快照;候选迁出时旧账号在线端一并刷新
    if let Some(old) = departed {
        crate::ws::notify_devices_changed(&state, &old);
    }
    crate::ws::notify_devices_changed(&state, &account_id);
    Ok(Json(serde_json::json!({
        "status": "ok",
        "jwt": ok.jwt,
        "refresh": ok.refresh,
        "device_id": ok.device_id,
        "account_id": ok.account_id,
    })))
}

// ---------- 扫码登录端点(spec §3.2) ----------

#[derive(Deserialize)]
pub struct QrBeginReq {
    pub pubkey: String,
    pub name: String,
    pub platform: String,
}

/// POST /auth/qr-begin {pubkey,name,platform}(免认证;per-IP 10/h):PC 侧
/// 发起扫码登录,铸 128bit 单次 qr_token(120s)。二维码内容由客户端拼
/// `https://www.mtdss.top/qr?t=<token>`,服务端不参与。400 候选 pubkey 非法形。
pub async fn qr_begin(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    Json(req): Json<QrBeginReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("qrbegin:ip:{ip}");
    if !state.throttle.allow(&key, 10, Duration::from_secs(3600)) {
        return Err(throttled(&state, &key, Duration::from_secs(3600)));
    }
    match auth::qr_begin(&state, &req.pubkey, &req.name, &req.platform) {
        Ok(b) => Ok(Json(serde_json::json!({
            "qr_token": b.qr_token,
            "expires_at": b.expires_at,
        }))),
        Err(auth::QrBeginErr::BadPubkey(e)) => Err(AuthErr::Status(
            StatusCode::BAD_REQUEST,
            format!("invalid pubkey: {e:#}"),
        )),
        Err(auth::QrBeginErr::Db(e)) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct QrClaimReq {
    pub qr_token: String,
}

/// POST /auth/qr-claim {qr_token}(Bearer;per-IP 30/min):手机扫后认领,首
/// 认领者钉死 claimed_by(先到先裁),同认领者幂等,他者 409;不存在/已过期
/// 404。响应带确认弹层面(candidate name/platform)+ token 时限。
pub async fn qr_claim(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<QrClaimReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("qrclaim:ip:{ip}");
    if !state.throttle.allow(&key, 30, Duration::from_secs(60)) {
        return Err(throttled(&state, &key, Duration::from_secs(60)));
    }
    let claims = bearer_claims(&state, &headers)?;
    match auth::qr_claim(&state, &req.qr_token, &claims.sub, &claims.device) {
        Ok(c) => Ok(Json(serde_json::json!({
            "candidate": { "name": c.name, "platform": c.platform },
            "expires_at": c.expires_at,
        }))),
        Err(auth::QrClaimErr::UnknownToken) => Err(AuthErr::Status(
            StatusCode::NOT_FOUND,
            "unknown or expired qr token".into(),
        )),
        Err(auth::QrClaimErr::Contested) => Err(AuthErr::Status(
            StatusCode::CONFLICT,
            "qr token already claimed by another device or settled".into(),
        )),
        Err(auth::QrClaimErr::Db(e)) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct QrApproveReq {
    pub qr_token: String,
    pub allow: bool,
}

/// POST /auth/qr-approve {qr_token, allow}(Bearer;per-IP 30/min):认领者
/// 裁决(须=claimant;未认领/他端/他账号 403)。allow=true → state='approved'
/// (票由 PC 轮询经 enroll_device 收口单次领取);false → 'denied' 终态。
/// 未知 404 / 已裁决或已过期 409(单次)。
pub async fn qr_approve(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<QrApproveReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("qrapprove:ip:{ip}");
    if !state.throttle.allow(&key, 30, Duration::from_secs(60)) {
        return Err(throttled(&state, &key, Duration::from_secs(60)));
    }
    let claims = bearer_claims(&state, &headers)?;
    match auth::qr_approve(&state, &req.qr_token, &claims.sub, &claims.device, req.allow) {
        Ok(()) => Ok(Json(serde_json::json!({}))),
        Err(auth::QrApproveErr::UnknownToken) => Err(AuthErr::Status(
            StatusCode::NOT_FOUND,
            "unknown qr token".into(),
        )),
        Err(auth::QrApproveErr::Forbidden) => Err(AuthErr::Status(
            StatusCode::FORBIDDEN,
            "approver must be the claimant".into(),
        )),
        Err(auth::QrApproveErr::NotClaimable) => Err(AuthErr::Status(
            StatusCode::CONFLICT,
            "qr token already settled or expired".into(),
        )),
        Err(auth::QrApproveErr::Db(e)) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        )),
    }
}

#[derive(Deserialize)]
pub struct QrStatusQuery {
    pub t: String,
}

/// GET /auth/qr-status?t=<token>(免认证轮询,2s 档;per-IP 120/min):
/// `{status:"pending|ok|denied|expired|limit"}`,ok 带票且单次有效(enroll
/// 收口),limit 带 enroll_token+devices;未知/已消费 token 恒按终态 expired
/// 呈现(轮询面无 404 通道,login-status 同款)。
pub async fn qr_status(
    State(state): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    axum::extract::Query(q): axum::extract::Query<QrStatusQuery>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let key = format!("qrstatus:ip:{ip}");
    if !state.throttle.allow(&key, 120, Duration::from_secs(60)) {
        return Err(throttled(&state, &key, Duration::from_secs(60)));
    }
    match auth::qr_status(&state, &q.t) {
        Ok(auth::QrStatus::Pending { expires_at }) => Ok(Json(serde_json::json!({
            "status": "pending",
            "expires_at": expires_at,
        }))),
        Ok(auth::QrStatus::Denied) => Ok(Json(serde_json::json!({ "status": "denied" }))),
        Ok(auth::QrStatus::Expired) => Ok(Json(serde_json::json!({ "status": "expired" }))),
        Ok(auth::QrStatus::Limit { enroll_token, devices }) => Ok(Json(serde_json::json!({
            "status": "limit",
            "enroll_token": enroll_token,
            "devices": devices,
        }))),
        Ok(auth::QrStatus::Ok(ok)) => Ok(Json(serde_json::json!({
            "status": "ok",
            "jwt": ok.jwt,
            "refresh": ok.refresh,
            "device_id": ok.device_id,
            "account_id": ok.account_id,
        }))),
        Err(e) => Err(AuthErr::Status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e:#}"),
        )),
    }
}

// ---------- 账号数据 KV(spec §3.x:per-account 小配置同步面) ----------

/// value 上限 256KiB(字节):客户端按字符串存,UTF-8 按 byte 数计。
pub const KV_VALUE_LIMIT: usize = 256 * 1024;

#[derive(Deserialize)]
pub struct DataPutReq {
    pub value: String,
}

/// GET /data/{key}(Bearer access):读本账号 KV。200 `{value, updated_at}` /
/// 404 键不存在(合法态,不是错)/ 401 鉴权失败。
pub async fn data_get(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let claims = bearer_claims(&state, &headers)?;
    let row: Option<(String, i64)> = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT value, updated_at FROM account_kv WHERE account_id=?1 AND key=?2",
            rusqlite::params![claims.sub, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?
    };
    match row {
        Some((value, updated_at)) => Ok(Json(serde_json::json!({
            "value": value,
            "updated_at": updated_at,
        }))),
        None => Err(AuthErr::Status(StatusCode::NOT_FOUND, "no such key".into())),
    }
}

/// PUT /data/{key} {value}(Bearer access):UPSERT 本账号 KV。value ≤256KiB
/// (超限 400,不落库)。成功后向同账号**其他**在线端尽力推
/// `AuthNotice{kind:"data_changed", data_key}`(写者本端不收;try_send,
/// 设备离线/队列满不回滚写入)。200 `{updated_at}` / 401 鉴权失败。
pub async fn data_put(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(key): axum::extract::Path<String>,
    Json(req): Json<DataPutReq>,
) -> Result<Json<serde_json::Value>, AuthErr> {
    let claims = bearer_claims(&state, &headers)?;
    if req.value.len() > KV_VALUE_LIMIT {
        return Err(AuthErr::Status(
            StatusCode::BAD_REQUEST,
            format!("value too large (max {KV_VALUE_LIMIT} bytes)"),
        ));
    }
    let updated_at = crate::db::now();
    {
        let conn = state.db.lock().unwrap();
        conn.execute(
            "INSERT INTO account_kv(account_id, key, value, updated_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(account_id,key) DO UPDATE SET value=?3, updated_at=?4",
            rusqlite::params![claims.sub, key, req.value, updated_at],
        )
        .map_err(|e| AuthErr::Status(StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}")))?;
    }
    // 变更通告(尽力推):只给同账号其他在线端,写者自身跳过
    let notice = mtdss_proto::ws::AuthNotice {
        kind: "data_changed".into(),
        data_key: key,
        ..Default::default()
    };
    for (peer_id, _) in state.presence.account_peers(&claims.sub) {
        if peer_id != claims.device {
            crate::ws::push_notice(&state, &peer_id, notice.clone());
        }
    }
    Ok(Json(serde_json::json!({ "updated_at": updated_at })))
}

pub fn routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/auth/request-code", axum::routing::post(request_code))
        .route("/auth/verify", axum::routing::post(verify))
        .route("/auth/approve-code", axum::routing::post(approve_code))
        .route("/auth/approval-respond", axum::routing::post(approval_respond))
        .route("/auth/login-status", axum::routing::post(login_status))
        .route("/auth/evict", axum::routing::post(evict))
        .route("/auth/qr-begin", axum::routing::post(qr_begin))
        .route("/auth/qr-claim", axum::routing::post(qr_claim))
        .route("/auth/qr-approve", axum::routing::post(qr_approve))
        .route("/auth/qr-status", axum::routing::get(qr_status))
        .route("/auth/refresh", axum::routing::post(refresh))
        .route("/devices", axum::routing::get(list_devices))
        .route(
            "/data/{key}",
            axum::routing::get(data_get).put(data_put),
        )
}

#[derive(Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub online: bool,
    pub pubkey: String,
}

pub async fn list_devices(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let jwt = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing bearer".into()))?;
    let claims = auth::verify_jwt(&state.jwt_secret, jwt)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "bad token".into()))?;
    if claims.typ != "access" {
        return Err((StatusCode::UNAUTHORIZED, "wrong token type".into()));
    }
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT device_id, name, platform, pubkey FROM devices WHERE account_id=?1")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let devices: Vec<DeviceInfo> = stmt
        .query_map([&claims.sub], |r| {
            Ok(DeviceInfo {
                device_id: r.get(0)?,
                name: r.get(1)?,
                platform: r.get(2)?,
                // 占位；下面的 map 按在线状态覆盖
                online: false,
                pubkey: r.get(3)?,
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter_map(|r| r.ok())
        .map(|d| DeviceInfo { online: state.presence.is_online(&d.device_id), ..d })
        .collect();
    drop(stmt);
    drop(conn);
    Ok(Json(serde_json::json!({ "devices": devices })))
}
