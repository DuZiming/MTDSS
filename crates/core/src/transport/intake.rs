//! 隐藏收件区与对账（`.mtdss/`）：transfer v2 的磁盘侧事实来源。
//!
//! 每个接收中的文件在 `out_dir/.mtdss/` 下留有两件东西：
//! - 数据 `.part`：`<sha256(rel)>.part`，`set_len` 预分配、seek 定位写；
//! - 记录 JSON：`<sha256(rel)>.json`，该文件已收块位图与完成态的唯一事实，
//!   断点续传据此对账。记录写入固定为「写 `.json.tmp` + `rename`」原子替换，
//!   任何时刻读到的记录都是完整一致的一份。
//!
//! 记录 JSON schema（`FileRecord`）：
//!
//! ```json
//! {
//!   "rel": "docs/a.txt",          // 相对路径（'/' 分隔，与清单一致）
//!   "size": 1048576,              // 整文件字节数
//!   "sha256": "64 位 hex",        // 整文件哈希
//!   "chunk_count": 1,             // 1 MiB 定长分块数（0 字节文件为 0）
//!   "bitmap": "hex",              // 位=1 表示该块已收；长度 = ceil(chunk_count/8)
//!   "done": false,                // true 表示已完整落盘
//!   "final_rel": null             // done 时的最终文件名（冲突改名人，'/' 分隔）
//! }
//! ```
//!
//! 对账（`reconcile`）按清单逐条产出计划，与 `man.files` 等长保序：
//! - 无可用记录 → `Full`（全新收）；
//! - 记录内容与清单条目不符（size/哈希/块数不一致，旧传输残留）→ 删旧
//!   记录与 `.part` 后 `Full`；
//! - done 且 final_rel 解析到的文件在场、size 吻合 → `Skip`；
//! - 其余（未完成）→ `.part` 在场则 `Resume(缺块位图)`（位=1 待收，与
//!   记录「已收」位互补），`.part` 丢失则数据不可信，降级 `Full`。
//!
//! `PartWriter` 负责续写：定位写块 → 置位 → 批量落盘记录（每 16 块或
//! 每 2 秒一次，崩溃最多重收 15 块；批量落盘不 fsync，只刷页缓存）；
//! `finalize` 顺序固定：整文件哈希读回校验 → `.part` fsync →
//! `unique_target` 空占位独占终名 → `.part` 原子改名 → 两侧目录 fsync →
//! 记 done + final_rel（tmp fsync + rename + 收件区 fsync）→ 清理
//! `.part`。有序落盘（spec §3①）保证断电后不存在「记录已 done 而终件
//! 损坏」的状态：任一步未完成，记录都还是 partial，由后续传输重收；
//! 改名失败同样记录保持 partial，由后续传输重收；占位文件自身会被
//! 清理，不阻塞下次改名。
//!
//! 收件区目录在 Windows 上带 `FILE_ATTRIBUTE_HIDDEN`，避免用户误删/
//! 误传（`.mtdss` 段同时被 `manifest::validate_rel_path` 列为保留段）。

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use mtdss_proto::manifest::{FileEntry, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// 隐藏收件区目录名（rel 首段保留字，见 `manifest::validate_rel_path`）。
pub const INTAKE_DIR: &str = ".mtdss";

/// 记录批量落盘阈值：累计 16 个新块刷一次。
const FLUSH_CHUNKS: usize = 16;
/// 记录批量落盘时间阈值：距上次落盘 2 秒强制刷一次。
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// 单个文件的对账计划。`Resume` 位 = 1 表示**缺块待收**
/// （与记录位图「已收=1」互补），Task 4 的 recv_files 直接按位重发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePlan {
    /// 全新接收（无记录 / 记录陈旧已清理 / `.part` 丢失 / done 但终件缺失）。
    Full,
    /// 已完整落盘（done 记录 + final 文件在场且 size 吻合），跳过。
    Skip,
    /// 断点续传：位图位 = 1 表示缺块待收。
    Resume(Vec<u8>),
}

/// JSON 里的 hex 编解码：位图等字节串以小写 hex 字符串落盘。
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(&s).map_err(serde::de::Error::custom)
    }
}

/// 磁盘上的接收记录（见模块文档 schema）。
#[derive(Serialize, Deserialize)]
struct FileRecord {
    rel: String,
    size: u64,
    /// 整文件 sha256 的 64 位小写 hex。
    sha256: String,
    chunk_count: usize,
    /// 已收位图；位=1 已收，长度恰为 ceil(chunk_count/8)，JSON 中为 hex。
    #[serde(with = "hex_bytes")]
    bitmap: Vec<u8>,
    done: bool,
    final_rel: Option<String>,
}

