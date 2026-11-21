//! 控制通道帧协议（ctrl 请求-响应，M3b 起步、M3c 扩面）。
//!
//! 每个请求 = 一条独立双向流，一问一答，流即弃：
//! - 请求端 `send_request`：开流 → `[u32 LE len][CtrlRequest bytes]` →
//!   读 `[u32 LE len][CtrlResponse bytes]`（整体 timeout 有界）；响应
//!   version 与 [`mtdss_proto::CTRL_VERSION`] 不符即报
//!   UNSUPPORTED_VERSION（新端遇旧端的最小可见故障）。
//! - 响应端 `serve_conn`：`accept_bi` 循环，每条新流 spawn 一个任务
//!   （读请求 → handler（**async**，M3c：daemon 侧 fs 分发在 handler 内
//!   经 spawn_blocking 落阻塞线程池，同步 handler 无法在不堵 worker 的
//!   前提下等它）→ 写响应后半关）；连接关闭即返回。
//!
//! V3-5 Stage B 多路复用：入池 mux 直连上的请求走 [`send_request_tagged`]
//! （开流后先写 [`crate::transport::mux::TAG_CTRL`] 一字节再进同一舞步），
//! 响应端 daemon 侧 mux serve 用 [`read_request_frame`]/
//! [`write_response_frame`] 消费带标签流；裸路径（每作业新拨连接、中继
//! 连接、旧端互通）不写标签，帧格式逐字节不变。
//!
//! V4-1 流式响应（预览数据面基座）：handler 可回 [`CtrlReply::Stream`]，
//! 响应端在请求流上泵 `DataBegin{stream_id} → DataChunk{stream_id,bytes}*
//! → DataEnd{stream_id}`（每帧独立 CtrlResponse 编码，version 门共用）；
//! 发起端走 [`send_request_streaming`]（mux 直连同款 tagged 变体）拿
//! [`CtrlStreamOutcome`]，首帧 DataBegin 即进逐块消费。背压沿既有
//! transport 模式：发起端不读 → QUIC 流控窗满 → 响应端 `write_all` 阻塞
//! → 生产端通道满等待，逐级回压；另有块间静默上界（有界失败纪律）与
//! 每连接在途流登记簿。中途失败以 CtrlError 帧收尾；旧端安全——只有
//! 流式请求（新变体）的发起端才期待帧序，旧发起端永不接收。
//!
//! 帧长上限 [`FRAME_CAP`] 在读侧分配前检查：恶意对端可用 u32 长度前缀
//! 诱导巨量分配。文件系统枚举逻辑不在此处（调用方 handler 提供）；本
//! 模块只管帧化与请求-响应编排。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use mtdss_proto::ctrl::{ctrl_response, CtrlError, CtrlRequest, CtrlResponse, DataBegin, DataChunk, DataEnd};
use prost::Message;
use quinn::{Connection, RecvStream, SendStream};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::transport::mux;

/// 单帧上限（读侧超限报错，分配前检查）。
pub const FRAME_CAP: usize = 16 * 1024 * 1024;

/// 写一帧：`[u32 LE len][prost bytes]`。内部 pub 供单测，业务走
/// [`send_request`]/[`serve_conn`]。
pub(crate) async fn write_frame(w: &mut SendStream, msg: &impl Message) -> Result<()> {
    write_frame_io(w, msg).await
}

/// 读一帧：长度超 [`FRAME_CAP`] 拒绝。内部 pub 供单测。
pub(crate) async fn read_frame<T: Message + Default>(r: &mut RecvStream) -> Result<T> {
    read_frame_io(r).await
}

/// 读一条 ctrl 请求帧（V3-5 Stage B pub 薄包，daemon 侧 mux serve 的
/// `TAG_CTRL` 臂消费；帧语义与内部 [`read_frame`] 完全一致）。
pub async fn read_request_frame(rx: &mut RecvStream) -> Result<CtrlRequest> {
    read_frame(rx).await
}

/// 写一条 ctrl 响应帧（pub 薄包，mux serve 臂消费；语义同
/// [`write_frame`]——调用方负责随后的半关）。
pub async fn write_response_frame(tx: &mut SendStream, resp: &CtrlResponse) -> Result<()> {
    write_frame(tx, resp).await
}

// ---------- 帧编解码（IO 泛化核心；quinn 流与单测 duplex 流共用） ----------

