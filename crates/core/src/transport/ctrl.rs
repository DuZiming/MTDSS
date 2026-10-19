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
//! 帧长上限 [`FRAME_CAP`] 在读侧分配前检查：恶意对端可用 u32 长度前缀
//! 诱导巨量分配。文件系统枚举逻辑不在此处（调用方 handler 提供）；本
//! 模块只管帧化与请求-响应编排。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse};
use prost::Message;
use quinn::{Connection, RecvStream, SendStream};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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
/// Err（UNSUPPORTED_VERSION 语义）。
pub async fn send_request(
    conn: &Connection,
    req: &CtrlRequest,
    timeout: Duration,
) -> Result<CtrlResponse> {
    let resp = tokio::time::timeout(timeout, async {
        let (mut tx, mut rx) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow!("open ctrl stream: {e}"))?;
        write_frame(&mut tx, req).await?;
        // 请求已按长度成帧，FIN 只是礼貌收尾（对端读侧无需 EOF）
        let _ = tx.finish();
        read_frame::<CtrlResponse>(&mut rx).await
    })
    .await
    .map_err(|_| anyhow!("ctrl request timed out after {timeout:?}"))??;

    if resp.version != mtdss_proto::CTRL_VERSION {
        return Err(anyhow!(
            "UNSUPPORTED_VERSION: response version {} (expected {})",
            resp.version,
            mtdss_proto::CTRL_VERSION
        ));
    }
    Ok(resp)
}

/// 响应端：accept_bi 循环，每流 spawn 一个任务（读请求 → async handler →
/// 写响应）；连接关闭即返回。handler 为异步（M3c 起 fs 分发经
/// spawn_blocking，同步闭包无法在 handler 线程上等待其完成而不堵 worker）。
pub async fn serve_conn<F, Fut>(conn: Connection, handler: F)
where
    F: Fn(CtrlRequest) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CtrlResponse> + Send + 'static,
{
    // Arc 化以便按流 clone 进 spawn（handler 闭包只要求 Send + Sync）
    let handler = Arc::new(handler);
    loop {
        match conn.accept_bi().await {
            Ok((tx, rx)) => {
                let handler = handler.clone();
                tokio::spawn(handle_stream(tx, rx, handler));
            }
            // 连接关闭（对端退出/网络断），响应端随之结束
            Err(_) => return,
        }
    }
}

/// 单条请求流的处理任务：读请求 → handler → 写响应 → 半关。
/// 请求读不出（对端断/坏帧）静默丢弃该流，不影响连接上的其余请求。
async fn handle_stream<F, Fut>(mut tx: SendStream, mut rx: RecvStream, handler: Arc<F>)
where
    F: Fn(CtrlRequest) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CtrlResponse> + Send + 'static,
{
    let req: CtrlRequest = match read_frame(&mut rx).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("ctrl request read failed, dropping stream: {e}");
            return;
        }
    };
    let resp = handler(req).await;
    if let Err(e) = write_frame(&mut tx, &resp).await {
        tracing::debug!("ctrl response write failed: {e}");
        return;
    }
    let _ = tx.finish();
}

#[cfg(test)]
mod tests {
    use super::*;

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
