use crate::identity::DeviceIdentity;
use anyhow::{anyhow, Context, Result};
use mtdss_proto::manifest::{ChunkDesc, FileEntry, Manifest};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub const CHUNK_SIZE: usize = 1024 * 1024;

/// 相对路径校验：非空、长度 ≤ 4096、'/' 分隔、拒绝 '\' 与 ':'（Windows 盘符/UNC），
/// 拒绝空段与 "." 段（'//'、'a/./b'），'..' 不得越过根且不能只含 '..'。CJK/emoji 合法。
pub fn validate_rel_path(rel: &str) -> bool {
    if rel.is_empty() || rel.len() > 4096 || rel == "." {
        return false;
    }
    if rel.starts_with('/') || rel.contains('\\') || rel.contains(':') {
        return false;
    }
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." {
            return false;
        }
    }
    let mut depth: i64 = 0;
    for seg in rel.split('/') {
        if seg == ".." {
            depth -= 1;
            if depth < 0 {
                return false;
            }
        } else {
            depth += 1;
        }
    }
    depth > 0
}

/// 签名覆盖的确定性字节：transfer_id || 各条目（rel_path, size LE, 整文件 sha256, 全部分块 sha256 串接）。
pub fn signable_bytes(transfer_id: &str, files: &[FileEntry]) -> Vec<u8> {
    let mut out = transfer_id.as_bytes().to_vec();
    for f in files {
        out.extend_from_slice(f.rel_path.as_bytes());
        out.extend_from_slice(&f.size.to_le_bytes());
        out.extend_from_slice(&f.sha256);
        for c in &f.chunks {
            out.extend_from_slice(&c.sha256);
        }
    }
    out
}

/// 构建单文件清单：按 1MiB 分块哈希 + 整文件哈希；transfer_id 为 32 随机字节的 hex；
/// sender_signature 覆盖 `signable_bytes`。
pub fn build_single(path: &Path, rel: &str, id: &DeviceIdentity) -> Result<Manifest> {
    anyhow::ensure!(validate_rel_path(rel), "invalid rel path");
    let mut file = std::fs::File::open(path).with_context(|| path.display().to_string())?;
    let size = file.metadata()?.len();

    let mut whole = Sha256::new();
    let mut chunks: Vec<ChunkDesc> = Vec::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut offset: u64 = 0;
    loop {
        let n = read_full(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut ch = Sha256::new();
        ch.update(&buf[..n]);
        whole.update(&buf[..n]);
        chunks.push(ChunkDesc {
            offset,
            length: n as u64,
            sha256: ch.finalize().to_vec(),
        });
        offset += n as u64;
    }

    use rand::RngCore;
    let mut rng = rand::rngs::OsRng;
    let mut tid = [0u8; 32];
    rng.fill_bytes(&mut tid);
    let transfer_id = hex::encode(tid);

    let entry = FileEntry {
        rel_path: rel.to_string(),
        size,
        sha256: whole.finalize().to_vec(),
        chunks,
    };
    let files = vec![entry];
    let sig = id.sign(&signable_bytes(&transfer_id, &files));
    Ok(Manifest {
        version: mtdss_proto::MANIFEST_VERSION,
        transfer_id,
        files,
        sender_signature: sig.to_bytes().to_vec(),
    })
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        let n = f.read(&mut buf[done..])?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}

/// 校验清单：版本、恰好 1 个条目（M1）、路径合法、分块偏移首尾相接、
/// 0 < 块长 ≤ CHUNK_SIZE、块总和 == size、哈希长度 32、Ed25519 签名。
/// 返回通过校验的条目。
pub fn verify(man: &Manifest, sender: &ed25519_dalek::VerifyingKey) -> Result<Vec<FileEntry>> {
    use ed25519_dalek::Verifier;
    anyhow::ensure!(
        man.version == mtdss_proto::MANIFEST_VERSION,
        "unsupported manifest version"
    );
    anyhow::ensure!(!man.files.is_empty(), "empty manifest");
    anyhow::ensure!(man.files.len() == 1, "M1 supports exactly one file");
    for f in &man.files {
        anyhow::ensure!(
            validate_rel_path(&f.rel_path),
            "illegal rel path: {}",
            f.rel_path
        );
        // 块描述一致性：offset/length 首尾相接、总和 == size
        let mut expect_off: u64 = 0;
        for c in &f.chunks {
            anyhow::ensure!(c.offset == expect_off, "chunk offset gap");
            anyhow::ensure!(
                c.length > 0 && c.length as usize <= CHUNK_SIZE,
                "chunk length bad"
            );
            expect_off += c.length;
        }
        anyhow::ensure!(expect_off == f.size, "chunk total != size");
        anyhow::ensure!(
            f.sha256.len() == 32 && c_len32(&f.chunks),
            "hash length bad"
        );
    }
    let sig = ed25519_dalek::Signature::from_slice(&man.sender_signature)
        .map_err(|_| anyhow!("bad signature encoding"))?;
    sender
        .verify(&signable_bytes(&man.transfer_id, &man.files), &sig)
        .map_err(|_| anyhow!("signature mismatch"))?;
    Ok(man.files.clone())
}

fn c_len32(chunks: &[ChunkDesc]) -> bool {
    chunks.iter().all(|c| c.sha256.len() == 32)
}
