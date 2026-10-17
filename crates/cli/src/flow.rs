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
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
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
                ClientEvent::Error { code: 0, message } => {
                    // 本地退出诊断(code=0):原因即断连说明
                    return Err(anyhow!("{message}"));
                }
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
                Some(ClientEvent::Error { code: 0, message }) => {
                    anyhow::bail!("{message}")
                }
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
/// 返回 (端点, 本端候选, 本侧对称 NAT 标记, STUN 可达标记)。候选收集是阻塞式查询
/// （最坏约 4s：primary 慢成功 + alt 超时，每次查询上限 2s）：必须在
/// socket 移交 quinn 之前完成，仍在对端 `SIGNAL_TIMEOUT`（10s）等待窗口内；
/// 通告为空或 STUN 不可达时降级为仅本地候选（stun_ok=false）。
fn setup_punch_endpoint(
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    stun: &[String],
) -> Result<(quinn::Endpoint, Vec<SocketAddr>, bool, bool)> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let parsed: Vec<SocketAddr> = stun.iter().filter_map(|s| s.parse().ok()).collect();
    let (primary, alt) = match (parsed.first(), parsed.get(1)) {
        (Some(p), Some(a)) => (*p, *a),
        // 单条通告:主备同址。备端口查询与主端口同映射(自然判非对称),
        // gather 照常发生——不再误入本地降级分支。
        (Some(p), None) => (*p, *p),
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
    Ok((epn, g.addrs, nat_symmetric, g.stun_ok))
}

/// punch 失败的统一诊断包装（spec §7：含本侧 NAT 类型与建议）。
fn punch_err(e: anyhow::Error, nat_symmetric: bool, mine: usize) -> anyhow::Error {
    let mut msg = format!("{e:#}; local candidates: {mine}");
    if nat_symmetric {
        msg.push_str("; local NAT appears symmetric (直连成功率较低,建议换网络或同网段)");
    }
    anyhow::anyhow!(msg)
}

// ---------- send / recv 编排选项 ----------

/// 重连策略:发送侧重试轮数与退避基数,接收侧断后接收窗口。
/// 退避 = backoff_base × 2^(attempt-1)(默认 1/2/4/8/16s,attempt 从 1 起)。
#[derive(Debug, Clone, Copy)]
pub struct RetryTuning {
    /// 发送方最大重连次数(总尝试 = max_reconnects + 1)。
    pub max_reconnects: u32,
    /// 退避基数(第 n 次重试前睡 backoff_base × 2^(n-1))。
    pub backoff_base: Duration,
    /// 接收方首次断连后的接收窗口:窗口内继续等来件,超窗报错。
    pub recv_window: Duration,
}

impl Default for RetryTuning {
    fn default() -> Self {
        Self {
            max_reconnects: 5,
            backoff_base: Duration::from_secs(1),
            recv_window: Duration::from_secs(300),
        }
    }
}

/// 发送编排选项(测试可注入小预算/短退避)。
#[derive(Debug, Clone, Copy, Default)]
pub struct SendOpts {
    pub retry: RetryTuning,
    pub punch: punch::PunchConfig,
}

/// 接收编排选项。`fault_close_after_files` 为重连 e2e 的注错钩子
/// (第 n 个新 finalize 落盘后主动断连),生产恒为 None。
#[derive(Debug, Clone, Copy, Default)]
pub struct RecvOpts {
    pub retry: RetryTuning,
    pub punch: punch::PunchConfig,
    #[doc(hidden)]
    pub fault_close_after_files: Option<usize>,
}

// ---------- 多路径输入收集 ----------

