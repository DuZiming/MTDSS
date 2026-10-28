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
//! 冲突策略（M3c）：`reconcile_with_policy` 在上述对账前叠加 skip/
//! overwrite 策略（由信令层携带，与清单/线协议无关——manifest 及其
//! 签名域保持字节不变）：skip 命中直接 `Skip`；overwrite 命中先清掉
//! 目标处与各级祖先上的既有**文件**（`prepare_overwrite`，类型不匹配
//! 清障），再按既有语义对账。目录与符号链接从不清除——留给终件阶段的
//! 冲突改名安全网兜底，绝不递归删除用户目录。
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
//!
//! v2-P3 T5：上述磁盘面经 [`DestIo`] trait 参数化——对账/清理/续写器只
//! 依赖 trait（记录 JSON 的 schema、键空间与原子写语义统一留在本模块），
//! [`FsDestIo`] 承接既有全部布局与语义（桌面行为逐字节不变）；Android
//! 收件箱形态（daemon 侧 `InboxDestIo`——FsDestIo 的私有暂存薄包装，
//! P4a 起）在同一 trait 上实现。

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use mtdss_proto::manifest::{FileEntry, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// 隐藏收件区目录名（rel 首段保留字，见 `manifest::validate_rel_path`）。
pub const INTAKE_DIR: &str = ".mtdss";

/// 收件箱哨兵(v2-P3 T6):download/upload 的 `dir` 取本值时,目的地路由
/// 到 Android 收件箱(daemon 侧 `InboxDestIo`:FsDestIo 薄包装,私有暂存区
/// `{home}/inbox-staging`,P4a 起)而非本机路径。放本模块是因为它是
/// **接收侧目的地语义**的一部分——引擎/daemon 两侧共用,不随具体实现
/// 漂移。
pub const INBOX_SENTINEL: &str = "inbox";

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
    record_file(&out_dir.join(INTAKE_DIR), rel)
}

/// 记录目录直接定位的记录文件路径（通用面：引擎侧经
/// `DestIo::records_dir` 拿到目录后由此拼装，与目的地布局无关）。
fn record_file(records_dir: &Path, rel: &str) -> PathBuf {
    records_dir.join(format!("{}.json", record_key(rel)))
}

/// 记录原子写的 tmp 路径（通用面，同 [`record_file`]）。
fn record_tmp_file(records_dir: &Path, rel: &str) -> PathBuf {
    records_dir.join(format!("{}.json.tmp", record_key(rel)))
}

// ---------- DestIo：接收侧目的地抽象（v2-P3 T5） ----------

/// [`DestIo::commit`] 的返回：终件落点 + 记录用的可移植 final_rel。
#[derive(Debug, Clone)]
pub struct Committed {
    /// 终件在本目的地的路径（接收结果的落盘路径面：引擎的 landed paths
    /// 直接收录；FsDestIo = recv_dir 下的路径）。
    pub path: PathBuf,
    /// 记录 `final_rel` 字段值：'/' 分隔、相对目的地的可移植形态
    /// （FsDestIo = 终件路径剥 recv_dir 前缀；对账 Skip 的
    /// `final_state` 查询以它为键）。
    pub final_rel: String,
}

