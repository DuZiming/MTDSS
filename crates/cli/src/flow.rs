//! send/recv 信令 + 连接编排。
//!
//! 安全模型（不可妥协）：
//! - 对端公钥永远从服务器目录（GET /devices，按 device_id）解析，再用
//!   `PeersCache` 做 TOFU 固定：首次记录，之后必须一致。信封内的
//!   `sender_device_id` 字段是未签名数据，绝不用于密钥解析——验签一律用
//!   服务器目录 + TOFU 得到的公钥。
//! - 任何面向对端的信封（seal）之前必须先完成 TOFU 校验。
//! - 所有信令等待整体受 `SIGNAL_TIMEOUT` 约束，错误帧/连接关闭立即上抛；
//!   互拨连接（punch_connect）受其 PunchConfig 预算约束（默认 10s）。

use crate::session::{PeersCache, Session};
use anyhow::{anyhow, Context, Result};
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::net::stun::gather_candidates;
use mtdss_core::signaling::client::{ClientEvent, SignalingClient};
use mtdss_core::signaling::envelope::{open, seal, SignalBody};
use mtdss_core::transport::{endpoint as ep, punch, transfer};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

/// 信令体等待的整体超时（ConnectAccept / Candidates / ConnectRequest 之外
/// 的往返各算一次等待）。
pub(crate) const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);
/// 单次 relay 写入的超时：`SignalingClient::relay` 内部无超时，WS sink
/// 停滞/半死连接会让 mpsc send 永久 pending，必须在外层截断。
pub(crate) const RELAY_TIMEOUT: Duration = Duration::from_secs(10);
/// REST 请求超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

// ---------- 身份与服务器目录 ----------

