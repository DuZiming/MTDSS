use crate::identity::DeviceIdentity;
use anyhow::{anyhow, Context, Result};
use mtdss_proto::manifest::{ChunkDesc, FileEntry, Manifest};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;

pub const CHUNK_SIZE: usize = 1024 * 1024;

/// rel 首段保留名（接收方私有布局锚，大小写不敏感）→ 保留语义名。
/// 发送侧构建与接收侧手签清单统一拒绝；仅首段保留（深层同名段不拒）：
/// - `.mtdss`：接收方 intake 区（隐藏收件区）；
/// - `photo-serve`：照片导出临时根（iOS home 直下；R8 终审 I-2——serve
///   目录已迁出 `.mtdss` 暂存区命名空间，此拒绝面保 rel 不与 home 级
///   布局锚相撞，纵深防御）。
fn reserved_first_segment(rel: &str) -> Option<&'static str> {
    match rel.split('/').next()? {
        seg if seg.eq_ignore_ascii_case(".mtdss") => Some("intake"),
        seg if seg.eq_ignore_ascii_case("photo-serve") => Some("photo-serve"),
        _ => None,
    }
}

/// 相对路径校验：非空、长度 ≤ 4096、'/' 分隔、拒绝 '\' 与 ':'（Windows 盘符/UNC），
/// 首段为保留名（见 [`reserved_first_segment`]）一律拒绝（大小写不敏感），拒绝空段
/// 与 "." 段（'//'、'a/./b'），'..' 不得越过根且不能只含 '..'。CJK/emoji 合法。
///
/// **目录行（空目录传输,spec §15.3）**:`rel` 恰带一个尾 `'/'` = 目录行
/// （接收端 mkdir 即成,零文件零分块）——剥掉这一个尾分隔符后按上述规则
/// 校验;`a//`（双尾）、`a//b`（中部空段）等仍拒。
pub fn validate_rel_path(rel: &str) -> bool {
    // 目录行形态:恰一个尾 '/' 先剥掉(共享规则照旧);余下的拒绝面不变
    let rel = rel.strip_suffix('/').unwrap_or(rel);
    // 保留首段是接收方私有布局锚：发送侧构建与接收侧手签清单统一拒绝
    // （大小写不敏感，与 fs 列表过滤、daemon collect_items 同口径）
    if reserved_first_segment(rel).is_some() {
        return false;
    }
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

/// 发送源条目的源侧形态(v2-P3 T4 源侧泛化):路径(桌面)或已解析的
/// 文件描述符(Android fd 桥)。`Fd` 变体仅 unix——`std::os::fd` 在
/// Windows 编译单元缺席(桌面无 fdpass 通道,bridge 形态在 Windows 侧
/// 起步即「源不可用」收口,IPC 契约面不受影响);借用处(清单构建/块流
/// 打开)经 `try_clone` 复制出本段使用的 fd,原 fd 归调用方的源表持有
/// (hash+send 两段共用,见 daemon jobs 的 SourceResolver 文档)。
#[derive(Debug)]
pub enum SourceFile {
    /// 本机路径(桌面路径形态,行为与泛化前逐字节一致)。
    Path(PathBuf),
    /// 已解析的源 fd(bridge 形态:fdpass 0x01 交付,fd 生命周期归
    /// daemon 源表)。
    #[cfg(unix)]
    Fd(std::os::fd::OwnedFd),
}

/// v2 签名域标签:防跨协议签名重用(同一设备密钥还签信令体)。
pub const SIGN_DOMAIN: &[u8] = b"MTDSS-MAN-SIG-V2";

/// 签名覆盖的确定性字节(v2 帧化):域标签 || 长度前缀(变长字段) ||
/// 计数前缀(file_count/chunk_count,消除结构歧义)|| 定宽数值。
/// 格式:域标签 || u32BE len(tid) || tid || u32BE file_count ||
/// 每文件 { u32BE len(rel) || rel || u64LE size || sha256(32) ||
///          u32BE chunk_count || 每块 { u64LE offset || u64LE length || sha256(32) } }
pub fn signable_bytes(transfer_id: &str, files: &[FileEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(SIGN_DOMAIN);
    let tid = transfer_id.as_bytes();
    out.extend_from_slice(&(tid.len() as u32).to_be_bytes());
    out.extend_from_slice(tid);
    out.extend_from_slice(&(files.len() as u32).to_be_bytes());
    for f in files {
        let rp = f.rel_path.as_bytes();
        out.extend_from_slice(&(rp.len() as u32).to_be_bytes());
        out.extend_from_slice(rp);
        out.extend_from_slice(&f.size.to_le_bytes());
        out.extend_from_slice(&f.sha256);
        out.extend_from_slice(&(f.chunks.len() as u32).to_be_bytes());
        for c in &f.chunks {
            out.extend_from_slice(&c.offset.to_le_bytes());
            out.extend_from_slice(&c.length.to_le_bytes());
            out.extend_from_slice(&c.sha256);
        }
    }
    out
}

/// 构建多文件清单：按 items 顺序逐个读文件，做 1MiB 分块哈希 + 整文件哈希；
/// items[i].1 即 files[i].rel_path（保序契约，后续任务依赖）；items[i].0 为
/// 源侧形态(v2-P3 T4 泛化:路径照旧 `File::open`,fd 经 `try_clone` +
/// `File::from` 同读——原 fd 归调用方,本函数只复制本段使用的副本)。
/// transfer_id 仍为一次性 32 随机字节的 hex（整份清单共用一个）；
/// sender_signature 覆盖 `signable_bytes`（已按多文件帧化）。
pub fn build_manifest(
    items: &[(SourceFile, String)],
    id: &DeviceIdentity,
) -> Result<Manifest> {
    let mut files: Vec<FileEntry> = Vec::with_capacity(items.len());
    let mut buf = vec![0u8; CHUNK_SIZE];
    for (source, rel) in items {
        anyhow::ensure!(validate_rel_path(rel), "invalid rel path");
        // 目录行（空目录传输,spec §15.3）：源是**目录**,不打开——逐文件
        // 读/分块哈希全程跳过,产出 size=0、零分块、整哈希 = 空内容哈希的
        // 目录行（与 0 字节文件同形,仅 rel 尾 '/' 有别）。接收端按目录行
        // mkdir（见 transfer::recv_files_body）。
        if is_dir_rel(rel) {
            files.push(FileEntry {
                rel_path: rel.clone(),
                size: 0,
                sha256: Sha256::digest([]).to_vec(),
                chunks: Vec::new(),
            });
            continue;
        }
        let mut file = match source {
            SourceFile::Path(path) => {
                std::fs::File::open(path).with_context(|| path.display().to_string())?
            }
            #[cfg(unix)]
            SourceFile::Fd(fd) => {
                // dup 出本段副本:借用的 OwnedFd 不能移出,原 fd 由调用方
                // 源表持有(供后续块流重开),读完即关本副本
                let dup = fd
                    .try_clone()
                    .with_context(|| "dup source fd for manifest")?;
                std::fs::File::from(dup)
            }
        };
        let size = file.metadata()?.len();

        let mut whole = Sha256::new();
        let mut chunks: Vec<ChunkDesc> = Vec::new();
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

        files.push(FileEntry {
            rel_path: rel.clone(),
            size,
            sha256: whole.finalize().to_vec(),
            chunks,
        });
    }

    use rand::RngCore;
    let mut rng = rand::rngs::OsRng;
    let mut tid = [0u8; 32];
    rng.fill_bytes(&mut tid);
    let transfer_id = hex::encode(tid);

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

/// 目录行判定（全链路同一口径,勿散落 `ends_with('/')`）:rel 尾恰一个
/// `'/'` = 目录行（空目录条目,spec §15.3）。`pub`:传输引擎两侧
/// （push_file 跳源打开 / 接收侧 mkdir 臂）与清单校验共用。
pub fn is_dir_rel(rel: &str) -> bool {
    rel.ends_with('/')
}

/// 校验清单：版本、1..=65535 个条目、rel 无重复、路径合法（首段保留名
/// 拒绝，见 [`reserved_first_segment`]，大小写不敏感；尾 '/' 目录行合法,
/// 但必须零 size 零分块——spec §15.3 空目录传输）、分块偏移首尾相接、
/// 0 < 块长 ≤ CHUNK_SIZE、块总和 == size、哈希长度 32、Ed25519 签名。
/// 返回通过校验的条目（保持清单原顺序）。
pub fn verify(man: &Manifest, sender: &ed25519_dalek::VerifyingKey) -> Result<Vec<FileEntry>> {
    use ed25519_dalek::Verifier;
    anyhow::ensure!(
        man.version == mtdss_proto::MANIFEST_VERSION,
        "unsupported manifest version"
    );
    anyhow::ensure!(!man.files.is_empty(), "empty manifest");
    anyhow::ensure!(
        man.files.len() <= 65535,
        "too many files: {} (cap 65535)",
        man.files.len()
    );
    let mut seen = std::collections::HashSet::with_capacity(man.files.len());
    for f in &man.files {
        anyhow::ensure!(
            seen.insert(f.rel_path.as_str()),
            "duplicate rel path: {}",
            f.rel_path
        );
        // 保留首段单列校验（大小写不敏感）：validate_rel_path 同样拒绝；
        // 这里在错误信息里点名保留语义，防御手签清单
        if let Some(label) = reserved_first_segment(&f.rel_path) {
            anyhow::bail!("reserved {label} segment rejected: {}", f.rel_path);
        }
        anyhow::ensure!(
            validate_rel_path(&f.rel_path),
            "illegal rel path: {}",
            f.rel_path
        );
        // 目录行结构约束（spec §15.3）:目录行零 size 零分块——手签清单
        // 不得借目录行走私数据（带分块的目录行在接收端无落盘路径）
        if is_dir_rel(&f.rel_path) {
            anyhow::ensure!(
                f.size == 0 && f.chunks.is_empty(),
                "dir entry must be size 0 with no chunks: {}",
                f.rel_path
            );
        }
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
