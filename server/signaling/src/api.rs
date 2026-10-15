use crate::auth;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct RequestCodeReq {
    pub email: String,
}

#[derive(Serialize)]
pub struct CodeResp {
    pub code: String,
}

pub async fn request_code(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RequestCodeReq>,
) -> Result<Json<CodeResp>, (StatusCode, String)> {
    auth::request_code(&state, &req.email)
        .map(|code| Json(CodeResp { code }))
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
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
}
