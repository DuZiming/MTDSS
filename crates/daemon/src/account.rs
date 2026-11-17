//! 账号 REST 客户端:登录链路的 HTTP 侧(request-code / verify / refresh /
//! devices)+ V3-1 登录状态机的 REST 面(approve-code / approval-respond /
//! login-status / evict / qr 四端点 / kv.get/put)+ 本机身份加载。
//! Ruling R8 增补:手机侧扫码授权透传(qr-claim / qr-approve,Bearer=本机
//! 会话 JWT 代打,令牌不出 IPC)。
//!
//! REST 段自 `crates/cli/src/flow.rs` 搬移(Task 4,彼时 flow 尚在 cli);
//! flow.rs 整体迁入 daemon(Task 5)后,本机身份改调 `flow::load_identity`
//! 归一,不再保留重复实现。安全口径不变:对端公钥永远从服务器目录解析,
//! 信封内字段绝不用于密钥解析。
//!
//! V3-1:登录响应统一为 `status` 判别形态(§3.1 三形态 + 轮询终态),解析
//! 收口在 [`parse_login_step`];jwt/refresh 只在本 crate 内流转,IPC 投影
//! 永不携带(api.rs 的 step_json 只取 device_id/name 面束)。

use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::session::Session;

/// REST 请求超时(与 flow.rs 的 HTTP_TIMEOUT 同值)。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// 扫码二维码内容前缀(daemon 拼 URL,服务端不参与;spec §3.2)。
pub(crate) const QR_URL_PREFIX: &str = "https://www.mtdss.top/qr?t=";

// ---------- 登录状态机裁定形态 ----------

/// ok 形态的凭据束:jwt/refresh 为 daemon 内部持有(会话落盘),IPC 投影
/// 永不携带;device_id 出投影,name 由 IPC 层按请求/槽内候选名补(服务器
/// 目录才是名字权威,ok 响应不携名)。
#[derive(Debug, Clone)]
pub(crate) struct LoginOk {
    pub(crate) jwt: String,
    pub(crate) refresh: String,
    pub(crate) device_id: String,
}

/// 登录链路统一裁定形态:verify / submit_code / login.status / evict /
/// qr_status 同族响应的解析收口([`parse_login_step`])。Pending 仅来自
/// qr_status/轮询面(扫码等待);其余形态全族共用。
/// (相对计划钉束的一处增补:qr_status 的 pending 态需要独立变体——服务器
/// 轮询面 `{status:"pending",expires_at}` 无 session_id,挂 Approval 会
/// 误导投影;增补为加法变体,IPC 契约按 `status:"pending"` 落 golden。)
#[derive(Debug, Clone)]
pub(crate) enum LoginStep {
    /// 直入/收口:凭据入库,立即登录。
    Ok(LoginOk),
    /// 扫码等待(token 仍 pending/claimed)。
    Pending { expires_at: i64 },
    /// 审批挂起(受信终端弹窗/输码)。
    Approval { session_id: String, expires_at: i64 },
    /// 配额超额:单次踢选凭据 + 现有设备清单(强制选择屏)。
    Limit { enroll_token: String, devices: Vec<DeviceBrief> },
    /// 一票否决终态。
    Denied,
    /// 过期/已消费终态(轮询面对未知会话也按此呈现)。
    Expired,
}

/// limit 形态设备清单条目(踢选屏数据源;字段面与服务器 DeviceInfo 对齐,
/// 序列化键序随 serde_json Map 字母序:device_id/name/online/platform)。
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct DeviceBrief {
    pub(crate) device_id: String,
    pub(crate) name: String,
    pub(crate) platform: String,
    pub(crate) online: bool,
}

/// 扫码 begin 结果:token(128bit hex)与到期时刻。
pub(crate) struct QrBegin {
    pub(crate) qr_token: String,
    pub(crate) expires_at: i64,
}

/// 二维码内容(客户端拼 URL,服务端不参与):`https://www.mtdss.top/qr?t=<token>`。
pub(crate) fn qr_url(token: &str) -> String {
    format!("{QR_URL_PREFIX}{token}")
}