/// 加载 `home` 下的本机身份；首次运行自动生成并落盘 `identity.seed`（0600）。
/// home 由调用方显式传入（CLI 入口构造一次 mtdss_home() 后下传），同一进程
/// 内可并发驱动多个互不干扰的设备目录（集成测试依赖这一点）。
pub fn load_identity(home: &Path) -> Result<DeviceIdentity> {
    let p = home.join("identity.seed");
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

/// REST 客户端：`ca` 存在时把它加为根证书（自签 https 场景），
/// None 用默认 webpki 根。
fn http_client(ca: Option<&Path>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(HTTP_TIMEOUT);
    if let Some(ca) = ca {
        let pem = std::fs::read(ca).with_context(|| format!("read ca {}", ca.display()))?;
        let cert = reqwest::Certificate::from_pem(&pem)?;
        b = b.add_root_certificate(cert);
    }
    b.build().context("build http client")
}

/// 信令 WS 地址：server 的 http(s) 前缀重写为 ws(s) 后接 `/ws`。
/// 前缀逐字匹配，不做子串替换（旧实现对任意位置出现的 "http" 都会改写，
/// 会在带该子串的 host/路径里破坏 URL）；已是 ws(s):// 则透传；裸地址按
/// ws:// 处理。
fn ws_url(server: &str) -> String {
    let s = server.trim_end_matches('/');
    if let Some(rest) = s.strip_prefix("https://") {
        format!("wss://{rest}/ws")
    } else if let Some(rest) = s.strip_prefix("http://") {
        format!("ws://{rest}/ws")
    } else if s.starts_with("ws://") || s.starts_with("wss://") {
        format!("{s}/ws")
    } else {
        format!("ws://{s}/ws")
    }
}

/// 连接信令通道（握手本身也有超时，避免对端无响应时无限挂起）。
async fn signaling_connect(
    session: &Session,
    device_id: &str,
) -> Result<(SignalingClient, mpsc::Receiver<ClientEvent>)> {
    let url = ws_url(&session.server);
    let (client, ev) = tokio::time::timeout(
        SIGNAL_TIMEOUT,
        SignalingClient::connect_with(
            &url,
            &session.jwt,
            device_id,
            session.ca.as_deref(),
            mtdss_core::signaling::client::Keepalive::default(),
        ),
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
/// 缓存文件固定在 `home`（每设备目录独立）。
fn tofu_pin(home: &Path, device_id: &str, pk: &VerifyingKey) -> Result<()> {
    let path = home.join("peers.json");
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

// ---------- 有界等待 ----------

/// 给任何可能无限 pending 的 `Result` 型 future 加超时上限（超时即 Err）。
/// 契约：future 在 `d` 内不完成 → 返回带 `label` 的超时错误，绝不永久挂起。
/// 公开以便测试直接用永不完成的 future 固定该契约。
pub async fn bounded<T>(
    label: &'static str,
    d: Duration,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(d, fut)
        .await
        .map_err(|_| anyhow!("{label} timed out after {d:?} (peer unresponsive)"))?
}

/// `SignalingClient::relay` 没有内部超时（mpsc 满且 WS 任务卡在停滞的
/// sink 写入上会永久 pending），所有 relay 一律经此有界包装。
async fn relay_with_timeout(client: &SignalingClient, to: &str, payload: &[u8]) -> Result<()> {
    bounded("relay to peer", RELAY_TIMEOUT, client.relay(to, payload)).await
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
                ClientEvent::HelloOk { .. } => {} // 握手确认：忽略
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

/// 从事件流取 HelloOk 的 stun 下发（连接后第一帧，确定性顺序）。必须在
/// 其他 wait_* 之前调用——它们会把 HelloOk 当握手确认消费丢弃。
async fn wait_hello_stun(ev: &mut mpsc::Receiver<ClientEvent>) -> Result<Vec<String>> {
    let wait = async {
        loop {
            match ev.recv().await {
                Some(ClientEvent::HelloOk { stun_addrs }) => return Ok(stun_addrs),
                Some(ClientEvent::Presence { .. }) | Some(ClientEvent::Relayed { .. }) => continue,
                Some(ClientEvent::Error { code, message }) => {
                    anyhow::bail!("signaling error {code}: {message}")
                }
                Some(ClientEvent::Closed) | None => {
                    anyhow::bail!("signaling closed before HelloOk")
                }
            }
        }
    };
    tokio::time::timeout(SIGNAL_TIMEOUT, wait)
        .await
        .map_err(|_| anyhow!("timed out after {SIGNAL_TIMEOUT:?} waiting for HelloOk"))?
}

// ---------- 打洞编排 ----------

/// 解析对端候选字符串 → SocketAddr（跳过不可解析并告警，截断 MAX_CANDIDATES）。
fn parse_candidates(addrs: &[String]) -> Vec<SocketAddr> {
    addrs
        .iter()
        .filter_map(|a| match a.parse::<SocketAddr>() {
            Ok(s) => Some(s),
            Err(_) => {
                tracing::warn!("peer sent unparsable candidate {a}");
                None
            }
        })
        .take(mtdss_core::net::MAX_CANDIDATES)
        .collect()
}

/// 绑定 raw socket → STUN 候选收集 → 移交 quinn 建双角色端点（白名单=对端）。
/// 返回 (端点, 本端候选, 本侧对称 NAT 标记)。候选收集是阻塞式查询
/// （最坏约 4s：primary 慢成功 + alt 超时，每次查询上限 2s）：必须在
/// socket 移交 quinn 之前完成，仍在对端 `SIGNAL_TIMEOUT`（10s）等待窗口内；
/// 通告为空或 STUN 不可达时降级为仅本地候选。
fn setup_punch_endpoint(
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    stun: &[String],
) -> Result<(quinn::Endpoint, Vec<SocketAddr>, bool)> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let parsed: Vec<SocketAddr> = stun.iter().filter_map(|s| s.parse().ok()).collect();
    let (primary, alt) = match (parsed.first(), parsed.get(1)) {
        (Some(p), Some(a)) => (*p, *a),
        _ => {
            tracing::warn!("no usable STUN endpoints advertised; local candidates only");
            let local = sock.local_addr()?;
            (local, local) // primary 查询必失败 → gather 降级本地
        }
    };
    let g = gather_candidates(&sock, primary, alt)?;
    if !g.stun_ok {
        tracing::warn!("STUN unreachable; local candidates only");
    }
    let nat_symmetric = g.nat_symmetric;
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(peer_id.to_string(), *peer_pk);
    let epn = ep::make_dual_endpoint(sock, me, &allowed)?;
    Ok((epn, g.addrs, nat_symmetric))
}

/// punch 失败的统一诊断包装（spec §7：含本侧 NAT 类型与建议）。
fn punch_err(e: anyhow::Error, nat_symmetric: bool, mine: usize) -> anyhow::Error {
    let mut msg = format!("{e:#}; local candidates: {mine}");
    if nat_symmetric {
        msg.push_str("; local NAT appears symmetric (直连成功率较低,建议换网络或同网段)");
    }
    anyhow::anyhow!(msg)
}

// ---------- send ----------

/// 发送单个文件给账号内另一设备。`home` 是本侧设备数据目录（身份种子 +
/// TOFU 缓存），由调用方显式传入。
pub async fn run_send(
    session: &Session,
    home: &Path,
    file: &Path,
    to_device: &str,
    out_report: &dyn Fn(&str),
) -> Result<()> {
    let me = load_identity(home)?;
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
    tofu_pin(home, to_device, &peer_pk)?;

    // 信令：连接后第一帧取服务器通告的 STUN（空 → 本地候选降级），再
    // seal ConnectRequest → 等 ConnectAccept → 双向交换 Candidates → 互拨
    let (client, mut ev) = signaling_connect(session, &me.device_id).await?;
    let stun = wait_hello_stun(&mut ev).await?;
    let session_id = rand_session_id();
    relay_with_timeout(
        &client,
        to_device,
        &seal(&me, &peer_pk, &SignalBody::ConnectRequest { session_id: session_id.clone() }),
    )
    .await?;

    wait_opened(&mut ev, to_device, &me, &peer_pk, |b| {
        matches!(b, SignalBody::ConnectAccept { session_id: s } if *s == session_id)
    })
    .await
    .context("waiting for ConnectAccept")?;

    // Accept 后建双角色端点，回发本端 Candidates，再等对端 Candidates，
    // 双方互拨（punch_connect 内部完成 accept 竞速与角色固定，无需预起 acceptor）
    let (epn, mine, nat_symmetric) = setup_punch_endpoint(&me, to_device, &peer_pk, &stun)?;
    relay_with_timeout(
        &client,
        to_device,
        &seal(&me, &peer_pk, &SignalBody::Candidates {
            session_id: session_id.clone(),
            addrs: mine.iter().map(|a| a.to_string()).collect(),
        }),
    )
    .await?;
    let theirs = match wait_opened(&mut ev, to_device, &me, &peer_pk, |b| {
        matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
    })
    .await
    .context("waiting for peer Candidates")?
    {
        SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
        other => anyhow::bail!("unexpected signal body {other:?}"),
    };

    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(to_device.to_string(), peer_pk);
    let conn = punch::punch_connect(
        &epn,
        &me,
        &allowed,
        &peer_pk,
        &theirs,
        &punch::PunchConfig::default(),
    )
    .await
    .map_err(|e| punch_err(e, nat_symmetric, mine.len()))?;

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
    home: &Path,
    ev: &mut mpsc::Receiver<ClientEvent>,
    me: &DeviceIdentity,
) -> Result<(String, VerifyingKey, Vec<u8>)> {
    loop {
        let (from, payload) = match ev.recv().await.context("signaling closed")? {
            ClientEvent::Relayed { from, payload } => (from, payload),
            ClientEvent::Presence { .. } => continue,
            ClientEvent::HelloOk { .. } => continue, // 握手确认：忽略
            ClientEvent::Error { code, message } => {
                return Err(anyhow!("signaling error {code}: {message}"))
            }
            ClientEvent::Closed => return Err(anyhow!("signaling connection closed")),
        };
        // 密钥解析只信服务器目录（信封 sender 字段未签名）
        let devices = fetch_devices(session).await?;
        let pk = peer_pk_from(&devices, &from)?;
        // 打开任何信封之前先做 TOFU 固定：首次记录，之后必须一致
        tofu_pin(home, &from, &pk)?;
        let body = open(&payload, me, &pk)
            .map_err(|e| anyhow!("cannot open envelope from {from}: {e}"))?;
        if let SignalBody::ConnectRequest { session_id } = body {
            return Ok((from, pk, session_id));
        }
        // 非 ConnectRequest（例如迟到/多余的体）：忽略，继续等待
    }
}

/// 接收模式：等待来件请求并完成一次传输，返回落盘路径。
/// `home` 是本侧设备数据目录（身份种子 + TOFU 缓存），由调用方显式传入。
pub async fn run_recv(session: &Session, home: &Path, out_dir: &Path) -> Result<std::path::PathBuf> {
    let me = load_identity(home)?;
    anyhow::ensure!(
        me.device_id == session.device_id,
        "identity seed does not match session.json device_id"
    );
    // 先扫一遍上一轮遗留的中间文件，再开始等待
    sweep_part_files(out_dir);

    let (client, mut ev) = signaling_connect(session, &me.device_id).await?;
    // HelloOk 是连接后第一帧：先取服务器通告的 STUN（空 → 本地候选降级），
    // 必须在 wait_connect_request 之前——它会把 HelloOk 当握手确认消费丢弃
    let stun = wait_hello_stun(&mut ev).await?;
    // wait_connect_request 在打开任何信封之前已完成发送方的 TOFU 固定，
    // 因此这里回信（seal）之前密钥必然已通过服务器目录 + TOFU 校验。
    let (from_device, peer_pk, session_id) =
        wait_connect_request(session, home, &mut ev, &me).await?;

    // 双角色端点（STUN 候选收集 → socket 移交 quinn）→ 回 Accept + 本端
    // Candidates → 等发送方 Candidates → 双方互拨；任何失败/超时清理 .part
    let result = async {
        let (epn, mine, nat_symmetric) =
            setup_punch_endpoint(&me, &from_device, &peer_pk, &stun)?;
        relay_with_timeout(
            &client,
            &from_device,
            &seal(&me, &peer_pk, &SignalBody::ConnectAccept { session_id: session_id.clone() }),
        )
        .await?;
        relay_with_timeout(
            &client,
            &from_device,
            &seal(&me, &peer_pk, &SignalBody::Candidates {
                session_id: session_id.clone(),
                addrs: mine.iter().map(|a| a.to_string()).collect(),
            }),
        )
        .await?;
        let theirs = match wait_opened(&mut ev, &from_device, &me, &peer_pk, |b| {
            matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
        })
        .await
        .context("waiting for sender Candidates")?
        {
            SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
            other => anyhow::bail!("unexpected signal body {other:?}"),
        };

        let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
        allowed.insert(from_device.clone(), peer_pk);
        let conn = punch::punch_connect(
            &epn,
            &me,
            &allowed,
            &peer_pk,
            &theirs,
            &punch::PunchConfig::default(),
        )
        .await
        .map_err(|e| punch_err(e, nat_symmetric, mine.len()))?;
        transfer::recv_files(&conn, out_dir, &me).await.context("transfer")
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
/// 写 session.json。身份种子由 load_identity 首次自动生成（落在 `home`）。
/// `ca` 是服务端 TLS CA 证书（自签 https/wss 场景），None 用默认根；
/// 与会话一起持久化到 session.json。
pub async fn register(
    home: &Path,
    server: &str,
    email: &str,
    name: String,
    ca: Option<&Path>,
) -> Result<()> {
    let id = load_identity(home)?;
    let client = http_client(ca)?;

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
        ca: ca.map(|p| p.to_path_buf()),
    };
    s.save(home)?;
    println!("registered device {} ({})", s.device_id, name);
    Ok(())
}

/// 列出账号设备（device_id / name / platform / online）。会话由调用方加载
/// （CLI 入口负责 `--ca` 覆盖后再传入）。
pub async fn devices_cmd(s: &Session) -> Result<()> {
    for d in fetch_devices(s).await? {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_prefix_rewrite() {
        assert_eq!(ws_url("http://127.0.0.1:8787"), "ws://127.0.0.1:8787/ws");
        assert_eq!(ws_url("https://signal.example.com"), "wss://signal.example.com/ws");
        assert_eq!(ws_url("http://h/api/"), "ws://h/api/ws"); // 尾部斜杠收敛
        assert_eq!(ws_url("ws://127.0.0.1:1"), "ws://127.0.0.1:1/ws");
        assert_eq!(ws_url("wss://s"), "wss://s/ws");
        assert_eq!(ws_url("127.0.0.1:8787"), "ws://127.0.0.1:8787/ws");
        // 子串不改写：只有前缀位置才发生替换
        assert_eq!(ws_url("http://http-trap.example"), "ws://http-trap.example/ws");
    }
}
