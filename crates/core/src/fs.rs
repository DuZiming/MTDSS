//! 远程文件浏览的磁盘侧服务(纯 std,无网络/QUIC)。
//!
//! daemon ctrl 通道(`transport::ctrl`)把这里的 `list_dir`/`stat_path`
//! 直接接到 `ListRequest`/`StatRequest` 上,UI 只见 `DirEntryData`。
//!
//! 路径策略(两个入口判定一致):
//! - `""` = 根卷列表:Windows 对 `A:\`..`Z:\` 逐一 `std::fs::metadata`
//!   试错探测(本地调用亚毫秒级),在位盘符各一条,任一探测失败视为
//!   不在位跳过(缺盘/锁盘不抬高整个列表为错误);Unix 恒为单条 `/`。
//! - 其余路径必须是绝对形态:Windows 以 `X:`(盘符)开头,长度>2 时第三
//!   字符必须是 `\`/`/`——盘符相对形态 `C:foo` 意为「C 盘当前目录下的
//!   foo」,语义随进程当前目录漂移,→ `InvalidPath`;Unix 以 `/`
//!   开头,否则 → `InvalidPath`(UNC `\\server\share` V1 不支持)。
//!   内嵌 NUL → `InvalidPath`。Windows 两字符裸盘符 `C:` 规范化为
//!   `C:\` 再入盘。
//!
//! 列表语义(钉死,V1):
//! - 保留名过滤:`.mtdss`(隐藏收件区,见 `transport::intake::INTAKE_DIR`)
//!   与 `.mtdss-part`(预留给 M3c 分卷传输)大小写不敏感匹配滤除
//!   (`eq_ignore_ascii_case`):Windows 大小写不敏感文件系统上的变体
//!   (如 `.MTDSS`)同样滤除,与 manifest 首段校验、daemon collect_items
//!   三处同口径;后缀形(如 `x.mtdss-part`)非保留名本体,不滤。
//! - 符号链接条目按 `entry.file_type()` 不跟随:目录链接显示为文件
//!   (V1 简化,不提供穿越目录链接的浏览);`stat_path` 用
//!   `symlink_metadata` 同判定,保证列表/单查对同一路径同形态。
//! - mtime 取 `metadata().modified()`,不可得(含早于 Unix 纪元)置 0;
//!   目录 size 恒 0;条目自身 metadata 拿不到时 size/mtime 兜底 (0, 0)。
//! - 名称经 `to_string_lossy`(非 UTF-8 名 V1 有损显示,其 `path` 字段
//!   同损、再入可能 `NOT_FOUND`——不 panic 即 V1 底线)。
//! - 排序:目录优先,组内名称字节升序(Rust `str` 序即 UTF-8 字节序;
//!   混排大小写名称保持字节序排列)。
//! - 超过 `LIST_ENTRY_CAP` 条即截断,`ListOut.truncated = true`;截断
//!   保留排序后的前 cap 条(确定子集,与 UI 所见一致)。
//!
//! io 错误按 `ErrorKind` 映射:`NotFound`/`PermissionDenied` 对应同名
//! `FsError`,其余一律 `Internal`(保留 io 消息便于排查)。
//!
//! 操作面(M3c,Task 4/7 经 ctrl/jobs 消费):
//! - `delete_paths`:回收优先(trash crate;Windows=IFileOperation+ALLOWUNDO,
//!   Unix=freedesktop/macOS Trash),`force=true` 永久删。回收**设施级**
//!   不可用(`io::ErrorKind::Unsupported`,生产由 `recycle_io` 从
//!   `trash::Error::Unknown` 映射)→ 批级 `RecycleUnavailable` 上抛——
//!   操作端据此整体确认后 force 重发;目标级失败(被锁/权限/缺失)逐条
//!   进失败表,不半途弃批。android(P3):trash 无 android 平台实现,亦无
//!   回收设施,恒按永久删处理(等价 force=true,recycled 恒 false;签名
//!   不变——安卓 v1 无删除 UI,本函数纯引擎侧 plumbing,见设计 §4.1)。
//! - `move_entries`:rename 直通;跨卷(rename 报 EXDEV/ERROR_NOT_SAME_DEVICE)
//!   回退「目标目录 `.mtdss` 暂存复制 + 原子改名落位 + 源永久删除」,
//!   **对调用方透明**——成功即移动完成,源删除为永久(数据已在目的地)。
//! - `copy_paths`:整树拷入**目标目录** `.mtdss` 暂存(`loc-<uuid>.part`
//!   键,与网络收件区 `<sha256(rel)>` 键空间天然不撞)→ 原子改名落位,
//!   目标名不存在半成品可见窗口;同名走 `unique_target` 自动改名
//!   (`name (1)`…,冲突策略判定在调用方),暂存完成后落空。
//! - 冲突策略面(M4a-3,ctrl 同机 copy/move 消费):`copy_paths_with`/
//!   `move_entries_with` 接入 `TransferPolicy`(与传输面同口径的
//!   `policy_hits` 匹配)——skip 命中整项跳过、overwrite 命中清文件型
//!   占用原位落位(move 直通 rename)、未命中沿上述 legacy;空策略与
//!   旧签名函数逐字节同行为(旧签名即空策略委托)。
//! - 操作对象守卫(终审修复):delete/move/copy 的**用户操作对象**拒绝
//!   卷根(Windows `X:`/`X:\`、Unix `/`)与任意段命中保留名 `.mtdss`
//!   (大小写不敏感,祖先或自身)的路径——`rm dev:C:\ --permanent` 直捣
//!   活盘根是灾难级 foot-gun,`.mtdss` 是引擎内部面(收件/暂存,操作
//!   对象进入即破坏对账与隐藏语义)。逐条 `INVALID_PATH`(消息点名
//!   「拒绝以卷根/.mtdss 为操作对象」);引擎 INTAKE/暂存写入不经此判定
//!   (`copy_paths`/`move_entries` 内部的 `.mtdss` 暂存照常,守卫只挂
//!   用户提交的操作数)。
//!
//! 命名提示:本模块名 `fs` 与 `std::fs` 同名,模块内部一律写全路径
//! `std::fs::…`,避免自引用混淆。

/// 单次列表条目上限,超出即截断(ctrl 响应置 `truncated` 标志)。
pub const LIST_ENTRY_CAP: usize = 10_000;

