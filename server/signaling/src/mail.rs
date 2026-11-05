//! 邮件传输层(spec §3.5/§3.6):验证码投递抽象 + 阿里云邮件推送 DirectMail
//! (SingleSendMail,RPC v1 手写签名 HMAC-SHA1,不引 SDK)+ dev 回显实现
//! + 未配置 fail-closed 实现。
//!
//! 装配([`assemble`],main 启动时一次):`--dev-echo-code` 开 → [`DevEcho`]
//! (优先,永不发信);否则 DM env 四键齐 → [`AliyunDm`];两者皆无 →
//! [`Unconfigured`](tracing::warn 于启动时暴露,request_code 一律 500)。
//!
//! 语义钉死:**发信成功才落库**(api 层先 send 后 INSERT;失败 500,不产半态)。

use anyhow::{anyhow, Context, Result};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::sync::Arc;
use std::time::Duration;

/// 验证码邮件发送器。`send` 返回 Err = 投递失败:调用方(api::request_code)
/// 必须先发信后落库,失败即 500 且不产生 codes 行(spec §3.5 无半态)。
/// 实现体为阻塞网络调用,调用方经 `tokio::task::spawn_blocking` 调度。
pub trait CodeMailer: Send + Sync {
    fn send(&self, email: &str, code: &str) -> Result<()>;
}

/// 验证码邮件纯文本正文(V3-2 §3.4:验证码独占一行,便于整行选中复制;
/// 作为 HtmlBody 的降级回退体,与 HTML 体携带同一码)。
pub fn code_mail_body(code: &str) -> String {
    format!("你的验证码\n\n{code}\n\n10 分钟内有效。若非你本人操作,请忽略。")
}

/// 验证码邮件 HTML 正文(V3-2 §3.4):SYNSHU 品牌字头 + 验证码独占文本
/// 节点(大号等宽、卡片框内、前后无粘连标点——长按/双击即整码复制)+
/// 时效句 + 定位表述脚注。自包含:纯内联样式、无外链无图片,主流客户端
/// (QQ/163/Gmail/Outlook)table 布局兼容。
pub fn code_mail_html(code: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN"><body style="margin:0;padding:0;background-color:#f2f3f8;">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background-color:#f2f3f8;padding:28px 12px;">
<tr><td align="center">
<table role="presentation" width="380" cellpadding="0" cellspacing="0" border="0" style="width:380px;max-width:100%;background-color:#ffffff;border-radius:14px;font-family:'Segoe UI','PingFang SC','Microsoft YaHei',sans-serif;overflow:hidden;">
<tr><td style="padding:30px 32px 4px;font-size:21px;font-weight:600;letter-spacing:6px;color:#1a2148;">SYNSHU</td></tr>
<tr><td style="padding:0 32px 22px;font-size:12px;letter-spacing:2px;color:#8a90ad;">星 枢 · 跨公网设备互传</td></tr>
<tr><td style="padding:0 32px;">
<div style="border:1px solid #e3e6f0;border-radius:10px;background-color:#f7f8fd;padding:20px 0;text-align:center;">
<div style="font-family:Consolas,'Courier New',monospace;font-size:30px;letter-spacing:8px;color:#1a2148;">{code}</div>
</div>
</td></tr>
<tr><td style="padding:14px 32px 30px;font-size:14px;line-height:1.7;color:#3c4256;">你的验证码,10 分钟内有效。若非你本人操作,请忽略。</td></tr>
</table>
</td></tr>
</table>
</body></html>"#
    )
}

/// dev 回显(--dev-echo-code / 测试基建):`send` 恒 Ok,永不发信。
pub struct DevEcho;

impl CodeMailer for DevEcho {
    fn send(&self, _email: &str, _code: &str) -> Result<()> {
        Ok(())
    }
}

/// 未装配(DM env 缺失且非 dev 门):`send` 恒 Err → request_code 500 fail-closed。
pub struct Unconfigured;

impl CodeMailer for Unconfigured {
    fn send(&self, _email: &str, _code: &str) -> Result<()> {
        Err(anyhow!(
            "mailer not configured: MTDSS_DM_* env unset and --dev-echo-code off"
        ))
    }
}

/// DM 配置(环境注入,不落库;spec §3.5 四键)。
#[derive(Clone)]
pub struct DmConfig {
    pub access_key_id: String,
    pub access_key_secret: String,
    /// 发信地址(MTDSS_DM_ACCOUNT,如 noreply@mtdss.top)
    pub account: String,
    /// 发件人名(MTDSS_DM_ALIAS,V3-2 起值=Synshu)
    pub alias: String,
}