/// 接收侧目的地 IO 抽象（v2-P3 T5）：intake 的全部磁盘面——记账、续传
/// 素材、终件落盘、对账查询——经本 trait 参数化，引擎与具体目的地形态
/// 解耦。桌面实现 [`FsDestIo`]（收件目录布局，行为与 v2-P3 前逐字节
/// 一致）；Android 收件箱（daemon 侧 `InboxDestIo`：FsDestIo 私有暂存
/// 薄包装，P4a 起）在同一 trait 上实现。
///
/// 语义分工（方法面即语义面）：
/// - **记账（记录 JSON）**：schema、键空间（`record_key`）、tmp+rename
///   原子写与 durable 落盘全部留在 intake——各实现共享同一记录格式，
///   trait 只回答「记录目录在哪」（`records_dir`）。
/// - **续传素材（`.part`）**：开写（`open_writer`）、在场查询
///   （`partial_exists`）、废弃（`discard_partial`）。
/// - **终件**：在位查询（`final_state`）与提交（`commit`）。
/// - **目的地环境**：可用空间（`free_bytes`）与 overwrite 清障
///   （`prepare_overwrite`）。
///
/// 同步/异步分界：对账/清理是同步重 IO 面（引擎在阻塞线程池调用，见
/// `transfer::recv_files_body`），查询类方法为同步；写盘/终件沿
/// PartWriter 的异步面（tokio::fs），`open_writer`/`commit` 以
/// `BoxFuture` 返回（保 `dyn` 兼容，不引 async-trait 依赖）。
///
/// 调用侧序（实现者须知；引擎/intake 侧的固定编排）：
/// - 对账（阻塞线程池）：`reconcile_with_policy` 逐清单条目——overwrite
///   命中先 `prepare_overwrite`，再读记录（`records_dir`）判陈旧（陈旧
///   → 删记录 + `discard_partial`）、done 校验 `final_state`、未完成查
///   `partial_exists`。
/// - 接收（异步）：每文件恰一次 `open_writer`（Full=resume=false /
///   Resume=resume=true）→ 块直写句柄 → 缺块清零后 finalize：
///   `commit` → 记录 done 落盘 → `discard_partial`（防御性）。
/// - 清理（阻塞线程池）：`cleanup_rels` 逐 rel 读/删记录（`records_dir`）
///   + `discard_partial`。
///
/// 实现约束：
/// - `Send + Sync`：引擎以 `Arc<dyn DestIo>` 跨任务/线程池共享。
/// - **记录目录生命周期**：`records_dir` 是纯路径查询（不建目录、零
///   IO）；目录的存在性由 `open_writer` 保证（返回前 `records_dir()` 必
///   须已可用于记录 IO——intake 在全新收路径随开箱即写记录）。对账/
///   清理只读不建：目录缺席时记录读失败一律按「无记录」处理，绝不因
///   对账在干净目的地上凭空建出记录目录。
pub trait DestIo: Send + Sync {
    /// 记录目录路径（纯查询，不建目录、零 IO）。记录/记录 tmp 的文件名
    /// （`<sha256(rel)>.json` / `.json.tmp`）由 intake 统一拼装，实现只
    /// 回答目录本身（FsDestIo = `recv_dir/.mtdss`）。
    fn records_dir(&self) -> PathBuf;

    /// 终件在位查询：把记录里的 `final_rel`（'/' 分隔、相对目的地）解
    /// 析到实际位置，在场且能取到尺寸 → `Some(size)`，否则 `None`。
    /// 这是 done 记录 Skip 的唯一判据（尺寸与清单条目吻合才跳过）；
    /// 实现不得因查询失败 panic（一律按 `None`）。
    fn final_state(&self, final_rel: &str) -> Option<u64>;

    /// 该 rel 的续传素材（`.part` 或等价暂存）是否在场。未完成记录 +
    /// 素材在场 → Resume；素材丢失 → Full（数据不可信）。素材在
    /// provider 侧的形态（fd 直写）可恒按记录面回答 true，把缺席交给
    /// `open_writer(resume=true)` 的失败回退处理。
    fn partial_exists(&self, rel: &str) -> bool;

    /// 废弃该 rel 的续传素材（尽力而为，失败静默：调用点语义均为陈旧
    /// 记录清理 / finalize 收尾的防御性兜底，失败不阻断也不计数）。陈旧
    /// 记录清理、`cleanup_rels` 记录删除成功路径、finalize 收尾共用。
    fn discard_partial(&self, rel: &str);