use std::path::{Path, PathBuf};

/// 列表滤除的保留名(大小写不敏感匹配):`.mtdss` 为隐藏收件区,
/// `.mtdss-part` 预留给 M3c 分卷传输。与 `transport::intake::INTAKE_DIR`
/// 同源,防漂移;`.MTDSS` 等大小写变体同样滤除,与 manifest 首段校验、
/// daemon collect_items 三处同口径。
const RESERVED_NAMES: [&str; 2] = [
    crate::transport::intake::INTAKE_DIR,
    ".mtdss-part",
];

/// 单条目录项:显示名 + 可再入绝对路径 + 元数据。
///
/// `name` 无路径(根卷条目 Windows=`"C:"`、Unix=`"/"`);`path` 供后续
/// `list_dir`/`stat_path` 再入;`size` 目录恒 0;`mtime_ms` Unix 毫秒、
/// 不可得为 0;`is_dir` 不跟随符号链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryData {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
    pub is_dir: bool,
}

/// 列表结果:`entries` 已排序(目录优先、组内字节升序);`truncated`
/// 表示目录在 `LIST_ENTRY_CAP`(或注入 cap)之外还有更多条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOut {
    pub entries: Vec<DirEntryData>,
    pub truncated: bool,
}

/// FS 服务错误;`code()` 给 ctrl 通道透传用的稳定错误码。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FsError {
    #[error("路径不存在: {0}")]
    NotFound(String),
    #[error("无权限访问: {0}")]
    PermissionDenied(String),
    #[error("非法路径: {0}")]
    InvalidPath(String),
    #[error("{0}")]
    Internal(String),
    #[error("回收站不可用: {0}")]
    RecycleUnavailable(String),
}

impl FsError {
    /// ctrl `CtrlError.code` 的取值(NOT_FOUND/PERMISSION_DENIED/
    /// INVALID_PATH/INTERNAL/RECYCLE_UNAVAILABLE),与 proto 约定一字不差;
    /// `RECYCLE_UNAVAILABLE` 是操作端「确认后 force 重发」升级判定的依据。
    pub fn code(&self) -> &'static str {
        match self {
            FsError::NotFound(_) => "NOT_FOUND",
            FsError::PermissionDenied(_) => "PERMISSION_DENIED",
            FsError::InvalidPath(_) => "INVALID_PATH",
            FsError::Internal(_) => "INTERNAL",
            FsError::RecycleUnavailable(_) => "RECYCLE_UNAVAILABLE",
        }
    }
}

/// 列出一个目录(`""` = 根卷列表)。路径要求与语义见模块文档。
pub fn list_dir(path: &str) -> Result<ListOut, FsError> {
    list_dir_capped(path, LIST_ENTRY_CAP)
}

/// 与 `list_dir` 同逻辑但 cap 可注入,截断测试面。`#[doc(hidden)]` 不入
/// 文档;生产路径一律走 `list_dir`(cap = `LIST_ENTRY_CAP`)。
#[doc(hidden)]
pub fn list_dir_capped(path: &str, cap: usize) -> Result<ListOut, FsError> {
    if path.is_empty() {
        return root_listing(cap);
    }
    let path = normalize(path)?;
    let mut entries: Vec<DirEntryData> = Vec::new();
    for entry in std::fs::read_dir(&path).map_err(|e| map_io(e, &path))? {
        let entry = entry.map_err(|e| map_io(e, &path))?;
        let raw = entry.file_name();
        let name: String = raw.to_string_lossy().into_owned();
        if RESERVED_NAMES.iter().any(|r| name.eq_ignore_ascii_case(r)) {
            continue;
        }
        entries.push(entry_data(&entry, &path));
    }
    // 先全量收集并排序,再截断:cap 保留排序后的前 cap 条(确定子集,
    // UI 显示有序),而非 OS 读取顺序的任意切片。恰等于 cap 不算截断。
    sort_entries(&mut entries);
    let mut truncated = false;
    if entries.len() > cap {
        entries.truncate(cap);
        truncated = true;
    }
    Ok(ListOut { entries, truncated })
}

/// 单条路径查询,判定与 `list_dir` 一致(`""` → `InvalidPath`;盘根如
/// `C:\` 合法;符号链接不跟随)。
pub fn stat_path(path: &str) -> Result<DirEntryData, FsError> {
    if path.is_empty() {
        return Err(FsError::InvalidPath(path.to_string()));
    }
    let path = normalize(path)?;
    // symlink_metadata 与列表的 entry.metadata()/file_type() 同为
    // "不跟随链接"语义,列表/单查对同一路径给出同一形态。
    let md = std::fs::symlink_metadata(&path).map_err(|e| map_io(e, &path))?;
    let is_dir = md.is_dir();
    Ok(DirEntryData {
        name: display_name(&path),
        path: path.clone(),
        size: if is_dir { 0 } else { md.len() },
        mtime_ms: modified_ms(&md),
        is_dir,
    })
}

// ---------- 操作面:删除 / 移动 / 本地复制(M3c) ----------

/// 单条路径操作的失败记录:`code` 为稳定错误码(与 `FsError::code()` 同
/// 表,ctrl 透传),`message` 保留 io/判定原文便于排查;逐条上报、批不
/// 半途弃批(设施级 `RecycleUnavailable` 是唯一的批级中止)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathOpFailure {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// 操作对象守卫(终审修复):delete/move/copy 的用户操作对象拒绝卷根与
/// `.mtdss` 保留段。卷根判定吃裸盘符(`C:`)/带分隔符(`C:\`、`C:/` 及
/// 冗余尾分隔符)全形态;`.mtdss` 按**段**精确匹配(大小写不敏感,与
/// `RESERVED_NAMES`/manifest 首段校验同口径;`x.mtdss-part` 后缀名不
/// 误伤)——祖先或自身段命中即拒。只挂三个用户 fs 操作入口,先于存在性
/// 预检(对象拒绝与目标在否无关);引擎 INTAKE/暂存写入不经此判定。
fn check_operand(path: &str) -> Result<(), FsError> {
    let trimmed = path.trim_end_matches(['\\', '/']);
    #[cfg(windows)]
    let is_root = trimmed.len() == 2
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && trimmed.as_bytes()[1] == b':';
    #[cfg(unix)]
    let is_root = trimmed.is_empty();
    if is_root {
        return Err(FsError::InvalidPath(format!("拒绝以卷根为操作对象: {path}")));
    }
    if path
        .split(['\\', '/'])
        .any(|seg| seg.eq_ignore_ascii_case(crate::transport::intake::INTAKE_DIR))
    {
        return Err(FsError::InvalidPath(format!(
            "拒绝以 .mtdss 保留段为操作对象: {path}"
        )));
    }
    Ok(())
}