impl DmConfig {
    /// 四键任一缺失即 Err(装配降级为 Unconfigured 并 warn)。
    pub fn from_env() -> Result<Self> {
        let key = |k: &str| -> Result<String> {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| anyhow!("missing env {k}"))
        };
        Ok(Self {
            access_key_id: key("MTDSS_DM_ACCESS_KEY_ID")?,
            access_key_secret: key("MTDSS_DM_ACCESS_KEY_SECRET")?,
            account: key("MTDSS_DM_ACCOUNT")?,
            alias: key("MTDSS_DM_ALIAS")?,
        })
    }
}

/// DM endpoint(host;A 记录直连 443)。
const DM_HOST: &str = "dm.aliyuncs.com";
const DM_VERSION: &str = "2015-11-23";
/// 邮件主题(V3-2 品牌化)。发件人显示名走部署 env `MTDSS_DM_ALIAS`
/// (值改 Synshu,见部署手册;代码只读 env)。
const DM_SUBJECT: &str = "Synshu 登录验证码";

/// 阿里云邮件推送 DirectMail(SingleSendMail)。阻塞 HTTPS POST
/// (rustls StreamOwned + std TcpStream;webpki 公共 CA 信任锚)。
pub struct AliyunDm {
    cfg: DmConfig,
    tls: Arc<rustls::ClientConfig>,
}

impl AliyunDm {
    pub fn from_env() -> Result<Self> {
        let cfg = DmConfig::from_env()?;
        Ok(Self {
            cfg,
            tls: Arc::new(dm_tls_config()),
        })
    }
}

