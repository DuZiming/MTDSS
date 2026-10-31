//! 授权根守卫(v2-P5 T2;spec §3「授权目录集」的引擎侧正身)。
//!
//! iOS 形态(spec §3/§5.1):授权集 = 收件箱(App 初始授权,恒在)+ 信任
//! 目录(用户经设置添加,T10 书签解析结果经 `DaemonOpts::authorized_roots`
//! 注入)。授权集内引擎可读 → ①其他设备可浏览进来(ctrl 面的 List/Stat 及
//! 既有 delete/move/copy 等本机路径操作)②可作 App 内发送源(jobs 面的
//! upload 源路径,含编排回显与重发)。集外路径一律拒。
//!
//! 桌面默认 `None` = 未配置 = 守卫直通,**行为逐字节不变**(全局约束:
//! 桌面 bin 行为不变;本模块只在 Some 形态下收口)。
//!
//! 手法与 Windows put「平台无关绝对形态预检」同源(flow::remote_absolute_form
//! 先例):先过 NUL + 本平台绝对形态关(`mtdss_core::fs::normalize`——守卫
//! 的是**本机**路径面,平台规则即本机规则),再**词法规范化**(逐组件消解
//! `.`/`..`,`..` 越出根即拒),最后按组件做授权前缀判定(`Path::components`
//! 语义,杜绝 `/a/bc` 字节前缀误判 `/a/b` 的经典坑)。纯字符串/组件操作,
//! 不 canonicalize:授权根与受检路径都可能尚不存在(发送预扫先于落盘),
//! 且词法判定与既有预检口径一致。
//!
//! 已知边界(如实记录,非本任务扩张面):符号链接指到集外的路径,词法判定
//! 拦不住(引擎读取时才跟随链接;iOS 沙箱内 Documents/信任目录由系统选择器
//! 授权,App 自身无法在集内植入外指链接,远端对端无法借 `..` 之外的形态触
//! 达——纵深加固归 T10 书签面落地时一并评估)。
//!
//! Windows 形态说明:授权集生产上只在 iOS(UNIX 路径)注入;Windows 组件
//! 比较做 ASCII 大小写 fold(NTFS 大小写不敏感),盘符同折,防御性覆盖
//! 测试机自注入形态。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// 授权根守卫:构造时规范化授权根(非法根丢弃留痕,fail-closed——坏根缩小
/// 授权集而非放大),`check` 做单路径判定。`Clone`/`Arc` 友好(状态内嵌,
/// ctrl/jobs 面经 `state.guard()` 共享只读句柄)。
#[derive(Debug, Clone)]
pub(crate) struct RootGuard {
    /// None = 未配置(桌面默认):一切路径直通。Some = 规范化后的授权根集。
    roots: Option<Arc<[PathBuf]>>,
}

impl RootGuard {
    /// 由注入面构造(iOS 形态:收件箱 + 信任目录;桌面 None)。
    pub(crate) fn new(roots: Option<Vec<PathBuf>>) -> Self {
        let roots = roots.map(|list| {
            list.into_iter()
                .filter_map(|r| match normalize_lexical(&r) {
                    Ok(p) => Some(p),
                    Err(err) => {
                        tracing::error!(root = %r.display(), %err, "授权根非法,已丢弃(fail-closed)");
                        None
                    }
                })
                .collect::<Arc<[PathBuf]>>()
        });
        Self { roots }
    }

    /// 是否启用(未配置 = false = 直通;诊断/测试面,生产分派不查它——
    /// check 内部 None 直通,零分支面重复)。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn enabled(&self) -> bool {
        self.roots.is_some()
    }

    /// 单路径守卫:`Some(roots)` 时路径须过绝对形态关、词法规范化后落在任一
    /// 授权根内(含根自身——收件箱根目录本身必须可浏览/可作发送源),否则
    /// Err(业务文案,ctrl INVALID_PATH 与 jobs 业务 Err 两个消费面同源)。
    /// 非 UTF-8 路径在启用形态下拒绝(注入面与 IPC 面均为 UTF-8 字符串,
    /// 出现即异常形态,fail-closed)。
    pub(crate) fn check(&self, path: &Path) -> Result<(), String> {
        let roots = match &self.roots {
            None => return Ok(()),
            Some(r) => r,
        };
        let s = path
            .to_str()
            .ok_or_else(|| format!("路径非 UTF-8,拒绝: {}", path.display()))?;
        // 本机路径面:先过 NUL + 本平台绝对形态关(平台无关预检同款手法),
        // 再词法规范化,最后授权前缀判定。
        let abs = mtdss_core::fs::normalize(s).map_err(|e| format!("路径非法: {e}"))?;
        let norm = normalize_lexical(Path::new(&abs))?;
        for root in roots.iter() {
            if path_under_root(root, &norm) {
                return Ok(());
            }
        }
        Err(format!(
            "路径不在授权目录集内: {}",
            norm.display()
        ))
    }
}

/// 词法规范化:逐组件消解 `.`/`..`(ParentDir 越出根即拒——逃逸形态,
/// fail-closed)。保留平台锚(Prefix/RootDir),输出可再次 components 的
/// 规范 PathBuf。
fn normalize_lexical(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().to_string_lossy().contains('\0') {
        return Err(format!("路径含 NUL: {}", path.display()));
    }
    let mut norm = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => norm.push(p.as_os_str()),
            Component::RootDir => norm.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => {
                if !norm.pop() {
                    return Err(format!(
                        "路径 `..` 越出根,按越界拒绝: {}",
                        path.display()
                    ));
                }
            }
            Component::Normal(c) => norm.push(c),
        }
    }
    if norm.as_os_str().is_empty() {
        return Err(format!("路径规范化后为空: {}", path.display()));
    }
    Ok(norm)
}