/// 删除一批绝对路径(文件或目录整树)。`force=false` 先试回收(trash
/// crate;Windows=IFileOperation+ALLOWUNDO,Unix=freedesktop Trash),
/// 回收设施不可用 → `RecycleUnavailable`;`force=true` 永久删除。逐条
/// 执行,某条失败只记失败表不弃批。返回 (成功数, 是否走了回收, 逐条
/// 失败表)。存在性先经 `symlink_metadata` 预检(不跟随,与列表同口径),
/// 缺失逐条 `NOT_FOUND`,失败表确定、不依赖回收/删除调用的报错形态。
/// android 例外:无回收设施,恒永久删(等价 force=true;见模块文档)。
pub fn delete_paths(
    paths: &[String],
    force: bool,
) -> Result<(usize, bool, Vec<PathOpFailure>), FsError> {
    #[cfg(target_os = "android")]
    {
        // android(P3,设计 §4.1):trash crate 无 android 平台实现,系统
        // 亦无回收设施——force 一律按 true 走永久删,recycled 恒 false。
        // 安卓 v1 不给删除 UI(cut 恒 false,评审裁定④),本函数是引擎侧
        // plumbing(ctrl delete / jobs cut 收尾等既有调用面),签名不变。
        // 回收闭包在 force=true 下永不触达,unreachable 即语义哨兵。
        let _ = force; // force 无消费面(恒永久删),android 编译单元显式弃用
        delete_paths_inner(paths, true, |_p| -> std::io::Result<()> {
            unreachable!("android 无回收设施,force=true 下回收闭包不可达")
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        delete_paths_inner(paths, force, recycle_io)
    }
}

/// `delete_paths` 的注入面:recycle 替身 `trash::delete`,供「无回收」
/// 分支单测(真实不可回收环境难以搭建;生产传 `recycle_io`)。
/// `#[doc(hidden)]` 不入文档。
#[doc(hidden)]
pub fn delete_paths_inner(
    paths: &[String],
    force: bool,
    recycle: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<(usize, bool, Vec<PathOpFailure>), FsError> {
    let mut deleted = 0usize;
    let mut recycled = false;
    let mut failed: Vec<PathOpFailure> = Vec::new();
    for p in paths {
        // 操作对象守卫(终审修复):卷根/.mtdss 逐条拒,批不弃废
        if let Err(e) = check_operand(p) {
            failed.push(failure_from_fs(e, p));
            continue;
        }
        let norm = match normalize(p) {
            Ok(n) => n,
            Err(_) => {
                failed.push(invalid_failure(p));
                continue;
            }
        };
        let md = match std::fs::symlink_metadata(&norm) {
            Ok(md) => md,
            Err(e) => {
                failed.push(failure_from_io(e, &norm));
                continue;
            }
        };
        if force {
            // 永久删除:目录整树删,其余 unlink
            let r = if md.is_dir() {
                std::fs::remove_dir_all(&norm)
            } else {
                std::fs::remove_file(&norm)
            };
            match r {
                Ok(()) => deleted += 1,
                Err(e) => failed.push(failure_from_io(e, &norm)),
            }
        } else {
            match recycle(Path::new(&norm)) {
                Ok(()) => {
                    deleted += 1;
                    recycled = true;
                }
                // 设施级不可用:整批中止上抛(操作端确认后 force 重发全量;
                // 之前已记的逐条失败随之丢弃——批级失败本就要求整批重发)
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                    return Err(FsError::RecycleUnavailable(e.to_string()));
                }
                Err(e) => failed.push(failure_from_io(e, &norm)),
            }
        }
    }
    Ok((deleted, recycled, failed))
}

/// 生产回收调用:`trash::delete` 包成 io 错误面,kind 承载批级/逐条分流
/// 信号(见 `delete_paths_inner` 的 `Unsupported` 臂):
/// - `Unknown` = 设施级(freedesktop「无 home trash 也无可用挂载点回收区」、
///   环境变量缺失;Windows 端 IFileOperation 几乎恒可用,个别误入 Unknown
///   也安全——批级上抛只会让操作端走确认-force 重发,错误在重发中以逐条
///   形态再现,无数据风险);
/// - `FileSystem` = 目标卷 IO(被锁/权限),沿用 source 的 kind,逐条;
/// - `CouldNotAccess` = 目标不可达(存在性已预检,落到这里即权限类);
/// - 其余(TargetedRoot/CanonicalizePath 等边缘)→ `Other`,逐条 INTERNAL。
///
/// android 不编译(trash 依赖已随目标剔除,见 `delete_paths`)。
#[cfg(not(target_os = "android"))]
fn recycle_io(path: &Path) -> std::io::Result<()> {
    trash::delete(path).map_err(|e| {
        use trash::Error as TE;
        let msg = e.to_string();
        let kind = match &e {
            TE::Unknown { .. } => std::io::ErrorKind::Unsupported,
            // freedesktop 后端专属变体:目标卷 IO(被锁/权限)沿 source
            // kind 保留为逐条失败
            #[cfg(all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            ))]
            TE::FileSystem { source, .. } => source.kind(),
            TE::CouldNotAccess { .. } => std::io::ErrorKind::PermissionDenied,
            _ => std::io::ErrorKind::Other,
        };
        std::io::Error::new(kind, msg)
    })
}

/// 同设备移动/改名:逐对 rename(文件目标覆盖既有文件、目录撞既有名
/// 报错,与 rename 原生语义一致,冲突策略判定在调用方);跨卷回退
/// 「暂存复制 + 源永久删除」(对调用方透明,见模块文档)。逐对执行,
/// 失败只记表不弃批。返回 (成功数, 逐条失败表)。冲突策略面见
/// [`move_entries_with`](空策略时两者同行为)。
pub fn move_entries(pairs: &[(String, String)]) -> Result<(usize, Vec<PathOpFailure>), FsError> {
    move_entries_policy(
        pairs,
        &crate::transport::intake::TransferPolicy::default(),
        |f, t| std::fs::rename(f, t),
    )
    .map(|out| (out.moved.len(), out.failed))
}

/// 策略版同设备移动(与 [`move_entries`] 同链路,冲突判定接入
/// `TransferPolicy`,与传输面同口径):skip 命中 → 整对跳过(源与目标
/// 全不动,判定键 = 发起端原样 `to` 串);overwrite 命中 → rename 直通
/// (既有文件覆盖、目录撞名逐对失败,与原生语义一致);未命中 → legacy。
/// 空策略时与 [`move_entries`] 同行为。
pub fn move_entries_with(
    pairs: &[(PathBuf, PathBuf)],
    policy: &crate::transport::intake::TransferPolicy,
) -> Result<MoveOutcome, FsError> {
    let conv: Vec<(String, String)> = pairs
        .iter()
        .map(|(f, t)| {
            (
                f.to_string_lossy().into_owned(),
                t.to_string_lossy().into_owned(),
            )
        })
        .collect();
    move_entries_policy(&conv, policy, |f, t| std::fs::rename(f, t))
}

/// `move_entries` 的注入面:rename 替身供跨卷回退单测(真实第二卷/跨
/// tmpfs 挂载在测试环境不可行;注入报 EXDEV 的 rename 即可驱动回退全
/// 链路,真实跨卷的失败形态正是 `CrossesDevices`/EXDEV)。生产传
/// `std::fs::rename`。`#[doc(hidden)]` 不入文档。
#[doc(hidden)]
pub fn move_entries_inner(
    pairs: &[(String, String)],
    rename_impl: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(usize, Vec<PathOpFailure>), FsError> {
    move_entries_policy(
        pairs,
        &crate::transport::intake::TransferPolicy::default(),
        rename_impl,
    )
    .map(|out| (out.moved.len(), out.failed))
}

/// [`move_entries`]/[`move_entries_with`]/[`move_entries_inner`] 共同实现
/// (policy 为空即逐字节 legacy)。
fn move_entries_policy(
    pairs: &[(String, String)],
    policy: &crate::transport::intake::TransferPolicy,
    rename_impl: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<MoveOutcome, FsError> {
    let mut out = MoveOutcome {
        moved: Vec::with_capacity(pairs.len()),
        failed: Vec::new(),
        skipped: Vec::new(),
    };
    for (from, to) in pairs {
        // skip 判定先于一切守卫/触盘:命中即整对跳过,源与目标全不动
        // (与传输面「磁盘全不动」同口径);判定键 = 发起端原样 to 串
        if crate::transport::intake::policy_hits(&policy.skip, to) {
            out.skipped.push(to.clone());
            continue;
        }
        match move_one(from, to, &rename_impl) {
            Ok(()) => out.moved.push(to.clone()),
            Err(f) => out.failed.push(f),
        }
    }
    Ok(out)
}

fn move_one(
    from: &str,
    to: &str,
    rename_impl: &impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), PathOpFailure> {
    // 操作对象守卫(终审修复):卷根/.mtdss 的 from 与 to 都拒,先于
    // 存在性预检(失败按对应操作数路径上报)
    check_operand(from).map_err(|e| failure_from_fs(e, from))?;
    check_operand(to).map_err(|e| failure_from_fs(e, to))?;
    let f = normalize(from).map_err(|_| invalid_failure(from))?;
    let t = normalize(to).map_err(|_| invalid_failure(to))?;
    let fp = Path::new(&f);
    let md = std::fs::symlink_metadata(fp).map_err(|e| failure_from_io(e, &f))?;
    match rename_impl(fp, Path::new(&t)) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            move_cross_volume(fp, Path::new(&t), md.is_dir()).map_err(|e| failure_from_fs(e, &f))
        }
        Err(e) => Err(failure_from_io(e, &f)),
    }
}

/// 跨卷回退:暂存复制(目标父目录 `.mtdss`,原子改名落位,与 `copy_paths`
/// 同一套)→ 源永久删除。落位失败清掉半截暂存、源保持原状(整对失败,
/// 不产生副本);源删除失败(复制落位已成功)也整对报错——双份并存、
/// 无丢失,重试时 rename 覆盖语义可无损完成。失败一律按源路径上报。
fn move_cross_volume(src: &Path, dst: &Path, is_dir: bool) -> Result<(), FsError> {
    let src_str = src.display().to_string();
    let dst_parent = dst
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| FsError::Internal(format!("目标无父目录: {}", dst.display())))?;
    let staged = stage_copy(src, dst_parent, is_dir)?;
    if let Err(e) = std::fs::rename(&staged, dst) {
        discard_staged(&staged, is_dir);
        return Err(map_io(e, &src_str));
    }
    // 源删除为永久(钉死):数据已在目的地
    let r = if is_dir {
        std::fs::remove_dir_all(src)
    } else {
        std::fs::remove_file(src)
    };
    r.map_err(|e| map_io(e, &src_str))
}

/// rename 的跨卷失败判定:`CrossesDevices` kind(Rust ≥1.83 稳定)加原始
/// OS 错误码双查(EXDEV=18 Unix / ERROR_NOT_SAME_DEVICE=17 Windows),
/// 双保险防 kind 映射在工具链间漂移。
fn is_cross_device(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::CrossesDevices {
        return true;
    }
    matches!(e.raw_os_error(), Some(17) | Some(18))
}

/// 本机复制(文件或整目录树):拷入**目标目录**的 `.mtdss` 暂存
/// (`loc-<uuid>.part` 键)→ 原子改名落位,目标名不存在半成品可见窗口;
/// 目标同名走 `unique_target` 自动改名(冲突策略判定在调用方,本层一律
/// 改名)。逐条顺序复制,任一条失败即整批 `Err`(已落盘条目保持,不
/// 回滚);完成后暂存落空。返回逐条落盘路径(与 sources 等序)。
/// 冲突策略面见 [`copy_paths_with`](空策略时两者逐字节同行为)。
pub fn copy_paths(sources: &[String], target_dir: &str) -> Result<Vec<String>, FsError> {
    Ok(
        copy_paths_policy(sources, target_dir, &crate::transport::intake::TransferPolicy::default())?
            .copied,
    )
}

/// 复制结果(策略面):`copied` = 实际落盘路径(成功项,自动改名时即
/// 终名);`skipped` = 策略 skip 命中而整项跳过的判定键;`failed` 恒空
/// (copy 沿 legacy「任一条失败整批 `Err`」语义,失败不进表——字段仅为
/// ctrl 响应面与 move 结果同形而设)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOutcome {
    pub copied: Vec<String>,
    pub failed: Vec<PathOpFailure>,
    pub skipped: Vec<String>,
}

