//! 多文件计划式传输引擎 v2（QUIC 固定连接上的断点续传传输）。
//!
//! 一次传输 = 一份签名清单 + 计划协商 + 有界并发的块流：
//!
//! S→R（控制流为发送方发起的双向流）：
//! 1. 控制流首帧 `[u32 BE man_len][manifest bytes]`；
//! 2. 每块一条独立单向流 `[u32 BE file_idx][u64 BE offset][u64 BE len][data]`，
//!    `(file_idx, offset)` 必须命中签名清单的块表；
//! 3. 一轮（波）内全部块推完后，控制流发 checkpoint `[0xFF]`。
//!
//! R→S（接收方在控制流本侧回帧，均带 tag）：
//! - 拒绝 `[0x00][u32 BE len][reason]`（磁盘预检不过等）；
//! - 计划 `[0x01][u32 BE file_count]{[u8 kind][u32 BE bmp_len][bmp]}`，
//!   kind 0=FULL / 1=SKIP / 2=RESUME（仅 RESUME 带 bmp，位=1 表示缺块待收）；
//! - 单文件完成 `[0x02][u32 BE file_idx]`（每文件恰好一次）；
//! - 终局 `[0x03]`（全部就绪；发送方收妥后半关控制流）。
//!
//! 接收侧以 `.mtdss/` 收件区（intake）为磁盘事实来源：计划由 `reconcile`
//! 对账得出，块先验哈希后定位写 `.part`，缺块清零即就地 finalize
//! （整文件读回校验 + 原子改名 + 记录 done），因此 FileDone 可能先于本轮
//! checkpoint 到达。checkpoint 到达时若仍有缺块，接收方复核并发新计划
//! （RESUME 位图），发送方最多重发 3 轮（round 0..=3），超出即
//! `SendError::Protocol`——有界重请求，坏 peer 不能让传输无限循环。
//!
//! 调度：每轮内计划文件按 size 升序、以 `max_concurrent_files` 为一波
//! （小文件优先出结果），波内文件并发、文件内块以 `max_inflight_per_file`
//! 有界并发（quinn 流控负责背压）；默认 4×4。
//!
//! 验证顺序（v1 保持，不可调整）：manifest 验签（对端证书恢复的
//! Ed25519 公钥）→ hex32 transfer_id → 磁盘预检 → 逐块哈希校验
//! （先验后写：`set_len` 预分配 + seek 定位写，坏块永不落盘）→
//! finalize 整文件哈希读回。EOF 舞步（v1 保持）：接收方发 `[0x03]` 后
//! 等发送方控制流半关的 EOF——否则本任务一返回、连接句柄被释放，QUIC
//! 的隐式 CONNECTION_CLOSE 会作废在途的终局帧，让已完整落盘的传输在
//! 发送方一侧误报失败。
//!
//! 首条服务端双向流已被 `endpoint::connect_pinned`/`accept_pinned` 的
//! 接受令牌占用；本模块的控制流为客户端发起，互不冲突。

use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures_util::{StreamExt as _, TryStreamExt as _};
use mtdss_proto::manifest::{ChunkDesc, FileEntry, Manifest};
use prost::Message as _;
use quinn::{Connection, RecvStream, SendStream};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;
use tokio::sync::{Mutex as TokioMutex, Notify};
use tokio::task::JoinSet;

use crate::identity::{DeviceIdentity, ED25519_SPKI_PREFIX};
use crate::protocol::manifest;
use crate::transport::intake::{self, FilePlan, PartWriter};

pub use crate::protocol::manifest::CHUNK_SIZE;

// ---------- 线协议常量 ----------

/// R→S 拒绝帧 tag。
const TAG_REJECT: u8 = 0x00;
/// R→S 计划帧 tag。
const TAG_PLAN: u8 = 0x01;
/// R→S 单文件完成帧 tag。
const TAG_FILEDONE: u8 = 0x02;
/// R→S 终局帧 tag。
const TAG_FINAL: u8 = 0x03;
/// S→R checkpoint（一轮推完）标记。
const TAG_CHECKPOINT: u8 = 0xFF;
/// 计划条目 kind：全新收。
const PLAN_KIND_FULL: u8 = 0;
/// 计划条目 kind：已完整落盘，跳过。
const PLAN_KIND_SKIP: u8 = 1;
/// 计划条目 kind：断点续传（位=1 缺块待收）。
const PLAN_KIND_RESUME: u8 = 2;

/// checkpoint 重请求轮数上限（round 0..=3，共 4 波）。
const MAX_ROUNDS: u32 = 3;
/// manifest 帧长上限（分配前检查，防恶意长度打爆内存）。
const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
/// 拒绝理由长度上限（防御性：对端可用 u32 长度诱导巨量分配）。
const MAX_REASON_BYTES: usize = 64 * 1024;
/// 计划位图长度上限（清单最多 65535 文件 × 每位图 ≤ ~175KB，1 MiB 足够）。
const MAX_PLAN_BITMAP_BYTES: usize = 1024 * 1024;
/// 计划文件数上限（与 manifest::verify 的 65535 一致）。
const MAX_PLAN_FILES: usize = 65535;

// ---------- 公开类型 ----------

/// 发送结果：已发送的应用层文件字节数（不含协议帧头）。
#[derive(Debug)]
pub struct SendReport {
    pub bytes_sent: u64,
}