/// 服务器登录族响应 → 统一裁定形态。旧服务器容错(spec §7 部署顺序保险丝):
/// **响应缺 `status` 字段 → 按 ok 形态解析**;approval 形态响应可缺
/// session_id(login-status 轮询面不带),此时回退 `session_hint`(调用方
/// 传 op 请求里的会话号,投影仍回显完整形态)。未知 status 串与字段畸形
/// 一律 Err——调用方映射为 RPC 业务错,绝不静默当 ok。
pub(crate) fn parse_login_step(v: &Value, session_hint: &str) -> Result<LoginStep> {
    match v.get("status").and_then(Value::as_str) {
        None | Some("ok") => Ok(LoginStep::Ok(LoginOk {
            jwt: v["jwt"]
                .as_str()
                .context("no jwt in response")?
                .to_string(),
            refresh: v["refresh"]
                .as_str()
                .context("no refresh in response")?
                .to_string(),
            device_id: v["device_id"]
                .as_str()
                .context("no device_id in response")?
                .to_string(),
        })),
        Some("pending") => Ok(LoginStep::Pending {
            expires_at: v["expires_at"]
                .as_i64()
                .context("no expires_at in pending response")?,
        }),
        Some("approval") => Ok(LoginStep::Approval {
            session_id: v["session_id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| session_hint.to_string()),
            expires_at: v["expires_at"]
                .as_i64()
                .context("no expires_at in approval response")?,
        }),
        Some("limit") => Ok(LoginStep::Limit {
            enroll_token: v["enroll_token"]
                .as_str()
                .context("no enroll_token in limit response")?
                .to_string(),
            devices: v["devices"]
                .as_array()
                .context("no devices in limit response")?
                .iter()
                .map(|d| {
                    Ok(DeviceBrief {
                        device_id: d["device_id"]
                            .as_str()
                            .context("device entry missing device_id")?
                            .to_string(),
                        name: d["name"].as_str().unwrap_or_default().to_string(),
                        platform: d["platform"].as_str().unwrap_or_default().to_string(),
                        online: d["online"].as_bool().unwrap_or(false),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }),
        Some("denied") => Ok(LoginStep::Denied),
        Some("expired") => Ok(LoginStep::Expired),
        Some(other) => Err(anyhow!("unknown login status: {other}")),
    }
}

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

/// POST JSON,带回 (状态码, 响应文本):错误体可能是纯文本(axum 元组形态),
/// 由调用方按状态码分派(如 approve-code 的 409 = 已裁决,回退轮询收口)。
async fn post_json_reply(
    client: &reqwest::Client,
    url: String,
    body: &serde_json::Value,
) -> Result<(u16, String)> {
    let resp = client.post(url).json(body).send().await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    Ok((status, text))
}

// ---------- 登录链路 REST ----------

/// POST /auth/request-code → dev 回显门开时响应带 `code`(本地/e2e/冒烟),
/// 产线真实投递响应为 `{}`——返回 None 表示无回显(验证码走邮件,客户端
/// 手输),「有则填」不依赖自动预填(spec §3.6)。
pub(crate) async fn request_code(
    server: &str,
    email: &str,
    ca: Option<&Path>,
) -> Result<Option<String>> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/request-code"),
        &serde_json::json!({ "email": email }),
    )
    .await
    .context("request-code")?;
    Ok(v["code"].as_str().map(str::to_owned))
}

/// POST /auth/verify → 统一裁定形态(直入/审批挂起/配额超额;spec §3.1)。
pub(crate) async fn verify(
    server: &str,
    email: &str,
    code: &str,
    pubkey_hex: &str,
    name: &str,
    ca: Option<&Path>,
) -> Result<LoginStep> {
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
    parse_login_step(&v, "")
}

/// POST /auth/approve-code 的结局:approved(新裁决)或已裁决/过期(409,
/// 由调用方回退 login-status 收口——并发双路先到先裁的正常竞态)。
pub(crate) enum ApproveOutcome {
    Approved,
    AlreadySettled,
}

/// POST /auth/approve-code {session_id, code}(免认证):新终端直接输受信
/// 终端弹窗上的审批码 settle。200 → [`ApproveOutcome::Approved`];409(已
/// 裁决或已过期)→ `AlreadySettled`;其余(404 未知会话/401 码错[错满 5 次
/// 会话作废]/429 限速)→ Err(带 HTTP 上下文)。
pub(crate) async fn approve_code(
    server: &str,
    session_id: &str,
    code: &str,
    ca: Option<&Path>,
) -> Result<ApproveOutcome> {
    let (status, text) = post_json_reply(
        &http_client(ca)?,
        format!("{server}/auth/approve-code"),
        &serde_json::json!({ "session_id": session_id, "code": code }),
    )
    .await
    .context("approve-code")?;
    match status {
        200 => Ok(ApproveOutcome::Approved),
        409 => Ok(ApproveOutcome::AlreadySettled),
        _ => bail!("HTTP {status}: {text}"),
    }
}

/// POST /auth/login-status {session_id}(免认证轮询)→ 统一裁定形态。
/// approved 单次领票:并发/重复轮询只有一次拿到 ok,其余按 Expired 呈现。
pub(crate) async fn login_status(
    server: &str,
    session_id: &str,
    ca: Option<&Path>,
) -> Result<LoginStep> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/login-status"),
        &serde_json::json!({ "session_id": session_id }),
    )
    .await
    .context("login-status")?;
    // 轮询面响应不带 session_id:回填请求参数,投影形态仍完整
    parse_login_step(&v, session_id)
}

/// POST /auth/evict {enroll_token, device_id}(免认证踢选)→ 统一裁定形态
/// (ok 带新票;token 单次,自踢不可)。失败(401 票无效/404 目标不属此
/// 账号)带 HTTP 上下文 Err。
pub(crate) async fn evict(
    server: &str,
    enroll_token: &str,
    device_id: &str,
    ca: Option<&Path>,
) -> Result<LoginStep> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/evict"),
        &serde_json::json!({ "enroll_token": enroll_token, "device_id": device_id }),
    )
    .await
    .context("evict(凭据无效或目标设备不属此账号?)")?;
    parse_login_step(&v, "")
}