/// 移动结果(策略面):`moved` = 成功对的目标路径;`skipped` = skip 命中
/// 而整对原样不动的 to;`failed` 沿 legacy 逐对失败表(批不弃废)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    pub moved: Vec<String>,
    pub failed: Vec<PathOpFailure>,
    pub skipped: Vec<String>,
}

/// 策略版本机复制(与 [`copy_paths`] 同链路,冲突判定接入
/// `TransferPolicy`,与传输面同口径;判定键见 [`policy_key`]):
/// - skip 命中 → 整项跳过(不落盘、不计 copied,计入 skipped);
/// - overwrite 命中 → 先清目标处**文件型**占用再原位落位;目录型占用
///   绝不清(不递归删用户目录),退化 `unique_target` 改名安全网;占用
///   清除失败(锁定等)同样退化改名;占用即源自身时不清(自复制刷新,
///   暂存后原子落回原名);
/// - 未命中 → legacy 自动改名 `name (1)`。
///
/// 空策略时与 [`copy_paths`] 逐字节同行为(逐条失败仍整批 `Err`)。
pub fn copy_paths_with(
    paths: &[PathBuf],
    dir: &Path,
    policy: &crate::transport::intake::TransferPolicy,
) -> Result<CopyOutcome, FsError> {
    let sources: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    copy_paths_policy(&sources, &dir.to_string_lossy(), policy)
}