async fn write_frame_io(
    w: &mut (impl tokio::io::AsyncWrite + Unpin),
    msg: &impl Message,
) -> Result<()> {
    let bytes = msg.encode_to_vec();
    let len = u32::try_from(bytes.len())
        .map_err(|_| anyhow!("frame of {} bytes exceeds u32 length prefix", bytes.len()))?;
    w.write_all(&len.to_le_bytes())
        .await
        .context("write frame length")?;
    w.write_all(&bytes).await.context("write frame body")?;
    Ok(())
}

async fn read_frame_io<T: Message + Default>(
    r: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .await
        .map_err(|e| anyhow!("read frame length: {e}"))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > FRAME_CAP {
        return Err(anyhow!("frame length {len} exceeds cap {FRAME_CAP}"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .await
        .map_err(|e| anyhow!("read frame body: {e}"))?;
    T::decode(buf.as_slice()).map_err(|e| anyhow!("decode frame: {e}"))
}

// ---------- 请求-响应编排 ----------

/// 打开双向流，写请求读响应（timeout 有界）；响应 version 不符 →
/// Err（UNSUPPORTED_VERSION 语义）。裸流入口（每作业新拨连接/中继
/// 连接）；mux 直连（暖连接）走 [`send_request_tagged`]。
pub async fn send_request(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
) -> Result<CtrlResponse> {
    request_within(conn, req, timeout, None).await.and_then(check_response_version)
}

/// [`send_request`] 的 mux 直连变体（V3-5 Stage B）：open_bi 后先写
/// [`mux::TAG_CTRL`] 一字节再进同一帧舞步——仅入池 mux 直连使用
/// （接收端 mux serve 按标签分派到 [`read_request_frame`] /
/// [`write_response_frame`]），裸路径互通零改动。
pub async fn send_request_tagged(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
) -> Result<CtrlResponse> {
    request_within(conn, req, timeout, Some(mux::TAG_CTRL))
        .await
        .and_then(check_response_version)
}

/// 请求-响应共用体：open_bi →（可选流类型标签字节）→ 写请求帧 → 半关
/// → 读响应帧，整体 timeout 有界。`tag = None`（裸入口）时线上字节与
/// 历史 [`send_request`] 逐字节一致（纯搬移，逻辑零改）。
async fn request_within(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
    tag: Option<u8>,
) -> Result<CtrlResponse> {
    tokio::time::timeout(timeout, async {
        let (tx, mut rx) = open_request_stream(conn, req, tag).await?;
        let resp = read_frame::<CtrlResponse>(&mut rx).await;
        // 写半随首帧读毕即弃(与历史 handle_stream/request_within 的作用域
        // 收尾完全一致——写半早弃会扰动 quinn 发送侧收尾时序,实测全量
        // ctrl_e2e 偶发原生段错误,见 V4-1 Task 2 报告)
        drop(tx);
        resp
    })
    .await
    .map_err(|_| anyhow!("ctrl request timed out after {timeout:?}"))?
}

/// 请求头共用段（单帧 [`request_within`] 与流式 [`request_stream_within`]
/// 各自的舞步前半）：open_bi →（可选标签）→ 写请求帧 → FIN。返回流两半
/// ——写半由调用方在各自首帧读毕后弃(收尾时序与历史形态一致,勿提前)。
async fn open_request_stream(
    conn: &Connection,
    req: &CtrlRequest,
    tag: Option<u8>,
) -> Result<(SendStream, RecvStream)> {
    let (mut tx, rx) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow!("open ctrl stream: {e}"))?;
    if let Some(t) = tag {
        tx.write_all(&[t])
            .await
            .map_err(|e| anyhow!("write stream tag: {e}"))?;
    }
    write_frame(&mut tx, req).await?;
    // 请求已按长度成帧，FIN 只是礼貌收尾（对端读侧无需 EOF）
    let _ = tx.finish();
    Ok((tx, rx))
}

/// 响应 version 闸门（两入口共用）：与 [`mtdss_proto::CTRL_VERSION`]
/// 不符即报 UNSUPPORTED_VERSION（新端遇旧端的最小可见故障）。
fn check_response_version(resp: CtrlResponse) -> Result<CtrlResponse> {
    if resp.version != mtdss_proto::CTRL_VERSION {
        return Err(anyhow!(
            "UNSUPPORTED_VERSION: response version {} (expected {})",
            resp.version,
            mtdss_proto::CTRL_VERSION
        ));
    }
    Ok(resp)
}

// ---------- 流式响应帧序（V4-1 Task 2，预览数据面基座） ----------

/// 单个 DataChunk 帧的数据帽（响应端泵拆帧；发起端逐帧读回）。既远小于
/// [`FRAME_CAP`]（prost 包裹后恒有余量），又让慢消费端的背压粒度不至于
/// 过粗（应用级窗口 = 通道容量 × 本帽）。
pub const STREAM_CHUNK_CAP: usize = 256 * 1024;

/// 单连接在途流式响应上限（登记簿限「响应端为每条流挂泵」的应用面；开
/// 流面由 QUIC MAX_STREAMS 另管）。
pub const STREAM_CONCURRENT_CAP: usize = 8;

/// 发起端逐块读的块间静默上界：流中任意相邻两帧超过该间隔未到即报错收
/// 流。源盘读段的 spawn_blocking 以秒计，60s 已宽；既有传输面同款「有界
/// 失败」纪律——无此界，响应端卡死时发起端将在对端 keep-alive 心跳的掩
/// 护下永久悬挂。
const STREAM_CHUNK_IDLE: Duration = Duration::from_secs(60);

/// 流式响应的中途失败形态（对端 CtrlError 帧的透传；code 即既有稳定码）。
#[derive(Debug, Clone)]
pub struct CtrlStreamError {
    pub code: String,
    pub message: String,
}

/// 响应端生产者投递的单项：数据块 | 流中失败（泵改写 CtrlError 帧收尾，
/// 不再发 DataEnd）。
pub type CtrlChunk = Result<Vec<u8>, CtrlStreamError>;

/// 流式响应的应答半边（handler 持生产端）：建 mpsc 通道即窗口——生产端
/// `send` 在通道满处等待，经泵的 `write_all` 与 QUIC 流控（既有 transport
/// 模式）逐级背压到源头。
pub struct CtrlStreamOut {
    pub chunks: tokio::sync::mpsc::Receiver<CtrlChunk>,
}

/// ctrl handler 的应答形态（V4-1 起）：单帧（既有语义——错误或一次性
/// 消息）或流式多帧（begin → chunk* → end）。
pub enum CtrlReply {
    /// 单帧响应（Frame 臂线上字节与历史 serve_conn 逐字节一致）。
    Frame(CtrlResponse),
    /// 流式响应：stream_id 由响应端登记簿分配，泵逐块拆帧写出。
    Stream(CtrlStreamOut),
}

/// 流式请求的首帧结果：DataBegin = 进流式面逐块消费；其余形态（典型
/// CtrlError——对端拒绝/旧端「缺 body」INTERNAL/未接线桩）= 单帧面，
/// 调用方按既有 body 匹配降级。
pub enum CtrlStreamOutcome {
    Stream(CtrlStreamResponse),
    Frame(CtrlResponse),
}

/// 发起端的流式消费手柄（持请求流的读半）：逐帧读回 DataChunk，DataEnd
/// 收尾；中途 CtrlError 帧以 Err 显形。消费慢时读侧停摆 → QUIC 流控回压
/// 响应端泵 → 生产端通道；丢弃本手柄 = 流关（STOP_SENDING），响应端写侧
/// 即败收流、登记 id 回收。
pub struct CtrlStreamResponse {
    stream_id: u64,
    rx: RecvStream,
    done: bool,
}

impl CtrlStreamResponse {
    /// 响应端登记簿分配的连接内唯一流 id（DataBegin 帧）。
    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    /// 下一块数据：`Some(Ok)` = 数据帧；`Some(Err)` = 流中失败（CtrlError
    /// 帧透传/传输面破损/块间静默超界）；`None` = DataEnd 正常收尾。任何
    /// 终止形态后恒 `None`。
    pub async fn next_chunk(&mut self) -> Option<CtrlChunk> {
        if self.done {
            return None;
        }
        use ctrl_response::Body as Resp;
        let resp = match tokio::time::timeout(
            STREAM_CHUNK_IDLE,
            read_frame::<CtrlResponse>(&mut self.rx),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                self.done = true;
                return Some(Err(CtrlStreamError {
                    code: "INTERNAL".into(),
                    message: format!("流式响应读取失败: {e:#}"),
                }));
            }
            Err(_) => {
                self.done = true;
                return Some(Err(CtrlStreamError {
                    code: "INTERNAL".into(),
                    message: format!("流式响应块间静默超 {STREAM_CHUNK_IDLE:?}"),
                }));
            }
        };
        if resp.version != mtdss_proto::CTRL_VERSION {
            self.done = true;
            return Some(Err(CtrlStreamError {
                code: "UNSUPPORTED_VERSION".into(),
                message: format!(
                    "流式响应 version {} (expected {})",
                    resp.version,
                    mtdss_proto::CTRL_VERSION
                ),
            }));
        }
        let answer = match resp.body {
            Some(Resp::DataChunk(c)) if c.stream_id == self.stream_id => Ok(c.data),
            Some(Resp::DataEnd(e)) if e.stream_id == self.stream_id => {
                self.done = true;
                return None;
            }
            Some(Resp::Error(e)) => {
                self.done = true;
                return Some(Err(CtrlStreamError {
                    code: e.code,
                    message: e.message,
                }));
            }
            // stream_id 不符 / 单帧体混入流中 / 缺 body：协议面破约
            _ => Err(CtrlStreamError {
                code: "INTERNAL".into(),
                message: "流式响应帧形态不符".into(),
            }),
        };
        if answer.is_err() {
            self.done = true;
        }
        Some(answer)
    }
}

/// 流式请求（裸流入口，每作业新拨连接/中继连接）；mux 直连（暖连接）走
/// [`send_request_streaming_tagged`]。首帧（begin 或单帧体）在 `timeout`
/// 内有界；其后逐块读的界在 [`CtrlStreamResponse::next_chunk`]。
pub async fn send_request_streaming(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
) -> Result<CtrlStreamOutcome> {
    request_stream_within(conn, req, timeout, None).await
}

/// [`send_request_streaming`] 的 mux 直连变体（仅入池 mux 直连使用，见
/// [`send_request_tagged`]）。
pub async fn send_request_streaming_tagged(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
) -> Result<CtrlStreamOutcome> {
    request_stream_within(conn, req, timeout, Some(mux::TAG_CTRL)).await
}

/// 流式请求共用体：open_bi →（可选标签）→ 写请求帧 → FIN → 读首帧（
/// 整体 timeout 有界，与 [`request_within`] 同口径）。首帧 DataBegin →
/// 手柄接管读半进逐块面；其余形态原帧带回（单帧面，version 门共用）。
/// 写半与单帧面同律:随首帧读毕即弃。
async fn request_stream_within(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
    tag: Option<u8>,
) -> Result<CtrlStreamOutcome> {
    tokio::time::timeout(timeout, async {
        let (tx, mut rx) = open_request_stream(conn, req, tag).await?;
        let first = read_frame::<CtrlResponse>(&mut rx).await;
        drop(tx);
        let first = first?;
        let first = check_response_version(first)?;
        Ok(match first.body {
            Some(ctrl_response::Body::DataBegin(b)) => {
                CtrlStreamOutcome::Stream(CtrlStreamResponse {
                    stream_id: b.stream_id,
                    rx,
                    done: false,
                })
            }
            _ => CtrlStreamOutcome::Frame(first),
        })
    })
    .await
    .map_err(|_| anyhow!("ctrl streaming request timed out after {timeout:?}"))?
}

/// 每连接的流式响应登记簿（serve_conn 与 mux serve 各持一份）：连接内
/// 单调分配 stream_id + 限在途流数（[`STREAM_CONCURRENT_CAP`]）；守卫随
/// 泵收尾 drop，id 即回收。
#[derive(Default)]
pub struct StreamRegistry {
    inner: Mutex<StreamRegInner>,
}

#[derive(Default)]
struct StreamRegInner {
    next: u64,
    live: std::collections::HashSet<u64>,
}

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配 id 并登记（占满回 Err 文案，调用方以一帧 CtrlError 回执）；
    /// 返回的守卫由泵持至收尾，drop 即回收。
    fn allocate(self: &Arc<Self>) -> Result<(u64, StreamRegGuard), String> {
        let mut g = self.inner.lock().unwrap();
        if g.live.len() >= STREAM_CONCURRENT_CAP {
            return Err(format!("流式响应并发超限({} 路在途)", g.live.len()));
        }
        // u64 单调耗尽不现实，防御性显错而非回卷撞活 id
        g.next = g
            .next
            .checked_add(1)
            .ok_or_else(|| "stream_id 分配耗尽".to_string())?;
        let id = g.next;
        g.live.insert(id);
        Ok((id, StreamRegGuard { reg: Arc::clone(self), id }))
    }
}