/// 发送调度与测试注入选项。
#[derive(Debug, Clone)]
pub struct SendOptions {
    /// 同时推进的文件数上限（波宽；每轮内文件按 size 升序入波）。
    pub max_concurrent_files: usize,
    /// 单文件在途块流数上限（quinn 流控另管窗口背压）。
    pub max_inflight_per_file: usize,
    /// 测试注错：第 r 轮（0 起）且 r ≤ fault_drop_until_round 时，
    /// 跳过表中 `(file_idx, offset)` 命中的块（模拟丢流）。
    #[doc(hidden)]
    pub fault_drop_chunks: std::collections::HashSet<(u32, u64)>,
    /// 测试注错：丢块生效的最大轮号。
    #[doc(hidden)]
    pub fault_drop_until_round: u32,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            max_concurrent_files: 4,
            max_inflight_per_file: 4,
            fault_drop_chunks: HashSet::new(),
            fault_drop_until_round: 0,
        }
    }
}

/// 发送进度事件：单文件完成 / 单文件累计字节推进。
#[derive(Debug)]
pub enum SendProgress<'a> {
    /// 一个文件在接收方完整落盘（FileDone 帧到达）。
    FileDone { rel: &'a str },
    /// 一个文件又发出若干字节；done 为跨轮累计值，total 为文件大小。
    Bytes { rel: &'a str, done: u64, total: u64 },
}

/// 发送错误分类（Task 6 的重连编排据此分派）。
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// 网络层（含本地源文件 io）：连接/流读写失败或中断。
    #[error("network error: {0}")]
    Network(String),
    /// 接收方拒绝（0x00 帧带回的原因）。
    #[error("receiver rejected: {reason}")]
    Rejected { reason: String },
    /// 协议违规：坏 tag、计划不符、重请求轮数耗尽等。
    #[error("protocol violation: {reason}")]
    Protocol { reason: String },
}

fn net(msg: impl Into<String>) -> SendError {
    SendError::Network(msg.into())
}

fn proto(msg: impl Into<String>) -> SendError {
    SendError::Protocol { reason: msg.into() }
}

/// HRTB 回调桥：把 `&(dyn Fn(&SendProgress) + Sync)` 包成具体类型再
/// 下传。若让生成器直接持有 higher-ranked `dyn Fn(&T)` 引用，rustc 的
/// 泄漏检查无法在 `tokio::spawn` 处证明 Send（"Send is not general
/// enough"，#102211 系）；桥接后生成器只捕获具体结构体引用。
struct ProgressBridge<'a>(&'a (dyn Fn(&SendProgress) + Sync));

impl ProgressBridge<'_> {
    fn emit(&self, p: SendProgress<'_>) {
        (self.0)(&p);
    }
}

/// 路径所在卷的可用字节数。查询失败按"无限"处理——不因未知空间误拒。
pub fn free_bytes_at(path: &Path) -> u64 {
    fs2::available_space(path).unwrap_or(u64::MAX)
}

/// 磁盘预检：所需字节数超过可用即应拒绝接收。
pub fn reject_for_space(needed: u64, available: u64) -> bool {
    needed > available
}

// ---------- 发送端 ----------