    /// overwrite 策略清障：对账前清掉 rel 终点处与各级祖先上的既有
    /// **文件**型占位——祖先上的文件必须清（`d/e` 落盘需要 `d` 能被建成
    /// 目录，类型不匹配清障即为此而生）；目录/符号链接绝不清（留给终件
    /// 阶段的冲突改名安全网，绝不递归删用户目录）。失败只告警不阻断。
    /// 无本地目标树的形态可 no-op。
    fn prepare_overwrite(&self, rel: &str);

    /// 目的地可用字节数（接收前磁盘预检）。查询失败按「无限」处理——
    /// 不因未知空间误拒。
    fn free_bytes(&self) -> u64;

    /// 打开该 entry 的续写句柄：`resume=true` 续写既有素材（缺席/不可
    /// 用即 Err——调用方应先 `reconcile` 得到 Resume 计划）；`resume=false`
    /// 全新截断重建。返回可读可写、尺寸恰为 `entry.size` 的句柄
    /// （FsDestIo：`set_len` 预分配，容忍上次异常退出截断的素材）。
    /// **返回前必须保证 `records_dir()` 已存在**（见 trait 文档：全新收
    /// 随开箱即写记录）。错误文案为实现自由面。
    fn open_writer<'a>(
        &'a self,
        entry: &'a FileEntry,
        resume: bool,
    ) -> BoxFuture<'a, Result<tokio::fs::File>>;

    /// 终件提交：把已整文件校验过的 `.part` 句柄落到终名。顺序钉死
    /// （spec §3①，断电安全的前提）：`.part` 内容 fsync → 独占终名
    /// （冲突改名在实现内，如 FsDestIo 的 `unique_target` ` (n)` 序列）
    /// → 原子改名/转移 → 终件直接父目录 + 记录目录两侧目录项 fsync
    /// （best-effort，失败只告警）。**不写记录**（done/final_rel 的
    /// 持久化由 PartWriter::finalize 在本方法成功后落，顺序不可倒）。
    /// 句柄借用至本 future 完成后归还调用方（PartWriter 持有至记录落盘
    /// 后随 Drop 释放）。返回终件路径与 final_rel（[`Committed`]）；
    /// 提交失败须清理自建占位（记录保持 partial 由调用方上抛重收）。
    fn commit<'a>(
        &'a self,
        rel: &'a str,
        part: &'a mut tokio::fs::File,
    ) -> BoxFuture<'a, Result<Committed>>;
}

/// 路径所在卷的可用字节数。查询失败按「无限」处理——不因未知空间误拒。
pub fn free_bytes_at(path: &Path) -> u64 {
    fs2::available_space(path).unwrap_or(u64::MAX)
}

/// 桌面/文件系统目的地：接收落盘到 `recv_dir`。承接 v2-P3 前的全部既有
/// 行为（布局、路径、错误文案、原子语义逐字节不变）：记录与 `.part` 同
/// 居 `recv_dir/.mtdss/`（Windows 隐藏属性，[`ensure_intake_dir`]），
/// 终件 = `recv_dir/rel`（同名冲突 `name (n).ext` 改名，[`unique_target`]）。
#[derive(Debug, Clone)]
pub struct FsDestIo {
    recv_dir: PathBuf,
}

impl FsDestIo {
    /// 以收件目录构建（终件与 `.mtdss` 收件区都在它之下）。
    pub fn new(recv_dir: impl AsRef<Path>) -> Self {
        Self {
            recv_dir: recv_dir.as_ref().to_path_buf(),
        }
    }
}

impl DestIo for FsDestIo {
    fn records_dir(&self) -> PathBuf {
        self.recv_dir.join(INTAKE_DIR)
    }