/// rel → 记录/数据文件键：sha256(rel) 全长 hex。避免 rel 中的非法/怪异
/// 字符直接落进文件名，且天然防路径穿越。
pub fn record_key(rel: &str) -> String {
    let mut h = Sha256::new();
    h.update(rel.as_bytes());
    hex::encode(h.finalize())
}

/// 该 rel 的 `.part` 数据文件路径（out_dir/.mtdss/<key>.part）。
pub fn part_path(out_dir: &Path, rel: &str) -> PathBuf {
    out_dir
        .join(INTAKE_DIR)
        .join(format!("{}.part", record_key(rel)))
}

/// 该 rel 的记录文件路径（out_dir/.mtdss/<key>.json）。
pub fn record_path(out_dir: &Path, rel: &str) -> PathBuf {
    out_dir
        .join(INTAKE_DIR)
        .join(format!("{}.json", record_key(rel)))
}

fn record_tmp_path(out_dir: &Path, rel: &str) -> PathBuf {
    out_dir
        .join(INTAKE_DIR)
        .join(format!("{}.json.tmp", record_key(rel)))
}

/// 对账：与 `man.files` 等长保序，逐条给出 Full / Skip / Resume 计划。
/// 陈旧记录（内容与本清单不符）在此删除，避免残留记录挡住全新接收。
pub fn reconcile(out_dir: &Path, man: &Manifest) -> Vec<FilePlan> {
    man.files.iter().map(|e| plan_for(out_dir, e)).collect()
}