/// 发送多文件：控制流交换清单与计划 → 按计划有界并发推块 → checkpoint
/// 协商补缺（≤3 轮）→ 终局确认。`man` 由调用方预建并签名（Task 1），
/// `sources[i]` 与 `man.files[i]` 一一对应（rel 必须相等）。
/// 返回实际发送的应用层字节总数（重发也计入；SKIP 不计）。
pub async fn send_files(
    conn: &Connection,
    man: &Manifest,
    sources: &[(PathBuf, String)],
    progress: &(dyn Fn(&SendProgress) + Sync),
    opts: &SendOptions,
) -> Result<SendReport, SendError> {
    // 参数自洽：源与清单逐条对位（build_manifest 的保序契约由调用方保证，
    // 此处兜底防错位导致读错文件）。
    if opts.max_concurrent_files == 0 || opts.max_inflight_per_file == 0 {
        return Err(proto("SendOptions concurrency limits must be at least 1"));
    }
    if sources.len() != man.files.len() {
        return Err(proto(format!(
            "sources ({}) do not match manifest files ({})",
            sources.len(),
            man.files.len()
        )));
    }
    for (i, (_, rel)) in sources.iter().enumerate() {
        if man.files[i].rel_path != *rel {
            return Err(proto(format!(
                "sources[{i}] rel {rel:?} does not match manifest entry {:?}",
                man.files[i].rel_path
            )));
        }
    }

    // 1) 控制流：发 manifest
    let (mut ctl_tx, ctl_rx) = conn
        .open_bi()
        .await
        .map_err(|e| net(format!("open control stream: {e}")))?;
    let man_bytes = man.encode_to_vec();
    ctl_tx
        .write_all(&(man_bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|e| net(format!("write manifest length: {e}")))?;
    ctl_tx
        .write_all(&man_bytes)
        .await
        .map_err(|e| net(format!("write manifest: {e}")))?;

    // 2) 控制读取任务 → mpsc（Plan / FileDone / Reject / Final / Closed）
    let (msg_tx, mut msg_rx) = mpsc::channel(64);
    let reader = tokio::spawn(read_control(ctl_rx, msg_tx));
    let bridge = ProgressBridge(progress);
    let result =
        send_rounds(conn, man, sources, &bridge, opts, &mut ctl_tx, &mut msg_rx).await;
    // Final 后读取任务已自退；错误路径中止阻塞中的读取，防任务泄漏。
    reader.abort();
    result
}

/// 控制流回帧（解析后的事件）。
enum ControlMsg {
    Plan(Vec<FilePlan>),
    FileDone(u32),
    Reject(String),
    Final,
    /// 流结束（EOF/错误）且未见 Final。
    Closed,
    /// 帧解析失败（协议违规）。
    Failed(SendError),
}

/// 主发送循环：首轮计划 → 波次推块 → checkpoint → 消化回帧 → 下一轮。
#[allow(clippy::too_many_arguments)]
async fn send_rounds(
    conn: &Connection,
    man: &Manifest,
    sources: &[(PathBuf, String)],
    progress: &ProgressBridge<'_>,
    opts: &SendOptions,
    ctl_tx: &mut SendStream,
    msg_rx: &mut mpsc::Receiver<ControlMsg>,
) -> Result<SendReport, SendError> {
    let n = man.files.len();

    // 首帧必须是计划或拒绝
    let mut plans = match msg_rx.recv().await {
        Some(ControlMsg::Plan(p)) => p,
        Some(ControlMsg::Reject(r)) => {
            // 半关后再返回:接收方的拒绝路径在等本侧 EOF(v1 舞步的拒绝版),
            // 否则本侧连接句柄释放的隐式 CLOSE 可能作废在途的确认。
            let _ = ctl_tx.finish();
            return Err(SendError::Rejected { reason: r });
        }
        Some(ControlMsg::Failed(e)) => return Err(e),
        Some(ControlMsg::Closed) => return Err(net("control stream closed before plan")),
        Some(_) => return Err(proto("first control frame must be a plan or reject")),
        None => return Err(net("control reader exited before plan")),
    };
    if plans.len() != n {
        return Err(proto(format!(
            "plan covers {} files, manifest has {n}",
            plans.len()
        )));
    }

    let mut round: u32 = 0;
    let mut bytes_total: u64 = 0;
    let mut done: HashSet<u32> = HashSet::new();
    // 每文件累计已发字节（跨轮，Bytes 进度的累计基数）
    let mut file_progress: Vec<u64> = vec![0; n];

    'rounds: loop {
        // 本轮待推文件：计划非 SKIP 且未收到 FileDone；按 size 升序入波
        let mut todo: Vec<usize> = (0..n)
            .filter(|&i| {
                !done.contains(&(i as u32)) && !matches!(plans[i], FilePlan::Skip)
            })
            .collect();
        todo.sort_by_key(|&i| (man.files[i].size, i));

        for wave in todo.chunks(opts.max_concurrent_files) {
            let mut futs = Vec::with_capacity(wave.len());
            for &i in wave {
                let chunks = planned_chunks(&man.files[i], &plans[i], round, opts, i as u32)?;
                futs.push(push_file(
                    conn,
                    &sources[i].0,
                    &man.files[i].rel_path,
                    i as u32,
                    man.files[i].size,
                    chunks,
                    opts.max_inflight_per_file,
                    progress,
                    file_progress[i],
                ));
            }
            // 任一失败即返回，同波其余 future 被 drop（取消在途发送）
            let deltas = futures_util::future::try_join_all(futs).await?;
            for (k, &i) in wave.iter().enumerate() {
                file_progress[i] += deltas[k];
                bytes_total += deltas[k];
            }
        }

        // 3) 本轮推完：checkpoint，然后消化回帧
        ctl_tx
            .write_all(&[TAG_CHECKPOINT])
            .await
            .map_err(|e| net(format!("write checkpoint: {e}")))?;
        loop {
            match msg_rx.recv().await {
                Some(ControlMsg::FileDone(i)) => {
                    let i = usize::try_from(i).map_err(|_| {
                        proto(format!("filedone index {i} out of range"))
                    })?;
                    if i >= n {
                        return Err(proto(format!("filedone index {i} not in manifest")));
                    }
                    if done.insert(i as u32) {
                        progress.emit(SendProgress::FileDone {
                            rel: &man.files[i].rel_path,
                        });
                    }
                }
                Some(ControlMsg::Plan(p)) => {
                    if p.len() != n {
                        return Err(proto(format!(
                            "plan covers {} files, manifest has {n}",
                            p.len()
                        )));
                    }
                    round += 1;
                    if round > MAX_ROUNDS {
                        return Err(proto(format!(
                            "checkpoint rounds exhausted after {MAX_ROUNDS} re-request rounds"
                        )));
                    }
                    plans = p;
                    continue 'rounds;
                }
                Some(ControlMsg::Final) => {
                    // 半关闭控制流发送侧：让接收方（recv_files 末尾等待本侧
                    // EOF/连接结束）干净地结束，而不是靠连接关闭竞态。
                    ctl_tx
                        .finish()
                        .map_err(|e| net(format!("finish control stream: {e}")))?;
                    return Ok(SendReport {
                        bytes_sent: bytes_total,
                    });
                }
                Some(ControlMsg::Reject(r)) => {
                    let _ = ctl_tx.finish();
                    return Err(SendError::Rejected { reason: r });
                }
                Some(ControlMsg::Failed(e)) => return Err(e),
                Some(ControlMsg::Closed) => {
                    return Err(net("control stream closed before final ack"))
                }
                None => return Err(net("control reader exited unexpectedly")),
            }
        }
    }
}

/// 由计划得出本轮要推的块序列：FULL=全部、RESUME=位 1 的块、SKIP=空；
/// 再剔除 fault_drop 命中（r ≤ until_round）的块。
fn planned_chunks<'a>(
    entry: &'a FileEntry,
    plan: &FilePlan,
    round: u32,
    opts: &SendOptions,
    file_idx: u32,
) -> Result<Vec<&'a ChunkDesc>, SendError> {
    let selected: Vec<&ChunkDesc> = match plan {
        FilePlan::Full => entry.chunks.iter().collect(),
        FilePlan::Skip => Vec::new(),
        FilePlan::Resume(bmp) => {
            let want = entry.chunks.len().div_ceil(8);
            if bmp.len() != want {
                return Err(proto(format!(
                    "resume bitmap is {} bytes, expected {want} for {}",
                    bmp.len(),
                    entry.rel_path
                )));
            }
            entry
                .chunks
                .iter()
                .enumerate()
                .filter(|(i, _)| bmp[i / 8] & (1 << (i % 8)) != 0)
                .map(|(_, c)| c)
                .collect()
        }
    };
    Ok(selected
        .into_iter()
        .filter(|c| {
            !(round <= opts.fault_drop_until_round
                && opts.fault_drop_chunks.contains(&(file_idx, c.offset)))
        })
        .collect())
}