/// POST /auth/qr-begin {pubkey, name, platform}(免认证)→ token 与到期
/// 时刻。二维码内容由 daemon 拼([`qr_url`]),服务端不参与。
pub(crate) async fn qr_begin(
    server: &str,
    pubkey_hex: &str,
    name: &str,
    ca: Option<&Path>,
) -> Result<QrBegin> {
    let v = post_json(
        &http_client(ca)?,
        format!("{server}/auth/qr-begin"),
        &serde_json::json!({
            "pubkey": pubkey_hex,
            "name": name,
            "platform": std::env::consts::OS,
        }),
    )
    .await
    .context("qr-begin")?;
    Ok(QrBegin {
        qr_token: v["qr_token"]
            .as_str()
            .context("no qr_token in response")?
            .to_string(),
        expires_at: v["expires_at"]
            .as_i64()
            .context("no expires_at in response")?,
    })
}

/// GET /auth/qr-status?t=<token>(免认证轮询)→ 统一裁定形态(pending/
/// ok/denied/expired/limit;ok 单次领票)。
pub(crate) async fn qr_status(server: &str, qr_token: &str, ca: Option<&Path>) -> Result<LoginStep> {
    let resp = http_client(ca)?
        .get(format!("{server}/auth/qr-status"))
        .query(&[("t", qr_token)])
        .send()
        .await?
        .error_for_status()
        .context("qr-status")?;
    let v: serde_json::Value = resp.json().await?;
    parse_login_step(&v, "")
}

// ---------- 扫码授权透传(手机侧;Ruling R8) ----------

/// qr-claim 成功的候选面(确认弹层数据源;服务端 candidate 对象拍平前的
/// 原始三字段)。
pub(crate) struct QrCandidate {
    pub(crate) name: String,
    pub(crate) platform: String,
    pub(crate) expires_at: i64,
}

/// POST /auth/qr-claim 的裁定:200 认领成功(候选面,同认领者幂等回同面);
/// 404 二维码已死(不存在或已过期——服务端惰性落库后同 404);409 他端
/// 已认领或已 settle(先到先裁)。其余(401 鉴权/限速/网络)Err 带上下文。
pub(crate) enum QrClaimOutcome {
    Claimed(QrCandidate),
    Gone,
    Contested,
}

/// POST /auth/qr-claim {qr_token}(Bearer):手机扫后认领,首认领者钉死
/// claimed_by(daemon 持本机会话 JWT 代打,令牌不出 IPC——T8 卫生口径)。
pub(crate) async fn qr_claim(s: &Session, qr_token: &str) -> Result<QrClaimOutcome> {
    let resp = http_client(s.ca.as_deref())?
        .post(format!("{}/auth/qr-claim", s.server))
        .bearer_auth(&s.jwt)
        .json(&serde_json::json!({ "qr_token": qr_token }))
        .send()
        .await
        .context("qr-claim")?;
    match resp.status().as_u16() {
        200 => {
            let v: serde_json::Value = resp.json().await?;
            Ok(QrClaimOutcome::Claimed(QrCandidate {
                name: v["candidate"]["name"]
                    .as_str()
                    .context("no candidate.name in response")?
                    .to_string(),
                platform: v["candidate"]["platform"]
                    .as_str()
                    .context("no candidate.platform in response")?
                    .to_string(),
                expires_at: v["expires_at"]
                    .as_i64()
                    .context("no expires_at in response")?,
            }))
        }
        404 => Ok(QrClaimOutcome::Gone),
        409 => Ok(QrClaimOutcome::Contested),
        st => {
            let text = resp.text().await.unwrap_or_default();
            bail!("HTTP {st}: {text}")
        }
    }
}