/// 在途登记守卫：泵任务持至收尾，drop 即从登记簿摘除该 id。
struct StreamRegGuard {
    reg: Arc<StreamRegistry>,
    id: u64,
}

impl Drop for StreamRegGuard {
    fn drop(&mut self) {
        self.reg.inner.lock().unwrap().live.remove(&self.id);
    }
}

/// CtrlError 单帧（ok=false；流式并发占满拒与流中失败两处共用）。
fn err_frame(code: &str, message: String) -> CtrlResponse {
    CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: false,
        body: Some(ctrl_response::Body::Error(CtrlError {
            code: code.to_string(),
            message,
        })),
    }
}

/// 响应端：accept_bi 循环，每流 spawn 一个任务（读请求 → async handler →
/// 单帧应答或流式泵）；连接关闭即返回。handler 为异步（M3c 起 fs 分发经
/// spawn_blocking，同步闭包无法在 handler 线程上等待其完成而不堵 worker），
/// V4-1 起应答 [`CtrlReply`]（Frame 臂 = 历史单帧，Stream 臂 = 流式多帧，
/// 见 [`serve_stream`]）。
pub async fn serve_conn<F, Fut>(conn: Connection, handler: F)
where
    F: Fn(CtrlRequest) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CtrlReply> + Send + 'static,
{
    // Arc 化以便按流 clone 进 spawn（handler 闭包只要求 Send + Sync）；
    // 登记簿每连接一份（并发流 id 唯一 + 限在途）。
    let handler = Arc::new(handler);
    let registry = Arc::new(StreamRegistry::new());
    loop {
        match conn.accept_bi().await {
            Ok((tx, rx)) => {
                let handler = handler.clone();
                let registry = registry.clone();
                tokio::spawn(serve_stream(tx, rx, registry, move |req| handler(req)));
            }
            // 连接关闭（对端退出/网络断），响应端随之结束
            Err(_) => return,
        }
    }
}