/// 单文件单轮的块推流：每块 seek+读源区间 → open_uni → 帧头+数据 →
/// finish，以 `inflight` 有界并发。返回本轮实际发送字节；Bytes 进度以
/// `base`（该文件跨轮累计值）为基数回调。
#[allow(clippy::too_many_arguments)]
async fn push_file(
    conn: &Connection,
    path: &Path,
    rel: &str,
    file_idx: u32,
    total: u64,
    chunks: Vec<&ChunkDesc>,
    inflight: usize,
    progress: &ProgressBridge<'_>,
    base: u64,
) -> Result<u64, SendError> {
    // 每文件一个句柄：块任务并发读同一文件必须互斥（seek+read 非定位读）
    let file = Arc::new(TokioMutex::new(
        tokio::fs::File::open(path)
            .await
            .map_err(|e| net(format!("open source {}: {e}", path.display())))?,
    ));
    let sent = Arc::new(AtomicU64::new(base));
    // 注意:先把块参数解引用成 owned 值再建流。若让流适配器持有
    // 参数为引用(HRTB)的闭包,该闭包进入生成器状态,rustc 的泄漏检查
    // 无法在 spawn 处证明 Send("FnOnce/Send is not general enough")。
    let chunk_list: Vec<(u64, u64)> = chunks.iter().map(|c| (c.offset, c.length)).collect();
    futures_util::stream::iter(chunk_list)
        .map(|(off, len)| {
            let file = file.clone();
            async move {
                let mut data = vec![0u8; len as usize];
                {
                    let mut f = file.lock().await;
                    f.seek(SeekFrom::Start(off))
                        .await
                        .map_err(|e| net(format!("seek source: {e}")))?;
                    f.read_exact(&mut data)
                        .await
                        .map_err(|e| net(format!("read source chunk: {e}")))?;
                }
                let mut uni = conn
                    .open_uni()
                    .await
                    .map_err(|e| net(format!("open chunk stream: {e}")))?;
                uni.write_all(&file_idx.to_be_bytes())
                    .await
                    .map_err(|e| net(format!("write chunk header: {e}")))?;
                uni.write_all(&off.to_be_bytes())
                    .await
                    .map_err(|e| net(format!("write chunk header: {e}")))?;
                uni.write_all(&len.to_be_bytes())
                    .await
                    .map_err(|e| net(format!("write chunk header: {e}")))?;
                uni.write_all(&data)
                    .await
                    .map_err(|e| net(format!("write chunk data: {e}")))?;
                uni.finish()
                    .map_err(|e| net(format!("finish chunk stream: {e}")))?;
                Ok::<u64, SendError>(len)
            }
        })
        .buffer_unordered(inflight)
        .try_for_each(|len| {
            let now = sent.fetch_add(len, Ordering::Relaxed) + len;
            progress.emit(SendProgress::Bytes {
                rel,
                done: now,
                total,
            });
            std::future::ready(Ok(()))
        })
        .await?;
    Ok(sent.load(Ordering::Relaxed) - base)
}

/// 控制流读取任务：解析 R→S 帧推入 mpsc。流结束未见 Final → `Closed`；
/// 帧畸形 → `Failed(Protocol)`；收到 Final 后自退。
async fn read_control(mut rx: RecvStream, tx: mpsc::Sender<ControlMsg>) {
    loop {
        let mut tag = [0u8; 1];
        let closed = match rx.read(&mut tag).await {
            Ok(Some(_)) => false,
            Ok(None) | Err(_) => true,
        };
        if closed {
            let _ = tx.send(ControlMsg::Closed).await;
            return;
        }
        let msg = match read_frame(&mut rx, tag[0]).await {
            Ok(m) => m,
            Err(FrameErr::Closed) => {
                let _ = tx.send(ControlMsg::Closed).await;
                return;
            }
            Err(FrameErr::Bad(e)) => {
                let _ = tx.send(ControlMsg::Failed(e)).await;
                return;
            }
        };
        let is_final = matches!(msg, ControlMsg::Final);
        if tx.send(msg).await.is_err() {
            return;
        }
        if is_final {
            return;
        }
    }
}

enum FrameErr {
    Closed,
    Bad(SendError),
}

fn bad(msg: impl Into<String>) -> FrameErr {
    FrameErr::Bad(proto(msg))
}

/// 解析一帧（tag 已读出）。
async fn read_frame(rx: &mut RecvStream, tag: u8) -> Result<ControlMsg, FrameErr> {
    match tag {
        TAG_REJECT => {
            let reason = read_len_prefixed(rx, MAX_REASON_BYTES, "reject reason").await?;
            Ok(ControlMsg::Reject(
                String::from_utf8_lossy(&reason).into_owned(),
            ))
        }
        TAG_PLAN => {
            let count = read_u32(rx).await?;
            if count as usize > MAX_PLAN_FILES {
                return Err(bad(format!("plan file count {count} too large")));
            }
            let mut plans = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let mut k = [0u8; 1];
                r_ex(rx, &mut k).await?;
                match k[0] {
                    PLAN_KIND_FULL => plans.push(FilePlan::Full),
                    PLAN_KIND_SKIP => plans.push(FilePlan::Skip),
                    PLAN_KIND_RESUME => {
                        let bmp =
                            read_len_prefixed(rx, MAX_PLAN_BITMAP_BYTES, "resume bitmap").await?;
                        plans.push(FilePlan::Resume(bmp));
                    }
                    t => return Err(bad(format!("bad plan kind 0x{t:02x}"))),
                }
            }
            Ok(ControlMsg::Plan(plans))
        }
        TAG_FILEDONE => {
            let idx = read_u32(rx).await?;
            Ok(ControlMsg::FileDone(idx))
        }
        TAG_FINAL => Ok(ControlMsg::Final),
        t => Err(bad(format!("bad control tag 0x{t:02x}"))),
    }
}