/// 多路径输入收集:文件 → file_name;目录 → 递归展开(read_dir 按名排序
/// 保稳定序),rel = `输入目录名/内部相对路径`。预检:不存在 Err、符号链接
/// 跳过(记 warn)、空目录跳过(全部输入都空 → Err)、跨输入 rel 重复 Err、
/// 输入名 `.mtdss`(目录或文件)Err(接收方 intake 保留段)。目录内部路径
/// 以 `/` 拼接,与清单校验的分隔约定一致。
pub fn collect_items(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>> {
    let mut items: Vec<(PathBuf, String)> = Vec::new();
    for input in paths {
        // symlink_metadata 不跟随符号链接:顶层 symlink 输入跳过(含存在性检查)
        let meta = std::fs::symlink_metadata(input)
            .with_context(|| format!("input path not accessible: {}", input.display()))?;
        if meta.is_symlink() {
            tracing::warn!("skip symlink input {}", input.display());
            continue;
        }
        let base = input
            .file_name()
            .and_then(|n| n.to_str())
            .context("input path has no UTF-8 file name")?
            .to_string();
        anyhow::ensure!(
            base != ".mtdss",
            "input name .mtdss is reserved for the receiver intake area: {}",
            input.display()
        );
        if meta.is_dir() {
            walk_dir(input, &base, &mut items)?;
        } else {
            anyhow::ensure!(
                meta.is_file(),
                "input {} is neither a regular file nor a directory",
                input.display()
            );
            items.push((input.clone(), base));
        }
    }
    // 跨输入 rel 重复:两份内容会争同一个落盘路径,发送侧直接拒绝
    let mut seen: HashSet<String> = HashSet::new();
    for (_, rel) in &items {
        anyhow::ensure!(
            seen.insert(rel.clone()),
            "duplicate rel path across inputs: {rel}"
        );
    }
    anyhow::ensure!(
        !items.is_empty(),
        "nothing to send (inputs missing, empty directories, or all symlinks)"
    );
    Ok(items)
}

/// 深度优先展开目录:read_dir 按文件名排序(同目录内容跨平台稳定输出),
/// 子目录递归;符号链接条目跳过并告警;竞态消失的条目跳过。
fn walk_dir(dir: &Path, prefix: &str, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .with_context(|| format!("read dir {}", dir.display()))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry
            .file_name()
            .to_str()
            .context("non-UTF-8 file name inside input directory")?
            .to_string();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue; // 竞态消失:跳过
        };
        if meta.is_symlink() {
            tracing::warn!("skip symlink {} in input directory", path.display());
            continue;
        }
        // 接收方 intake 保留段:目录内名为 .mtdss 的条目(文件或目录)
        // 整段跳过,避免清单 rel 与接收端 intake 目录相撞
        if name == ".mtdss" {
            tracing::warn!("skip reserved .mtdss entry {}", path.display());
            continue;
        }
        let rel = format!("{prefix}/{name}");
        if meta.is_dir() {
            walk_dir(&path, &rel, out)?;
        } else {
            out.push((path, rel));
        }
    }
    Ok(())
}

// ---------- send ----------

/// 发送多文件给账号内另一设备。`home` 是本侧设备数据目录(身份种子 +
/// TOFU 缓存),由调用方显式传入;`paths` 为多路径输入(文件/目录混合)。
pub async fn run_send(
    session: &Session,
    home: &Path,
    paths: &[PathBuf],
    to_device: &str,
    out_report: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    run_send_with(
        session,
        home,
        paths,
        to_device,
        out_report,
        &SendOpts::default(),
    )
    .await
}

