//! 分块校验传输引擎（QUIC 固定连接上的可靠文件传输）。
//!
//! 流协议（发送方视角）：
//! 1. 控制双向流（客户端发起）：`[u32 BE manifest_len][manifest bytes]`，
//!    然后等待判决 `[u8 verdict][u32 BE reason_len][reason]`
//!    （verdict 1=接受，0=拒绝并附原因）。
//! 2. 每个块一条独立单向流：`[u64 BE chunk_id][u64 BE length][data]`，
//!    其中 chunk_id 为清单中该块的起始字节偏移（清单已保证偏移唯一）。
//! 3. 全部块完成后控制流发送 `[0xFF]`；接收方逐块验哈希后定位写 `.part`，
//!    最后顺序读回整文件校验并原子改名，回复 `[0x01]`，发送方收到即完成。
//!
//! 接收方验证顺序（固定，不可调整）：manifest 验签（对端证书恢复的
//! Ed25519 公钥）→ 磁盘预检 → accept → 逐块哈希校验（先验后写：
//! `set_len` 预分配 + seek 定位写，内存 O(1 块)，坏块永不落盘）→
//! 整文件哈希（顺序读回 `.part` 第二遍计算）→ 原子改名。任何一步失败
//! 都不产生最终文件；中间 `.mtdss-part` 文件保留在落盘目录中，直至
//! 成功（被改名）或由上层取消时清理。
//!
//! 首条服务端双向流已被 `endpoint::connect_pinned`/`accept_pinned` 的
//! 接受令牌占用；本模块的控制流为客户端发起，互不冲突。

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use mtdss_proto::manifest::{ChunkDesc, Manifest};
use prost::Message as _;
use quinn::{Connection, RecvStream, SendStream};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::identity::{DeviceIdentity, ED25519_SPKI_PREFIX};
use crate::protocol::manifest;
// unique_target 已移入 intake（收件区 finalize 与 M1 落盘共用同一冲突改名逻辑）
use crate::transport::intake::unique_target;

pub use crate::protocol::manifest::CHUNK_SIZE;

/// 发送结果：已发送的应用层文件字节数（不含协议帧头）。
pub struct SendReport {
    pub bytes_sent: u64,
}

/// 路径所在卷的可用字节数。查询失败按"无限"处理——不因未知空间误拒。
pub fn free_bytes_at(path: &Path) -> u64 {
    fs2::available_space(path).unwrap_or(u64::MAX)
}

/// 磁盘预检：所需字节数超过可用即应拒绝接收。
pub fn reject_for_space(needed: u64, available: u64) -> bool {
    needed > available
}