/// copy 的策略判定键:目标目录 + 条目名。分隔符随目录形态(含 `\` 用
/// `\`,否则 `/`;目录自带尾分隔符不重复)——操作端(CLI/UI)在不知目
/// 标平台的前提下按同一规则拼 skip/overwrite 表,两端独立实现零漂移;
/// 匹配走 `policy_hits`(字节精确/段边界前缀,不归一大小写/分隔符)。
fn policy_key(dir: &str, name: &str) -> String {
    let sep = if dir.contains('\\') { '\\' } else { '/' };
    if dir.ends_with('\\') || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}{sep}{name}")
    }
}

/// [`copy_paths`]/[`copy_paths_with`] 共同实现(policy 为空即逐字节 legacy)。
fn copy_paths_policy(
    sources: &[String],
    target_dir: &str,
    policy: &crate::transport::intake::TransferPolicy,
) -> Result<CopyOutcome, FsError> {
    // 操作对象守卫(终审修复):目标目录与逐源都拒卷根/.mtdss(先于
    // 存在性/metadata 检查;内部 `.mtdss` 暂存不经此判定)
    check_operand(target_dir)?;
    let td = normalize(target_dir)?;
    let tdp = Path::new(&td);
    let md = std::fs::metadata(tdp).map_err(|e| map_io(e, &td))?;
    if !md.is_dir() {
        return Err(FsError::Internal(format!("{td}: 目标不是目录")));
    }
    let mut out = CopyOutcome {
        copied: Vec::with_capacity(sources.len()),
        failed: Vec::new(),
        skipped: Vec::new(),
    };
    for s in sources {
        check_operand(s)?;
        let src = normalize(s)?;
        let sp = Path::new(&src);
        let smd = std::fs::symlink_metadata(sp).map_err(|e| map_io(e, &src))?;
        // 复制名 = 源最后一段;卷根/无名形态不可复制
        let name = sp
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| FsError::InvalidPath(src.clone()))?;
        // 策略判定(键 = 目标目录形态拼名,见 policy_key):skip 命中整项
        // 跳过,不落盘不计 copied
        let key = policy_key(&td, &name);
        if crate::transport::intake::policy_hits(&policy.skip, &key) {
            out.skipped.push(key);
            continue;
        }
        // 终名:overwrite 命中清**文件型**占用后原位落位(目录/链接型
        // 占用绝不清——no clobber,退化 unique_target 冲突改名安全网;
        // 清除失败同样退化;占用即源自身时不清,暂存后原子落回原名)
        let final_path = if crate::transport::intake::policy_hits(&policy.overwrite, &key) {
            let natural = tdp.join(&name);
            // 终审 I-1:自身判定不能只走字节等值——NTFS 大小写不敏感,目标
            // 目录拼成大小写变体(源 `C:\Foo\a.txt` → 目标 `C:\foo`)时字节
            // 不等,占用解析到源自身 metadata,remove_file 自删源、暂存
            // NotFound 整批 Err,数据永久丢失。精确等值快路径优先,再退
            // ASCII 大小写不敏感等值(覆盖盘符/词内大小写漂移);同址一律
            // 视为自身,不清、走暂存后原位落回(occ=None 路线)。
            let occ = if sp == natural.as_path()
                || sp.to_string_lossy().eq_ignore_ascii_case(&natural.to_string_lossy())
            {
                None
            } else {
                std::fs::symlink_metadata(&natural).ok()
            };
            let land_natural = match occ {
                // 无占用(或占用即源自身):原位落位
                None => true,
                // 文件型占用:先清再落位;清除失败(锁定等)退化改名
                Some(md) => md.is_file() && std::fs::remove_file(&natural).is_ok(),
            };
            if land_natural {
                natural
            } else {
                crate::transport::intake::unique_target(tdp, &name)
                    .map_err(|e| FsError::Internal(format!("{td}/{name}: {e}")))?
            }
        } else {
            // 占位独占终名(unique_target 语义);占位是本调用刚建的空壳,
            // 后续失败一律清掉,不阻塞下次重试拿到同名
            crate::transport::intake::unique_target(tdp, &name)
                .map_err(|e| FsError::Internal(format!("{td}/{name}: {e}")))?
        };
        let is_dir = smd.is_dir();
        let staged = match stage_copy(sp, tdp, is_dir) {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_file(&final_path);
                return Err(e);
            }
        };
        if is_dir {
            // unique_target 留下的空文件占位会挡住目录改名,先移除
            // (V1 接受此微小竞窗,与占位独占的防并发窗口同级)
            let _ = std::fs::remove_file(&final_path);
        }
        match std::fs::rename(&staged, &final_path) {
            Ok(()) => out.copied.push(final_path.to_string_lossy().into_owned()),
            Err(e) => {
                discard_staged(&staged, is_dir);
                if !is_dir {
                    let _ = std::fs::remove_file(&final_path);
                }
                return Err(map_io(e, &final_path.display().to_string()));
            }
        }
    }
    Ok(out)
}