/// 单条请求流的处理任务（裸 [`serve_conn`] 与 daemon mux TAG_CTRL 臂共用
/// 的收口）：读请求 → handler → [`CtrlReply::Frame`] 单帧回执（线上字节
/// 与历史 handle_stream 逐字节一致）| [`CtrlReply::Stream`] 流式泵（
/// [`pump_stream`]）。请求读不出（对端断/坏帧）静默丢弃该流，不影响
/// 连接上的其余请求。
pub async fn serve_stream<F, Fut>(
    mut tx: SendStream,
    mut rx: RecvStream,
    registry: Arc<StreamRegistry>,
    handler: F,
) where
    F: Fn(CtrlRequest) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CtrlReply> + Send + 'static,
{
    let req: CtrlRequest = match read_frame(&mut rx).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("ctrl request read failed, dropping stream: {e}");
            return;
        }
    };
    match handler(req).await {
        CtrlReply::Frame(resp) => {
            if let Err(e) = write_frame(&mut tx, &resp).await {
                tracing::debug!("ctrl response write failed: {e}");
                return;
            }
            let _ = tx.finish();
        }
        CtrlReply::Stream(out) => {
            // 请求半边读完即弃（发起端 FIN 已至，无剩余读面）
            drop(rx);
            pump_stream(tx, registry, out).await;
        }
    }
}

