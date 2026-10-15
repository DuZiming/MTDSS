//! send/recv 信令 + 连接编排。
//!
//! 安全模型（不可妥协）：
//! - 对端公钥永远从服务器目录（GET /devices，按 device_id）解析，再用
//!   `PeersCache` 做 TOFU 固定：首次记录，之后必须一致。信封内的
//!   `sender_device_id` 字段是未签名数据，绝不用于密钥解析——验签一律用
//!   服务器目录 + TOFU 得到的公钥。
//! - 任何面向对端的信封（seal）之前必须先完成 TOFU 校验。
//! - 所有信令等待整体受 `SIGNAL_TIMEOUT` 约束，错误帧/连接关闭立即上抛；
//!   每个候选地址的 QUIC 连接受 `CONNECT_TIMEOUT` 约束。

use crate::session::{mtdss_home, PeersCache, Session};
use anyhow::{anyhow, Context, Result};
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::client::{ClientEvent, SignalingClient};
use mtdss_core::signaling::envelope::{open, seal, SignalBody};
use mtdss_core::transport::{endpoint as ep, transfer};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

/// 信令体等待的整体超时（ConnectAccept / Candidates / ConnectRequest 之外
/// 的往返各算一次等待）。
pub(crate) const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);
/// 单个候选地址的 QUIC 握手超时。
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 发送方在收到 Candidates 后应发起 QUIC 连接的最长等待
/// （发送方逐候选尝试整体约 15s，这里对齐放宽一档）。
pub(crate) const ACCEPT_TIMEOUT: Duration = Duration::from_secs(15);
/// REST 请求超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

// ---------- 身份与服务器目录 ----------

/// 加载本机身份；首次运行自动生成并落盘 `identity.seed`（0600）。
pub fn load_identity() -> Result<DeviceIdentity> {
    let p = mtdss_home().join("identity.seed");
    if p.exists() {
        DeviceIdentity::from_seed_file(&p)
    } else {
        let id = DeviceIdentity::generate();
        id.save_seed(&p)?;
        Ok(id)
    }
}

/// GET /devices（Bearer）→ 账号设备列表（原始 JSON 数组）。
pub async fn fetch_devices(s: &Session) -> Result<Vec<serde_json::Value>> {
    let v: serde_json::Value = http_client()?
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

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("build http client")
}

/// 信令 WS 地址：server 的 http(s) 前缀替换为 ws(s) 后接 `/ws`。
fn ws_url(server: &str) -> String {
    format!("{server}/ws").replace("http", "ws")
}

/// 连接信令通道（握手本身也有超时，避免对端无响应时无限挂起）。
async fn signaling_connect(
    session: &Session,
    device_id: &str,
) -> Result<(SignalingClient, mpsc::Receiver<ClientEvent>)> {
    let url = ws_url(&session.server);
    let (client, ev) = tokio::time::timeout(
        SIGNAL_TIMEOUT,
        SignalingClient::connect(&url, &session.jwt, device_id),
    )
    .await
    .map_err(|_| anyhow!("timed out connecting signaling at {url}"))?
    .context("connect signaling ws")?;
    Ok((client, ev))
}

/// 从服务器目录解析某 device_id 的 Ed25519 公钥。
/// 目录里没有该设备（或字段畸形）一律报错——绝不回退到信封内字段。
fn peer_pk_from(devices: &[serde_json::Value], device_id: &str) -> Result<VerifyingKey> {
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

/// TOFU 固定并持久化：首次记录，之后必须一致，变更即硬错误。
fn tofu_pin(device_id: &str, pk: &VerifyingKey) -> Result<()> {
    let path = mtdss_home().join("peers.json");
    let mut cache = PeersCache::load(&path)?;
    cache.expected_peer(device_id, pk)?;
    cache.save(&path)?;
    Ok(())
}

fn rand_session_id() -> Vec<u8> {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b.to_vec()
}

// ---------- 候选地址枚举 ----------

/// 本机 QUIC 候选地址列表：环回 + 全部非环回 IPv4 接口地址（含端口）。
pub fn local_candidates(port: u16) -> Vec<String> {
    let mut out = vec![format!("127.0.0.1:{port}")];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for ia in ifaces {
            if ia.ip().is_ipv4() && !ia.ip().is_loopback() {
                out.push(SocketAddr::new(ia.ip(), port).to_string());
            }
        }
    }
    out
}