fn plan_for(out_dir: &Path, entry: &FileEntry) -> FilePlan {
    let rec = match load_record(out_dir, &entry.rel_path) {
        Some(r) => r,
        // 无记录 / 记录损坏：当全新收（后续 open(resume=false) 会覆写记录）
        None => return FilePlan::Full,
    };
    // 与清单条目不符 = 上次传输的残留（内容不同）→ 清理后重收
    if rec.size != entry.size
        || rec.chunk_count != entry.chunks.len()
        || rec.sha256 != hex::encode(&entry.sha256)
    {
        let _ = std::fs::remove_file(record_path(out_dir, &entry.rel_path));
        let _ = std::fs::remove_file(part_path(out_dir, &entry.rel_path));
        return FilePlan::Full;
    }
    if rec.done {
        // Skip 校验 final_rel 的 size：在场且吻合才算跳过
        return match rec.final_rel {
            Some(fin) if final_matches(out_dir, &fin, entry.size) => FilePlan::Skip,
            _ => FilePlan::Full,
        };
    }
    // 未完成：.part 在场才能续传
    if part_path(out_dir, &entry.rel_path).exists() {
        FilePlan::Resume(missing_bitmap(&rec.bitmap, rec.chunk_count))
    } else {
        FilePlan::Full
    }
}
/// done 记录的 final_rel 解析：'/' → 平台分隔符后用 metadata 查在场与 size。
fn final_matches(out_dir: &Path, final_rel: &str, size: u64) -> bool {
    let p = out_dir.join(final_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    match std::fs::metadata(&p) {
        Ok(m) => m.len() == size,
        Err(_) => false,
    }
}

/// 读取并校验一条记录。读失败 / JSON 畸形 / 字段不自洽（rel 不符、
/// 位图长度不对、哈希不是 32 字节 hex）→ None，绝不 panic。
/// 载入时把 chunk_count 之外的填充位清零（canonical 化）：磁盘损坏的
/// 填充位若不掩掉，「已收」计数会虚高，missing_count 的 usize 减法
/// 将下溢回绕——这是所有位图进入内存的唯一入口，在此归一即可。
fn load_record(out_dir: &Path, rel: &str) -> Option<FileRecord> {
    let bytes = std::fs::read(record_path(out_dir, rel)).ok()?;
    let mut rec: FileRecord = serde_json::from_slice(&bytes).ok()?;
    if rec.rel != rel {
        return None;
    }
    if rec.bitmap.len() != rec.chunk_count.div_ceil(8) {
        return None;
    }
    if hex::decode(&rec.sha256).ok()?.len() != 32 {
        return None;
    }
    for i in rec.chunk_count..(rec.bitmap.len() * 8) {
        clear_bit(&mut rec.bitmap, i);
    }
    Some(rec)
}

/// 记录位图（已收=1）→ 缺块位图（待收=1）：按位取反后掩掉 chunk_count
/// 之外的高位填充位（load_record 已 canonical 化，此处掩码为纵深防御）。
fn missing_bitmap(bitmap: &[u8], chunk_count: usize) -> Vec<u8> {
    let mut miss: Vec<u8> = bitmap.iter().map(|b| !b).collect();
    for i in chunk_count..(bitmap.len() * 8) {
        clear_bit(&mut miss, i);
    }
    miss
}

fn set_bit(bitmap: &mut [u8], i: usize) {
    bitmap[i / 8] |= 1 << (i % 8);
}

fn clear_bit(bitmap: &mut [u8], i: usize) {
    bitmap[i / 8] &= !(1u8 << (i % 8));
}

/// 确保收件区目录存在；Windows 上补隐藏属性（FILE_ATTRIBUTE_HIDDEN）。
fn ensure_intake_dir(out_dir: &Path) -> Result<PathBuf> {
    let dir = out_dir.join(INTAKE_DIR);
    std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    #[cfg(windows)]
    set_hidden(&dir).with_context(|| format!("hide {}", dir.display()))?;
    Ok(dir)
}

/// Windows：把目录置为隐藏。读现属性按位或 HIDDEN 后写回（保留只读等
/// 其他可设属性）；不引第三方依赖，直接声明 kernel32 的 FFI。
#[cfg(windows)]
fn set_hidden(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    // FFI 符号名必须与 kernel32 导出名完全一致，不得 snake_case 化
    #[allow(non_snake_case)]
    #[link(name = "kernel32")]
    extern "system" {
        fn SetFileAttributesW(file_name: *const u16, attributes: u32) -> i32;
    }
    let attrs = std::fs::metadata(path)?.file_attributes() | FILE_ATTRIBUTE_HIDDEN;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: file_name 指向以 NUL 结尾的 UTF-16 路径，调用期间有效。
    let ok = unsafe { SetFileAttributesW(wide.as_ptr(), attrs) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

// ---------- 有序落盘助手（fsync） ----------

/// cfg(test) 下的有序落盘事件日志：记录每次 fsync 的目标路径，供单元
/// 测试断言 finalize 的落盘顺序（仅测试构建存在）。
#[cfg(test)]
static SYNC_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 把一条有序落盘事件记入 `SYNC_LOG`（仅测试构建；正式构建无操作）。
#[cfg(test)]
fn log_sync(event: String) {
    SYNC_LOG.lock().unwrap().push(event);
}

#[cfg(not(test))]
fn log_sync(_event: String) {}

/// 文件内容 fsync（数据/记录 tmp 通用）；cfg(test) 下同时记录事件。
async fn sync_file_logged(path: &Path, file: &tokio::fs::File) -> std::io::Result<()> {
    let r = file.sync_all().await;
    log_sync(format!("sync-file {}", path.display()));
    r
}

/// 目录 fsync：把目录项变更（改名/新建/删除）本身落盘。Unix 可直接
/// `File::open` 目录后 `sync_all`；Windows 目录句柄需自定义标志
/// `FILE_FLAG_BACKUP_SEMANTICS` 才能打开，且 `FlushFileBuffers` 要求
/// 句柄带 `GENERIC_WRITE`，故须以读写方式打开。
#[cfg(unix)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

/// finalize 临界区内的目录落盘（best-effort）：失败只告警不阻塞——
/// 目录 fsync 失败的断电后果是回退 partial 重收，属可恢复状态。
fn sync_dir_logged(path: &Path) {
    let r = sync_dir(path);
    log_sync(match &r {
        Ok(()) => format!("sync-dir {}", path.display()),
        Err(_) => format!("sync-dir-failed {}", path.display()),
    });
    if let Err(e) = r {
        tracing::warn!("dir fsync failed (best-effort): {}: {e}", path.display());
    }
}

/// `.part` 续写器：持有 `.part` 文件句柄与内存中的当前记录，块写入与
/// 记录批量落盘都在此完成。`finalize` 把校验过的 `.part` 原子改名落盘。
pub struct PartWriter {
    out_dir: PathBuf,
    part: PathBuf,
    file: tokio::fs::File,
    rec: FileRecord,
    /// 距上次记录落盘的新块数。
    dirty: usize,
    last_flush: Instant,
}

impl PartWriter {
    /// 打开 `.part` 续写器。`resume=true` 载入既有记录并续写（记录缺失或
    /// 与 entry 不符即报错——调用方应先 `reconcile` 得到 Resume 计划）；
    /// `resume=false` 截断重建：`set_len` 预分配 + 全零位图 + 立即落一次记录。
    pub async fn open(out_dir: &Path, entry: &FileEntry, resume: bool) -> Result<Self> {
        ensure_intake_dir(out_dir)?;
        let part = part_path(out_dir, &entry.rel_path);
        let rec = if resume {
            let r = load_record(out_dir, &entry.rel_path)
                .ok_or_else(|| anyhow!("no usable resume record for {}", entry.rel_path))?;
            anyhow::ensure!(
                r.size == entry.size
                    && r.chunk_count == entry.chunks.len()
                    && r.sha256 == hex::encode(&entry.sha256),
                "resume record does not match manifest entry: {}",
                entry.rel_path
            );
            r
        } else {
            FileRecord {
                rel: entry.rel_path.clone(),
                size: entry.size,
                sha256: hex::encode(&entry.sha256),
                chunk_count: entry.chunks.len(),
                bitmap: vec![0u8; entry.chunks.len().div_ceil(8)],
                done: false,
                final_rel: None,
            }
        };
        // 读写打开（Windows 下仅写句柄 finalize 读回校验会拒绝访问）；
        // 全新收则截断。set_len 对齐清单尺寸，容忍上次异常退出截断的 .part。
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(!resume)
            .truncate(!resume)
            .open(&part)
            .await
            .with_context(|| part.display().to_string())?;
        file.set_len(entry.size)
            .await
            .with_context(|| format!("preallocate {}", part.display()))?;
        let mut w = Self {
            out_dir: out_dir.to_path_buf(),
            part,
            file,
            rec,
            dirty: 0,
            last_flush: Instant::now(),
        };
        if !resume {
            // 全新收：立即落一次全零位图记录，让崩溃窗口从第一块起就被记录
            w.flush_record().await?;
        }
        Ok(w)
    }

    /// 写第 `idx` 块到 `offset`（调用方负责块哈希已验、idx/offset 来自同一条
    /// ChunkDesc）：seek 定位写 → 置已收位 → 计脏；达批量阈值自动落记录。
    pub async fn write_chunk(&mut self, idx: usize, offset: u64, data: &[u8]) -> Result<()> {
        anyhow::ensure!(
            idx < self.rec.chunk_count,
            "chunk idx {idx} out of range for {}",
            self.rec.rel
        );
        self.file.seek(SeekFrom::Start(offset)).await?;
        self.file.write_all(data).await?;
        set_bit(&mut self.rec.bitmap, idx);
        self.dirty += 1;
        if self.dirty >= FLUSH_CHUNKS || self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush_record().await?;
        }
        Ok(())
    }

    /// 把当前记录原子落盘（写 `.json.tmp` + rename）。**不 fsync**：周期
    /// 性批量落盘（16 块/2 秒）只求页缓存加速、不放大 IO（spec 裁定 4）；
    /// 完成态的持久化由 finalize 临界区的 `flush_record_durable` 兜底。
    pub async fn flush_record(&mut self) -> Result<()> {
        let json = serde_json::to_vec(&self.rec)?;
        let tmp = record_tmp_path(&self.out_dir, &self.rec.rel);
        tokio::fs::write(&tmp, &json)
            .await
            .with_context(|| tmp.display().to_string())?;
        let dst = record_path(&self.out_dir, &self.rec.rel);
        tokio::fs::rename(&tmp, &dst)
            .await
            .with_context(|| dst.display().to_string())?;
        self.dirty = 0;
        self.last_flush = Instant::now();
        Ok(())
    }

    /// 把当前记录**持久化**落盘（仅 finalize 完成临界区调用）：写
    /// `.json.tmp` → fsync tmp → rename → fsync 收件区目录。内容与目录项
    /// 都落盘后，done=true 才可信——断电最坏退回「记录 partial + 终件
    /// 已完整」，由重发路径重做 finalize 而非永久 SKIP。
    pub async fn flush_record_durable(&mut self) -> Result<()> {
        let json = serde_json::to_vec(&self.rec)?;
        let tmp = record_tmp_path(&self.out_dir, &self.rec.rel);
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .await
            .with_context(|| tmp.display().to_string())?;
        f.write_all(&json).await?;
        sync_file_logged(&tmp, &f)
            .await
            .with_context(|| tmp.display().to_string())?;
        drop(f);
        let dst = record_path(&self.out_dir, &self.rec.rel);
        tokio::fs::rename(&tmp, &dst)
            .await
            .with_context(|| dst.display().to_string())?;
        sync_dir_logged(&self.out_dir.join(INTAKE_DIR));
        self.dirty = 0;
        self.last_flush = Instant::now();
        Ok(())
    }

    /// 尚未收到的块数（位图中 0 位的个数）。
    pub fn missing_count(&self) -> usize {
        let received: usize = self
            .rec
            .bitmap
            .iter()
            .map(|b| b.count_ones() as usize)
            .sum();
        self.rec.chunk_count - received
    }

    /// 完成落盘，顺序固定：缺块清零检查 → 整文件哈希流式读回校验 →
    /// `.part` fsync → `unique_target` 空占位独占终名 → `.part` 原子
    /// 改名 → 两侧目录 fsync → 记 done + final_rel 持久化落盘 → 清理
    /// `.part`（rename 后已不在场，防御性再删）。有序落盘保证断电后
    /// 记录要么 partial（可重收）、要么 done 且终件已完整，绝无「done
    /// 而终件损坏」。返回最终文件路径。改名失败：记录保持 partial、
    /// Err 上抛（占位文件清理，不阻塞下次 finalize 重试）。
    pub async fn finalize(self) -> Result<PathBuf> {
        let mut w = self;
        anyhow::ensure!(
            w.missing_count() == 0,
            "finalize with {} chunks missing for {}",
            w.missing_count(),
            w.rec.rel
        );

        // 1) 整文件哈希：seek(0) 流式顺序读回（第二遍顺序读，磁盘缓存友好）
        w.file.seek(SeekFrom::Start(0)).await?;
        let mut whole = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = w.file.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            whole.update(&buf[..n]);
        }
        anyhow::ensure!(
            hex::encode(whole.finalize()) == w.rec.sha256,
            "whole-file hash mismatch for {}",
            w.rec.rel
        );

        // 2) 数据落盘：`.part` 内容 fsync 后才允许改名——断电最坏留下
        //    partial 记录 + `.part` 重收，绝不留下「记录 done 而终件损坏」
        sync_file_logged(&w.part, &w.file).await?;

        // 3) 终名占位（create_new 原子独占）+ 原子改名
        let final_path = unique_target(&w.out_dir, &w.rec.rel)?;
        if let Err(e) = tokio::fs::rename(&w.part, &final_path).await {
            // 改名失败：占位文件是本调用刚建的空壳，清掉以便重试
            let _ = tokio::fs::remove_file(&final_path).await;
            return Err(anyhow!(e).context(format!(
                "rename {} -> {}",
                w.part.display(),
                final_path.display()
            )));
        }

        // 4) 目录项落盘：改名涉及的两侧目录（终件直接父目录 + 收件区）
        //    best-effort fsync，失败只告警
        if let Some(parent) = final_path.parent() {
            sync_dir_logged(parent);
        }
        sync_dir_logged(&w.out_dir.join(INTAKE_DIR));

        // 5) 记 done + final_rel（冲突改名人），持久化落盘
        //    （tmp fsync + rename + 收件区 fsync）
        w.rec.done = true;
        w.rec.final_rel = Some(portable_rel(&final_path, &w.out_dir));
        w.flush_record_durable().await?;

        // 6) .part 已随 rename 消失；防御性清理，忽略 NotFound
        let _ = tokio::fs::remove_file(&w.part).await;
        Ok(final_path)
    }
}

/// 最终路径 → 可移植 final_rel：剥掉 out_dir 前缀，平台分隔符归一为 '/'。
fn portable_rel(final_path: &Path, out_dir: &Path) -> String {
    let rel = final_path.strip_prefix(out_dir).unwrap_or(final_path);
    rel.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// 计算最终落盘路径：rel 中 `/` 换为平台分隔符；目标已存在时依次尝试
/// `name (1).ext`、`name (2).ext`…。用 `create_new` 原子占位独占名字，
/// 防止并发同名传输在"存在性检查 → 改名"窗口内互相覆盖；占位成功后
/// 由调用方把已校验的 `.part` 原子改名覆盖自身占位文件。
pub(crate) fn unique_target(out_dir: &Path, rel: &str) -> Result<PathBuf> {
    // 防御深度：manifest::verify 已拒绝越根的 ".."，这里拒绝一切 ".." 段，
    // 保证 join 后绝不可能写出 out_dir 之外。
    anyhow::ensure!(
        !rel.split('/').any(|seg| seg == ".."),
        "rel path must not contain '..'"
    );
    let target = out_dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
    }
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    for n in 0u32.. {
        let cand = if n == 0 {
            target.clone()
        } else {
            target.with_file_name(format!("{stem} ({n}){ext}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&cand)
        {
            Ok(f) => {
                drop(f);
                return Ok(cand);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(anyhow!(e)).with_context(|| cand.display().to_string());
            }
        }
    }
    Err(anyhow!("cannot find unique name for {}", target.display()))
}

// ---------- 单元测试：finalize 有序落盘 ----------

#[cfg(test)]
mod tests {
    use super::*;
    use mtdss_proto::manifest::{ChunkDesc, FileEntry};

    /// SYNC_LOG 是全局单例，两个用例必须串行执行，避免事件互相污染。
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn log() -> Vec<String> {
        SYNC_LOG.lock().unwrap().clone()
    }

    /// 单块 entry + 自洽数据：确定性伪随机内容（与集成测试 make_source
    /// 同一 xorshift 种子法），sha256 与内容对得上，finalize 哈希可过。
    fn one_chunk(rel: &str, size: u64) -> (FileEntry, Vec<u8>) {
        let mut data = vec![0u8; size as usize];
        let mut s: u64 = size | 1;
        for b in rel.bytes() {
            s = s.wrapping_mul(0x100000001B3) ^ b as u64;
        }
        for b in data.iter_mut() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *b = (s & 0xFF) as u8;
        }
        let mut ch = Sha256::new();
        ch.update(&data);
        let mut whole = Sha256::new();
        whole.update(&data);
        let entry = FileEntry {
            rel_path: rel.to_string(),
            size,
            sha256: whole.finalize().to_vec(),
            chunks: vec![ChunkDesc {
                offset: 0,
                length: size,
                sha256: ch.finalize().to_vec(),
            }],
        };
        (entry, data)
    }

    async fn finalize_rel_at(out: &Path, rel: &str, size: u64) -> PathBuf {
        SYNC_LOG.lock().unwrap().clear();
        let (entry, data) = one_chunk(rel, size);
        let mut w = PartWriter::open(out, &entry, false).await.unwrap();
        w.write_chunk(0, 0, &data).await.unwrap();
        w.finalize().await.unwrap()
    }

    /// 自带临时目录的顶层 finalize（临时目录随函数退出删除——断言只用
    /// 日志字符串，不需要目录存活）。
    async fn finalize_rel(rel: &str, size: u64) -> PathBuf {
        let out = tempfile::tempdir().unwrap();
        finalize_rel_at(out.path(), rel, size).await
    }

    /// 顶层 rel 的 5 事件有序序列（spec §3①）：part fsync → 改名 →
    /// 直接父目录 + .mtdss → 记录 tmp fsync → .mtdss。
    #[tokio::test]
    async fn finalize_sequence_toplevel() {
        let _g = SERIAL.lock().await;
        let p = finalize_rel("a.bin", 4096).await;
        let out = p.parent().unwrap();
        let intake = out.join(INTAKE_DIR);
        assert_eq!(
            log(),
            vec![
                format!("sync-file {}", part_path(out, "a.bin").display()),
                format!("sync-dir {}", out.display()),
                format!("sync-dir {}", intake.display()),
                format!("sync-file {}", record_tmp_path(out, "a.bin").display()),
                format!("sync-dir {}", intake.display()),
            ]
        );
    }

    /// 深层 rel：目录 fsync 打在**直接父目录**（out/d），不打 out_dir
    /// 本身；.mtdss 恰好两次（rename 后一次、记录落盘后一次）。
    #[tokio::test]
    async fn finalize_sequence_nested_rel_syncs_immediate_parent() {
        let _g = SERIAL.lock().await;
        let out = tempfile::tempdir().unwrap();
        let p = finalize_rel_at(out.path(), "d/e.bin", 4096).await;
        let dirs: Vec<String> = log()
            .iter()
            .filter(|e| e.starts_with("sync-dir "))
            .cloned()
            .collect();
        assert!(
            dirs.contains(&format!("sync-dir {}", p.parent().unwrap().display())),
            "必须 fsync 直接父目录: {dirs:?}"
        );
        assert!(
            !dirs.contains(&format!("sync-dir {}", out.path().display())),
            "深层 rel 不得打 out_dir 本身: {dirs:?}"
        );
        assert_eq!(
            dirs.iter().filter(|e| e.ends_with(INTAKE_DIR)).count(),
            2,
            ".mtdss 应恰好 fsync 两次: {dirs:?}"
        );
    }
}