async fn read_u32(rx: &mut RecvStream) -> Result<u32, FrameErr> {
    let mut b = [0u8; 4];
    r_ex(rx, &mut b).await?;
    Ok(u32::from_be_bytes(b))
}

/// 读 `[u32 BE len][bytes]`，长度超上限按协议违规处理（分配前检查）。
async fn read_len_prefixed(
    rx: &mut RecvStream,
    cap: usize,
    what: &str,
) -> Result<Vec<u8>, FrameErr> {
    let len = read_u32(rx).await? as usize;
    if len > cap {
        return Err(bad(format!("oversized {what} ({len} bytes)")));
    }
    let mut buf = vec![0u8; len];
    r_ex(rx, &mut buf).await?;
    Ok(buf)
}

async fn r_ex(rx: &mut RecvStream, buf: &mut [u8]) -> Result<(), FrameErr> {
    rx.read_exact(buf).await.map_err(|_| FrameErr::Closed)
}

// ---------- 接收端 ----------

/// recv_files_fault 的结果：错误时也带回**已 finalize 落盘**的路径——
/// 真实断连（非 fault 钩子）下，本轮已完成的文件是磁盘事实，调用方
/// （Task 6 重连编排）据此跨轮累计，而不是随 Err 丢弃。
#[doc(hidden)]
pub struct RecvOutcome {
    /// 本次新完成落盘的路径（完成顺序；失败时为已落盘前缀）。
    pub paths: Vec<PathBuf>,
    /// None = 干净完成；Some = 传输错误（paths 仍然有效）。
    pub error: Option<anyhow::Error>,
}

/// 接收文件（v2）：对账发计划 → 按计划收块/续传 → checkpoint 补缺 →
/// 终局。返回**本次新完成落盘**的路径（SKIP 的不在其中）。
/// 失败时 Err；`.mtdss/` 记录保留断点，下次对同一清单续传。
pub async fn recv_files(
    conn: &Connection,
    out_dir: &Path,
    _me: &DeviceIdentity,
) -> Result<Vec<PathBuf>> {
    let out = recv_files_fault(conn, out_dir, _me, None).await;
    match out.error {
        None => Ok(out.paths),
        Some(e) => Err(e),
    }
}

/// 测试钩子：`close_after_files = Some(n)` 时，第 n 个**新 finalize** 完成
/// 后 `conn.close(0u32, b"fault")` 并立即返回已完成路径——供 Task 6 的
/// 重连 e2e 注入中途断连（SKIP 文件无 finalize，不计入）。任何失败路径
/// 都经 [`RecvOutcome`] 同时带出已落盘路径（见该类型文档）。
#[doc(hidden)]
pub async fn recv_files_fault(
    conn: &Connection,
    out_dir: &Path,
    _me: &DeviceIdentity,
    close_after_files: Option<usize>,
) -> RecvOutcome {
    // 已落盘路径经共享句柄穿出：body 的任何 Err 出口都不丢"已 finalize"
    // 的磁盘事实（body 内部上下文持有同一 Arc，finalize 处实时写入）。
    let paths = Arc::new(TokioMutex::new(Vec::new()));
    let error = recv_files_body(conn, out_dir, close_after_files, paths.clone())
        .await
        .err();
    let landed = paths.lock().await.clone();
    RecvOutcome {
        paths: landed,
        error,
    }
}

