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
//! 命名提示:本模块名 `fs` 与 `std::fs` 同名,模块内部一律写全路径
//! `std::fs::…`,避免自引用混淆。

/// 单次列表条目上限,超出即截断(ctrl 响应置 `truncated` 标志)。
pub const LIST_ENTRY_CAP: usize = 10_000;

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
}

impl FsError {
    /// ctrl `CtrlError.code` 的取值(NOT_FOUND/PERMISSION_DENIED/
    /// INVALID_PATH/INTERNAL),与 proto 约定一字不差。
    pub fn code(&self) -> &'static str {
        match self {
            FsError::NotFound(_) => "NOT_FOUND",
            FsError::PermissionDenied(_) => "PERMISSION_DENIED",
            FsError::InvalidPath(_) => "INVALID_PATH",
            FsError::Internal(_) => "INTERNAL",
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