/// 带选项的发送编排:清单一次构建(全部重试共用同一份字节与 transfer_id,
/// 接收方按 rel 对账,断点续传才可能跨轮成立);每轮重连信令并重打洞,
/// 仅 Network(含信令/打洞失败)续轮退避重试,Rejected/Protocol 立即终局;
/// 终局失败按 rel→原始输入路径出成败清单。
pub async fn run_send_with(
    session: &Session,
    home: &Path,
    paths: &[PathBuf],
    to_device: &str,
    out_report: &(dyn Fn(&str) + Sync),
    opts: &SendOpts,
) -> Result<()> {
    let me = load_identity(home)?;
    anyhow::ensure!(
        me.device_id == session.device_id,
        "identity seed does not match session.json device_id"
    );

    let items = collect_items(paths)?;
    let man =
        mtdss_core::protocol::manifest::build_manifest(&items, &me).context("building manifest")?;

    // 对端公钥:服务器目录 + TOFU(首次记录/之后必须一致),整个 run 固定
    let devices = fetch_devices(session).await?;
    let peer_pk = peer_pk_from(&devices, to_device)?;
    tofu_pin(home, to_device, &peer_pk)?;

    let tracker = Mutex::new(SendTracker::default());
    let ctx = SendCtx {
        session,
        me: &me,
        to_device,
        peer_pk: &peer_pk,
        man: &man,
        items: &items,
        punch: opts.punch,
        tracker: &tracker,
    };

    // 重试即策略:每轮 = 新信令连接 + 重打洞 + send_files(接收方按清单
    // 续传);轮次耗尽或终局错误 → 按文件成败清单报告
    let mut attempts_made: u32 = 0;
    let mut last_err: Option<transfer::SendError> = None;
    for attempt in 0..=opts.retry.max_reconnects {
        if attempt > 0 {
            // 指数退避 backoff_base × 2^(attempt-1);指数封顶 16 防 u32 溢出
            let shift = (attempt - 1).min(16);
            tokio::time::sleep(opts.retry.backoff_base * (1u32 << shift)).await;
        }
        attempts_made = attempt + 1;
        match send_round(&ctx).await {
            Ok(report) => {
                out_report(&format!(
                    "sent {} bytes ({} files)",
                    report.bytes_sent,
                    items.len()
                ));
                return Ok(());
            }
            Err(e) => {
                let go_on = retryable(&e);
                tracing::warn!("send attempt {attempt} failed (retryable={go_on}): {e}");
                last_err = Some(e);
                if !go_on {
                    break; // Rejected/Protocol:立即终局,不空耗轮次
                }
            }
        }
    }
    let tracker = tracker
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Err(failure_report(
        &items,
        &tracker,
        attempts_made,
        &last_err.expect("failure path always records the last error"),
    ))
}

/// run_send_with 的跨尝试不变上下文。
struct SendCtx<'a> {
    session: &'a Session,
    me: &'a DeviceIdentity,
    to_device: &'a str,
    peer_pk: &'a VerifyingKey,
    man: &'a mtdss_proto::manifest::Manifest,
    items: &'a [(PathBuf, String)],
    punch: punch::PunchConfig,
    tracker: &'a Mutex<SendTracker>,
}