async fn recv_files_body(
    conn: &Connection,
    out_dir: &Path,
    close_after_files: Option<usize>,
    paths: Arc<TokioMutex<Vec<PathBuf>>>,
) -> Result<()> {
    let (mut ctl_tx_raw, mut ctl_rx) = conn.accept_bi().await?;

    // 1) 读 manifest：长度上限先检（分配前），再验签 + hex32 校验
    let mut len = [0u8; 4];
    read_exact(&mut ctl_rx, &mut len).await?;
    let man_len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(
        man_len <= MAX_MANIFEST_BYTES,
        "manifest too large ({man_len} bytes, cap {MAX_MANIFEST_BYTES})"
    );
    let mut man_bytes = vec![0u8; man_len];
    read_exact(&mut ctl_rx, &mut man_bytes).await?;
    let man = Manifest::decode(man_bytes.as_slice())?;

    let peer_pk = peer_verifying_key(conn)?;
    let files = manifest::verify(&man, &peer_pk)?;
    // transfer_id 强制 hex(32 字节) 格式：防止（已认证但恶意的）对端用
    // "../.." 之类的 id 参与任何后续路径构造（v1 语义保留）。
    anyhow::ensure!(
        man.transfer_id.len() == 64
            && man.transfer_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "transfer_id is not hex(32 bytes)"
    );

    // 2) 磁盘预检（同 v1：全清单尺寸合计，超可用即 0x00 拒绝）。
    //    拒绝后半关 + 等发送方 EOF（其 Rejected 路径会 finish）再返回：
    //    直接 drop 连接会让隐式 CONNECTION_CLOSE 作废在途的拒绝帧,
    //    发送方将误判为 Network 断连（v1 EOF 舞步的拒绝版）。
    let total: u64 = files.iter().map(|f| f.size).sum();
    if reject_for_space(total, free_bytes_at(out_dir)) {
        reply_reject(&mut ctl_tx_raw, "insufficient disk space").await?;
        let _ = ctl_tx_raw.finish();
        let mut sink = [0u8; 1];
        let _ = ctl_rx.read(&mut sink).await;
        return Err(anyhow!("insufficient disk space"));
    }

    // 3) 对账 → 计划帧（SKIP 立即回 FileDone，其余开 PartWriter）
    let plans = intake::reconcile(out_dir, &man);
    let ctl = Arc::new(TokioMutex::new(ctl_tx_raw));
    write_plan(&ctl, &plans).await?;

    let ctx = Arc::new(RecvCtx {
        ctl,
        conn: conn.clone(),
        fault_after: close_after_files,
        finalized: AtomicUsize::new(0),
        fault_hit: AtomicBool::new(false),
        fault_notify: Notify::new(),
        paths,
    });

    // 签名块表：file_idx → offset → (块序号, 块长, 块 sha256)
    let table: Arc<ChunkTable> = Arc::new(
        files
            .iter()
            .map(|f| {
                f.chunks
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (c.offset, ChunkSlot { idx: i, len: c.length, sha: c.sha256.clone() }))
                    .collect()
            })
            .collect(),
    );

    // 4) 主循环之前先把任务集与结果外提：任何退出（含 `?` 早退）都统一
    // 经过函数尾部的收尾排干——错误/切断退出时，在途块任务可能正卡在
    // finalize 的本地收尾段（改名之后的记录落盘 + paths 记账），直接随
    // AbortSet abort 会把"已改名的文件"丢出 paths（磁盘有、报告无）。
    let mut join_set = AbortSet(JoinSet::new());
    let result: Result<()> = async {
        let mut states: Vec<Arc<TokioMutex<FileState>>> = Vec::with_capacity(files.len());
        for (i, entry) in files.iter().enumerate() {
            let st = match &plans[i] {
                FilePlan::Skip => {
                    send_filedone(&ctx.ctl, i).await?;
                    Arc::new(TokioMutex::new(FileState {
                        missing: HashSet::new(),
                        writer: None,
                        done: true,
                    }))
                }
                FilePlan::Full => {
                    let writer = PartWriter::open(out_dir, entry, false).await?;
                    Arc::new(TokioMutex::new(FileState {
                        missing: (0..entry.chunks.len()).collect(),
                        writer: Some(writer),
                        done: false,
                    }))
                }
                FilePlan::Resume(bmp) => {
                    let writer = PartWriter::open(out_dir, entry, true).await?;
                    Arc::new(TokioMutex::new(FileState {
                        missing: missing_from_bmp(bmp, entry.chunks.len()),
                        writer: Some(writer),
                        done: false,
                    }))
                }
            };
            states.push(st);
        }
        // 0 块文件（或记录已齐）开箱即完整：立即 finalize + FileDone
        for (i, st) in states.iter().enumerate() {
            finalize_if_complete(st, i, &ctx).await?;
            if ctx.fault_hit.load(Ordering::SeqCst) {
                return Ok(());
            }
        }

        // 4) 主循环：收块流（spawn 处理）/ 块任务收尾 / 控制流 checkpoint
        let states = Arc::new(states);
        let mut tag = [0u8; 1];
        loop {
            tokio::select! {
                biased;
                _ = ctx.fault_notify.notified() => {
                    return Ok(());
                }
                Some(res) = join_set.0.join_next() => {
                    attribute_chunk_result(res, &ctx)?;
                    if ctx.fault_hit.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                }
                uni = conn.accept_uni() => {
                    match uni {
                        Ok(u) => {
                            join_set.0.spawn(handle_chunk(
                                u,
                                states.clone(),
                                table.clone(),
                                ctx.clone(),
                            ));
                        }
                        Err(e) => {
                            if ctx.fault_hit.load(Ordering::SeqCst) {
                                return Ok(());
                            }
                            return Err(anyhow!("uni stream accept failed: {e}"));
                        }
                    }
                }
                n = ctl_rx.read(&mut tag) => {
                    match n {
                        Ok(Some(_)) if tag[0] == TAG_CHECKPOINT => {
                            // checkpoint：等所有已 spawn 的块任务收尾（其内完成
                            // finalize + FileDone），再复核缺块
                            while let Some(res) = join_set.0.join_next().await {
                                attribute_chunk_result(res, &ctx)?;
                            }
                            if ctx.fault_hit.load(Ordering::SeqCst) {
                                return Ok(());
                            }
                            let mut new_plans = Vec::with_capacity(files.len());
                            let mut missing_any = false;
                            for (i, st) in states.iter().enumerate() {
                                let st = st.lock().await;
                                if st.done {
                                    new_plans.push(FilePlan::Skip);
                                } else if st.missing.is_empty() {
                                    // 防御：缺块清零但未 finalize（理论上任务已
                                    // 处理；就地补齐）
                                    drop(st);
                                    finalize_if_complete(&states[i], i, &ctx).await?;
                                    new_plans.push(FilePlan::Skip);
                                } else {
                                    missing_any = true;
                                    let n_chunks = files[i].chunks.len();
                                    new_plans.push(FilePlan::Resume(bitmap_from(
                                        &st.missing,
                                        n_chunks,
                                    )));
                                }
                            }
                            if missing_any {
                                write_plan(&ctx.ctl, &new_plans).await?;
                            } else {
                                // 全部就绪：终局帧 + EOF 舞步（同 v1：等发送方
                                // 半关的 EOF/连接结束，防隐式 CLOSE 作废终局帧）
                                send_frame(&ctx.ctl, &[TAG_FINAL]).await?;
                                let _ = ctl_rx.read(&mut tag).await;
                                return Ok(());
                            }
                        }
                        Ok(Some(_)) => {
                            return Err(anyhow!("bad control tag 0x{:02x}", tag[0]));
                        }
                        Ok(None) => {
                            if ctx.fault_hit.load(Ordering::SeqCst) {
                                return Ok(());
                            }
                            return Err(anyhow!(
                                "control stream closed before completion marker"
                            ));
                        }
                        Err(e) => {
                            if ctx.fault_hit.load(Ordering::SeqCst) {
                                return Ok(());
                            }
                            return Err(anyhow!("control stream read failed: {e}"));
                        }
                    }
                }
            }
        }
    }
    .await;
    // 收尾排干：干净路径在终局 checkpoint 已排干（此处 no-op）；错误路径先
    // 关连接让在途任务的死连接读写立刻失败（否则排干可能挂在等待对端数据
    // 上），再等它们跑完 finalize 的本地收尾段，保证已改名文件必进 paths。
    // 切断(fault)路径连接已关，同样快速收敛。排干中的错误按 fault 归因后
    // 忽略——真正的错误已由 result 携带。
    if result.is_err() {
        ctx.conn.close(quinn::VarInt::from_u32(0), b"recv aborted");
    }
    while let Some(res) = join_set.0.join_next().await {
        let _ = attribute_chunk_result(res, &ctx);
    }
    result
}

