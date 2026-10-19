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
//!   进失败表,不半途弃批。
//! - `move_entries`:rename 直通;跨卷(rename 报 EXDEV/ERROR_NOT_SAME_DEVICE)
//!   回退「目标目录 `.mtdss` 暂存复制 + 原子改名落位 + 源永久删除」,
//!   **对调用方透明**——成功即移动完成,源删除为永久(数据已在目的地)。
//! - `copy_paths`:整树拷入**目标目录** `.mtdss` 暂存(`loc-<uuid>.part`
//!   键,与网络收件区 `<sha256(rel)>` 键空间天然不撞)→ 原子改名落位,
//!   目标名不存在半成品可见窗口;同名走 `unique_target` 自动改名
//!   (`name (1)`…,冲突策略判定在调用方),暂存完成后落空。
//!
//! 命名提示:本模块名 `fs` 与 `std::fs` 同名,模块内部一律写全路径
//! `std::fs::…`,避免自引用混淆。

/// 单次列表条目上限,超出即截断(ctrl 响应置 `truncated` 标志)。
pub const LIST_ENTRY_CAP: usize = 10_000;

use std::path::Path;

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

/// 删除一批绝对路径(文件或目录整树)。`force=false` 先试回收(trash
/// crate;Windows=IFileOperation+ALLOWUNDO,Unix=freedesktop Trash),
/// 回收设施不可用 → `RecycleUnavailable`;`force=true` 永久删除。逐条
/// 执行,某条失败只记失败表不弃批。返回 (成功数, 是否走了回收, 逐条
/// 失败表)。存在性先经 `symlink_metadata` 预检(不跟随,与列表同口径),
/// 缺失逐条 `NOT_FOUND`,失败表确定、不依赖回收/删除调用的报错形态。
pub fn delete_paths(
    paths: &[String],
    force: bool,
) -> Result<(usize, bool, Vec<PathOpFailure>), FsError> {
    delete_paths_inner(paths, force, recycle_io)
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
/// 失败只记表不弃批。返回 (成功数, 逐条失败表)。
pub fn move_entries(pairs: &[(String, String)]) -> Result<(usize, Vec<PathOpFailure>), FsError> {
    move_entries_inner(pairs, |f, t| std::fs::rename(f, t))
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
    let mut moved = 0usize;
    let mut failed: Vec<PathOpFailure> = Vec::new();
    for (from, to) in pairs {
        match move_one(from, to, &rename_impl) {
            Ok(()) => moved += 1,
            Err(f) => failed.push(f),
        }
    }
    Ok((moved, failed))
}

fn move_one(
    from: &str,
    to: &str,
    rename_impl: &impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), PathOpFailure> {
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
pub fn copy_paths(sources: &[String], target_dir: &str) -> Result<Vec<String>, FsError> {
    let td = normalize(target_dir)?;
    let tdp = Path::new(&td);
    let md = std::fs::metadata(tdp).map_err(|e| map_io(e, &td))?;
    if !md.is_dir() {
        return Err(FsError::Internal(format!("{td}: 目标不是目录")));
    }
    let mut landed: Vec<String> = Vec::with_capacity(sources.len());
    for s in sources {
        let src = normalize(s)?;
        let sp = Path::new(&src);
        let smd = std::fs::symlink_metadata(sp).map_err(|e| map_io(e, &src))?;
        // 复制名 = 源最后一段;卷根/无名形态不可复制
        let name = sp
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| FsError::InvalidPath(src.clone()))?;
        // 占位独占终名(unique_target 语义);占位是本调用刚建的空壳,
        // 后续失败一律清掉,不阻塞下次重试拿到同名
        let final_path = crate::transport::intake::unique_target(tdp, &name)
            .map_err(|e| FsError::Internal(format!("{td}/{name}: {e}")))?;
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
            Ok(()) => landed.push(final_path.to_string_lossy().into_owned()),
            Err(e) => {
                discard_staged(&staged, is_dir);
                if !is_dir {
                    let _ = std::fs::remove_file(&final_path);
                }
                return Err(map_io(e, &final_path.display().to_string()));
            }
        }
    }
    Ok(landed)
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
/// 盘符 `C:` 规范化为 `C:\`。返回规范化后的 owned 路径。
fn normalize(path: &str) -> Result<String, FsError> {
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
