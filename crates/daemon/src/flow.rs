//! send/recv 信令 + 连接编排。
//!
//! 安全模型（不可妥协）：
//! - 对端公钥永远从服务器目录（GET /devices，按 device_id）解析，再用
//!   `PeersCache` 做 TOFU 固定：首次记录，之后必须一致。信封内的
//!   `sender_device_id` 字段是未签名数据，绝不用于密钥解析——验签一律用
//!   服务器目录 + TOFU 得到的公钥。入站信令的解析/固定/验签收口在枢纽
//!   (`crate::hub`,单连接收敛后唯一信令连接由它独占),本文件只编排
//!   发送面与接收落盘。
//! - 任何面向对端的信封（seal）之前必须先完成 TOFU 校验。
//! - 所有信令等待整体受 `SIGNAL_TIMEOUT` 约束,错误帧/连接关闭立即上抛;
//!   互拨连接（punch_connect）受其 PunchConfig 预算约束（默认 10s）。

use crate::hub::{wait_signal, SignalHub};
use crate::session::{PeersCache, Session};
use anyhow::{anyhow, Context, Result};
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::net::stun::gather_candidates;
use mtdss_core::signaling::client::{ClientEvent, SignalingClient};
use mtdss_core::signaling::envelope::{seal, SignalBody};
use mtdss_core::transport::{endpoint as ep, punch, transfer};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