// ---------- 信令等待 ----------

/// 等待来自 `from` 的一条信令体：逐条 `Relayed{from}` 用 `sender_pk` 解封
/// 验签，返回第一条满足 `pred` 的信令体。`SIGNAL_TIMEOUT` 包裹整个等待；
/// 服务器 Error 帧（例如对端离线 code=2）与连接关闭立即作为错误上抛，
/// 绝不挂死。
async fn wait_opened(
    ev: &mut mpsc::Receiver<ClientEvent>,
    from: &str,
    me: &DeviceIdentity,
    sender_pk: &VerifyingKey,
    pred: impl Fn(&SignalBody) -> bool,
) -> Result<SignalBody> {
    let wait = async {
        loop {
            match ev.recv().await.context("signaling closed")? {
                ClientEvent::Relayed { from: f, payload } if f == from => {
                    let body = open(&payload, me, sender_pk)
                        .map_err(|e| anyhow!("cannot open envelope from {from}: {e}"))?;
                    if pred(&body) {
                        return Ok(body);
                    }
                    // 同一会话的其他信令体：忽略，继续等
                }
                ClientEvent::Relayed { .. } => {} // 其他发送者：忽略
                ClientEvent::Presence { .. } => {}
                ClientEvent::Error { code, message } => {
                    return Err(anyhow!("signaling error {code}: {message}"))
                }
                ClientEvent::Closed => return Err(anyhow!("signaling connection closed")),
            }
        }
    };
    tokio::time::timeout(SIGNAL_TIMEOUT, wait)
        .await
        .map_err(|_| anyhow!("timed out after {SIGNAL_TIMEOUT:?} waiting for signal from {from}"))?
}

// ---------- send ----------

/// 发送单个文件给账号内另一设备。
pub async fn run_send(
    session: &Session,
    file: &Path,
    to_device: &str,
    out_report: &dyn Fn(&str),
) -> Result<()> {
    let me = load_identity()?;
    anyhow::ensure!(
        me.device_id == session.device_id,
        "identity seed does not match session.json device_id"
    );

    // 发送方自己也要校验 rel：从 file_name 推导，必须通过 T10 校验
    let rel = file
        .file_name()
        .and_then(|n| n.to_str())
        .context("file path has no UTF-8 file name")?
        .to_string();
    anyhow::ensure!(
        mtdss_core::protocol::manifest::validate_rel_path(&rel),
        "derived rel path is invalid: {rel}"
    );

    // 对端公钥：服务器目录 + TOFU（首次记录 / 之后必须一致）
    let devices = fetch_devices(session).await?;
    let peer_pk = peer_pk_from(&devices, to_device)?;
    tofu_pin(to_device, &peer_pk)?;

    // 信令：seal ConnectRequest → 等 ConnectAccept → 等 Candidates
    let (client, mut ev) = signaling_connect(session, &me.device_id).await?;
    let session_id = rand_session_id();
    client
        .relay(
            to_device,
            &seal(&me, &peer_pk, &SignalBody::ConnectRequest { session_id: session_id.clone() }),
        )
        .await?;

    wait_opened(&mut ev, to_device, &me, &peer_pk, |b| {
        matches!(b, SignalBody::ConnectAccept { session_id: s } if *s == session_id)
    })
    .await
    .context("waiting for ConnectAccept")?;

    let addrs = match wait_opened(&mut ev, to_device, &me, &peer_pk, |b| {
        matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
    })
    .await
    .context("waiting for Candidates")?
    {
        SignalBody::Candidates { addrs, .. } => addrs,
        other => anyhow::bail!("unexpected signal body {other:?}"),
    };

    // 逐候选 QUIC 连接（每候选 CONNECT_TIMEOUT，最多约 15s）
    let ep_a = ep::make_endpoint("0.0.0.0:0".parse().expect("valid socket addr"))?;
    let mut last_err: Option<anyhow::Error> = None;
    let mut conn = None;
    for addr in &addrs {
        let Ok(sa) = addr.parse::<SocketAddr>() else {
            last_err = Some(anyhow!("peer sent unparsable candidate {addr}"));
            continue;
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, ep::connect_pinned(&ep_a, sa, &peer_pk, &me))
            .await
        {
            Ok(Ok(c)) => {
                conn = Some(c);
                break;
            }
            Ok(Err(e)) => last_err = Some(e.context(format!("connect to {addr}"))),
            Err(_) => last_err = Some(anyhow!("connect timeout to {addr}")),
        }
    }
    let conn = conn.ok_or_else(|| {
        last_err.unwrap_or_else(|| anyhow!("peer offered no usable candidates"))
    })?;

    // 传输（大文件耗时合法，不限时；QUIC 断连会自行报错）
    let report = transfer::send_file(&conn, file, &rel, &me, &|done, total| {
        eprintln!("progress {done}/{total} bytes");
    })
    .await
    .context("transfer")?;
    drop(client);
    drop(ev);
    out_report(&format!("sent {} bytes ({rel})", report.bytes_sent));
    Ok(())
}