/// 单轮发送尝试:新信令连接 → HelloOk → ConnectRequest/Accept → 候选交换 →
/// punch → send_files。信令连接随本轮结束释放(下一轮重连);握手与打洞
/// 失败一律按 Network 分类(连接层问题,值得重试),send_files 的错误分类
/// 原样上抛由外层策略分派。
async fn send_round(ctx: &SendCtx<'_>) -> Result<transfer::SendReport, transfer::SendError> {
    let me = ctx.me;
    let to = ctx.to_device;
    let net = |e: anyhow::Error| transfer::SendError::Network(format!("{e:#}"));

    let (client, mut ev) = signaling_connect(ctx.session, &me.device_id)
        .await
        .map_err(net)?;
    let stun = wait_hello_stun(&mut ev).await.map_err(net)?;
    let session_id = rand_session_id();
    relay_with_timeout(
        &client,
        to,
        &seal(
            me,
            ctx.peer_pk,
            &SignalBody::ConnectRequest {
                session_id: session_id.clone(),
            },
        ),
    )
    .await
    .map_err(net)?;
    wait_opened(
        &mut ev,
        to,
        me,
        ctx.peer_pk,
        |b| matches!(b, SignalBody::ConnectAccept { session_id: s } if *s == session_id),
    )
    .await
    .context("waiting for ConnectAccept")
    .map_err(net)?;

    // Accept 后建双角色端点,回发本端 Candidates,再等对端 Candidates,
    // 双方互拨(punch_connect 内部完成 accept 竞速与角色固定)
    let (epn, mine, nat_symmetric, _stun_ok) =
        setup_punch_endpoint(me, to, ctx.peer_pk, &stun).map_err(net)?;
    relay_with_timeout(
        &client,
        to,
        &seal(
            me,
            ctx.peer_pk,
            &SignalBody::Candidates {
                session_id: session_id.clone(),
                addrs: mine.iter().map(|a| a.to_string()).collect(),
            },
        ),
    )
    .await
    .map_err(net)?;
    let theirs = match wait_opened(
        &mut ev,
        to,
        me,
        ctx.peer_pk,
        |b| matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id),
    )
    .await
    .context("waiting for peer Candidates")
    .map_err(net)?
    {
        SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
        other => {
            return Err(transfer::SendError::Protocol {
                reason: format!("unexpected signal body {other:?}"),
            })
        }
    };

    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(to.to_string(), *ctx.peer_pk);
    let conn = punch::punch_connect(&epn, me, &allowed, ctx.peer_pk, &theirs, &ctx.punch)
        .await
        .map_err(|e| {
            transfer::SendError::Network(format!("{}", punch_err(e, nat_symmetric, mine.len())))
        })?;

    // 传输(大文件耗时合法,不限时;QUIC 断连自行报错,由外层重试策略分派)
    let progress = |p: &transfer::SendProgress<'_>| ctx.tracker.lock().unwrap().track(p);
    transfer::send_files(
        &conn,
        ctx.man,
        ctx.items,
        &progress,
        &transfer::SendOptions::default(),
    )
    .await
}

/// 重试分类:仅网络层错误值得重连;对端明确拒绝与协议违规是终局,
/// 空耗重试轮次没有意义。
fn retryable(e: &transfer::SendError) -> bool {
    matches!(e, transfer::SendError::Network(_))
}

/// 跨尝试累计的发送进度(终局失败报告的数据源):FileDone 记已在对端完整
/// 落盘的文件;Bytes 记每文件最后一次快照(后续轮次计数从 0 重新累计,
/// 因此跨尝试取最大值)。
#[derive(Default)]
struct SendTracker {
    done: HashSet<String>,
    bytes: HashMap<String, (u64, u64)>,
}

impl SendTracker {
    fn track(&mut self, p: &transfer::SendProgress<'_>) {
        match p {
            transfer::SendProgress::FileDone { rel } => {
                self.done.insert(rel.to_string());
            }
            transfer::SendProgress::Bytes { rel, done, total } => {
                let e = self.bytes.entry(rel.to_string()).or_insert((0, *total));
                if *done > e.0 {
                    e.0 = *done;
                }
                e.1 = *total;
            }
        }
    }
}

/// 终局失败报告:rel 映射回用户原始输入路径,按 已完成/部分传输/未完成
/// 分栏列出(空栏保留标题,便于调用方稳定解析文案)。
fn failure_report(
    items: &[(PathBuf, String)],
    tracker: &SendTracker,
    attempts: u32,
    last: &transfer::SendError,
) -> anyhow::Error {
    let mut done: Vec<String> = Vec::new();
    let mut partial: Vec<String> = Vec::new();
    let mut never: Vec<String> = Vec::new();
    for (orig, rel) in items {
        if tracker.done.contains(rel) {
            done.push(format!("  {} ({})", orig.display(), rel));
        } else if let Some(&(d, t)) = tracker.bytes.get(rel).filter(|(d, _)| *d > 0) {
            partial.push(format!("  {} ({}) {d}/{t} 字节", orig.display(), rel));
        } else {
            never.push(format!("  {} ({})", orig.display(), rel));
        }
    }
    let mut msg = format!("发送失败(共尝试 {attempts} 轮,最后错误: {last})");
    for (title, list) in [
        ("已完成", &done),
        ("部分传输", &partial),
        ("未完成", &never),
    ] {
        msg.push_str(&format!("\n{} {} 项:", title, list.len()));
        if list.is_empty() {
            msg.push_str("(无)");
        } else {
            for line in list {
                msg.push_str(&format!("\n{line}"));
            }
        }
    }
    anyhow::anyhow!(msg)
}