    fn final_state(&self, final_rel: &str) -> Option<u64> {
        let p = self
            .recv_dir
            .join(final_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::metadata(&p).ok().map(|m| m.len())
    }

    fn partial_exists(&self, rel: &str) -> bool {
        part_path(&self.recv_dir, rel).exists()
    }

    fn discard_partial(&self, rel: &str) {
        let _ = std::fs::remove_file(part_path(&self.recv_dir, rel));
    }

    fn prepare_overwrite(&self, rel: &str) {
        let mut acc = self.recv_dir.to_path_buf();
        for seg in rel.split('/') {
            acc.push(seg);
            remove_file_placeholder(&acc);
        }
    }

    fn free_bytes(&self) -> u64 {
        free_bytes_at(&self.recv_dir)
    }

    fn open_writer<'a>(
        &'a self,
        entry: &'a FileEntry,
        resume: bool,
    ) -> BoxFuture<'a, Result<tokio::fs::File>> {
        let recv_dir = self.recv_dir.clone();
        async move {
            // 收件区 ensure（Windows 隐藏属性）先于开件：全新收路径随开箱
            // 即写记录，目录必须在场（沿既有 PartWriter::open 的次序）。
            ensure_intake_dir(&recv_dir)?;
            let part = part_path(&recv_dir, &entry.rel_path);
            // 读写打开（Windows 下仅写句柄 finalize 读回校验会拒绝访问）；
            // 全新收则截断。set_len 对齐清单尺寸，容忍上次异常退出截断的
            // .part。
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
            Ok(file)
        }
        .boxed()
    }

    fn commit<'a>(
        &'a self,
        rel: &'a str,
        part: &'a mut tokio::fs::File,
    ) -> BoxFuture<'a, Result<Committed>> {
        let recv_dir = self.recv_dir.clone();
        let part_file = part_path(&recv_dir, rel);
        async move {
            // 1) 数据落盘：`.part` 内容 fsync 后才允许改名——断电最坏留下
            //    partial 记录 + `.part` 重收，绝不留下「记录 done 而终件
            //    损坏」
            sync_file_logged(&part_file, part).await?;

            // 2) 终名占位（create_new 原子独占）+ 原子改名
            let final_path = unique_target(&recv_dir, rel)?;
            if let Err(e) = tokio::fs::rename(&part_file, &final_path).await {
                // 改名失败：占位文件是本调用刚建的空壳，清掉以便重试
                let _ = tokio::fs::remove_file(&final_path).await;
                return Err(anyhow!(e).context(format!(
                    "rename {} -> {}",
                    part_file.display(),
                    final_path.display()
                )));
            }

            // 3) 目录项落盘：改名涉及的两侧目录（终件直接父目录 + 记录
            //    目录）best-effort fsync，失败只告警
            if let Some(parent) = final_path.parent() {
                sync_dir_logged(parent);
            }
            sync_dir_logged(&recv_dir.join(INTAKE_DIR));

            Ok(Committed {
                final_rel: portable_rel(&final_path, &recv_dir),
                path: final_path,
            })
        }
        .boxed()
    }
}