// ---------- recv ----------

/// 清理输出目录中的陈旧 `*.mtdss-part` 中间文件（传输中止/出错后遗留）。
fn sweep_part_files(out_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(out_dir) else {
        tracing::debug!("cannot list {} for part sweep", out_dir.display());
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".mtdss-part") {
                match std::fs::remove_file(&p) {
                    Ok(()) => tracing::info!("removed stale part {}", p.display()),
                    Err(e) => tracing::warn!("cannot remove {}: {e}", p.display()),
                }
            }
        }
    }
}

/// 等待第一条 ConnectRequest：对每个 `Relayed` 事件用服务器目录解析发送方
/// 公钥（未知发送者即报错），解封验签后只接受 ConnectRequest。recv 的语义
/// 就是长期等待来件，因此此循环不设超时；Error/Closed 立即上抛。
async fn wait_connect_request(
    session: &Session,
    ev: &mut mpsc::Receiver<ClientEvent>,
    me: &DeviceIdentity,
) -> Result<(String, VerifyingKey, Vec<u8>)> {
    loop {
        let (from, payload) = match ev.recv().await.context("signaling closed")? {
            ClientEvent::Relayed { from, payload } => (from, payload),
            ClientEvent::Presence { .. } => continue,
            ClientEvent::Error { code, message } => {
                return Err(anyhow!("signaling error {code}: {message}"))
            }
            ClientEvent::Closed => return Err(anyhow!("signaling connection closed")),
        };
        // 密钥解析只信服务器目录（信封 sender 字段未签名）
        let devices = fetch_devices(session).await?;
        let pk = peer_pk_from(&devices, &from)?;
        // 打开任何信封之前先做 TOFU 固定：首次记录，之后必须一致
        tofu_pin(&from, &pk)?;
        let body = open(&payload, me, &pk)
            .map_err(|e| anyhow!("cannot open envelope from {from}: {e}"))?;
        if let SignalBody::ConnectRequest { session_id } = body {
            return Ok((from, pk, session_id));
        }
        // 非 ConnectRequest（例如迟到/多余的体）：忽略，继续等待
    }
}