/// POST /auth/qr-approve 的裁定:200 受理(裁决方向由 allow 投影:同意 →
/// 'approved',拒 → 'denied');409 已 settle 或已过期(单次裁决,重放/
/// 迟到同面);403 非认领者(他端先认领)。其余 Err 带上下文。
pub(crate) enum QrApproveOutcome {
    Accepted,
    Dead,
    Forbidden,
}

/// POST /auth/qr-approve {qr_token, allow}(Bearer):认领者裁决。allow=true
/// → state='approved',票由 PC 轮询单次领取收口(配额超额时 PC 面落 limit
/// 走踢选——手机侧不轮询,绝不染指单次票);false → 'denied' 终态。
pub(crate) async fn qr_approve(s: &Session, qr_token: &str, allow: bool) -> Result<QrApproveOutcome> {
    let resp = http_client(s.ca.as_deref())?
        .post(format!("{}/auth/qr-approve", s.server))
        .bearer_auth(&s.jwt)
        .json(&serde_json::json!({ "qr_token": qr_token, "allow": allow }))
        .send()
        .await
        .context("qr-approve")?;
    match resp.status().as_u16() {
        200 => Ok(QrApproveOutcome::Accepted),
        409 => Ok(QrApproveOutcome::Dead),
        403 => Ok(QrApproveOutcome::Forbidden),
        st => {
            let text = resp.text().await.unwrap_or_default();
            bail!("HTTP {st}: {text}")
        }
    }
}

/// POST /auth/approval-respond {session_id, allow}(Bearer):受信终端裁决
/// 弹窗。失败(404 未知会话/409 已裁决或已过期/403 非在线受信设备)带
/// HTTP 上下文 Err。
pub(crate) async fn approval_respond(
    server: &str,
    jwt: &str,
    session_id: &str,
    allow: bool,
    ca: Option<&Path>,
) -> Result<()> {
    let resp = http_client(ca)?
        .post(format!("{server}/auth/approval-respond"))
        .bearer_auth(jwt)
        .json(&serde_json::json!({ "session_id": session_id, "allow": allow }))
        .send()
        .await
        .context("approval-respond")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("HTTP {status}: {text}");
    }
    Ok(())
}

// ---------- 账号数据 KV 透传(spec §3.8) ----------

/// GET /data/{key}(Bearer)→ 键值。404 = 合法未命(无此键),返回 None
/// 与其余失败(401 鉴权/网络)区分开——调用方把未命映射为 typed not_found,
/// 其余为业务错。
pub(crate) async fn kv_get(s: &Session, key: &str) -> Result<Option<String>> {
    let resp = http_client(s.ca.as_deref())?
        .get(format!("{}/data/{key}", s.server))
        .bearer_auth(&s.jwt)
        .send()
        .await?;
    match resp.status().as_u16() {
        200 => {
            let v: serde_json::Value = resp.json().await?;
            Ok(Some(
                v["value"]
                    .as_str()
                    .context("no value in data response")?
                    .to_string(),
            ))
        }
        404 => Ok(None),
        st => {
            let text = resp.text().await.unwrap_or_default();
            bail!("HTTP {st}: {text}")
        }
    }
}

/// PUT /data/{key} {value}(Bearer;value ≤256KiB 由服务端校验)→ UPSERT。
pub(crate) async fn kv_put(s: &Session, key: &str, value: &str) -> Result<()> {
    let resp = http_client(s.ca.as_deref())?
        .put(format!("{}/data/{key}", s.server))
        .bearer_auth(&s.jwt)
        .json(&serde_json::json!({ "value": value }))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("HTTP {status}: {text}");
    }
    Ok(())
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

/// 401 的可检测根错误:`fetch_devices` 把「访问令牌被拒」钉成可 downcast
/// 的标记([`DevicesUnauthorized`] 在 anyhow 链根),外层 Display 与既有
/// 文案一致(「list devices: HTTP status client error (401 …)」),`{:#}`
/// 渲染逐字节不变——识别面只对内,由 [`fetch_devices_with_refresh`]
/// downcast 触发就地轮换,日志/错误面照旧。
#[derive(Debug)]
struct DevicesUnauthorized {
    /// 原始 reqwest 错误(error_for_status 产物),作 source 保既有文案。
    source: reqwest::Error,
}

impl std::fmt::Display for DevicesUnauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("list devices")
    }
}