/// 接收侧共享上下文。
struct RecvCtx {
    /// 控制流发送侧（FileDone / 计划 / 终局帧的多任务写互斥）。
    ctl: Arc<TokioMutex<SendStream>>,
    conn: Connection,
    /// 测试钩子阈值（None 关闭）。
    fault_after: Option<usize>,
    /// 新 finalize 计数。
    finalized: AtomicUsize,
    /// 阈值命中标记。
    fault_hit: AtomicBool,
    /// 命中唤醒（主循环 promptly 返回）。
    fault_notify: Notify,
    /// 本次新完成落盘的路径（完成顺序）；与 recv_files_fault 的穿出句柄
    /// 共享同一 Arc——任何 Err 出口都不丢已 finalize 的磁盘事实。
    paths: Arc<TokioMutex<Vec<PathBuf>>>,
}

/// 单个块任务结果的归因（Task 6 重连编排依赖）：任务自身失败时，若 fault
/// 切断钩子已触发，则该失败由本侧主动断连引起——切断会击落同波在途的其余
/// 块任务与控制写，`res??` 会把这种"自己造成的失败"当真实错误上抛，已
/// finalize 落盘的路径随 Err 丢失，违反钩子"切断并立即返回已完成路径"的
/// 契约。因此 fault_hit 置位时的任务失败折算为成功，由调用方随后的 fault
/// 检查统一返回已完成路径；其余错误原样上抛。
fn attribute_chunk_result(
    res: Result<Result<()>, tokio::task::JoinError>,
    ctx: &RecvCtx,
) -> Result<()> {
    match res {
        Ok(Ok(())) => Ok(()),
        _ if ctx.fault_hit.load(Ordering::SeqCst) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(anyhow!("chunk task failed: {e}")),
    }
}

/// 单文件的接收状态。
struct FileState {
    /// 待收块索引集合（重复块幂等丢弃的判据）。
    missing: HashSet<usize>,
    /// `None` 且 !done = finalize 进行中已被取走（仅瞬态，持锁期间）。
    writer: Option<PartWriter>,
    /// 已完整落盘（finalize 成功或 SKIP）。
    done: bool,
}

/// 签名块表的槽位。
#[derive(Clone)]
struct ChunkSlot {
    idx: usize,
    len: u64,
    sha: Vec<u8>,
}

type ChunkTable = Vec<HashMap<u64, ChunkSlot>>;

/// Drop 时中止全部残留任务：错误路径不再等它们，也绝不泄漏到调用方
/// 生命周期之外（成功路径的集合在终局 checkpoint 前已排干）。
struct AbortSet(JoinSet<Result<()>>);

impl Drop for AbortSet {
    fn drop(&mut self) {
        self.0.abort_all();
    }
}

/// 单条块流的处理任务：读帧 → 查签名块表 → 长度/哈希校验 → 定位写 →
/// 缺块清零时就地 finalize（持文件锁直至完成，天然每文件恰好一次）。
async fn handle_chunk(
    mut uni: RecvStream,
    states: Arc<Vec<Arc<TokioMutex<FileState>>>>,
    table: Arc<ChunkTable>,
    ctx: Arc<RecvCtx>,
) -> Result<()> {
    let mut hdr = [0u8; 4 + 8 + 8];
    uni.read_exact(&mut hdr)
        .await
        .map_err(|e| anyhow!("chunk stream closed: {e:?}"))?;
    let file_idx = u32::from_be_bytes(hdr[0..4].try_into().unwrap());
    let offset = u64::from_be_bytes(hdr[4..12].try_into().unwrap());
    let len = u64::from_be_bytes(hdr[12..20].try_into().unwrap());
    // 长度先于分配检查：oversized 帧不得诱导巨量内存
    anyhow::ensure!(len <= CHUNK_SIZE as u64, "oversized chunk frame ({len} bytes)");
    let slot = table
        .get(file_idx as usize)
        .and_then(|m| m.get(&offset))
        .with_context(|| format!("chunk (file {}, offset {}) not in manifest", file_idx, offset))?;
    anyhow::ensure!(
        len == slot.len,
        "chunk length mismatch at offset {offset} (file {file_idx})"
    );
    let mut data = vec![0u8; len as usize];
    uni.read_exact(&mut data)
        .await
        .map_err(|e| anyhow!("chunk stream closed: {e:?}"))?;
    let mut h = Sha256::new();
    h.update(&data);
    anyhow::ensure!(
        h.finalize().as_slice() == slot.sha.as_slice(),
        "chunk hash mismatch at offset {offset} (file {file_idx})"
    );
    // 先验后写：坏块永远不落盘
    let st = &states[file_idx as usize];
    {
        let mut st = st.lock().await;
        if st.done || !st.missing.remove(&slot.idx) {
            return Ok(()); // 重复块：直接丢弃（幂等）
        }
        let w = st
            .writer
            .as_mut()
            .context("chunk arrived for file without writer")?;
        w.write_chunk(slot.idx, offset, &data).await?;
    }
    finalize_if_complete(st, file_idx as usize, &ctx).await
}