/// 单条信令往返的整体超时:枢纽 `wait_signal` 等对端信令(Accept/Candidates)
/// 各受它约束;信令连接建立另有同值上界(signaling_connect)。常驻接收的
/// 等来件无期限(recv 的语义就是长期等待),不受此常量约束。
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
/// Task 8 撤 shim 后无外部调用者,收敛为 pub(crate)（对外入口是 account::
/// load_identity 与 daemon 内部编排）。
pub(crate) fn load_identity(home: &Path) -> Result<DeviceIdentity> {
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
/// `pub(crate)`：Task 6 在线循环复用同一连接入口。
pub(crate) async fn signaling_connect(
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
/// `pub(crate)`:枢纽控制通道(Task 4)按同一口径解析目标设备。
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

/// TOFU 固定并持久化：首次记录，之后必须一致，变更即硬错误。
/// 缓存文件固定在 `home`（每设备目录独立）。`pub(crate)`:枢纽对入站信令
/// 开箱前同样经此固定(收发两侧同一份 peers.json,口径一致)。
pub(crate) fn tofu_pin(home: &Path, device_id: &str, pk: &VerifyingKey) -> Result<()> {
    let path = home.join("peers.json");
    let mut cache = PeersCache::load(&path)?;
    cache.expected_peer(device_id, pk)?;
    cache.save(&path)?;
    Ok(())
}

/// 会话 id 随机数(16 字节,防跨会话信令串扰)。`pub(crate)`:枢纽控制
/// 通道(ctrl.rs,Task 4)与发送/接收编排共用同一随机面。
pub(crate) fn rand_session_id() -> Vec<u8> {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b.to_vec()
}

// ---------- 有界等待 ----------

/// 给任何可能无限 pending 的 `Result` 型 future 加超时上限（超时即 Err）。
/// 契约：future 在 `d` 内不完成 → 返回带 `label` 的超时错误，绝不永久挂起。
/// 公开以便测试直接用永不完成的 future 固定该契约;`SignalHub::relay`
/// 经它对无内建超时的 relay 写入截断。
pub async fn bounded<T>(
    label: &'static str,
    d: Duration,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(d, fut)
        .await
        .map_err(|_| anyhow!("{label} timed out after {d:?} (peer unresponsive)"))?
}

// ---------- 信令等待 ----------

// 入站信令等待已收口到枢纽:hub.rs 的 wait_signal(定向等一条已验签信令)
// 与 wait_incoming_request(常驻接收等 ConnectRequest,发送方未知)取代旧
// wait_opened/wait_hello_stun/wait_connect_request——信封开箱/密钥解析/
// TOFU 固定统一在枢纽完成,本文件不再直接消费 mpsc 信令流。

// ---------- 打洞编排 ----------

/// 解析对端候选字符串 → SocketAddr（跳过不可解析并告警，截断 MAX_CANDIDATES）。
/// `pub(crate)`:枢纽控制通道(Task 4)复用同一候选交换消息格式。
pub(crate) fn parse_candidates(addrs: &[String]) -> Vec<SocketAddr> {
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

/// 绑定 raw socket → STUN 候选收集 → 移交 quinn 建双角色端点(白名单=对端)。
/// 返回 (端点, 本端候选, 本侧对称 NAT 标记, STUN 可达标记)。候选收集是阻塞式查询
/// （最坏约 4s：primary 慢成功 + alt 超时，每次查询上限 2s）：必须在
/// socket 移交 quinn 之前完成，仍在对端 `SIGNAL_TIMEOUT`（10s）等待窗口内；
/// 通告为空或 STUN 不可达时降级为仅本地候选（stun_ok=false）。
/// `tuning` 注入到端点服务端 config:本端 **accept** 的连接按它做 keep-alive/
/// 空闲超时(拨号侧经 `PunchConfig.tuning` 另行注入,两侧调优由此贯通)。
/// `pub(crate)`:枢纽控制通道(Task 4)按同一编舞建端点。
pub(crate) fn setup_punch_endpoint(
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    stun: &[String],
    tuning: ep::TransportTuning,
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
    // 候选行稳定格式(netns 矩阵 grep 依赖 "candidates: local=[" 前缀):
    // local 不含 srflx 重复项;srflx 为空串表示无映射(STUN 不可达或映射==本地)。
    let srflx = g.srflx.map(|a| a.to_string()).unwrap_or_default();
    let locals: Vec<String> = g
        .addrs
        .iter()
        .filter(|a| Some(**a) != g.srflx)
        .map(|a| a.to_string())
        .collect();
    tracing::info!(
        "candidates: local=[{}] srflx=[{}] symmetric={}",
        locals.join(","),
        srflx,
        g.nat_symmetric
    );
    let nat_symmetric = g.nat_symmetric;
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(peer_id.to_string(), *peer_pk);
    let epn = ep::make_dual_endpoint_tuned(sock, me, &allowed, tuning)?;
    Ok((epn, g.addrs, nat_symmetric, g.stun_ok))
}

/// punch 失败的统一诊断包装（spec §7：文案恒含本侧 NAT 类型）。
/// 对称时附建议；非对称追加 "local NAT non-symmetric"（排障可据此排除
/// 本侧 NAT 因素）。`pub(crate)`:枢纽控制通道(ctrl.rs,Task 4)把诊断
/// 全文收进 CtrlErr::Punch,与传输面同一文案口径。
pub(crate) fn punch_err(e: anyhow::Error, nat_symmetric: bool, mine: usize) -> anyhow::Error {
    let mut msg = format!("{e:#}; local candidates: {mine}");
    if nat_symmetric {
        msg.push_str("; local NAT appears symmetric (直连成功率较低,建议换网络或同网段)");
    } else {
        msg.push_str("; local NAT non-symmetric");
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

/// 读单个环境调优值:缺失 → None;非法(非数字/空串/超界)→ warn 并返回
/// None(保持默认,绝不 panic)。`get` 抽象环境访问,便于单测注入。
fn test_env_num<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    key: &str,
) -> Option<T> {
    let raw = get(key)?;
    match raw.parse::<T>() {
        Ok(v) => Some(v),
        Err(_) => {
            tracing::warn!("ignored invalid {key}={raw:?} (keep default)");
            None
        }
    }
}

/// 测试注入面:从环境读五个调优旋钮写入两侧编排选项。生产 main.rs 以
/// `|k| std::env::var(k).ok()` 调用;netns 矩阵/混沌测试靠它对**真实二进制**
/// 调参(没有源码级入口)。非法值 warn 后保持默认;`MTDSS_TEST_KEEPALIVE_MS=0`
/// 表示关闭 keep-alive(静默死链超时测试专用)。毫秒级注入不触碰 Task 4 在
/// transport_config 里的亚毫秒 debug_assert(0 已由本函数转成 None)。
///
/// 旋钮覆盖:`MAX_RECONNECTS` 只作用于**发送侧**重连循环(接收侧重连由
/// 窗口驱动,不读该字段,故不写 recv)。任一旋钮成功解析即 info 命名生效项。
pub fn apply_test_env(
    send: &mut SendOpts,
    recv: &mut RecvOpts,
    get: impl Fn(&str) -> Option<String>,
) {
    let mut applied: Vec<&str> = Vec::new();
    // IDLE_TIMEOUT_MS=0 不在表内语义中:quinn 把 0ms 空闲超时当作"不限",
    // 与本旋钮意图相反,且会触发 transport_config 的亚毫秒 debug_assert
    // (Task 4)——按非法值处理保持默认,保证注入面永不触碰该断言。
    if let Some(ms) = test_env_num::<u64>(&get, "MTDSS_TEST_IDLE_TIMEOUT_MS") {
        if ms == 0 {
            tracing::warn!("MTDSS_TEST_IDLE_TIMEOUT_MS=0 would disable the idle timeout; keep default");
        } else {
            let d = Duration::from_millis(ms);
            send.punch.tuning.max_idle_timeout = d;
            recv.punch.tuning.max_idle_timeout = d;
            applied.push("MTDSS_TEST_IDLE_TIMEOUT_MS");
        }
    }
    if let Some(ms) = test_env_num::<u64>(&get, "MTDSS_TEST_KEEPALIVE_MS") {
        // 0 → None:关闭 keep-alive(其余值换算毫秒间隔)
        let ka = (ms > 0).then(|| Duration::from_millis(ms));
        send.punch.tuning.keep_alive_interval = ka;
        recv.punch.tuning.keep_alive_interval = ka;
        applied.push("MTDSS_TEST_KEEPALIVE_MS");
    }
    if let Some(n) = test_env_num::<u32>(&get, "MTDSS_TEST_MAX_RECONNECTS") {
        send.retry.max_reconnects = n;
        applied.push("MTDSS_TEST_MAX_RECONNECTS(send)");
    }
    if let Some(ms) = test_env_num::<u64>(&get, "MTDSS_TEST_BACKOFF_MS") {
        send.retry.backoff_base = Duration::from_millis(ms);
        applied.push("MTDSS_TEST_BACKOFF_MS");
    }
    if let Some(ms) = test_env_num::<u64>(&get, "MTDSS_TEST_RECV_WINDOW_MS") {
        recv.retry.recv_window = Duration::from_millis(ms);
        applied.push("MTDSS_TEST_RECV_WINDOW_MS");
    }
    if !applied.is_empty() {
        tracing::info!("test tuning applied via MTDSS_TEST_* env: {}", applied.join(","));
    }
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

/// 带选项的发送编排:清单一次构建(全部重试共用同一份字节与 transfer_id,
/// 接收方按 rel 对账,断点续传才可能跨轮成立);每轮经枢纽在当前信令连接
/// 上重打洞并重传(枢纽离线即轮失败,在线循环退避重连后自愈),仅
/// Network(含信令/打洞失败)续轮退避重试,Rejected/Protocol 立即终局;
/// 终局失败按 rel→原始输入路径出成败清单。`pub(crate)`:send RPC(api.rs)
/// 经它编排,签名含枢纽句柄,不外泄到 crate 面。
pub(crate) async fn run_send_with(
    session: &Session,
    home: &Path,
    hub: &SignalHub,
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

    // 清单就绪行:send_log 事件/CLI 输出的首行(清单规模先于传输可见,e2e
    // 事件断言以「清单行」为传输已进入编排的锚点)。
    let total: u64 = items
        .iter()
        .map(|(p, _)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    out_report(&format!("清单: {} 个文件,共 {total} 字节", items.len()));

    // 对端公钥:服务器目录 + TOFU(首次记录/之后必须一致),整个 run 固定。
    // 发送目标现拉目录(REST 为权威),不经枢纽缓存——目标刚刚上线/注册的
    // 场景下新鲜目录才是准的。
    let devices = fetch_devices(session).await?;
    let peer_pk = peer_pk_from(&devices, to_device)?;
    tofu_pin(home, to_device, &peer_pk)?;

    let tracker = Mutex::new(SendTracker::default());
    let ctx = SendCtx {
        me: &me,
        hub,
        to_device,
        peer_pk: &peer_pk,
        man: &man,
        items: &items,
        punch: opts.punch,
        tracker: &tracker,
    };

    // 重试即策略:每轮 = 经枢纽走信令 + 重打洞 + send_files(接收方按清单
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

/// run_send_with 的跨尝试不变上下文(会话只服务轮前的目录拉取,不进轮内:
/// 信令面经枢纽,不再需要每轮会话)。
struct SendCtx<'a> {
    me: &'a DeviceIdentity,
    hub: &'a SignalHub,
    to_device: &'a str,
    peer_pk: &'a VerifyingKey,
    man: &'a mtdss_proto::manifest::Manifest,
    items: &'a [(PathBuf, String)],
    punch: punch::PunchConfig,
    tracker: &'a Mutex<SendTracker>,
}

/// 单轮发送尝试:经枢纽在当前信令连接上走 ConnectRequest/Accept → 候选
/// 交换 → punch → send_files。枢纽离线(无当前代际)或代际更替(订阅收
/// Closed)一律按 Network 分类(连接层问题,值得重试,在线循环退避重连后
/// 自愈);send_files 的错误分类原样上抛由外层策略分派。
async fn send_round(ctx: &SendCtx<'_>) -> Result<transfer::SendReport, transfer::SendError> {
    let me = ctx.me;
    let to = ctx.to_device;
    let hub = ctx.hub;
    let net = |e: anyhow::Error| transfer::SendError::Network(format!("{e:#}"));

    // 先订阅再发信令:Accept/Candidates 不会早于订阅窗口(漏帧即轮失败重试)
    let mut rx = hub
        .subscribe()
        .ok_or_else(|| anyhow!("信令未在线,无法发起发送"))
        .map_err(net)?;
    let stun = hub.stun();
    let session_id = rand_session_id();
    hub.relay(
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
    wait_signal(
        &mut rx,
        to,
        |b| matches!(b, SignalBody::ConnectAccept { session_id: s } if *s == session_id),
    )
    .await
    .context("waiting for ConnectAccept")
    .map_err(net)?;

    // Accept 后建双角色端点,回发本端 Candidates,再等对端 Candidates,
    // 双方互拨(punch_connect 内部完成 accept 竞速与角色固定)
    let (epn, mine, nat_symmetric, _stun_ok) =
        setup_punch_endpoint(me, to, ctx.peer_pk, &stun, ctx.punch.tuning).map_err(net)?;
    hub.relay(
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
    let theirs = match wait_signal(
        &mut rx,
        to,
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

// 单连接收敛后,接收编排只剩常驻接收一路(等 ConnectRequest 在枢纽的来件
// 请求通道上;信封开箱/密钥解析/TOFU 固定统一由枢纽完成)。旧的一次性
// CLI 接收面(run_recv/run_recv_with:自建信令连接 + 接收窗口)随 CLI 转
// 纯 IPC 客户端、信令连接收敛为枢纽单连一并移除——自建连接正是 presence
// 槽互抢 bug 的根源。

/// 直读枢纽来件请求通道等第一条 ConnectRequest:发送方未知,不能用
/// `wait_signal` 的定向过滤。无期限(常驻接收语义:recv 就是长期等来件);
/// 通道随枢纽代际安装即存在,取件晚于来件时来件在缓冲排队(广播无订阅者
/// 即丢,来件请求不可丢);通道关闭(连接代际更替)→ Err,调用方回外层
/// 重取下一代际的接收端。
async fn wait_incoming_request(
    rx: &mut mpsc::Receiver<crate::hub::SignalEvt>,
) -> Result<(String, Vec<u8>)> {
    loop {
        match rx.recv().await {
            Some((from, SignalBody::ConnectRequest { session_id })) => {
                return Ok((from, session_id));
            }
            Some(_) => {} // 其他体(通道只投递请求类,防御分支):忽略
            None => anyhow::bail!("信令连接代际更替(等待来件被打断)"),
        }
    }
}

/// 接收轮重进前的固定小歇(防快败热旋:信令不可达/打洞失败等)。
const RECV_ROUND_PACE: Duration = Duration::from_millis(500);

/// 接收单轮上下文(常驻接收;一次性 CLI 接收面已随单连接收敛移除——
/// 枢纽订阅是唯一信令来源,常驻循环是唯一接收编排)。
struct RecvRound<'a> {
    out_dir: &'a Path,
    me: &'a DeviceIdentity,
    punch: punch::PunchConfig,
    /// 注错钩子(第 n 个新 finalize 落盘后主动断连),生产恒 None。
    fault: Option<usize>,
}

/// 一轮接收的结果:错误时也带回本轮已落盘路径(磁盘事实不随 Err 丢弃)。
struct RoundLanded {
    paths: Vec<PathBuf>,
    error: Option<anyhow::Error>,
}

/// 已握手连接上的接收落盘(传输进行中不可打断,活性由 QUIC 空闲超时治理)。
/// 错误也带回已落盘路径(磁盘事实不随 Err 丢弃)。
async fn recv_on_conn(ctx: &RecvRound<'_>, conn: quinn::Connection) -> RoundLanded {
    let out = transfer::recv_files_fault(&conn, ctx.out_dir, ctx.me, ctx.fault).await;
    RoundLanded {
        paths: out.paths,
        error: out.error.map(|e| e.context("transfer")),
    }
}

/// 已收到 ConnectRequest 后的传输前握手:双角色端点(hub.stun 候选收集 →
/// socket 移交 quinn)→ 经枢纽回 Accept + 本端 Candidates → 等发送方
/// Candidates(wait_signal,信令往返各有 SIGNAL_TIMEOUT 内界,punch 有自身
/// 预算)→ 双方互拨,返回打通的 QUIC 连接。发送方 TOFU 已由枢纽开箱时
/// 固定,这里回信(seal)之前密钥必然已通过服务器目录 + TOFU 校验。
async fn recv_handshake(
    ctx: &RecvRound<'_>,
    hub: &SignalHub,
    from_device: &str,
    session_id: Vec<u8>,
    peer_pk: &VerifyingKey,
    rx: &mut tokio::sync::broadcast::Receiver<crate::hub::SignalEvt>,
) -> Result<quinn::Connection> {
    let me = ctx.me;
    let (epn, mine, nat_symmetric, _stun_ok) =
        setup_punch_endpoint(me, from_device, peer_pk, &hub.stun(), ctx.punch.tuning)?;
    // 双角色端点建好后回 Accept + 本端 Candidates,再等发送方 Candidates
    hub.relay(
        from_device,
        &seal(
            me,
            peer_pk,
            &SignalBody::ConnectAccept {
                session_id: session_id.clone(),
            },
        ),
    )
    .await?;
    hub.relay(
        from_device,
        &seal(
            me,
            peer_pk,
            &SignalBody::Candidates {
                session_id: session_id.clone(),
                addrs: mine.iter().map(|a| a.to_string()).collect(),
            },
        ),
    )
    .await?;
    let theirs = match wait_signal(
        rx,
        from_device,
        |b| matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id),
    )
    .await
    .context("waiting for sender Candidates")?
    {
        SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
        other => anyhow::bail!("unexpected signal body {other:?}"),
    };

    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(from_device.to_string(), *peer_pk);
    punch::punch_connect(&epn, me, &allowed, peer_pk, &theirs, &ctx.punch)
        .await
        .map_err(|e| punch_err(e, nat_symmetric, mine.len()))
}

// ---------- 常驻接收(守护进程) ----------

/// 关停等待:watch 边沿安全——先 `borrow_and_update` 查现值,再等边沿,
/// 防多个 select 站点先后消费同一次关停边沿后漏判。发送端消失(状态已弃)
/// 同样按关停处理。
async fn wait_shutdown(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// 常驻接收循环(守护进程):单连接收敛后的唯一接收编排。在线后对枢纽取
/// 「广播订阅 + 来件请求通道」各一次、跨代际持有——等 ConnectRequest 走
/// 请求通道(随代际安装即存在,取件晚于来件的窗口里来件在缓冲排队,不再
/// 依赖订阅时机;round 内的 Accept/Candidates 等待继续走广播订阅,「先订阅
/// 后发信令」的轮纪律照旧);连接代际更替(通道收 None / 广播收 Closed)
/// 回外层对下一代重取。轮循环:等 ConnectRequest(无期限)→ 枢纽查 pk →
/// Accept/候选交换 → punch → 传输;轮错误 → warn + RECV_ROUND_PACE 小歇
/// 重进,干净轮 → 直接回环等下一来件;永不因 Ok/Err 返回,仅在 shutdown
/// 触发或登出(登录态消失)时收尾。
///
/// 运行参数读取口径:接收目录每轮现读(set_recv_dir 对下一来件自然生效,
/// 不再有重启通知机制);身份与编排选项按连接代际现读(启动注入,代内
/// 不变;旧「令牌轮换即时生效」随自建连接一并消失——枢纽连接的令牌由
/// 在线循环维护)。
pub(crate) async fn resident_recv_loop(
    state: std::sync::Arc<crate::state::DaemonState>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut login_rx = state.login_watch();
    loop {
        // 未登录:通知驱动挂起(不轮询);关停即退
        while !state.logged_in() {
            tokio::select! {
                r = login_rx.changed() => {
                    if r.is_err() {
                        return; // 防御:发送端随 state 常驻,正常不可达
                    }
                }
                _ = wait_shutdown(&mut shutdown) => return,
            }
        }
        // 等连接代际并取「广播订阅 + 来件请求通道」:枢纽连接建立即可取
        // (请求通道随代际安装即存在,先到的 ConnectRequest 在通道排队等
        // 取件);离线/登出期间小步轮询等待。两取几乎原子,跨代际错配
        // (换代夹在两取之间)时旧代广播订阅/通道随代际弃置快速失败,回
        // 外层重取,自愈。
        let (mut rx, mut req_rx) = loop {
            if let (Some(rx), Some(req_rx)) =
                (state.hub().subscribe(), state.hub().take_request_rx())
            {
                break (rx, req_rx);
            }
            tokio::select! {
                _ = tokio::time::sleep(RECV_ROUND_PACE) => {}
                _ = wait_shutdown(&mut shutdown) => return,
                r = login_rx.changed() => {
                    if r.is_err() {
                        return;
                    }
                }
            }
        };
        // 按代际现读:身份(种子损坏属持久故障,小歇重试不热旋)与编排选项
        let home = state.home();
        let Ok(me) = load_identity(&home) else {
            tracing::warn!("常驻接收:加载本机身份失败");
            tokio::time::sleep(RECV_ROUND_PACE).await;
            continue;
        };
        let opts = state.recv_opts();

        // 轮循环:等来件 → 握手 → 传输
        loop {
            // 等第一条 ConnectRequest(无期限;关停/登出翻转可打断)
            let (from_device, session_id) = tokio::select! {
                r = wait_incoming_request(&mut req_rx) => match r {
                    Ok(v) => v,
                    Err(e) => {
                        // 代际更替(断连/重连)或防御性中断:回外层重取新代
                        tracing::warn!(%e, "常驻接收:来件请求通道中断,重取");
                        break;
                    }
                },
                _ = wait_shutdown(&mut shutdown) => return,
                r = login_rx.changed() => {
                    if r.is_err() {
                        return;
                    }
                    break; // 登出/重登翻转:弃当前代,外层按登录态重进
                }
            };
            // 发送方公钥:枢纽缓存(开箱时已经服务器目录解析 + TOFU 固定,
            // 常态必命中;未命中属防御分支,弃轮等重试)
            let Some(peer_pk) = state.hub().peer_pk(&from_device) else {
                tracing::warn!("常驻接收:来件方 {from_device} 不在公钥缓存,弃轮");
                if idle_between_rounds(&mut login_rx, &mut shutdown).await {
                    return;
                }
                continue;
            };
            // 每轮现读接收目录(set_recv_dir 对下一来件生效)
            let out_dir = state.recv_dir();
            let ctx = RecvRound {
                out_dir: &out_dir,
                me: &me,
                punch: opts.punch,
                fault: None, // 常驻循环无注错面(生产恒为干净路径)
            };
            // 握手(Accept/候选交换/互拨)→ 传输
            match recv_handshake(&ctx, state.hub(), &from_device, session_id, &peer_pk, &mut rx)
                .await
            {
                Ok(conn) => {
                    let landed = recv_on_conn(&ctx, conn).await;
                    if let Some(e) = landed.error {
                        tracing::warn!(%e, "常驻接收轮失败(传输阶段)");
                        if idle_between_rounds(&mut login_rx, &mut shutdown).await {
                            return;
                        }
                        continue;
                    }
                    // 干净轮:逐文件打「接收完成」终局行——nat-matrix/chaos 的
                    // 落盘断言与日常排障共用该信号(SKIP 的文件不在 paths,
                    // 不产生行);直接回环等下一来件
                    for p in &landed.paths {
                        tracing::info!("接收完成: {}", p.display());
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, "常驻接收轮失败(等待阶段)");
                    if idle_between_rounds(&mut login_rx, &mut shutdown).await {
                        return;
                    }
                }
            }
        }
    }
}

/// 轮间固定小歇(RECV_ROUND_PACE,防快败热旋)。返回 true 表示已请求关停
/// (或登录通知源消失)——仅此退出循环;登录翻转不退出,交外层按登录态
/// 分流(登出回等登录,重登即开新一轮)。
async fn idle_between_rounds(
    login_rx: &mut tokio::sync::watch::Receiver<bool>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(RECV_ROUND_PACE) => false,
        _ = wait_shutdown(shutdown) => true,
        r = login_rx.changed() => r.is_err(),
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
        let (ep, addrs, _, stun_ok) = setup_punch_endpoint(
            &me,
            "peer",
            &me.public_key(),
            &[pa.to_string()],
            ep::TransportTuning::default(),
        )
        .unwrap();
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
    fn punch_err_always_names_nat_type() {
        // spec §7:文案恒含本侧 NAT 类型(矩阵与人工排障依赖该稳定标签)
        let sym = punch_err(anyhow::anyhow!("boom"), true, 2);
        assert!(format!("{sym:#}").contains("local NAT appears symmetric"));
        let nonsym = punch_err(anyhow::anyhow!("boom"), false, 2);
        assert!(format!("{nonsym:#}").contains("non-symmetric"));
    }

    #[test]
    fn apply_test_env_sets_all_five_knobs() {
        let mut m: HashMap<&str, String> = HashMap::new();
        m.insert("MTDSS_TEST_IDLE_TIMEOUT_MS", "1234".into());
        m.insert("MTDSS_TEST_KEEPALIVE_MS", "0".into());
        m.insert("MTDSS_TEST_MAX_RECONNECTS", "2".into());
        m.insert("MTDSS_TEST_BACKOFF_MS", "50".into());
        m.insert("MTDSS_TEST_RECV_WINDOW_MS", "77".into());
        let get = |k: &str| m.get(k).cloned();
        let mut so = SendOpts::default();
        let mut ro = RecvOpts::default();
        apply_test_env(&mut so, &mut ro, get);
        // 五旋钮逐一断言;KEEPALIVE_MS=0 → keep-alive 关闭(静默超时测试)
        for t in [&so.punch.tuning, &ro.punch.tuning] {
            assert_eq!(t.max_idle_timeout, Duration::from_millis(1234));
            assert_eq!(t.keep_alive_interval, None);
        }
        assert_eq!(so.retry.max_reconnects, 2);
        // MAX_RECONNECTS 只写发送侧:接收侧重连窗口驱动,不读该字段
        assert_eq!(ro.retry.max_reconnects, RecvOpts::default().retry.max_reconnects);
        assert_eq!(so.retry.backoff_base, Duration::from_millis(50));
        assert_eq!(ro.retry.recv_window, Duration::from_millis(77));
    }

    #[test]
    fn apply_test_env_ignores_garbage() {
        // 非法值(非数字/空串/超界)逐项 warn 后保持默认,绝不 panic
        let mut m: HashMap<&str, String> = HashMap::new();
        m.insert("MTDSS_TEST_IDLE_TIMEOUT_MS", "abc".into());
        m.insert("MTDSS_TEST_KEEPALIVE_MS", "".into());
        m.insert("MTDSS_TEST_MAX_RECONNECTS", "4294967296".into()); // 超 u32
        m.insert("MTDSS_TEST_BACKOFF_MS", "1.5".into());
        m.insert("MTDSS_TEST_RECV_WINDOW_MS", "0x10".into());
        let get = |k: &str| m.get(k).cloned();
        let mut so = SendOpts::default();
        let mut ro = RecvOpts::default();
        apply_test_env(&mut so, &mut ro, get);
        let (dso, dro) = (SendOpts::default(), RecvOpts::default());
        assert_eq!(
            so.punch.tuning.max_idle_timeout,
            dso.punch.tuning.max_idle_timeout
        );
        assert_eq!(
            so.punch.tuning.keep_alive_interval,
            dso.punch.tuning.keep_alive_interval
        );
        assert_eq!(
            ro.punch.tuning.max_idle_timeout,
            dro.punch.tuning.max_idle_timeout
        );
        assert_eq!(
            ro.punch.tuning.keep_alive_interval,
            dro.punch.tuning.keep_alive_interval
        );
        assert_eq!(so.retry.max_reconnects, dso.retry.max_reconnects);
        assert_eq!(so.retry.backoff_base, dso.retry.backoff_base);
        assert_eq!(ro.retry.max_reconnects, dro.retry.max_reconnects);
        assert_eq!(ro.retry.recv_window, dro.retry.recv_window);
    }

    #[test]
    fn apply_test_env_idle_zero_keeps_default() {
        // IDLE=0 会被 quinn 当作"不限空闲"且触发 transport_config 的
        // debug_assert(Task 4):按非法值处理,保持默认——注入面永不触碰该断言
        let get = |k: &str| (k == "MTDSS_TEST_IDLE_TIMEOUT_MS").then(|| "0".to_string());
        let mut so = SendOpts::default();
        let mut ro = RecvOpts::default();
        apply_test_env(&mut so, &mut ro, get);
        assert_eq!(
            so.punch.tuning.max_idle_timeout,
            SendOpts::default().punch.tuning.max_idle_timeout
        );
        assert_eq!(
            ro.punch.tuning.max_idle_timeout,
            RecvOpts::default().punch.tuning.max_idle_timeout
        );
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
            let items = collect_items(std::slice::from_ref(&dir)).unwrap();
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