impl std::error::Error for DevicesUnauthorized {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// GET /devices(Bearer)→ 账号设备列表(原始 JSON 数组)。401(访问令牌
/// 失效)以 [`DevicesUnauthorized`] 为链根返回,文案不变;识别与自愈(轮换
/// 重试)收口在 [`fetch_devices_with_refresh`],直接调用本函数的行为与
/// 迁移前一致(401 同样是带「list devices」上下文的 Err)。
pub(crate) async fn fetch_devices(s: &Session) -> Result<Vec<serde_json::Value>> {
    let resp = http_client(s.ca.as_deref())?
        .get(format!("{}/devices", s.server))
        .bearer_auth(&s.jwt)
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let source = resp
            .error_for_status()
            .expect_err("status 已判 401,error_for_status 必为 Err");
        return Err(anyhow::Error::new(DevicesUnauthorized { source }));
    }
    let v: serde_json::Value = resp
        .error_for_status()
        .context("list devices")?
        .json()
        .await?;
    Ok(v["devices"].as_array().cloned().unwrap_or_default())
}

/// GET /devices + 401 就地轮换自愈(HTTP 面对齐 WS 面):访问令牌 TTL(24h)
/// 期间长连的 WS 只在握手验签,连接存活就永不触发轮换——下一次 job 起手的
/// REST 拉目录撞 401 是唯一暴露点。此处与 hub.rs 的 AuthDenied 处置同口径:
/// [`refresh`] 轮换恰一次,经 `persist` 写回(产线接 `state.update_session`,
/// 与 WS 面同一写锁)。persist 是**卫生面**而非保命面:服务端轮换无状态
/// (auth.rs `rotate_refresh` 只验签+查设备/账号在册,不吊销旧 refresh),
/// 落盘只是让新访问令牌即时可用、免下窗重复轮换;落盘失败告警沿用内存
/// 令牌重试,不弃自愈。再用新令牌重试**恰一次**(无循环)。其余错误原样
/// 透传(绝不轮换);轮换失败则浮出**原始 401**(拿轮换错误遮掩会让排障
/// 面失真),重试不发生。
pub(crate) async fn fetch_devices_with_refresh(
    s: &Session,
    persist: impl Fn(&Session) -> Result<()>,
) -> Result<Vec<serde_json::Value>> {
    match fetch_devices(s).await {
        Ok(devices) => Ok(devices),
        Err(err) if err.downcast_ref::<DevicesUnauthorized>().is_some() => {
            tracing::warn!("设备目录 401:访问令牌疑似过期(长连跨 TTL),就地轮换重试");
            let (jwt, refresh) = match refresh(&s.server, &s.refresh, s.ca.as_deref()).await {
                Ok(x) => x,
                Err(refresh_err) => {
                    tracing::warn!(
                        error = format!("{refresh_err:#}"),
                        "令牌轮换失败,沿用原始 401 上抛"
                    );
                    return Err(err);
                }
            };
            let mut rotated = s.clone();
            rotated.jwt = jwt;
            rotated.refresh = refresh;
            match persist(&rotated) {
                Ok(()) => tracing::info!("访问令牌已轮换并写回会话(HTTP 面)"),
                Err(err) => {
                    tracing::warn!(error = format!("{err:#}"), "轮换令牌落盘失败,沿用内存令牌重试")
                }
            }
            fetch_devices(&rotated).await
        }
        Err(err) => Err(err),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// 旧服务器容错(spec §7 部署顺序保险丝):verify 响应缺 `status` 字段
    /// → 按 ok 形态解析(升级窗口期三端先行、服务端后升)。
    #[test]
    fn legacy_ok_without_status_field_parses_as_ok() {
        let v = serde_json::json!({
            "jwt": "j", "refresh": "r", "device_id": "dev-1",
        });
        match parse_login_step(&v, "").expect("缺 status 字段必须按 ok 解析") {
            LoginStep::Ok(ok) => {
                assert_eq!(ok.jwt, "j");
                assert_eq!(ok.refresh, "r");
                assert_eq!(ok.device_id, "dev-1");
            }
            other => panic!("须为 Ok: {other:?}"),
        }
    }

    /// 新形态全族:status 判别面逐变体解析(含 ok 显式形态与轮询终态)。
    #[test]
    fn status_tagged_variants_parse() {
        let ok = parse_login_step(
            &serde_json::json!({
                "status": "ok", "jwt": "j2", "refresh": "r2",
                "device_id": "dev-2", "account_id": "acct",
            }),
            "",
        )
        .expect("ok 显式形态");
        assert!(matches!(ok, LoginStep::Ok(ref o) if o.device_id == "dev-2"));

        let approval = parse_login_step(
            &serde_json::json!({ "status": "approval", "session_id": "s1", "expires_at": 300 }),
            "",
        )
        .expect("approval 形态");
        assert!(
            matches!(approval, LoginStep::Approval { ref session_id, expires_at }
                if session_id == "s1" && expires_at == 300),
            "approval 形态: {approval:?}"
        );

        // 轮询面(login-status)的 approval 响应不带 session_id → 回退 hint
        let hinted = parse_login_step(
            &serde_json::json!({ "status": "approval", "expires_at": 42 }),
            "sess-hint",
        )
        .expect("approval 缺 session_id 须回退 hint");
        assert!(
            matches!(hinted, LoginStep::Approval { ref session_id, .. } if session_id == "sess-hint"),
            "hint 回填: {hinted:?}"
        );

        let limit = parse_login_step(
            &serde_json::json!({
                "status": "limit", "enroll_token": "t1",
                "devices": [
                    {"device_id": "d1", "name": "n1", "platform": "windows", "online": true},
                    {"device_id": "d2", "name": "n2", "platform": "linux", "online": false},
                ],
            }),
            "",
        )
        .expect("limit 形态");
        match limit {
            LoginStep::Limit { enroll_token, devices } => {
                assert_eq!(enroll_token, "t1");
                assert_eq!(devices.len(), 2);
                assert_eq!(devices[0].device_id, "d1");
                assert!(devices[0].online);
                assert!(!devices[1].online);
            }
            other => panic!("须为 Limit: {other:?}"),
        }

        let pending = parse_login_step(
            &serde_json::json!({ "status": "pending", "expires_at": 120 }),
            "",
        )
        .expect("pending 形态(qr 轮询)");
        assert!(matches!(pending, LoginStep::Pending { expires_at } if expires_at == 120));

        assert!(matches!(
            parse_login_step(&serde_json::json!({ "status": "denied" }), ""),
            Ok(LoginStep::Denied)
        ));
        assert!(matches!(
            parse_login_step(&serde_json::json!({ "status": "expired" }), ""),
            Ok(LoginStep::Expired)
        ));
    }

    /// 畸形面:ok 形态缺凭据 / 未知 status 串 / limit 缺设备清单 → Err
    /// (调用方映射业务错,绝不静默当 ok)。
    #[test]
    fn malformed_responses_are_errors() {
        // 旧形态容错只对「缺 status」生效,ok 形态本身缺凭据仍是错
        assert!(parse_login_step(&serde_json::json!({ "status": "ok" }), "").is_err());
        assert!(
            parse_login_step(&serde_json::json!({ "status": "mystery" }), "").is_err(),
            "未知 status 串必须 Err"
        );
        assert!(parse_login_step(
            &serde_json::json!({ "status": "limit", "enroll_token": "t" }),
            ""
        )
        .is_err());
        assert!(
            parse_login_step(&serde_json::json!({ "status": "approval" }), "").is_err(),
            "approval 缺 expires_at 必须 Err"
        );
    }

    /// 二维码内容拼接:固定前缀 + 单次 token(spec §3.2,服务端不参与)。
    #[test]
    fn qr_url_prefix_is_pinned() {
        assert_eq!(qr_url("abcd"), "https://www.mtdss.top/qr?t=abcd");
    }

    /// 回环请求的代理豁免(本模块全部 HTTP 测试共用一把 Once):reqwest
    /// 在 client 构建时读 NO_PROXY,http_client 按请求现建,设置先行即免疫。
    fn loopback_no_proxy() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
            std::env::set_var("no_proxy", "127.0.0.1,localhost");
        });
    }

    /// 本地 axum 回环服务:127.0.0.1 随机口,返回 base URL(测试进程同生共
    /// 死,不另设关停句柄)。
    async fn spawn_loopback(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// 待测会话:固定旧令牌面(过期场景由服务器 401 判定,令牌串本身不承载真伪)。
    fn test_session(server: &str) -> Session {
        Session {
            server: server.to_string(),
            jwt: "stale-jwt".into(),
            refresh: "stale-refresh".into(),
            device_id: "dev-loop".into(),
            ca: None,
        }
    }

    /// request_code 对 echo 关闭的 `{}` 响应容忍(spec §3.6「有则填」):
    /// 产线真实投递无回显,验证码走邮件,不得因缺 code 字段报错。
    #[tokio::test]
    async fn request_code_tolerates_echo_off() {
        loopback_no_proxy();
        let app = axum::Router::new().route(
            "/auth/request-code",
            axum::routing::post(|| async { axum::Json(serde_json::json!({})) }),
        );
        let server = spawn_loopback(app).await;
        let code = request_code(&server, "x@y.z", None)
            .await
            .expect("echo 关闭响应 {} 必须容忍");
        assert_eq!(code, None, "无回显 → None(邮件投递,手输)");
    }

    // ---------- fetch_devices_with_refresh(401 就地轮换自愈) ----------

    /// 401 自愈服务面:首次 /devices 撞旧令牌回 401,轮换后放行;记录每趟
    /// Authorization 头与 /auth/refresh 命中数,重试语义逐字可见。
    fn self_heal_app(
        fetches: Arc<AtomicUsize>,
        refreshes: Arc<AtomicUsize>,
        bearers: Arc<Mutex<Vec<String>>>,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                "/devices",
                axum::routing::get(move |headers: axum::http::HeaderMap| {
                    let (f, b) = (fetches.clone(), bearers.clone());
                    async move {
                        let n = f.fetch_add(1, Ordering::Relaxed);
                        b.lock().unwrap().push(
                            headers
                                .get(axum::http::header::AUTHORIZATION)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        );
                        if n == 0 {
                            (axum::http::StatusCode::UNAUTHORIZED, "expired".to_string())
                        } else {
                            let body = serde_json::json!({
                                "devices": [
                                    {"device_id": "d1", "name": "n1", "platform": "windows", "online": true},
                                ],
                            });
                            (axum::http::StatusCode::OK, body.to_string())
                        }
                    }
                }),
            )
            .route(
                "/auth/refresh",
                axum::routing::post(move || {
                    let r = refreshes.clone();
                    async move {
                        r.fetch_add(1, Ordering::Relaxed);
                        axum::Json(serde_json::json!({
                            "jwt": "rotated-jwt",
                            "refresh": "rotated-refresh",
                        }))
                    }
                }),
            )
    }

    /// 分支②:401 → 轮换恰一次 → persist 恰一次(rotated 会话,server/
    /// device_id 等原样保留)→ 新令牌重试恰一次成功(Authorization 逐字可见)。
    #[tokio::test]
    async fn wrapper_rotates_on_401_persists_and_retries_once() {
        loopback_no_proxy();
        let fetches = Arc::new(AtomicUsize::new(0));
        let refreshes = Arc::new(AtomicUsize::new(0));
        let bearers: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let server = spawn_loopback(self_heal_app(
            fetches.clone(),
            refreshes.clone(),
            bearers.clone(),
        ))
        .await;
        let s = test_session(&server);

        let persisted = Arc::new(Mutex::new(Vec::<Session>::new()));
        let p = persisted.clone();
        let devs = fetch_devices_with_refresh(&s, move |rotated: &Session| {
            p.lock().unwrap().push(rotated.clone());
            Ok(())
        })
        .await
        .expect("401 后自愈必须成功");

        assert_eq!(devs.len(), 1, "重试面须拿到设备清单");
        assert_eq!(fetches.load(Ordering::Relaxed), 2, "重试恰一次");
        assert_eq!(refreshes.load(Ordering::Relaxed), 1, "轮换恰一次");
        let p = persisted.lock().unwrap();
        assert_eq!(p.len(), 1, "persist 恰一次");
        assert_eq!(p[0].jwt, "rotated-jwt", "落盘须带轮换后 jwt");
        assert_eq!(p[0].refresh, "rotated-refresh", "落盘须带轮换后 refresh");
        assert_eq!(p[0].server, s.server, "rotated 会话其余字段原样保留");
        assert_eq!(p[0].device_id, s.device_id);
        drop(p);
        assert_eq!(
            bearers.lock().unwrap().as_slice(),
            ["Bearer stale-jwt", "Bearer rotated-jwt"],
            "首趟旧令牌撞 401,重试带轮换后令牌"
        );
    }

    /// 分支①:非 401(403 演示)原样透传——不轮换、不落盘、不重试。
    #[tokio::test]
    async fn wrapper_passes_through_non_401_without_rotation() {
        loopback_no_proxy();
        let refreshes = Arc::new(AtomicUsize::new(0));
        let fetches = Arc::new(AtomicUsize::new(0));
        let r = refreshes.clone();
        let f = fetches.clone();
        let app = axum::Router::new()
            .route(
                "/devices",
                axum::routing::get(move || {
                    let f = f.clone();
                    async move {
                        f.fetch_add(1, Ordering::Relaxed);
                        (axum::http::StatusCode::FORBIDDEN, "forbidden".to_string())
                    }
                }),
            )
            .route(
                "/auth/refresh",
                axum::routing::post(move || {
                    let r = r.clone();
                    async move {
                        r.fetch_add(1, Ordering::Relaxed);
                        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "nope".to_string())
                    }
                }),
            );
        let server = spawn_loopback(app).await;

        let persisted = Arc::new(Mutex::new(Vec::<Session>::new()));
        let p = persisted.clone();
        let err = fetch_devices_with_refresh(&test_session(&server), move |rotated: &Session| {
            p.lock().unwrap().push(rotated.clone());
            Ok(())
        })
        .await
        .expect_err("403 必须原样 Err");

        let rendered = format!("{err:#}");
        assert!(rendered.contains("list devices"), "文案面不变: {rendered}");
        assert!(rendered.contains("403"), "透传错误带 403 上下文: {rendered}");
        assert_eq!(fetches.load(Ordering::Relaxed), 1, "非 401 不重试");
        assert_eq!(refreshes.load(Ordering::Relaxed), 0, "非 401 绝不轮换");
        assert!(persisted.lock().unwrap().is_empty(), "非 401 绝不落盘");
    }

    /// 分支③:401 且轮换失败 → 原始 401 上浮(「list devices: HTTP …
    /// 401 …」文案不变,标记仍在链根),轮换错误绝不遮掩;不重试、不落盘。
    #[tokio::test]
    async fn wrapper_surfaces_original_401_when_refresh_fails() {
        loopback_no_proxy();
        let fetches = Arc::new(AtomicUsize::new(0));
        let f = fetches.clone();
        let app = axum::Router::new()
            .route(
                "/devices",
                axum::routing::get(move |_: axum::http::HeaderMap| {
                    let f = f.clone();
                    async move {
                        f.fetch_add(1, Ordering::Relaxed);
                        (axum::http::StatusCode::UNAUTHORIZED, "expired".to_string())
                    }
                }),
            )
            .route(
                "/auth/refresh",
                axum::routing::post(|| async {
                    (axum::http::StatusCode::UNAUTHORIZED, "refresh revoked".to_string())
                }),
            );
        let server = spawn_loopback(app).await;

        let persisted = Arc::new(Mutex::new(Vec::<Session>::new()));
        let p = persisted.clone();
        let err = fetch_devices_with_refresh(&test_session(&server), move |rotated: &Session| {
            p.lock().unwrap().push(rotated.clone());
            Ok(())
        })
        .await
        .expect_err("轮换失败仍须 Err(原始 401)");

        let rendered = format!("{err:#}");
        assert!(rendered.contains("list devices"), "原 401 文案逐字保留: {rendered}");
        assert!(rendered.contains("401"), "须浮出原始 401: {rendered}");
        assert!(
            !rendered.contains("auth/refresh"),
            "轮换错误不得遮掩原 401: {rendered}"
        );
        assert!(
            err.downcast_ref::<DevicesUnauthorized>().is_some(),
            "标记须仍在链根(识别面,后续调用方/日志可辨)"
        );
        assert_eq!(fetches.load(Ordering::Relaxed), 1, "轮换失败不重试");
        assert!(persisted.lock().unwrap().is_empty(), "轮换失败绝不落盘");
    }

    /// 落盘失败不折断自愈(hub.rs 同语义):persist 返回 Err 仅告警,
    /// 重试仍带轮换后令牌——自愈不因磁盘抖动失能(新令牌只在内存,
    /// 下一次 HTTP/WS 面命中时再走轮换落盘)。
    #[tokio::test]
    async fn wrapper_retries_with_rotated_token_even_if_persist_fails() {
        loopback_no_proxy();
        let fetches = Arc::new(AtomicUsize::new(0));
        let refreshes = Arc::new(AtomicUsize::new(0));
        let bearers: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let server = spawn_loopback(self_heal_app(
            fetches.clone(),
            refreshes.clone(),
            bearers.clone(),
        ))
        .await;

        let persist_calls = Arc::new(AtomicUsize::new(0));
        let c = persist_calls.clone();
        let devs = fetch_devices_with_refresh(&test_session(&server), move |_: &Session| {
            c.fetch_add(1, Ordering::Relaxed);
            Err(anyhow!("disk full"))
        })
        .await
        .expect("落盘失败不得折断自愈");

        assert_eq!(devs.len(), 1, "重试成功面不变");
        assert_eq!(persist_calls.load(Ordering::Relaxed), 1, "persist 恰一次(虽败仍试)");
        assert_eq!(fetches.load(Ordering::Relaxed), 2, "重试恰一次");
        assert_eq!(refreshes.load(Ordering::Relaxed), 1);
        assert_eq!(
            bearers.lock().unwrap().as_slice(),
            ["Bearer stale-jwt", "Bearer rotated-jwt"],
            "重试仍带轮换后令牌"
        );
    }
}