/// 缺块清零且未完成 → 就地 finalize：整文件校验 + 原子改名（intake）→
/// 记路径 → 回 FileDone（每文件恰好一次，done 标记防重）→ fault 钩子。
/// 持文件锁直至 finalize 完成：并发重复块在此排队后见到 done 即丢弃。
async fn finalize_if_complete(
    st: &Arc<TokioMutex<FileState>>,
    idx: usize,
    ctx: &RecvCtx,
) -> Result<()> {
    let mut st = st.lock().await;
    if st.done || !st.missing.is_empty() {
        return Ok(());
    }
    let w = st.writer.take().context("file complete without writer")?;
    let final_path = w.finalize().await?;
    st.done = true;
    drop(st);
    ctx.paths.lock().await.push(final_path);
    send_filedone(&ctx.ctl, idx).await?;
    if let Some(th) = ctx.fault_after {
        if ctx.finalized.fetch_add(1, Ordering::SeqCst) + 1 == th {
            ctx.fault_hit.store(true, Ordering::SeqCst);
            ctx.fault_notify.notify_one();
            ctx.conn.close(quinn::VarInt::from_u32(0), b"fault");
        }
    }
    Ok(())
}

/// 计划位图（位=1 缺块）→ 缺块索引集合。
fn missing_from_bmp(bmp: &[u8], chunk_count: usize) -> HashSet<usize> {
    (0..chunk_count)
        .filter(|&i| bmp[i / 8] & (1 << (i % 8)) != 0)
        .collect()
}

/// 缺块索引集合 → 计划位图（位=1 缺块）。
fn bitmap_from(missing: &HashSet<usize>, chunk_count: usize) -> Vec<u8> {
    let mut bmp = vec![0u8; chunk_count.div_ceil(8)];
    for &i in missing {
        bmp[i / 8] |= 1 << (i % 8);
    }
    bmp
}

/// 发计划帧：`[0x01][u32 BE file_count]{[u8 kind][u32 BE bmp_len][bmp]}`。
async fn write_plan(ctl: &Arc<TokioMutex<SendStream>>, plans: &[FilePlan]) -> Result<()> {
    let mut c = ctl.lock().await;
    c.write_all(&[TAG_PLAN]).await?;
    c.write_all(&(plans.len() as u32).to_be_bytes()).await?;
    for p in plans {
        match p {
            FilePlan::Full => c.write_all(&[PLAN_KIND_FULL]).await?,
            FilePlan::Skip => c.write_all(&[PLAN_KIND_SKIP]).await?,
            FilePlan::Resume(bmp) => {
                c.write_all(&[PLAN_KIND_RESUME]).await?;
                c.write_all(&(bmp.len() as u32).to_be_bytes()).await?;
                c.write_all(bmp).await?;
            }
        }
    }
    Ok(())
}

/// 发 FileDone 帧：`[0x02][u32 BE file_idx]`。
async fn send_filedone(ctl: &Arc<TokioMutex<SendStream>>, idx: usize) -> Result<()> {
    let mut c = ctl.lock().await;
    c.write_all(&[TAG_FILEDONE]).await?;
    c.write_all(&(idx as u32).to_be_bytes()).await?;
    Ok(())
}

/// 发单字节帧（终局 [0x03]）。
async fn send_frame(ctl: &Arc<TokioMutex<SendStream>>, tag: &[u8]) -> Result<()> {
    ctl.lock().await.write_all(tag).await?;
    Ok(())
}

/// 从 QUIC 对端（客户端）证书恢复 Ed25519 验证公钥：
/// 叶子证书 SPKI 必须是 44 字节且以 `ED25519_SPKI_PREFIX` 开头。
/// 畸形证书返回错误，绝不 panic。
fn peer_verifying_key(conn: &Connection) -> Result<ed25519_dalek::VerifyingKey> {
    let certs = conn
        .peer_identity()
        .and_then(|i| i.downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>().ok())
        .ok_or_else(|| anyhow!("peer presented no certificate"))?;
    let first = certs
        .first()
        .ok_or_else(|| anyhow!("peer presented an empty certificate chain"))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(first.as_ref())
        .map_err(|e| anyhow!("parse peer certificate: {e:?}"))?;
    let raw = parsed.public_key().raw;
    anyhow::ensure!(
        raw.len() == 44 && raw[..12] == ED25519_SPKI_PREFIX,
        "peer cert SPKI is {} bytes, not an Ed25519 device SPKI",
        raw.len()
    );
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&raw[12..44]);
    Ok(ed25519_dalek::VerifyingKey::from_bytes(&pk)?)
}

/// 控制流拒绝回复：`[0x00][u32 BE reason_len][reason]`。
async fn reply_reject(ctl: &mut SendStream, reason: &str) -> Result<()> {
    ctl.write_all(&[TAG_REJECT]).await?;
    ctl.write_all(&(reason.len() as u32).to_be_bytes()).await?;
    ctl.write_all(reason.as_bytes()).await?;
    Ok(())
}

async fn read_exact(s: &mut RecvStream, buf: &mut [u8]) -> Result<()> {
    s.read_exact(buf)
        .await
        .map_err(|e| anyhow!("control stream closed: {e:?}"))
}