/// 发送单个文件：构建并签名清单 → 控制流交换 → 逐块单向流传输 →
/// 等待接收方整文件校验确认。`progress(done_bytes, total_bytes)` 在
/// 每个块完成后回调一次（零字节文件没有块，不回调）。
pub async fn send_file(
    conn: &Connection,
    path: &Path,
    rel: &str,
    id: &DeviceIdentity,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<SendReport> {
    let man = manifest::build_manifest(&[(path.to_path_buf(), rel.to_string())], id)?;
    let total: u64 = man.files[0].size;

    // 1) 控制流：发送 manifest
    let (mut ctl_tx, mut ctl_rx) = conn.open_bi().await?;
    let man_bytes = man.encode_to_vec();
    ctl_tx
        .write_all(&(man_bytes.len() as u32).to_be_bytes())
        .await?;
    ctl_tx.write_all(&man_bytes).await?;

    // 2) 等待判决
    let mut verdict = [0u8; 1];
    read_exact(&mut ctl_rx, &mut verdict).await?;
    match verdict[0] {
        1 => {}
        0 => {
            let mut len = [0u8; 4];
            read_exact(&mut ctl_rx, &mut len).await?;
            let mut reason = vec![0u8; u32::from_be_bytes(len) as usize];
            read_exact(&mut ctl_rx, &mut reason).await?;
            return Err(anyhow!(
                "receiver rejected: {}",
                String::from_utf8_lossy(&reason)
            ));
        }
        v => return Err(anyhow!("bad verdict byte {v}")),
    }

    // 3) 每块一条独立单向流（chunk_id = 清单内该块的起始偏移）
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| path.display().to_string())?;
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut sent: u64 = 0;
    for chunk in &man.files[0].chunks {
        let mut remain = chunk.length as usize;
        let mut data: Vec<u8> = Vec::with_capacity(chunk.length as usize);
        while remain > 0 {
            let want = remain.min(buf.len());
            let n = file.read(&mut buf[..want]).await?;
            anyhow::ensure!(n > 0, "file truncated while sending");
            data.extend_from_slice(&buf[..n]);
            remain -= n;
        }
        let mut uni = conn.open_uni().await?;
        uni.write_all(&chunk.offset.to_be_bytes()).await?;
        uni.write_all(&chunk.length.to_be_bytes()).await?;
        uni.write_all(&data).await?;
        uni.finish()?;
        sent += chunk.length;
        progress(sent, total);
    }

    // 4) 完成标记 + 等待接收方最终确认
    ctl_tx.write_all(&[0xFF]).await?;
    let mut ack = [0u8; 1];
    read_exact(&mut ctl_rx, &mut ack).await?;
    anyhow::ensure!(ack[0] == 0x01, "final integrity check failed at receiver");
    // 半关闭控制流发送侧：让接收方（recv_files 末尾等待本侧 EOF/连接结束）
    // 干净地结束，而不是靠连接关闭竞态。
    ctl_tx.finish()?;
    Ok(SendReport { bytes_sent: sent })
}