/// 公共 CA 信任锚的 TLS 客户端配置(ring provider,与 core 信令客户端同源形态;
/// ensure_provider 先行——plain-http 产线进程未必装过默认 provider)。
fn dm_tls_config() -> rustls::ClientConfig {
    crate::tls::ensure_provider();
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("safe default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

impl CodeMailer for AliyunDm {
    fn send(&self, email: &str, code: &str) -> Result<()> {
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        let body = signed_single_send_mail(
            &self.cfg,
            email,
            &code_mail_body(code),
            &code_mail_html(code),
            &nonce,
            &now_dm_timestamp()?,
        );
        let (status, resp) = https_post_form(DM_HOST, &body, &self.tls)?;
        anyhow::ensure!(
            status == 200,
            "directmail http {status}: {}",
            truncate_for_log(&resp)
        );
        Ok(())
    }
}

/// UTC 时间戳(DM 要求 ISO8601 基本形 `YYYY-MM-DDTHH:MM:SSZ`)。
fn now_dm_timestamp() -> Result<String> {
    let fd =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    time::OffsetDateTime::now_utc()
        .format(&fd)
        .context("format dm timestamp")
}

fn truncate_for_log(s: &str) -> &str {
    let cut = s.char_indices().nth(200).map(|(i, _)| i).unwrap_or(s.len());
    &s[..cut]
}

/// 组装 SingleSendMail 请求体:参数按名排序的 urlencoded query + POP v1 签名
/// (`&Signature=`)。nonce/timestamp 由调用方注入(生产=随机 nonce + UTC;
/// 测试钉死可复现)。V3-2:TextBody(纯文本回退)与 HtmlBody(品牌 HTML)
/// 同时携带,同一验证码。
pub fn signed_single_send_mail(
    cfg: &DmConfig,
    to: &str,
    text: &str,
    html: &str,
    nonce: &str,
    timestamp: &str,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("AccessKeyId", &cfg.access_key_id),
        ("Action", "SingleSendMail"),
        ("AccountName", &cfg.account),
        ("AddressType", "1"),
        ("Format", "JSON"),
        ("FromAlias", &cfg.alias),
        ("HtmlBody", html),
        ("ReplyToAddress", "false"),
        ("SignatureMethod", "HMAC-SHA1"),
        ("SignatureNonce", nonce),
        ("SignatureVersion", "1.0"),
        ("Subject", DM_SUBJECT),
        ("TextBody", text),
        ("Timestamp", timestamp),
        ("ToAddress", to),
        ("Version", DM_VERSION),
    ];
    params.sort_by(|a, b| a.0.cmp(b.0));
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", pop_encode(k), pop_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let sig = pop_sign(&cfg.access_key_secret, &string_to_sign(&query));
    format!("{query}&Signature={}", pop_encode(&sig))
}

/// 阿里云 POP 签名专用百分号编码:RFC3986 unreserved(A-Za-z0-9-_.~)不编码,
/// 其余按 UTF-8 字节编 `%XX` 大写;特例 `*` 编为 `%2A`(空格是 %20 不是 +)。
pub fn pop_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'*' => out.push_str("%2A"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// RPC v1 待签串:`POST&%2F&` + pop_encode(排序后的 query)。
pub fn string_to_sign(sorted_query: &str) -> String {
    format!("POST&%2F&{}", pop_encode(sorted_query))
}

/// POP v1 签名:HMAC-SHA1(key = AccessKeySecret + "&",数据 = 待签串)→ base64。
pub fn pop_sign(access_key_secret: &str, sts: &str) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(format!("{access_key_secret}&").as_bytes())
            .expect("hmac accepts any key length");
    mac.update(sts.as_bytes());
    base64(&mac.finalize().into_bytes())
}

/// 标准 base64(RFC4648 字母表,带 padding)。仅服务 DM 签名,不追求通用。
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// 阻塞 HTTPS POST form(Connection: close,响应上限 64KiB)。
/// 返回 (HTTP 状态码, 响应体)。
fn https_post_form(
    host: &str,
    body: &str,
    tls: &Arc<rustls::ClientConfig>,
) -> Result<(u16, String)> {
    use std::io::{Read, Write};
    let sock = std::net::TcpStream::connect((host, 443))
        .with_context(|| format!("connect {host}:443"))?;
    sock.set_read_timeout(Some(Duration::from_secs(15)))?;
    sock.set_write_timeout(Some(Duration::from_secs(15)))?;
    let name: rustls::pki_types::ServerName<'static> = host
        .to_owned()
        .try_into()
        .with_context(|| format!("server name {host}"))?;
    let conn = rustls::ClientConnection::new(Arc::clone(tls), name)
        .context("tls client conn")?;
    let mut stream = rustls::StreamOwned::new(conn, sock);
    let req = format!(
        "POST / HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .context("tls write request")?;
    stream.flush().ok();
    // 读循环容忍「无 close_notify 的连接关闭」:阿里云 RPC 网关关连接不发
    // TLS close_notify,rustls 将其报为错误——但此时响应字节通常已完整到达
    // (Connection: close 语义下服务端发完即关)。已在读字节非空时采信并继续
    // 解析;一字节未得才是真失败。(2026-11-04 产线首触 DM 实测:curl 通、
    // 本客户端 100% 报 unexpected eof——read_to_end 把整单扔回。)
    let mut resp = Vec::with_capacity(4096);
    let mut buf = [0u8; 8 * 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                resp.extend_from_slice(&buf[..n]);
                if resp.len() >= 64 * 1024 {
                    break;
                }
            }
            Err(e) => {
                if resp.is_empty() {
                    return Err(anyhow::Error::from(e)).context("tls read response");
                }
                tracing::debug!(
                    "dm tls 流无 close_notify 关闭,采信已读 {} 字节继续解析",
                    resp.len()
                );
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&resp).into_owned();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("malformed http status line: {}", truncate_for_log(&text)))?;
    Ok((status, text))
}

/// 装配产物(main 启动时一次)。
pub enum Mailer {
    Aliyun(AliyunDm),
    Dev(DevEcho),
    Unconfigured(Unconfigured),
}

impl CodeMailer for Mailer {
    fn send(&self, email: &str, code: &str) -> Result<()> {
        match self {
            Mailer::Aliyun(m) => m.send(email, code),
            Mailer::Dev(m) => m.send(email, code),
            Mailer::Unconfigured(m) => m.send(email, code),
        }
    }
}

/// 装配(spec §3.5/§3.6):dev 门开 → DevEcho(优先,不发信);否则 DM env 四键
/// 齐 → AliyunDm;两者皆无 → Unconfigured + tracing::warn(启动时暴露;
/// request_code 将一律 500 fail-closed)。
pub fn assemble(dev_echo: bool) -> Mailer {
    if dev_echo {
        return Mailer::Dev(DevEcho);
    }
    match AliyunDm::from_env() {
        Ok(dm) => Mailer::Aliyun(dm),
        Err(e) => {
            tracing::warn!(
                "验证码邮件投递未配置(DM env 四键缺失且非 dev 门),request-code 将一律 500: {e:#}"
            );
            Mailer::Unconfigured(Unconfigured)
        }
    }
}