/// 组件级前缀判定:`path == root` 或 `path` 在 `root` 之下(整组件匹配,
/// 非字节前缀)。Windows 下组件比较 ASCII 大小写 fold(NTFS 语义)。
fn path_under_root(root: &Path, path: &Path) -> bool {
    let mut rc = root.components();
    let mut pc = path.components();
    loop {
        match (rc.next(), pc.next()) {
            // 根耗尽:路径等于根或更深 → 在集内
            (None, _) => return true,
            // 路径先耗尽:比根浅 → 不在集内
            (Some(_), None) => return false,
            (Some(r), Some(p)) => {
                if !components_eq(&r, &p) {
                    return false;
                }
            }
        }
    }
}

/// 组件相等:Windows 折 ASCII 大小写(含盘符);其余平台逐字节。
#[cfg(windows)]
fn components_eq(a: &Component<'_>, b: &Component<'_>) -> bool {
    match (a, b) {
        (Component::Normal(x), Component::Normal(y)) => {
            x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase()
        }
        (Component::Prefix(x), Component::Prefix(y)) => prefix_kinds_eq(x.kind(), y.kind()),
        _ => a == b,
    }
}

/// Windows 盘符类前缀的大小写折等(Disk(b'C') vs Disk(b'c'));其余前缀
/// 形态逐字段等价。
#[cfg(windows)]
fn prefix_kinds_eq(a: std::path::Prefix<'_>, b: std::path::Prefix<'_>) -> bool {
    use std::path::Prefix;
    match (a, b) {
        (Prefix::Disk(x), Prefix::Disk(y)) => x.eq_ignore_ascii_case(&y),
        _ => a == b,
    }
}

#[cfg(not(windows))]
fn components_eq(a: &Component<'_>, b: &Component<'_>) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未配置(None)= 直通:任何形态的路径都 Ok,enabled=false。
    #[test]
    fn unconfigured_is_passthrough() {
        let guard = RootGuard::new(None);
        assert!(!guard.enabled());
        assert!(guard.check(Path::new("/")).is_ok());
        assert!(guard.check(Path::new("relative/x")).is_ok());
        assert!(guard.check(Path::new("")).is_ok());
    }

    /// 授权根矩阵(UNIX 语义路径;Windows 宿主跑 WSL,与 daemon e2e 同法)。
    #[cfg(unix)]
    #[test]
    fn unix_roots_matrix() {
        let guard = RootGuard::new(Some(vec![
            PathBuf::from("/home/u/inbox"),
            PathBuf::from("/home/u/trusted/"),
        ]));
        assert!(guard.enabled());
        // 根自身与集内任意深度放行(含重复分隔符/`.` 噪声)
        assert!(guard.check(Path::new("/home/u/inbox")).is_ok());
        assert!(guard.check(Path::new("/home/u/inbox/a/b/c.txt")).is_ok());
        assert!(guard
            .check(Path::new("/home/u//inbox/./a/../b.txt"))
            .is_ok());
        // 第二根(尾随分隔符规范化后同判)
        assert!(guard.check(Path::new("/home/u/trusted")).is_ok());
        assert!(guard.check(Path::new("/home/u/trusted/sub")).is_ok());
        // 集外拒
        assert!(guard.check(Path::new("/home/u/other")).is_err());
        assert!(guard.check(Path::new("/etc/passwd")).is_err());
        // 字节前缀坑:/home/u/inbox-x 非集内(整组件判定)
        assert!(guard.check(Path::new("/home/u/inbox-x")).is_err());
        // `..` 逃逸集外:形似集内,规范化后出集
        assert!(guard.check(Path::new("/home/u/inbox/../evil")).is_err());
        // 相对/NUL/空:绝对形态关先拒
        assert!(guard.check(Path::new("inbox/relative")).is_err());
        assert!(guard.check(Path::new("")).is_err());
        assert!(guard.check(Path::new("/a\0b")).is_err());
    }

    /// 根集自净化:非法根(相对/`..` 逃逸/空)构造期丢弃,fail-closed
    /// (缩小授权集而非放大)。
    #[cfg(unix)]
    #[test]
    fn invalid_roots_dropped_at_construction() {
        let guard = RootGuard::new(Some(vec![
            PathBuf::from("relative/root"),
            PathBuf::from("/ok/root"),
            PathBuf::from("/escapes/.."),
        ]));
        assert!(guard.enabled());
        assert!(guard.check(Path::new("/ok/root")).is_ok());
        assert!(guard.check(Path::new("/escapes")).is_err());
    }

    /// Windows 形态矩阵:盘符与组件大小写 fold,`C:` 裸盘符规范化,尾部
    /// `..` 收敛(仅在 Windows 宿主编译运行;授权集生产面在 iOS,本矩阵
    /// 是防御性覆盖)。
    #[cfg(windows)]
    #[test]
    fn windows_roots_matrix() {
        let guard = RootGuard::new(Some(vec![PathBuf::from(r"c:\Users\u\inbox")]));
        assert!(guard.check(Path::new(r"C:\Users\u\inbox")).is_ok());
        assert!(guard.check(Path::new(r"c:\Users\u\INBOX\Sub\F.TXT")).is_ok());
        assert!(guard.check(Path::new(r"C:\Users\u\inbox-x")).is_err());
        assert!(guard.check(Path::new(r"C:\Users\u\inbox\..\evil")).is_err());
        assert!(guard.check(Path::new(r"D:\Users\u\inbox")).is_err());
        assert!(guard.check(Path::new("relative")).is_err());
    }
}