/// 跨设备残留清理（spec §5.3）：逐 rel 按 `sha256(rel)` 定位 `.mtdss/`
/// 记录，读出并核对身份（记录 `rel` 字段与请求 rel 一致）后才删记录与
/// 对应 `.part`——键位命中不算数，身份是删与不删的终审（防哈希碰撞/
/// 键复用误删他人传输的续传素材）。记录缺席 / JSON 畸形（身份无从
/// 核对）/ 身份不符一律静默跳过，故重复清理幂等（返回 0）。只动隐藏区
/// 内记录与 `.part` 两件套，正式文件绝不触碰；`.mtdss` 段保护不变。
/// 返回完成清理的 rel 数。
pub fn cleanup_rels(dest: &dyn DestIo, rels: &[String]) -> u32 {
    let records_dir = dest.records_dir();
    let mut n = 0usize;
    for rel in rels {
        let Ok(bytes) = std::fs::read(record_file(&records_dir, rel)) else {
            continue; // 记录缺席：无残留，幂等跳过
        };
        let Ok(rec) = serde_json::from_slice::<FileRecord>(&bytes) else {
            continue; // 畸形记录：身份无从核对，宁留勿误删
        };
        if rec.rel != *rel {
            continue; // 身份不符：键位上的他人记录，绝不动
        }
        // 记录删除成功才计 1(评审 carry:cleaned 必须如实反映记录面——删除
        // 失败仍计数会谎报清理完成,调用方「回执 Ok 才删记录」的语义失真);
        // `.part` 随成功路径尽力而为:记录已删,`.part` 纵残留也无碍(同 rel
        // 键位确定,重收时对账/覆写自愈)。
        if std::fs::remove_file(record_file(&records_dir, rel)).is_ok() {
            dest.discard_partial(rel);
            n += 1;
        }
    }
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 对账：与 `man.files` 等长保序，逐条给出 Full / Skip / Resume 计划。
/// 陈旧记录（内容与本清单不符）在此删除，避免残留记录挡住全新接收。
pub fn reconcile(dest: &dyn DestIo, man: &Manifest) -> Vec<FilePlan> {
    man.files.iter().map(|e| plan_for(dest, e)).collect()
}

fn plan_for(dest: &dyn DestIo, entry: &FileEntry) -> FilePlan {
    let rec = match load_record(&dest.records_dir(), &entry.rel_path) {
        Some(r) => r,
        // 无记录 / 记录损坏：当全新收（后续 open(resume=false) 会覆写记录）
        None => return FilePlan::Full,
    };
    // 与清单条目不符 = 上次传输的残留（内容不同）→ 清理后重收
    if rec.size != entry.size
        || rec.chunk_count != entry.chunks.len()
        || rec.sha256 != hex::encode(&entry.sha256)
    {
        let _ = std::fs::remove_file(record_file(&dest.records_dir(), &entry.rel_path));
        dest.discard_partial(&entry.rel_path);
        return FilePlan::Full;
    }
    if rec.done {
        // Skip 校验 final_rel 的 size：在场且吻合才算跳过
        return match rec.final_rel {
            Some(fin) if dest.final_state(&fin) == Some(entry.size) => FilePlan::Skip,
            _ => FilePlan::Full,
        };
    }
    // 未完成：.part 在场才能续传
    if dest.partial_exists(&entry.rel_path) {
        FilePlan::Resume(missing_bitmap(&rec.bitmap, rec.chunk_count))
    } else {
        FilePlan::Full
    }
}

// ---------- 冲突策略（M3c） ----------

/// 传输冲突策略：skip/overwrite 各是一组 rel 或目录前缀。来源是 UI 的
/// 冲突扫描（Task 6），经信令层 ConnectRequest 携带（Task 5）——刻意
/// 不进传输线协议、不进清单：清单与其签名域保持字节不变，旧端互通。
/// 匹配语义：条目 rel 命中 `p` 当且仅当 `rel == p` 或 `rel` 以
/// `p + "/"` 起头——前缀必须落在路径段边界上（`ab` 不命中 `abc`/
/// `ab-c`，命中 `ab/x`）；字节精确比较，不归一大小写/分隔符。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferPolicy {
    /// 命中即整条跳过：不落盘、不动记录与 `.part`（下次再传仍可续传），
    /// 接收端直接按 Skip 计 FileDone（发送方按完成计数）。
    pub skip: Vec<String>,
    /// 命中即覆盖：对账前清掉目标处与各级祖先上的既有**文件**
    /// （见 [`DestIo::prepare_overwrite`]），使终件恰好落在清单 rel 上（不触发
    /// ` (n)` 冲突改名）。
    pub overwrite: Vec<String>,
}