// ---------- recv ----------

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
            ClientEvent::Error { code: 0, message } => return Err(anyhow!("{message}")),
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

/// 接收模式:等待来件请求并完成一次传输,返回本次落盘路径(多文件)。
/// `home` 是本侧设备数据目录(身份种子 + TOFU 缓存),由调用方显式传入。
pub async fn run_recv(session: &Session, home: &Path, out_dir: &Path) -> Result<Vec<PathBuf>> {
    run_recv_with(session, home, out_dir, &RecvOpts::default()).await
}

/// 带选项的接收编排:首次等待无期限;断连(传输/信令错误)后起一个
/// `recv_window` 接收窗口,窗口内重建信令连接继续等来件,超窗才报错。
/// 窗口一旦起算,本轮**传输前的全部等待**(信令连接/HelloOk/等来件)都
/// 受 deadline 约束——等待中被兜住,而不是只在轮失败时才检查;进行中的
/// 传输不受 deadline 约束(它在推进,活性由 QUIC 空闲超时治理)。
/// 每个新 ConnectRequest 清窗重置(断连重新起算):多轮重连/长续传不被
/// 首次断连的旧窗口误伤,静默对端则窗口保持、到点退出。
/// 跨轮累计全部落盘路径(错误轮的已 finalize 路径同样计入)。空 Vec 是
/// 合法成功结果(对端重发时发现已全部持有,本轮无事可做)。
pub async fn run_recv_with(
    session: &Session,
    home: &Path,
    out_dir: &Path,
    opts: &RecvOpts,
) -> Result<Vec<PathBuf>> {
    let me = load_identity(home)?;
    anyhow::ensure!(
        me.device_id == session.device_id,
        "identity seed does not match session.json device_id"
    );

    let mut landed: Vec<PathBuf> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let round = RecvRound {
            session,
            home,
            out_dir,
            me: &me,
            punch: opts.punch,
            fault: opts.fault_close_after_files,
            deadline,
            recv_window: opts.retry.recv_window,
        };
        let mut request_seen = false;
        let r = recv_round(&round, &mut request_seen).await;
        // 错误轮的已落盘路径同样是磁盘事实:先累计再分派(I-2)
        let fault_cut = r.error.is_none() && !r.paths.is_empty() && round.fault.is_some();
        landed.extend(r.paths);
        // 新来件 = 对端在线的明证:清窗重置——下次断连重新起算。既保住
        // "断后有限窗口"的防挂死语义,又让多轮重连/长续传不被首次断连
        // 的旧 deadline 误伤(断连 #2 不对照断连 #1 的窗口;静默对端则
        // 无来件,窗口保持原样)。
        if request_seen {
            deadline = None;
        }
        match r.error {
            None => {
                if fault_cut {
                    // fault 注入:切断钩子触发(干净收尾轮必为全 SKIP → 空)。
                    // 与真实断连同路径:起窗口重进等发送方重试。
                    deadline.get_or_insert_with(|| Instant::now() + opts.retry.recv_window);
                    continue;
                }
                return Ok(landed);
            }
            Some(e) => {
                // 轮间固定小歇:防快败轮(信令不可达等)在整个窗口内热旋
                // TCP 连接;接收侧重进没有指数退避的语义需求(窗口即总预算)
                tokio::time::sleep(RECV_ROUND_PACE).await;
                // 首次断连起窗;窗口内继续(信令死了也重建),超窗终局
                let dl = *deadline.get_or_insert_with(|| Instant::now() + opts.retry.recv_window);
                if Instant::now() < dl {
                    tracing::warn!("recv round failed (window still open): {e:#}");
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// 接收轮重进前的固定小歇(防热旋;见 run_recv_with)。
const RECV_ROUND_PACE: Duration = Duration::from_millis(500);

/// 接收单轮上下文。
struct RecvRound<'a> {
    session: &'a Session,
    home: &'a Path,
    out_dir: &'a Path,
    me: &'a DeviceIdentity,
    punch: punch::PunchConfig,
    fault: Option<usize>,
    /// 已起窗的接收 deadline(None = 首次等待,无期限)。
    deadline: Option<Instant>,
    /// 窗口时长(超时文案携带)。
    recv_window: Duration,
}

/// 一轮接收的结果:错误时也带回本轮已落盘路径(磁盘事实不随 Err 丢弃,
/// 由 run_recv_with 跨轮累计)。
struct RoundLanded {
    paths: Vec<PathBuf>,
    error: Option<anyhow::Error>,
}

/// 接收窗口已起窗后的等待包装:deadline 到点即 Err(文案携带窗口时长),
/// 不让无界等待(如 wait_connect_request)把窗口卡成摆设;未起窗
/// (首次等待)时透传,不加限制。
async fn windowed<T>(
    deadline: Option<Instant>,
    window: Duration,
    label: &str,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match deadline {
        None => fut.await,
        Some(d) => match tokio::time::timeout_at(tokio::time::Instant::from(d), fut).await {
            Ok(r) => r,
            Err(_) => Err(anyhow!(
                "接收窗口超时: {window:?} 内未完成 {label}(断连后对端未恢复或未再来件)"
            )),
        },
    }
}

/// 一轮接收:新信令连接 → HelloOk → 等 ConnectRequest → Accept/候选交换 →
/// punch → recv_files_fault。信令连接随本轮结束释放。陈旧 `.part` 由
/// intake 对账接管,不做清扫。`request_seen` 在等到 ConnectRequest 时置位
/// (调用方据此清窗重置)。
async fn recv_round(ctx: &RecvRound<'_>, request_seen: &mut bool) -> RoundLanded {
    match recv_handshake(ctx, request_seen).await {
        Err(e) => RoundLanded {
            paths: Vec::new(),
            error: Some(e),
        },
        Ok(conn) => {
            // 传输进行中不受窗口 deadline 约束(它在推进;QUIC 空闲超时
            // 治理其活性)。错误也带回已落盘路径。
            let out = transfer::recv_files_fault(&conn, ctx.out_dir, ctx.me, ctx.fault).await;
            RoundLanded {
                paths: out.paths,
                error: out.error.map(|e| e.context("transfer")),
            }
        }
    }
}

/// 一轮接收的传输前握手:新信令连接 → HelloOk → 等 ConnectRequest →
/// Accept/候选交换 → punch,返回打通的 QUIC 连接。窗口已起窗时,前三段
/// 等待(信令连接/HelloOk/等来件)全部 deadline 感知——等待中超窗即
/// Err;Accept 之后的信令往返各有 SIGNAL_TIMEOUT 内界,punch 有自身预算。
/// 等到 ConnectRequest 即置位 `request_seen`:那是发送方在线的明证。
async fn recv_handshake(ctx: &RecvRound<'_>, request_seen: &mut bool) -> Result<quinn::Connection> {
    let me = ctx.me;
    let (client, mut ev) = windowed(
        ctx.deadline,
        ctx.recv_window,
        "信令连接",
        signaling_connect(ctx.session, &me.device_id),
    )
    .await?;
    // HelloOk 是连接后第一帧:先取服务器通告的 STUN(空 → 本地候选降级),
    // 必须在 wait_connect_request 之前——它会把 HelloOk 当握手确认消费丢弃
    let stun = windowed(
        ctx.deadline,
        ctx.recv_window,
        "HelloOk",
        wait_hello_stun(&mut ev),
    )
    .await?;
    // wait_connect_request 在打开任何信封之前已完成发送方的 TOFU 固定,
    // 因此这里回信(seal)之前密钥必然已通过服务器目录 + TOFU 校验。
    let (from_device, peer_pk, session_id) = windowed(
        ctx.deadline,
        ctx.recv_window,
        "等待来件请求",
        wait_connect_request(ctx.session, ctx.home, &mut ev, me),
    )
    .await?;
    *request_seen = true;

    // 双角色端点(STUN 候选收集 → socket 移交 quinn)→ 回 Accept + 本端
    // Candidates → 等发送方 Candidates → 双方互拨
    let (epn, mine, nat_symmetric, _stun_ok) =
        setup_punch_endpoint(me, &from_device, &peer_pk, &stun)?;
    relay_with_timeout(
        &client,
        &from_device,
        &seal(
            me,
            &peer_pk,
            &SignalBody::ConnectAccept {
                session_id: session_id.clone(),
            },
        ),
    )
    .await?;
    relay_with_timeout(
        &client,
        &from_device,
        &seal(
            me,
            &peer_pk,
            &SignalBody::Candidates {
                session_id: session_id.clone(),
                addrs: mine.iter().map(|a| a.to_string()).collect(),
            },
        ),
    )
    .await?;
    let theirs = match wait_opened(
        &mut ev,
        &from_device,
        me,
        &peer_pk,
        |b| matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id),
    )
    .await
    .context("waiting for sender Candidates")?
    {
        SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
        other => anyhow::bail!("unexpected signal body {other:?}"),
    };

    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(from_device.clone(), peer_pk);
    punch::punch_connect(&epn, me, &allowed, &peer_pk, &theirs, &ctx.punch)
        .await
        .map_err(|e| punch_err(e, nat_symmetric, mine.len()))
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
    let dev_code = code["code"]
        .as_str()
        .context("no code in response")?
        .to_string();
    eprintln!("dev-mode code for {email}: {dev_code}");

    print!("enter code: ");
    use std::io::Write as _;
    std::io::stdout().flush().ok();
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("read code from stdin")?;

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
        refresh: v["refresh"]
            .as_str()
            .context("no refresh in response")?
            .into(),
        device_id: v["device_id"]
            .as_str()
            .context("no device_id in response")?
            .into(),
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
        assert_eq!(
            ws_url("https://signal.example.com"),
            "wss://signal.example.com/ws"
        );
        assert_eq!(ws_url("http://h/api/"), "ws://h/api/ws"); // 尾部斜杠收敛
        assert_eq!(ws_url("ws://127.0.0.1:1"), "ws://127.0.0.1:1/ws");
        assert_eq!(ws_url("wss://s"), "wss://s/ws");
        assert_eq!(ws_url("127.0.0.1:8787"), "ws://127.0.0.1:8787/ws");
        // 子串不改写：只有前缀位置才发生替换
        assert_eq!(
            ws_url("http://http-trap.example"),
            "ws://http-trap.example/ws"
        );
    }

    // multi_thread:binding_query 用 std 阻塞 recv 占死当前线程,
    // current_thread 运行时下 spawn 的监听任务永远得不到轮询(与
    // core/tests/stun_client.rs 同款约束)。
    #[tokio::test(flavor = "multi_thread")]
    async fn single_stun_addr_uses_alt_primary() {
        // 回环真 STUN 单端口;单条通告下 gather 必须成功(stun 路径不降级为 local-only)
        let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let pa = s.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = stun::run_listener(s, 100, 100).await;
        });
        let me = mtdss_core::identity::DeviceIdentity::generate();
        let (ep, addrs, _, stun_ok) =
            setup_punch_endpoint(&me, "peer", &me.public_key(), &[pa.to_string()]).unwrap();
        assert!(!addrs.is_empty());
        // 降级路径 local-only 也非空,无法区分;stun_ok 才钉死"真实 gather 成功"
        assert!(stun_ok);
        drop(ep);
    }

    #[test]
    fn only_network_errors_are_retryable() {
        // 终局错误不空耗重试轮次:仅 Network 值得重连
        assert!(retryable(&transfer::SendError::Network("x".into())));
        assert!(!retryable(&transfer::SendError::Rejected {
            reason: "disk".into()
        }));
        assert!(!retryable(&transfer::SendError::Protocol {
            reason: "rounds".into()
        }));
    }

    #[test]
    fn collect_items_paths_and_errors() {
        let base = tempfile::tempdir().unwrap();
        // dir/{a.txt, sub/b.bin}:目录递归展开,read_dir 按名排序("a.txt" < "sub")
        let dir = base.path().join("dir");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("sub").join("b.bin"), b"b").unwrap();
        // 目录内保留段 .mtdss:嵌套 .mtdss 目录(dir/.mtdss/x.bin)与
        // .mtdss 文件(sub/.mtdss)都不进清单,其余条目保持不变
        std::fs::create_dir_all(dir.join(".mtdss")).unwrap();
        std::fs::write(dir.join(".mtdss").join("x.bin"), b"x").unwrap();
        std::fs::write(dir.join("sub").join(".mtdss"), b"f").unwrap();
        // 单文件 c.txt
        let c = base.path().join("c.txt");
        std::fs::write(&c, b"c").unwrap();

        let items = collect_items(&[dir.clone(), c.clone()]).unwrap();
        let rels: Vec<&str> = items.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            rels,
            vec!["dir/a.txt", "dir/sub/b.bin", "c.txt"],
            "保输入序"
        );
        // 目录内绝对路径必须指向真实文件
        assert!(items.iter().all(|(p, _)| p.is_file()));
        // .mtdss 保留段(文件或目录)整段不入清单(顶层输入名 .mtdss 另由
        // collect_items 入口检查报 Err,见本测试末尾)
        assert!(
            !items
                .iter()
                .any(|(_, r)| r.split('/').any(|seg| seg == ".mtdss")),
            "no rel may carry a .mtdss segment: {items:?}"
        );

        // 不存在路径 → Err
        assert!(collect_items(&[base.path().join("nope")]).is_err());
        // 空目录(仅此一项)→ Err
        let empty = base.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(collect_items(&[empty]).is_err());

        // 符号链接跳过:目录内 symlink 条目不进清单(Windows 无特权建链,仅 Unix 验证)
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&c, dir.join("link.txt")).unwrap();
            let items = collect_items(&[dir.clone()]).unwrap();
            assert!(
                !items.iter().any(|(_, r)| r.contains("link.txt")),
                "symlink entries must be skipped: {items:?}"
            );
            // symlink 作为顶层输入:跳过(唯一输入被跳过 → 全空 Err)
            let link = base.path().join("top-link.txt");
            std::os::unix::fs::symlink(&c, &link).unwrap();
            assert!(collect_items(&[link]).is_err());
        }

        // 两输入产生相同 rel → Err:不同父目录下同名目录,内部各有 f.txt
        let p1 = base.path().join("p1");
        let p2 = base.path().join("p2");
        std::fs::create_dir_all(p1.join("same")).unwrap();
        std::fs::create_dir_all(p2.join("same")).unwrap();
        std::fs::write(p1.join("same").join("f.txt"), b"1").unwrap();
        std::fs::write(p2.join("same").join("f.txt"), b"2").unwrap();
        assert!(collect_items(&[p1.join("same"), p2.join("same")]).is_err());

        // 输入名 ".mtdss"(目录或文件)→ Err:接收方 intake 保留段
        let reserved = base.path().join(".mtdss");
        std::fs::create_dir_all(&reserved).unwrap();
        assert!(collect_items(std::slice::from_ref(&reserved)).is_err());
        std::fs::remove_dir(&reserved).unwrap();
        std::fs::write(&reserved, b"x").unwrap();
        assert!(collect_items(std::slice::from_ref(&reserved)).is_err());
    }
}