/// 接收模式：等待来件请求并完成一次传输，返回落盘路径。
pub async fn run_recv(session: &Session, out_dir: &Path) -> Result<std::path::PathBuf> {
    let me = load_identity()?;
    anyhow::ensure!(
        me.device_id == session.device_id,
        "identity seed does not match session.json device_id"
    );
    // 先扫一遍上一轮遗留的中间文件，再开始等待
    sweep_part_files(out_dir);

    let (client, mut ev) = signaling_connect(session, &me.device_id).await?;
    // wait_connect_request 在打开任何信封之前已完成发送方的 TOFU 固定，
    // 因此这里回信（seal）之前密钥必然已通过服务器目录 + TOFU 校验。
    let (from_device, peer_pk, session_id) = wait_connect_request(session, &mut ev, &me).await?;

    // 服务端端点先建好（本地端口已知），再回 Accept + Candidates
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(from_device.clone(), peer_pk);
    let ep_b = ep::make_server_endpoint("0.0.0.0:0".parse().expect("valid socket addr"), &me, &allowed)
        .context("create QUIC server endpoint")?;
    let port = ep_b.local_addr()?.port();
    let cands = local_candidates(port);

    client
        .relay(
            &from_device,
            &seal(&me, &peer_pk, &SignalBody::ConnectAccept { session_id: session_id.clone() }),
        )
        .await?;
    client
        .relay(
            &from_device,
            &seal(&me, &peer_pk, &SignalBody::Candidates { session_id, addrs: cands }),
        )
        .await?;

    // 等发送方连上来并完成传输；失败/超时清理 .part
    let result = async {
        let (conn, _dev) = tokio::time::timeout(ACCEPT_TIMEOUT, ep::accept_pinned(&ep_b, &allowed))
            .await
            .map_err(|_| anyhow!("sender did not connect within {ACCEPT_TIMEOUT:?}"))?
            .context("pinned accept")?;
        let paths = transfer::recv_files(&conn, out_dir, &me)
            .await
            .context("transfer")?;
        Ok(paths)
    }
    .await;

    match result {
        Ok(paths) => paths
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("transfer produced no files")),
        Err(e) => {
            sweep_part_files(out_dir);
            Err(e)
        }
    }
}

// ---------- register / devices ----------

/// 登记本设备：请求验证码（dev 模式把码打到 stderr）→ 终端输入 → verify →
/// 写 session.json。身份种子由 load_identity 首次自动生成。
pub async fn register(server: &str, email: &str, name: &str) -> Result<()> {
    let id = load_identity()?;
    let client = http_client()?;

    let code: serde_json::Value = client
        .post(format!("{server}/auth/request-code"))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await?
        .error_for_status()
        .context("request-code")?
        .json()
        .await?;
    let dev_code = code["code"].as_str().context("no code in response")?.to_string();
    eprintln!("dev-mode code for {email}: {dev_code}");

    print!("enter code: ");
    use std::io::Write as _;
    std::io::stdout().flush().ok();
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).context("read code from stdin")?;

    let v: serde_json::Value = client
        .post(format!("{server}/auth/verify"))
        .json(&serde_json::json!({
            "email": email,
            "code": input.trim(),
            "pubkey": hex::encode(id.public_key().as_bytes()),
            "name": name,
            "platform": std::env::consts::OS,
        }))
        .send()
        .await?
        .error_for_status()
        .context("verify (bad code?)")?
        .json()
        .await?;
    let s = Session {
        server: server.to_string(),
        jwt: v["jwt"].as_str().context("no jwt in response")?.into(),
        refresh: v["refresh"].as_str().context("no refresh in response")?.into(),
        device_id: v["device_id"].as_str().context("no device_id in response")?.into(),
    };
    s.save()?;
    println!("registered device {} ({})", s.device_id, name);
    Ok(())
}

/// 列出账号设备（device_id / name / platform / online）。
pub async fn devices_cmd() -> Result<()> {
    let s = Session::load()?;
    for d in fetch_devices(&s).await? {
        println!(
            "{}\t{}\t{}\tonline={}",
            d["device_id"].as_str().unwrap_or("?"),
            d["name"].as_str().unwrap_or("?"),
            d["platform"].as_str().unwrap_or("?"),
            d["online"].as_bool().unwrap_or(false)
        );
    }
    Ok(())
}