/// rel 是否命中策略表(精确或段边界前缀)。`pub(crate)`:ctrl 同机
/// copy/move(`fs::copy_paths_with`/`move_entries_with`)与传输面共用
/// 同一匹配口径,防两处实现漂移。
pub(crate) fn policy_hits(list: &[String], rel: &str) -> bool {
    list.iter().any(|p| {
        rel == p.as_str()
            || rel
                .strip_prefix(p.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// reconcile 的策略版：先按策略改写条目命运，再走既有 [`plan_for`]——
/// - skip 命中 → `Skip`（无论记录状态；记录/`.part`/磁盘全不动）；
/// - overwrite 命中 → 先 [`DestIo::prepare_overwrite`] 清障，再按既有语义对账
///   （有匹配记录则 Resume/Skip-as-recorded，无记录/陈旧则 Full）；
/// - 未命中 → 与 [`reconcile`] 逐条完全一致。
///
/// 两表皆空时与 reconcile 行为等价（`recv_files`/`recv_files_fault` 的
/// 空 ctl 转发因此零行为差）。
pub fn reconcile_with_policy(
    dest: &dyn DestIo,
    man: &Manifest,
    policy: &TransferPolicy,
) -> Vec<FilePlan> {
    man.files
        .iter()
        .map(|e| {
            if policy_hits(&policy.skip, &e.rel_path) {
                FilePlan::Skip
            } else {
                if policy_hits(&policy.overwrite, &e.rel_path) {
                    dest.prepare_overwrite(&e.rel_path);
                }
                plan_for(dest, e)
            }
        })
        .collect()
}

/// 该路径若存在且为常规文件则移除（目录/符号链接/不存在：不动）。
fn remove_file_placeholder(p: &Path) {
    let is_file = std::fs::symlink_metadata(p)
        .map(|m| m.is_file())
        .unwrap_or(false);
    if is_file {
        if let Err(e) = std::fs::remove_file(p) {
            tracing::warn!(
                "overwrite clear failed at {}: {e} (rename safety net applies)",
                p.display()
            );
        }
    }
}

/// 读取并校验一条记录。读失败 / JSON 畸形 / 字段不自洽（rel 不符、
/// 位图长度不对、哈希不是 32 字节 hex）→ None，绝不 panic。
/// 载入时把 chunk_count 之外的填充位清零（canonical 化）：磁盘损坏的
/// 填充位若不掩掉，「已收」计数会虚高，missing_count 的 usize 减法
/// 将下溢回绕——这是所有位图进入内存的唯一入口，在此归一即可。
fn load_record(records_dir: &Path, rel: &str) -> Option<FileRecord> {
    let bytes = std::fs::read(record_file(records_dir, rel)).ok()?;
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

/// 确保收件区目录存在;Windows 上补隐藏属性(FILE_ATTRIBUTE_HIDDEN)。
/// `pub(crate)`:本机复制/移动的暂存(`fs::copy_paths`/跨卷 move 回退)
/// 沿同一目录与同一套口径(隐藏属性/保留名单),键空间经 `loc-<uuid>`
/// 前缀与收件记录 `<sha256(rel)>` 天然隔离。
pub(crate) fn ensure_intake_dir(out_dir: &Path) -> Result<PathBuf> {
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

/// `.part` 续写器：持有续写句柄与内存中的当前记录，块写入与记录批量
/// 落盘都在此完成。`finalize` 经目的地（`DestIo::commit`）把校验过的
/// `.part` 原子落位。
pub struct PartWriter {
    /// 目的地（终件提交与防御性清理经它；open 时克隆自引擎侧共享句柄）。
    dest: Arc<dyn DestIo>,
    /// 记录目录（open 时经 `dest.records_dir()` 解析一次；记录读写与
    /// 记录目录 fsync 都用它）。
    records_dir: PathBuf,
    file: tokio::fs::File,
    rec: FileRecord,
    /// 距上次记录落盘的新块数。
    dirty: usize,
    last_flush: Instant,
}

impl PartWriter {
    /// 打开 `.part` 续写器（目的地经 `dest` 注入：记录读写沿其
    /// `records_dir`，续写句柄经其 `open_writer`——FsDestIo 在其中
    /// ensure 收件区、读写打开、`set_len` 预分配）。`resume=true` 载入
    /// 既有记录并续写（记录缺失或与 entry 不符即报错——调用方应先
    /// `reconcile` 得到 Resume 计划）；`resume=false` 截断重建：全零
    /// 位图 + 立即落一次记录。持有 `dest` 的克隆直至 `finalize`（提交
    /// 经 `DestIo::commit`）。
    pub async fn open(dest: Arc<dyn DestIo>, entry: &FileEntry, resume: bool) -> Result<Self> {
        let records_dir = dest.records_dir();
        let rec = if resume {
            let r = load_record(&records_dir, &entry.rel_path)
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
        // 续写句柄经目的地打开（FsDestIo：ensure 收件区 → 读写打开 →
        // set_len 预分配，错误文案沿既有点缀，见 FsDestIo::open_writer）。
        let file = dest.open_writer(entry, resume).await?;
        let mut w = Self {
            dest,
            records_dir,
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
        let tmp = record_tmp_file(&self.records_dir, &self.rec.rel);
        tokio::fs::write(&tmp, &json)
            .await
            .with_context(|| tmp.display().to_string())?;
        let dst = record_file(&self.records_dir, &self.rec.rel);
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
        let tmp = record_tmp_file(&self.records_dir, &self.rec.rel);
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
        let dst = record_file(&self.records_dir, &self.rec.rel);
        tokio::fs::rename(&tmp, &dst)
            .await
            .with_context(|| dst.display().to_string())?;
        sync_dir_logged(&self.records_dir);
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
    /// 提交（`DestIo::commit`：`.part` fsync → 独占终名 → 原子改名 →
    /// 两侧目录 fsync）→ 记 done + final_rel（commit 返回的冲突改名人）
    /// 持久化落盘 → 防御性清理 `.part`（提交后已不在场，`discard_partial`
    /// 尽力而为）。有序落盘（spec §3①）保证断电后不存在「记录已 done
    /// 而终件损坏」的状态：任一步未完成，记录都还是 partial，由后续传输
    /// 重收；提交失败同样记录保持 partial，由后续传输重收；占位文件自身
    /// 会被清理，不阻塞下次提交。返回最终文件路径（`Committed::path`）。
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

        // 2) 提交（目的地）：`.part` fsync → 独占终名 → 原子改名 → 两侧
        //    目录 fsync——断电最坏留下 partial 记录 + `.part` 重收，绝不
        //    留下「记录 done 而终件损坏」
        let committed = w.dest.commit(&w.rec.rel, &mut w.file).await?;

        // 3) 记 done + final_rel（冲突改名人），持久化落盘
        //    （tmp fsync + rename + 记录目录 fsync）
        w.rec.done = true;
        w.rec.final_rel = Some(committed.final_rel);
        w.flush_record_durable().await?;

        // 4) `.part` 已随提交转移；防御性清理，忽略 NotFound
        w.dest.discard_partial(&w.rec.rel);
        Ok(committed.path)
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
            // Windows:create_new 撞**既有目录**报 AccessDenied(而非
            // AlreadyExists)——候选名上有目录就是同名冲突,换下一候选
            // (网络收件 rel 撞接收方既有目录、本机复制目录撞目录同源)
            Err(_e)
                if cand
                    .symlink_metadata()
                    .map(|m| m.is_dir())
                    .unwrap_or(false) =>
            {
                continue
            }
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
        let mut w = PartWriter::open(Arc::new(FsDestIo::new(out)), &entry, false).await.unwrap();
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
                format!("sync-file {}", record_tmp_file(&intake, "a.bin").display()),
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

    /// 半程残留速构：open（立即落初始记录）+ 写一块后丢弃（不 finalize）
    /// ——记录与 `.part` 双双在场，正是中断传输留下的盘面形态。
    async fn leave_partial(out: &Path, rel: &str) {
        let (entry, data) = one_chunk(rel, 4096);
        let mut w = PartWriter::open(Arc::new(FsDestIo::new(out)), &entry, false).await.unwrap();
        w.write_chunk(0, 0, &data).await.unwrap();
        drop(w);
    }

    /// 身份吻合即双删（spec §5.3）：记录 + `.part` 都删、计数 1；另一 rel
    /// 的残留不受牵连（逐 rel 定位，不做目录级清扫）。
    #[tokio::test]
    async fn cleanup_rels_removes_matching_identity() {
        let out = tempfile::tempdir().unwrap();
        leave_partial(out.path(), "docs/a.txt").await;
        leave_partial(out.path(), "docs/b.txt").await;
        assert!(record_path(out.path(), "docs/a.txt").exists());
        assert!(part_path(out.path(), "docs/a.txt").exists());
        assert!(record_path(out.path(), "docs/b.txt").exists());
        assert!(part_path(out.path(), "docs/b.txt").exists());

        let cleaned = cleanup_rels(&FsDestIo::new(out.path()), &["docs/a.txt".into()]);
        assert_eq!(cleaned, 1, "命中恰一条");
        assert!(
            !record_path(out.path(), "docs/a.txt").exists(),
            "记录必须删除"
        );
        assert!(
            !part_path(out.path(), "docs/a.txt").exists(),
            ".part 必须删除"
        );
        // 旁证：未被请求的 rel 残留原样
        assert!(record_path(out.path(), "docs/b.txt").exists());
        assert!(part_path(out.path(), "docs/b.txt").exists());
    }

    /// 幂等与身份终审（spec §5.3）：二清已清 = 0（缺席静默）；同键位上
    /// 记录 `rel` 字段与请求不符 → 绝不删（键位命中不够，身份是删与不删
    /// 的唯一依据——防哈希碰撞/键复用误删他人续传素材）。
    #[tokio::test]
    async fn cleanup_rels_idempotent_and_identity_checked() {
        let out = tempfile::tempdir().unwrap();
        leave_partial(out.path(), "a.bin").await;

        // 首清命中；二清（记录已不在）幂等 0
        assert_eq!(cleanup_rels(&FsDestIo::new(out.path()), &["a.bin".into()]), 1);
        assert_eq!(cleanup_rels(&FsDestIo::new(out.path()), &["a.bin".into()]), 0, "二清幂等");

        // 身份终审：sha256("c.bin") 键位上放一份 rel 字段为别的 rel 的
        // 记录（键复用/撞键形态），请求 c.bin 不得删除任何一件
        let rec_p = out
            .path()
            .join(INTAKE_DIR)
            .join(format!("{}.json", record_key("c.bin")));
        let part_p = out
            .path()
            .join(INTAKE_DIR)
            .join(format!("{}.part", record_key("c.bin")));
        let tampered = serde_json::json!({
            "rel": "elsewhere/other.bin", // 身份不符
            "size": 1,
            "sha256": format!("{:064}", 0),
            "chunk_count": 1,
            "bitmap": "00",
            "done": false,
            "final_rel": null,
        });
        std::fs::create_dir_all(out.path().join(INTAKE_DIR)).unwrap();
        std::fs::write(&rec_p, serde_json::to_vec(&tampered).unwrap()).unwrap();
        std::fs::write(&part_p, b"junk").unwrap();

        assert_eq!(cleanup_rels(&FsDestIo::new(out.path()), &["c.bin".into()]), 0, "身份不符不删");
        assert!(rec_p.exists(), "身份不符记录必须原样");
        assert!(part_p.exists(), "身份不符 .part 必须原样");

        // 从未传过的 rel：缺席静默，计数 0
        assert_eq!(cleanup_rels(&FsDestIo::new(out.path()), &["never/sent.bin".into()]), 0);
    }
}