/// 把 src(文件或整目录树)拷入 dest_parent 的 `.mtdss` 暂存,返回暂存
/// 路径:键 `loc-<uuid>.part`(与收件区 `<sha256(rel)>` 键空间天然不撞,
/// 残键也不会被任何对账/查找路径命中);目录源在暂存键名上建整树,文件
/// 源直拷为该键。暂存目录沿 `intake::ensure_intake_dir`(隐藏属性/保留
/// 名单同一口径)。中途失败即清理半截暂存再上抛。
fn stage_copy(
    src: &Path,
    dest_parent: &Path,
    is_dir: bool,
) -> Result<std::path::PathBuf, FsError> {
    let intake = crate::transport::intake::ensure_intake_dir(dest_parent)
        .map_err(|e| FsError::Internal(e.to_string()))?;
    let staged = intake.join(format!("loc-{}.part", uuid_v4()));
    let r = if is_dir {
        copy_tree(src, &staged)
    } else {
        std::fs::copy(src, &staged).map(|_| ())
    };
    if let Err(e) = r {
        discard_staged(&staged, is_dir);
        return Err(map_io(e, &src.display().to_string()));
    }
    Ok(staged)
}

/// 整目录树拷贝:顶层不跟随符号链接(`symlink_metadata` 判定,与列表
/// 同口径);内部条目按 read_dir 逐项递归,文件 `fs::copy`(沿源内容,
/// Unix 沿权限)。本机复制无落盘序 fsync 诉求(非断点续传面:断电即
/// 整树半成品留在暂存、目标名无痕),与 Explorer 同级耐久度。
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// 清理半截暂存(尽力而为):目录整树删、文件 unlink。失败忽略——
/// `loc-<uuid>` 键不会被任何查找路径命中,仅占磁盘。
fn discard_staged(staged: &Path, is_dir: bool) {
    let r = if is_dir {
        std::fs::remove_dir_all(staged)
    } else {
        std::fs::remove_file(staged)
    };
    let _ = r;
}

/// UUID v4 形态随机串:16 随机字节置版本/变位后按 8-4-4-4-12 hex 展开,
/// 作暂存键唯一性来源(rand 已在依赖面,不为此引 uuid 依赖)。
fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// 形态不合法路径的逐条失败记录(与 `FsError::InvalidPath` 同义)。
fn invalid_failure(path: &str) -> PathOpFailure {
    PathOpFailure {
        path: path.to_string(),
        code: "INVALID_PATH".to_string(),
        message: "非法路径".to_string(),
    }
}

/// io 错误 → 逐条失败记录:code 与 `FsError::code()` 同表,message 保留
/// io 原文。
fn failure_from_io(err: std::io::Error, path: &str) -> PathOpFailure {
    let code = match err.kind() {
        std::io::ErrorKind::NotFound => "NOT_FOUND",
        std::io::ErrorKind::PermissionDenied => "PERMISSION_DENIED",
        _ => "INTERNAL",
    };
    PathOpFailure {
        path: path.to_string(),
        code: code.to_string(),
        message: err.to_string(),
    }
}

/// `FsError` → 逐条失败记录(code 直取 `FsError::code()`,保留分流后的
/// 精确 kind,不再降级 INTERNAL)。
fn failure_from_fs(err: FsError, path: &str) -> PathOpFailure {
    PathOpFailure {
        path: path.to_string(),
        code: err.code().to_string(),
        message: err.to_string(),
    }
}

// ---------- 内部实现 ----------

/// 入口统一路径校验:内嵌 NUL、非绝对形态 → `InvalidPath`;Windows 裸
/// 盘符 `C:` 规范化为 `C:\`。返回规范化后的 owned 路径。`pub`:ctrl
/// cleanup_outbox 响应臂与 list/stat 同口径校验目标目录,规范化值直接
/// 用于 `.mtdss` 拼接(杜绝 `C:` + join 的盘符相对歧义)。
pub fn normalize(path: &str) -> Result<String, FsError> {
    if path.contains('\0') {
        return Err(FsError::InvalidPath(path.to_string()));
    }
    if !absolute_form(path) {
        return Err(FsError::InvalidPath(path.to_string()));
    }
    #[cfg(windows)]
    if path.len() == 2 {
        return Ok(format!("{path}\\"));
    }
    Ok(path.to_string())
}

/// 绝对形态判定:Windows = `X:` 前缀(盘符;UNC 不支持),长度>2 时第三
/// 字符必须是 `\`/`/`——`C:foo` 是「C 盘当前目录下的 foo」盘符相对形态,
/// 语义随进程当前目录漂移,拒不接受。Unix = `/` 前缀。
#[cfg(windows)]
fn absolute_form(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || b[2] == b'\\' || b[2] == b'/')
}

#[cfg(unix)]
fn absolute_form(path: &str) -> bool {
    path.starts_with('/')
}

/// 根卷列表:`""` 的语义(见模块文档)。
fn root_listing(cap: usize) -> Result<ListOut, FsError> {
    #[cfg(windows)]
    {
        let mut entries: Vec<DirEntryData> = Vec::new();
        let mut truncated = false;
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            // 试错式探测:任何 io 错误都视为"不在位"跳过
            let Ok(md) = std::fs::metadata(&root) else {
                continue;
            };
            if entries.len() >= cap {
                truncated = true;
                break;
            }
            entries.push(DirEntryData {
                name: format!("{}:", letter as char),
                path: root,
                size: 0,
                mtime_ms: modified_ms(&md),
                is_dir: true,
            });
        }
        Ok(ListOut { entries, truncated })
    }
    #[cfg(unix)]
    {
        let _ = cap;
        Ok(ListOut {
            entries: vec![DirEntryData {
                name: "/".to_string(),
                path: "/".to_string(),
                size: 0,
                mtime_ms: 0,
                is_dir: true,
            }],
            truncated: false,
        })
    }
}

