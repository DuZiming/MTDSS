use crate::auth;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
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

#[derive(Serialize)]
pub struct VerifyResp {
    pub jwt: String,
    pub refresh: String,
    pub device_id: String,
    pub account_id: String,
}

pub async fn verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyReq>,
) -> Result<Json<VerifyResp>, (StatusCode, String)> {
    auth::verify_code(
        &state,
        &req.email,
        &req.code,
        &req.pubkey,
        &req.name,
        &req.platform,
    )
    .map(|ok| {
        Json(VerifyResp {
            jwt: ok.jwt,
            refresh: ok.refresh,
            device_id: ok.device_id,
            account_id: ok.account_id,
        })
    })
    .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid code".into()))
}

#[derive(Deserialize)]
pub struct RefreshReq {
    pub refresh: String,
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RefreshReq>,
) -> Result<Json<VerifyResp>, (StatusCode, String)> {
    auth::rotate_refresh(&state, &req.refresh)
        .map(|ok| {
            Json(VerifyResp {
                jwt: ok.jwt,
                refresh: ok.refresh,
                device_id: ok.device_id,
                account_id: ok.account_id,
            })
        })
        .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid refresh".into()))
}

pub fn routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/auth/request-code", axum::routing::post(request_code))
        .route("/auth/verify", axum::routing::post(verify))
        .route("/auth/refresh", axum::routing::post(refresh))
        .route("/devices", axum::routing::get(list_devices))
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
