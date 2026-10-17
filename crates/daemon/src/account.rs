//! 账号 REST 客户端:登录链路的 HTTP 侧(request-code / verify / refresh /
//! devices)+ 本机身份加载。
//!
//! REST 段自 `crates/cli/src/flow.rs` 搬移(Task 4,彼时 flow 尚在 cli);
//! flow.rs 整体迁入 daemon(Task 5)后,本机身份改调 `flow::load_identity`
//! 归一,不再保留重复实现。安全口径不变:对端公钥永远从服务器目录解析,
//! 信封内字段绝不用于密钥解析。

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use std::path::Path;
use std::time::Duration;

use crate::session::Session;

/// REST 请求超时(与 flow.rs 的 HTTP_TIMEOUT 同值)。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

// ---------- 身份 ----------

/// 加载 `home` 下的本机身份;首次运行自动生成并落盘 `identity.seed`(0600)。
/// 委托 `flow::load_identity`(Task 5 归一,消除 Task 4 期间与 flow 的重复实现)。
pub(crate) fn load_identity(home: &Path) -> Result<DeviceIdentity> {
    crate::flow::load_identity(home)
}

// ---------- HTTP 基建 ----------

/// REST 客户端:`ca` 存在时把它加为根证书(自签 https 场景),None 用默认
/// webpki 根。与 flow.rs 的 http_client 原样一致。
pub(crate) fn http_client(ca: Option<&Path>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(HTTP_TIMEOUT);
    if let Some(ca) = ca {
        let pem = std::fs::read(ca).with_context(|| format!("read ca {}", ca.display()))?;
        let cert = reqwest::Certificate::from_pem(&pem)?;
        b = b.add_root_certificate(cert);
    }
    b.build().context("build http client")
}

/// POST JSON 并解析响应体;非 2xx 时把状态码与响应文本带进错误——登录业务
/// 失败(如「验证码错误」)的上下文来自这里,不能再靠 error_for_status 丢掉。
async fn post_json(
    client: &reqwest::Client,
    url: String,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let resp = client.post(url).json(body).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("HTTP {status}: {text}");
    }
    Ok(resp.json().await?)
}

// ---------- 登录链路 REST ----------

/// POST /auth/request-code → dev 模式回显的六位验证码。
pub(crate) async fn request_code(server: &str, email: &str, ca: Option<&Path>) -> Result<String> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/request-code"),
        &serde_json::json!({ "email": email }),
    )
    .await
    .context("request-code")?;
    v["code"]
        .as_str()
        .context("no code in response")
        .map(str::to_owned)
}

/// POST /auth/verify → (jwt, refresh, device_id)。
pub(crate) async fn verify(
    server: &str,
    email: &str,
    code: &str,
    pubkey_hex: &str,
    name: &str,
    ca: Option<&Path>,
) -> Result<(String /*jwt*/, String /*refresh*/, String /*device_id*/)> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/verify"),
        &serde_json::json!({
            "email": email,
            "code": code,
            "pubkey": pubkey_hex,
            "name": name,
            "platform": std::env::consts::OS,
        }),
    )
    .await
    .context("verify(验证码错误或已过期?)")?;
    let jwt = v["jwt"].as_str().context("no jwt in response")?.to_string();
    let refresh = v["refresh"]
        .as_str()
        .context("no refresh in response")?
        .to_string();
    let device_id = v["device_id"]
        .as_str()
        .context("no device_id in response")?
        .to_string();
    Ok((jwt, refresh, device_id))
}

/// POST /auth/refresh 轮换令牌 → (新 jwt, 新 refresh)。
pub(crate) async fn refresh(server: &str, refresh: &str, ca: Option<&Path>) -> Result<(String, String)> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/refresh"),
        &serde_json::json!({ "refresh": refresh }),
    )
    .await
    .context("auth/refresh")?;
    let jwt = v["jwt"].as_str().context("no jwt in response")?.to_string();
    let refresh = v["refresh"]
        .as_str()
        .context("no refresh in response")?
        .to_string();
    Ok((jwt, refresh))
}

/// GET /devices(Bearer)→ 账号设备列表(原始 JSON 数组)。
pub(crate) async fn fetch_devices(s: &Session) -> Result<Vec<serde_json::Value>> {
    let v: serde_json::Value = http_client(s.ca.as_deref())?
        .get(format!("{}/devices", s.server))
        .bearer_auth(&s.jwt)
        .send()
        .await?
        .error_for_status()
        .context("list devices")?
        .json()
        .await?;
    Ok(v["devices"].as_array().cloned().unwrap_or_default())
}

/// 从服务器目录解析某 device_id 的 Ed25519 公钥。
/// 目录里没有该设备(或字段畸形)一律报错——绝不回退到信封内字段。
#[allow(dead_code)] // Task 6(信令握手前的密钥解析)启用
pub(crate) fn peer_pk_from(devices: &[serde_json::Value], device_id: &str) -> Result<VerifyingKey> {
    let entry = devices
        .iter()
        .find(|d| d["device_id"].as_str() == Some(device_id))
        .ok_or_else(|| anyhow!("device {device_id} not found in account directory"))?;
    let hexk = entry["pubkey"].as_str().context("peer pubkey missing")?;
    let bytes = hex::decode(hexk).context("peer pubkey is not hex")?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("peer pubkey is not 32 bytes"))?;
    Ok(VerifyingKey::from_bytes(&arr)?)
}