/// read_dir 条目 → DirEntryData:名称 lossy;`path` = 父目录 join 原始
/// 文件名(可再入的绝对路径);`file_type()` 不跟随;条目 metadata 同
/// 不跟随,拿不到时整条 (0, 0) 兜底(与 mtime 失败置 0 同哲学)。
fn entry_data(entry: &std::fs::DirEntry, parent: &str) -> DirEntryData {
    let raw = entry.file_name();
    let name = raw.to_string_lossy().into_owned();
    let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
    let (size, mtime_ms) = match entry.metadata() {
        Ok(md) => (if is_dir { 0 } else { md.len() }, modified_ms(&md)),
        Err(_) => (0, 0),
    };
    let path = std::path::Path::new(parent).join(&raw);
    DirEntryData {
        name,
        path: path.to_string_lossy().into_owned(),
        size,
        mtime_ms,
        is_dir,
    }
}

/// 排序:目录优先,组内名称字节升序。
fn sort_entries(entries: &mut [DirEntryData]) {
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.as_bytes().cmp(b.name.as_bytes()),
    });
}

/// 显示名 = 路径最后一段;盘根/根目录无最后一段,回退:Windows 盘根
/// `C:\` → `C:`(与根卷列表条目同名),Unix `/` → `/`。
fn display_name(path: &str) -> String {
    if let Some(name) = std::path::Path::new(path).file_name() {
        return name.to_string_lossy().into_owned();
    }
    #[cfg(windows)]
    if path.len() == 3 && path.as_bytes()[1] == b':' {
        return path[..2].to_string();
    }
    path.to_string()
}

/// `Metadata::modified()` → Unix 毫秒;不可得(含早于 Unix 纪元)→ 0。
fn modified_ms(md: &std::fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// io 错误 → FsError:按 `ErrorKind` 分派,未列出的归 `Internal`。
fn map_io(err: std::io::Error, path: &str) -> FsError {
    match err.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound(path.to_string()),
        std::io::ErrorKind::PermissionDenied => FsError::PermissionDenied(path.to_string()),
        _ => FsError::Internal(format!("{path}: {err}")),
    }
}

// ---------- 冲突策略面单测(M4a-3;语义表钉死,先于实现) ----------

#[cfg(test)]
mod policy_tests {
    use super::*;
    use crate::transport::intake::TransferPolicy;

    fn pol(skip: &[String], overwrite: &[String]) -> TransferPolicy {
        TransferPolicy {
            skip: skip.to_vec(),
            overwrite: overwrite.to_vec(),
        }
    }

    /// 三态各一:skip 命中不出现在目标(既有内容原样)、overwrite 命中
    /// 内容翻新无 ` (1)`、未命中自动改名(legacy);空策略路径与旧签名
    /// `copy_paths` 逐字节同行为(委托面)。
    #[test]
    fn copy_policy_skip_overwrite_legacy() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a.txt"), b"new-a").unwrap();
        std::fs::write(src.path().join("b.txt"), b"new-b").unwrap();
        std::fs::write(src.path().join("c.txt"), b"new-c").unwrap();
        std::fs::write(dst.path().join("a.txt"), b"old-a").unwrap();
        std::fs::write(dst.path().join("b.txt"), b"old-b").unwrap();

        let td = dst.path().to_string_lossy().into_owned();
        let policy = pol(
            &[policy_key(&td, "a.txt")],
            &[policy_key(&td, "b.txt")],
        );
        let sources = [
            src.path().join("a.txt"),
            src.path().join("b.txt"),
            src.path().join("c.txt"),
        ];
        let out = copy_paths_with(&sources, dst.path(), &policy).unwrap();
        assert_eq!(out.copied.len(), 2, "skip 命中不计 copied: {out:?}");
        assert_eq!(out.skipped, vec![policy_key(&td, "a.txt")], "跳过项按判定键回带: {out:?}");
        assert!(out.failed.is_empty(), "copy 沿 legacy 整批 Err,无失败表: {out:?}");

        // skip 命中:目标原样不动
        assert_eq!(std::fs::read(dst.path().join("a.txt")).unwrap(), b"old-a");
        // overwrite 命中:内容翻新、原位落位无 ` (1)`
        assert_eq!(std::fs::read(dst.path().join("b.txt")).unwrap(), b"new-b");
        assert!(!dst.path().join("b (1).txt").exists(), "覆盖落位不得触发改名");
        // 未命中:legacy 自动改名
        assert_eq!(std::fs::read(dst.path().join("c.txt")).unwrap(), b"new-c");