/// 接收文件（M1 恒接受，除非磁盘空间不足）。返回成功落盘的路径列表。
///
/// 验证顺序见模块文档。失败时返回 Err；已创建的 `.mtdss-part`
/// 保留（M1 无断点续传，由上层在取消时清理）。
pub async fn recv_files(
    conn: &Connection,
    out_dir: &Path,
    _me: &DeviceIdentity,
) -> Result<Vec<PathBuf>> {
    let (mut ctl_tx, mut ctl_rx) = conn.accept_bi().await?;

    // 1) 读 manifest，并用对端（客户端）证书恢复的公钥验签
    let mut len = [0u8; 4];
    read_exact(&mut ctl_rx, &mut len).await?;
    let mut man_bytes = vec![0u8; u32::from_be_bytes(len) as usize];
    read_exact(&mut ctl_rx, &mut man_bytes).await?;
    let man = Manifest::decode(man_bytes.as_slice())?;

    let peer_pk = peer_verifying_key(conn)?;
    let files = manifest::verify(&man, &peer_pk)?;
    // transfer_id 直接拼入 .part 文件路径：强制 hex(32 字节) 格式，
    // 防止（已认证但恶意的）对端用 "../.." 之类的 id 把中间文件写出 out_dir。
    anyhow::ensure!(
        man.transfer_id.len() == 64
            && man.transfer_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "transfer_id is not hex(32 bytes)"
    );

    // 2) 磁盘预检 + accept（M1 恒接受，除非空间不足）
    let total: u64 = files.iter().map(|f| f.size).sum();
    if reject_for_space(total, free_bytes_at(out_dir)) {
        reply_reject(&mut ctl_tx, "insufficient disk space").await?;
        return Err(anyhow!("insufficient disk space"));
    }
    ctl_tx.write_all(&[1u8]).await?;

    // 3) 接收块（流式落盘）：验块哈希 → seek+offset 定位写；内存 O(1 块)
    let entry = &files[0];
    let part_path = out_dir.join(format!("{}.mtdss-part", man.transfer_id));
    // 读写打开（而非 File::create 的仅写）：Windows 下仅写句柄读回校验
    // 会报"拒绝访问"；create+truncate 保持原有 File::create 语义。
    let mut part = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part_path)
        .await
        .with_context(|| part_path.display().to_string())?;
    part.set_len(entry.size)
        .await
        .with_context(|| format!("preallocate {}", part_path.display()))?;
    let mut pending: HashMap<u64, &ChunkDesc> =
        entry.chunks.iter().map(|c| (c.offset, c)).collect();
    let mut remaining = pending.len();

    while remaining > 0 {
        let uni = conn.accept_uni().await?;
        let (id, data) = read_chunk(uni).await?;
        let desc = pending
            .remove(&id)
            .context("chunk id not in signed manifest")?;
        anyhow::ensure!(
            data.len() == desc.length as usize,
            "chunk length mismatch at offset {id}"
        );
        let mut h = Sha256::new();
        h.update(&data);
        anyhow::ensure!(
            h.finalize().to_vec() == desc.sha256,
            "chunk hash mismatch at offset {id}"
        );
        // 先验后写：坏块永远不落盘
        part.seek(SeekFrom::Start(desc.offset)).await?;
        part.write_all(&data).await?;
        remaining -= 1;
    }

    // 4) 整文件校验：顺序读回 .part 流式计算（第二遍顺序读，磁盘缓存友好）
    part.seek(SeekFrom::Start(0)).await?;
    let mut whole = Sha256::new();
    let mut rbuf = vec![0u8; 256 * 1024];
    loop {
        let n = part.read(&mut rbuf).await?;
        if n == 0 {
            break;
        }
        whole.update(&rbuf[..n]);
    }
    anyhow::ensure!(
        whole.finalize().to_vec() == entry.sha256,
        "whole-file hash mismatch"
    );

    // 5) 原子落盘（同名冲突自动改名）
    let final_path = unique_target(out_dir, &entry.rel_path)?;
    tokio::fs::rename(&part_path, &final_path).await?;

    // 6) 等待发送方完成标记，回最终确认
    let mut done = [0u8; 1];
    read_exact(&mut ctl_rx, &mut done).await?;
    anyhow::ensure!(done[0] == 0xFF, "sender completion marker missing");
    ctl_tx.write_all(&[0x01]).await?;
    // 等发送方收妥确认（其控制流发送侧 finish 的 EOF，或发送方完成/
    // 丢弃导致的连接关闭）再返回：否则本任务一返回、连接句柄被释放，
    // QUIC 的隐式 CONNECTION_CLOSE 会作废在途的最终确认字节，让已完整
    // 落盘的传输在发送方一侧误报失败。文件已验证落盘，两种结束方式
    // 都算成功。
    let _ = ctl_rx.read(&mut done).await;
    Ok(vec![final_path])
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

/// 读取一条块帧：`[u64 BE chunk_id][u64 BE length][data]`。
/// length 超过 CHUNK_SIZE 直接报错（在分配前检查）。
async fn read_chunk(mut uni: RecvStream) -> Result<(u64, Vec<u8>)> {
    let mut id = [0u8; 8];
    uni.read_exact(&mut id)
        .await
        .map_err(|e| anyhow!("chunk stream closed: {e:?}"))?;
    let mut len = [0u8; 8];
    uni.read_exact(&mut len)
        .await
        .map_err(|e| anyhow!("chunk stream closed: {e:?}"))?;
    let len = u64::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= CHUNK_SIZE, "oversized chunk frame ({len} bytes)");
    let mut data = vec![0u8; len];
    uni.read_exact(&mut data)
        .await
        .map_err(|e| anyhow!("chunk stream closed: {e:?}"))?;
    Ok((u64::from_be_bytes(id), data))
}

/// 控制流拒绝回复：`[0][u32 BE reason_len][reason]`。
async fn reply_reject(ctl: &mut SendStream, reason: &str) -> Result<()> {
    ctl.write_all(&[0u8]).await?;
    ctl.write_all(&(reason.len() as u32).to_be_bytes()).await?;
    ctl.write_all(reason.as_bytes()).await?;
    Ok(())
}

async fn read_exact(s: &mut RecvStream, buf: &mut [u8]) -> Result<()> {
    s.read_exact(buf)
        .await
        .map_err(|e| anyhow!("control stream closed: {e:?}"))
}