/// 流式响应泵：登记 id → DataBegin → 逐块拆帧（[`STREAM_CHUNK_CAP`]）写
/// 出 → 通道关闭（生产端完结）= DataEnd | 生产者中途失败 = CtrlError 帧
/// 收尾（无 DataEnd）。写侧任一帧失败即收流（对端断/STOP_SENDING，典型
/// = 消费端丢弃手柄）；登记守卫随函数收尾 drop 回收 id。背压：生产端
/// send 在通道满处等 → 本泵 write_all 在 QUIC 流控窗满处等——逐级回压
/// 到源头（既有 transport 模式）。
async fn pump_stream(
    mut tx: SendStream,
    registry: Arc<StreamRegistry>,
    mut out: CtrlStreamOut,
) {
    let (stream_id, _guard) = match registry.allocate() {
        Ok(v) => v,
        Err(msg) => {
            // 占满：一帧 CtrlError 回执（发起端按单帧面降级），数据半边
            // 弃置（生产端 send 即败自收）。
            tracing::warn!("ctrl 流式响应被拒: {msg}");
            drop(out.chunks);
            let _ = write_frame(&mut tx, &err_frame("INTERNAL", msg)).await;
            let _ = tx.finish();
            return;
        }
    };
    let version = mtdss_proto::CTRL_VERSION;
    let begin = CtrlResponse {
        version,
        ok: true,
        body: Some(ctrl_response::Body::DataBegin(DataBegin { stream_id })),
    };
    if let Err(e) = write_frame(&mut tx, &begin).await {
        tracing::debug!("ctrl data_begin write failed: {e}");
        return;
    }
    while let Some(item) = out.chunks.recv().await {
        match item {
            Ok(bytes) => {
                // 超帽拆帧：生产者可投任意大块，线上帧恒 ≤ 帽
                for data in bytes.chunks(STREAM_CHUNK_CAP) {
                    let frame = CtrlResponse {
                        version,
                        ok: true,
                        body: Some(ctrl_response::Body::DataChunk(DataChunk {
                            stream_id,
                            data: data.to_vec(),
                        })),
                    };
                    if let Err(e) = write_frame(&mut tx, &frame).await {
                        tracing::debug!("ctrl data_chunk write failed: {e}");
                        return;
                    }
                }
            }
            Err(e) => {
                // 流中失败：CtrlError 帧收尾，不再有 DataEnd
                tracing::info!("ctrl 流式响应中途失败({}): {}", e.code, e.message);
                let _ = write_frame(&mut tx, &err_frame(&e.code, e.message)).await;
                let _ = tx.finish();
                return;
            }
        }
    }
    let end = CtrlResponse {
        version,
        ok: true,
        body: Some(ctrl_response::Body::DataEnd(DataEnd { stream_id })),
    };
    if let Err(e) = write_frame(&mut tx, &end).await {
        tracing::debug!("ctrl data_end write failed: {e}");
        return;
    }
    let _ = tx.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 流式登记簿:并发帽占用 → 超限拒 → 守卫 drop 回收(回收后可再分配);
    /// id 连接内单调自 1 起,回收不复发旧号。
    #[test]
    fn stream_registry_cap_and_recycle() {
        let reg = Arc::new(StreamRegistry::new());
        let mut guards = Vec::new();
        for i in 1..=STREAM_CONCURRENT_CAP {
            let (id, g) = reg.allocate().expect("帽内分配必须成功");
            assert_eq!(id, i as u64, "id 连接内单调自 1 起");
            guards.push(g);
        }
        assert!(reg.allocate().is_err(), "满帽分配必须拒");
        drop(guards.pop());
        let (id, g2) = reg.allocate().expect("守卫回收后必须可再分配");
        assert_eq!(id, (STREAM_CONCURRENT_CAP + 1) as u64, "id 单调,不复发旧号");
        drop(g2);
    }

    /// 最小可编解码消息：duplex 流上测帧编解码，不占 QUIC。
    #[derive(Clone, PartialEq, prost::Message)]
    struct Ping {
        #[prost(uint32, tag = "1")]
        value: u32,
    }

    #[tokio::test]
    async fn frame_roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let msg = Ping { value: 42 };
        write_frame_io(&mut a, &msg).await.unwrap();
        let got: Ping = read_frame_io(&mut b).await.unwrap();
        assert_eq!(got, msg);
    }

    #[tokio::test]
    async fn oversized_frame_rejected_before_alloc() {
        let (mut a, mut b) = tokio::io::duplex(64);
        // 手写超限长度前缀（不写载荷）：读侧必须在分配前拒绝
        a.write_all(&(FRAME_CAP as u32 + 1).to_le_bytes()).await.unwrap();
        let res: Result<Ping> = read_frame_io(&mut b).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn truncated_frame_errors_cleanly() {
        let (mut a, mut b) = tokio::io::duplex(64);
        // 声称 100 字节只给 2 字节后断开（drop 写侧）→ 读侧报错而非挂起
        a.write_all(&100u32.to_le_bytes()).await.unwrap();
        a.write_all(&[1u8, 2]).await.unwrap();
        drop(a);
        let res: Result<Ping> = read_frame_io(&mut b).await;
        assert!(res.is_err());
    }
}