        // 空策略(legacy 委托):再次复制同名 → ` (1)` 改名
        let out2 =
            copy_paths_with(&[src.path().join("a.txt")], dst.path(), &TransferPolicy::default())
                .unwrap();
        assert!(
            out2.copied[0].ends_with("a (1).txt"),
            "空策略必须走 legacy 自动改名: {out2:?}"
        );
        // 旧签名委托面不破(与空策略同行为)
        let landed = copy_paths(
            &[src.path().join("a.txt").to_string_lossy().into_owned()],
            &td,
        )
        .unwrap();
        assert!(landed[0].ends_with("a (2).txt"), "旧签名保持 legacy: {landed:?}");
    }

    /// move 的 skip 命中:整对跳过、源仍在、目标原样;未命中沿 legacy
    /// rename 直通(既有文件覆盖);overwrite 命中同 rename 直通。
    #[test]
    fn move_policy_skip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s.txt"), b"src").unwrap();
        std::fs::write(dir.path().join("t.txt"), b"tgt").unwrap();

        // skip 命中:整对跳过,源与目标全不动
        let policy = pol(&[dir.path().join("t.txt").to_string_lossy().into_owned()], &[]);
        let out =
            move_entries_with(&[(dir.path().join("s.txt"), dir.path().join("t.txt"))], &policy)
                .unwrap();
        assert!(out.moved.is_empty(), "skip 命中对不计 moved: {out:?}");
        assert_eq!(
            out.skipped,
            vec![dir.path().join("t.txt").to_string_lossy().into_owned()],
            "跳过项按 to 回带: {out:?}"
        );
        assert!(out.failed.is_empty(), "跳过不是失败: {out:?}");
        assert!(dir.path().join("s.txt").exists(), "skip 命中源不得被移走");
        assert_eq!(std::fs::read(dir.path().join("t.txt")).unwrap(), b"tgt");

        // 未命中:legacy rename 直通(目标原无,纯改名),批不弃废
        let out2 =
            move_entries_with(&[(dir.path().join("s.txt"), dir.path().join("u.txt"))], &policy)
                .unwrap();
        assert_eq!(
            out2.moved,
            vec![dir.path().join("u.txt").to_string_lossy().into_owned()],
            "未命中对正常 rename: {out2:?}"
        );
        assert!(out2.skipped.is_empty(), "未命中不进跳过表: {out2:?}");
        assert_eq!(std::fs::read(dir.path().join("u.txt")).unwrap(), b"src");

        // overwrite 命中:rename 直通覆盖既有文件(内容翻新)
        std::fs::write(dir.path().join("s.txt"), b"src2").unwrap();
        let policy2 = pol(&[], &[dir.path().join("t.txt").to_string_lossy().into_owned()]);
        let out2 = move_entries_with(&[(dir.path().join("s.txt"), dir.path().join("t.txt"))], &policy2)
            .unwrap();
        assert_eq!(
            out2.moved,
            vec![dir.path().join("t.txt").to_string_lossy().into_owned()],
            "overwrite 命中直接 rename 覆盖: {out2:?}"
        );
        assert_eq!(std::fs::read(dir.path().join("t.txt")).unwrap(), b"src2");
        assert!(!dir.path().join("s.txt").exists());
        // 旧签名委托面不破
        std::fs::write(dir.path().join("v.txt"), b"v").unwrap();
        let (n, failed) = move_entries(&[(dir.path().join("v.txt").to_string_lossy().into_owned(), dir.path().join("w.txt").to_string_lossy().into_owned())]).unwrap();
        assert_eq!((n, failed.len()), (1, 0), "旧签名保持 legacy: n={n} failed={failed:?}");
        assert!(dir.path().join("w.txt").exists());
    }

    /// 目录撞目录(Review Focus 3):overwrite 命中但占用是目录型 →
    /// **不清**(绝不递归删用户目录),退化 `unique_target` 改名落位,
    /// 原目录完好、不报错;未命中沿 legacy 改名,同样不报错。
    #[test]
    fn copy_dir_target_policy_no_clobber() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("树")).unwrap();
        std::fs::write(src.path().join("树").join("叶.txt"), b"leaf").unwrap();
        std::fs::create_dir_all(dst.path().join("树")).unwrap();
        std::fs::write(dst.path().join("树").join("旧.txt"), b"keep").unwrap();

        let td = dst.path().to_string_lossy().into_owned();
        // overwrite 命中目录型占用:不清、退化改名、无错
        let out = copy_paths_with(
            &[src.path().join("树")],
            dst.path(),
            &pol(&[], &[policy_key(&td, "树")]),
        )
        .unwrap();
        assert_eq!(out.copied.len(), 1, "目录撞目录不报错: {out:?}");
        assert!(out.skipped.is_empty(), "overwrite 命中不算跳过: {out:?}");
        assert_eq!(
            std::fs::read(dst.path().join("树").join("旧.txt")).unwrap(),
            b"keep",
            "原目录不得被清(no clobber)"
        );
        assert!(
            out.copied[0].ends_with("树 (1)"),
            "目录占用退化走改名落位: {}",
            out.copied[0]
        );
        assert_eq!(
            std::fs::read(Path::new(&out.copied[0]).join("叶.txt")).unwrap(),
            b"leaf",
            "整树随改名落位"
        );

        // 未命中:沿 legacy 再改名(并列不报错)
        let out2 = copy_paths_with(
            &[src.path().join("树")],
            dst.path(),
            &TransferPolicy::default(),
        )
        .unwrap();
        assert!(
            out2.copied[0].ends_with("树 (2)"),
            "未命中沿 legacy 改名: {}",
            out2.copied[0]
        );
        assert_eq!(
            std::fs::read(dst.path().join("树").join("旧.txt")).unwrap(),
            b"keep"
        );
    }

    /// 终审 I-1 钉死:同设备自复制的**大小写变体**拼写(源 `C:\Foo\a.txt`,
    /// 目标目录拼作 `C:\foo` + overwrite)不得被字节等值判定漏判成真占用
    /// ——NTFS 大小写不敏感,旧码把占用解析到源自身 metadata,`remove_file`
    /// 自删源、暂存 NotFound 整批 Err,数据永久丢失。正确行为:视为自身,
    /// 暂存后原位落回,源存活、内容不变、无 ` (1)`、无 Err。
    /// (Windows 专属:依赖大小写不敏感的同址解析。)
    #[test]
    #[cfg(windows)]
    fn copy_self_overwrite_case_variant_keeps_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"self").unwrap();

        // 目标目录 = 源所在目录的大小写变体拼写(NTFS 同址;normalize 不
        // 归一大小写,字节等值判定因此漏判)——只翻末段,其余段原样。
        let mut td_variant = dir.path().to_path_buf();
        let flipped: String = td_variant
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect();
        assert_ne!(
            td_variant.file_name().unwrap().to_string_lossy(),
            flipped,
            "tempdir 末段须含 ASCII 字母,翻转才有别(测试自检)"
        );
        td_variant.set_file_name(flipped);
        assert!(
            td_variant.join("a.txt").exists(),
            "大小写变体路径必须与源同址(NTFS 不敏感;测试自检)"
        );
        let td = td_variant.to_string_lossy().into_owned();

        let policy = pol(&[], &[policy_key(&td, "a.txt")]);
        let out =
            copy_paths_with(std::slice::from_ref(&src), td_variant.as_path(), &policy)
                .expect("大小写变体自复制必须走自身占用路线,不得 Err(旧码自删源后暂存失败)");

        assert_eq!(out.copied.len(), 1, "自复制刷新落位成功: {out:?}");
        assert!(
            out.copied[0].ends_with("a.txt"),
            "原位落回原名,不得退化 ` (1)` 改名: {:?}",
            out.copied[0]
        );
        assert!(out.skipped.is_empty(), "overwrite 命中不算跳过: {out:?}");
        assert!(
            out.failed.is_empty(),
            "copy 沿 legacy 整批 Err,无失败表: {out:?}"
        );
        assert!(src.exists(), "源文件必须存活(旧码 remove_file 自删源)");
        assert_eq!(std::fs::read(&src).unwrap(), b"self", "源内容原样不变");
        assert!(
            !dir.path().join("a (1).txt").exists(),
            "不得触发改名安全网"
        );
    }
}
