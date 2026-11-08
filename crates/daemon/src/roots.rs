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
//!
//! V3-4 §4.1 起(roots.set 热注册面):授权根不再只经 `DaemonOpts` 构造期
//! 一次性注入——`RootSpec` 集可经 `state.set_roots` 运行时整根换入(安卓
//! App 启动/变更同步、Windows §5-2 切换共用同一 op)。每根带 `source`:
//! `Fs` = 真实本机路径(既有语义:check 过平台绝对形态关 + 词法前缀);
//! `Bridge` = App 应答式合成 vpath(应答桥本体在 §4.1 桥面接线,本模块只
//! 钉守卫面)——vpath 可能过不了本机平台绝对形态关(Windows 上的
//! `/照片图库` 本就不是合法盘符形态),check 桥臂**纯词法**前缀判定,先于
//! fs 管线。每根带 `visible`:false = 隐藏根——check 照常放行(可授权而
//! 不可见,暂存区语义),`roots_listing` 不出现。空集语义按输入区分:
//! **显式空 spec 集 = 直通 None**(全局约束:不调用 roots.set 或空集 =
//! 守卫直通,桌面行为零回归);非空集经构造全部被丢 = 启用零根守卫(全
//! 面拒绝,fail-closed 与 `new` 同款)。构造期注入面 `new` 语义原样保留
//! (None 直通;Some 空集保持启用形态)。

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// 授权根来源(V3-4 §4.1):`Fs` = 真实本机路径(既有注入面语义);
/// `Bridge` = App 应答式合成 vpath(守卫纯词法前缀判定,应答桥接线在
/// §4.1 桥面)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootSource {
    Fs,
    Bridge,
}

/// 单根注册描述(roots.set 注入面,V3-4 §4.1):`name` = 对端列表显示名
/// (空串 = 回退末段真实目录名,T13b 语义);`path` = 根(按 `source`
/// 解释:fs = 本机路径,bridge = 合成 vpath);`visible` = 对端根列表可
/// 见性(false = 隐藏根:check 照常放行,列表不出现——暂存区可授权不可
/// 见)。
#[derive(Debug, Clone)]
pub(crate) struct RootSpec {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) source: RootSource,
    pub(crate) visible: bool,
}

/// 守卫内单根条目(RootSpec 构造期投影:路径已词法规范化)。
#[derive(Debug, Clone)]
struct RootEntry {
    path: PathBuf,
    source: RootSource,
    visible: bool,
    /// 显示名(roots.set 注入面提供;None = 注入面无名字 → 列表回退末段
    /// 真实目录名,既有 T13b 语义)。
    name: Option<String>,
}

/// 桥根命中(服务桥路由产物,V3-4 §4.1):`root` = 规范化桥根 vpath
/// (fs.serve_* 事件的 "root" 字段——unix 生产面与注册形一致),`sub` =
/// 根下子路径("" = 根自身;事件的 "sub" 字段),`name` = 显示名(spec 名
/// 优先,回退根末段)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BridgeHit {
    pub(crate) root: String,
    pub(crate) sub: String,
    pub(crate) name: String,
}

/// 授权根守卫:构造时规范化授权根(非法根丢弃留痕,fail-closed——坏根缩小
/// 授权集而非放大;重复根去重保首次出现序——R2 修:信任书签可能解析到
/// 既有根,如 Documents 自身 = 收件箱,不去重会让虚拟根列表同路径两行),
/// `check` 做单路径判定。`Clone`/`Arc` 友好(V3-4 §4.1 起守卫可换:
/// state 持 `RwLock<Arc<Self>>`,ctrl/jobs 面经 `state.guard()` 取快照
/// 句柄)。
#[derive(Debug, Clone)]
pub(crate) struct RootGuard {
    /// None = 未配置(桌面默认):一切路径直通。Some = 规范化去重后的授权
    /// 根条目集(fs/bridge 混合,含可见性)。
    roots: Option<Arc<[RootEntry]>>,
}

impl RootGuard {
    /// 由注入面构造(iOS 形态:收件箱 + 信任目录;桌面 None)。既有语义
    /// 原样:None = 直通;Some 空集(含全非法丢弃后)保持启用形态。校验
    /// 与去重走 [`Self::build_entries`](词法规范化同款,不设平台绝对形态
    /// 关——与既有构造口径逐字节一致)。
    pub(crate) fn new(roots: Option<Vec<PathBuf>>) -> Self {
        let roots = roots.map(|list| {
            let entries = Self::build_entries(list.into_iter().map(|root| RootEntry {
                path: root,
                source: RootSource::Fs,
                visible: true,
                name: None,
            }));
            entries.into_iter().collect::<Arc<[RootEntry]>>()
        });
        Self { roots }
    }

    /// 由 spec 集构造(roots.set 热注册面,V3-4 §4.1):非法根丢弃留痕
    /// fail-closed(沿 `new` 语义:词法关拒 NUL/`..` 越根/空;**不设平台
    /// 绝对形态关**——bridge vpath 本就非本机形态,fs 相对根与既有注入面
    /// 同口径保录入、check 时过不了平台关)。空集语义**按输入区分**(评审
    /// Important 修):**显式空集 = 直通 None**(全局约束:不调用 roots.set
    /// 或空集 = 守卫直通,`roots.set []` = 清回默认合法);**非空集但全部
    /// 非法根被丢弃 = 启用零根守卫(全面拒绝)**——调用方意在授权却传了
    /// 坏集,绝不落全盘直通(fail-open 角),与 `new(Some([全非法]))` 的
    /// 启用零根 deny-all 同款。
    pub(crate) fn from_specs(specs: Vec<RootSpec>) -> Self {
        let roots = if specs.is_empty() {
            None
        } else {
            let entries = Self::build_entries(specs.into_iter().map(|spec| RootEntry {
                path: PathBuf::from(&spec.path),
                source: spec.source,
                visible: spec.visible,
                name: (!spec.name.is_empty()).then_some(spec.name),
            }));
            Some(entries.into_iter().collect::<Arc<[RootEntry]>>())
        };
        Self { roots }
    }

    /// 根集构造核心(注入面与 spec 面共用):逐根词法规范化,非法丢弃留
    /// 痕;重复根去重保首见序(跨源判等——同路径不同源属病理配置,列表
    /// 两行比丢一行更糟;严格前缀不算重复,两根并存)。
    fn build_entries(raw: impl Iterator<Item = RootEntry>) -> Vec<RootEntry> {
        let mut kept: Vec<RootEntry> = Vec::new();
        for mut entry in raw {
            let norm = match normalize_lexical(&entry.path) {
                Ok(p) => p,
                Err(err) => {
                    tracing::error!(
                        root = %entry.path.display(),
                        source = ?entry.source,
                        %err,
                        "授权根非法,已丢弃(fail-closed)"
                    );
                    continue;
                }
            };
            // 去重(R2 修):与已收根判等即丢,保首次出现序。判等沿守
            // 卫比较语义(组件级互含 = 相等,Windows 折大小写),严格
            // 前缀(如根与其子目录)不算重复,两根并存。
            if kept.iter().any(|k| same_root(&k.path, &norm)) {
                continue;
            }
            entry.path = norm;
            kept.push(entry);
        }
        kept
    }

    /// 是否启用(未配置 = false = 直通;ctrl 分派的虚拟根列表臂以它分
    /// 流,check 内部 None 直通,生产热路径零分支面重复)。
    pub(crate) fn enabled(&self) -> bool {
        self.roots.is_some()
    }

    /// 授权根虚拟列表(2026-11-01 T13b 修#7):对端列根(ctrl List path =
    /// "" = 桌面根卷列表语义)时,把授权根自身列成目录条目——对端由此
    /// 得以发现收件箱/信任目录并下钻(集内路径照旧过 `check`),而不是
    /// 整个设备「远端路径非法」不可浏览。未配置 → `None`(桌面走既有
    /// `fs::root_listing` 卷列表,行为逐字节不变)。V3-4 §4.1:只出
    /// `visible: true` 条目(隐藏根 check 放行、列表不出现——可授权而
    /// 不可见);条目名 = spec 提供名(非空)否则回退根路径末段真实目录
    /// 名(既有语义)。
    ///
    /// 条目语义:`path` = 规范化根路径(可直接下钻);`children` 按
    /// T5b 语义计直接子项;metadata 试探失败(根尚未落盘/失效书签/合成
    /// vpath)→ (size 0, mtime 0, is_dir true) 兜底——根仍可见、下钻报
    /// NOT_FOUND,与 `entry_data` 的「拿不到即兜底」同哲学。**桥根例外**
    /// (V3-4 §4.1):合成 vpath 非本机路径面,不 stat 本机——直接合成
    /// 条目(children 惰性 None,下钻列表由服务桥实答);条目可见性另有
    /// 桥探针收口(ctrl 根列表臂:桥不可达时隐藏)。
    pub(crate) fn roots_listing(&self) -> Option<mtdss_core::fs::ListOut> {
        let roots = self.roots.as_ref()?;
        let mut entries: Vec<mtdss_core::fs::DirEntryData> = roots
            .iter()
            .filter(|e| e.visible)
            .map(|entry| {
                let path = entry.path.to_string_lossy().into_owned();
                let name = entry.name.clone().unwrap_or_else(|| {
                    entry
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone())
                });
                if entry.source == RootSource::Bridge {
                    return mtdss_core::fs::DirEntryData {
                        name,
                        path,
                        size: 0,
                        mtime_ms: 0,
                        is_dir: true,
                        children: None,
                    };
                }
                match mtdss_core::fs::stat_path(&path) {
                    Ok(mut d) => {
                        // spec 提供名优先(可见面语义;既有注入面无名字,
                        // 保留 stat 的真实目录名,行为逐字节不变)。
                        if let Some(n) = &entry.name {
                            d.name = n.clone();
                        }
                        if d.is_dir {
                            d.children = mtdss_core::fs::child_count(&entry.path);
                        }
                        d
                    }
                    Err(_) => mtdss_core::fs::DirEntryData {
                        name,
                        path,
                        size: 0,
                        mtime_ms: 0,
                        is_dir: true,
                        children: None,
                    },
                }
            })
            .collect();
        // 目录优先 + 名称字节升序(core sort_entries 同口径;本集全为目录)
        entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        Some(mtdss_core::fs::ListOut {
            entries,
            truncated: false,
        })
    }

    /// 桥根路由判定(V3-4 §4.1 服务桥):原始路径 → 命中的桥根与根下子
    /// 路径。与 [`Self::check`] 桥臂**同一判定**(`normalize_lexical` 规范
    /// 化 + `path_under_root` 整组件前缀),不另造口径——`/收件箱/../x`
    /// 规范化后出集、`/收件箱-x` 字节前缀不匹配,均 None(回落 fs 面,
    /// 守卫按集外拒)。fs 根不参与(命中 fs 根的路径走既有 fs 面);嵌套
    /// 桥根(`/a` 与 `/a/b` 并存)取最长匹配(mount-point 语义)。
    pub(crate) fn bridge_route(&self, raw: &str) -> Option<BridgeHit> {
        let roots = self.roots.as_ref()?;
        let lex = normalize_lexical(Path::new(raw)).ok()?;
        let mut best: Option<&RootEntry> = None;
        let mut best_depth = 0usize;
        for entry in roots.iter() {
            if entry.source != RootSource::Bridge {
                continue;
            }
            if path_under_root(&entry.path, &lex) {
                let depth = entry.path.components().count();
                if best.is_none() || depth > best_depth {
                    best = Some(entry);
                    best_depth = depth;
                }
            }
        }
        let entry = best?;
        let root = entry.path.to_string_lossy().into_owned();
        let lex_s = lex.to_string_lossy().into_owned();
        // 规范化输出按 MAIN_SEPARATOR 逐组件拼接:根命中(整组件)时
        // lex_s 必以 root 字节串为前缀,切根安全。
        let sub = if lex_s == root {
            String::new()
        } else {
            lex_s[root.len()..]
                .trim_start_matches(std::path::MAIN_SEPARATOR)
                .trim_end_matches(std::path::MAIN_SEPARATOR)
                .to_string()
        };
        let name = entry.name.clone().unwrap_or_else(|| {
            entry
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.clone())
        });
        Some(BridgeHit { root, sub, name })
    }

    /// 单路径守卫:`Some(roots)` 时路径须落在任一授权根内(含根自身——
    /// 收件箱根目录本身必须可浏览/可作发送源),否则 Err(业务文案,ctrl
    /// INVALID_PATH 与 jobs 业务 Err 两个消费面同源)。V3-4 §4.1 起按源
    /// 分派:**桥根臂**纯词法前缀判定(整组件,`..` 同消解)且**先于
    /// fs 管线**——合成 vpath 可能过不了本机平台绝对形态关(Windows 上的
    /// `/照片图库` 本就不是合法盘符形态),先 gate 会把桥根面整体拒;
    /// **fs 根臂**保持既有管线原样(NUL + 平台绝对形态关 → 词法规范化 →
    /// 前缀判定),既有形态行为逐字节不变。非 UTF-8 路径在启用形态下拒绝
    /// (注入面与 IPC 面均为 UTF-8 字符串,出现即异常形态,fail-closed)。
    pub(crate) fn check(&self, path: &Path) -> Result<(), String> {
        let roots = match &self.roots {
            None => return Ok(()),
            Some(r) => r,
        };
        let s = path
            .to_str()
            .ok_or_else(|| format!("路径非 UTF-8,拒绝: {}", path.display()))?;
        // 桥根臂(§4.1):对原始入参做词法规范化后按 vpath 前缀判定(逃
        // 逸形态在规范化即拒/落集外);无桥根时整臂跳过,fs 形态零新增面。
        if roots.iter().any(|e| e.source == RootSource::Bridge) {
            if let Ok(lex) = normalize_lexical(path) {
                for entry in roots.iter() {
                    if entry.source == RootSource::Bridge && path_under_root(&entry.path, &lex) {
                        return Ok(());
                    }
                }
            }
        }
        // 本机路径面(fs 根臂,既有管线原样):先过 NUL + 本平台绝对形态
        // 关(平台无关预检同款手法),再词法规范化,最后授权前缀判定。
        let abs = mtdss_core::fs::normalize(s).map_err(|e| format!("路径非法: {e}"))?;
        let norm = normalize_lexical(Path::new(&abs))?;
        for entry in roots.iter() {
            if entry.source == RootSource::Fs && path_under_root(&entry.path, &norm) {
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
/// 规范 PathBuf。`pub(crate)`:照片虚拟根前缀判定(photo::photo_vpath)
/// 同源复用——`/照片图库/../x` 规范化为 `/x` 后不再是照片路径,无逃逸面。
pub(crate) fn normalize_lexical(path: &Path) -> Result<PathBuf, String> {
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

/// 根判等(去重面):两根互为组件级包含 = 相等(Windows 折大小写,与
/// `check` 同口径);严格前缀不判等(根与其子目录并存)。
fn same_root(a: &Path, b: &Path) -> bool {
    path_under_root(a, b) && path_under_root(b, a)
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

    /// 根集自净化:非法根(相对/`..` 越出根/空)构造期丢弃,fail-closed
    /// (缩小授权集而非放大)。忠实规范化语义:根按词法解析后即授权面
    /// (`/a/..` 解析为 `/` 等于全盘授权——注入方契约是传真实目录,iOS
    /// 注入面只传 App 沙箱内绝对路径;此处钉的是「非法根丢弃」,不是对
    /// 全盘根的背书)。
    #[cfg(unix)]
    #[test]
    fn invalid_roots_dropped_at_construction() {
        let guard = RootGuard::new(Some(vec![
            PathBuf::from("relative/root"),
            PathBuf::from("/ok/root"),
            PathBuf::from("/escapes/../../.."),
        ]));
        assert!(guard.enabled());
        assert!(guard.check(Path::new("/ok/root")).is_ok());
        assert!(guard.check(Path::new("/escapes")).is_err());
    }

    /// 重复根去重(R2 修):信任书签解析收敛到既有根(如 Documents 自身
    /// = 收件箱)构造期去重,虚拟根列表不再出同路径两行;首次出现序保留
    /// (内部根集序 = 注入序,展示排序是 roots_listing 的另一层);规范化
    /// 噪声形态(尾随分隔符/`.`)同路径即判重;严格前缀不算重复。
    #[cfg(unix)]
    #[test]
    fn duplicate_roots_dedup_keeps_first_occurrence_order() {
        let guard = RootGuard::new(Some(vec![
            PathBuf::from("/home/u/zz"),
            PathBuf::from("/home/u/aa"),
            // 完全重复 + 噪声形态(规范化后同路径)
            PathBuf::from("/home/u/zz"),
            PathBuf::from("/home/u/zz/."),
            // 严格前缀不是重复:aa 与 aa/sub 并存
            PathBuf::from("/home/u/aa/sub"),
        ]));
        assert!(guard.enabled());
        // 内部根集:恰三根,首见序(zz, aa, aa/sub)
        assert_eq!(
            guard
                .roots
                .as_ref()
                .unwrap()
                .iter()
                .map(|e| e.path.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/home/u/zz"),
                PathBuf::from("/home/u/aa"),
                PathBuf::from("/home/u/aa/sub"),
            ],
            "重复根丢弃、首见序保留、严格前缀并存"
        );
        // 虚拟根列表:每根恰一行(根未落盘走兜底条目,仍可见)
        let listing = guard.roots_listing().unwrap();
        let mut paths: Vec<String> = listing.entries.iter().map(|e| e.path.clone()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "/home/u/aa".to_string(),
                "/home/u/aa/sub".to_string(),
                "/home/u/zz".to_string(),
            ],
            "重复根只出一行"
        );
        // 判等语义与 check 同口径:任一根的集内路径照旧放行
        assert!(guard.check(Path::new("/home/u/zz/x")).is_ok());
        assert!(guard.check(Path::new("/home/u/aa/sub/y")).is_ok());
        assert!(guard.check(Path::new("/home/u/other")).is_err());
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

    // ---------- roots.set 形态(V3-4 §4.1:spec 面 → 守卫可换)----------
    //
    // tempdir 真实绝对路径,双平台可跑(UNIX 形注入面矩阵另有 cfg(unix)
    // 用例);断言只钉 ok/err 与条目名,不钉平台分歧的路径串形态(Windows
    // 词法规范化 RootDir → `\`,见 normalize_lexical)。

    /// spec 集(fs 根)check 翻转:集内放行(根自身/深层/`.` 噪声),集外
    /// 拒(对照目录/字节前缀坑/`..` 逃逸/相对形态)。
    #[test]
    fn roots_specs_fs_check_flips() {
        let home = tempfile::tempdir().unwrap();
        let docs = home.path().join("docs");
        let guard = RootGuard::from_specs(vec![RootSpec {
            name: String::new(),
            path: docs.to_string_lossy().into_owned(),
            source: RootSource::Fs,
            visible: true,
        }]);
        assert!(guard.enabled());
        assert!(guard.check(&docs).is_ok());
        assert!(guard.check(&docs.join("deep/f.txt")).is_ok());
        assert!(guard.check(&docs.join("./x/../y.txt")).is_ok());
        assert!(guard.check(&home.path().join("secret")).is_err());
        // 字节前缀坑:docs-x 非集内(整组件判定)
        assert!(guard.check(&home.path().join("docs-x")).is_err());
        // `..` 逃逸:形似集内,规范化后出集
        assert!(guard.check(&docs.join("../evil")).is_err());
        // 相对形态:check 的平台绝对形态关先拒(fs 根臂保持既有管线)
        assert!(guard.check(Path::new("relative/x")).is_err());
    }

    /// 隐藏根(visible=false):check 照常放行(可授权而不可见,暂存区语
    /// 义),对端根列表不出现;visible 根照常在列且名字取 spec——visible
    /// 根真实落盘,钉「stat 命中臂 spec 名仍胜」(stat 的真实目录名只在
    /// 无 spec 名时保留)。
    #[test]
    fn roots_specs_hidden_root_check_passes_listing_omits() {
        let home = tempfile::tempdir().unwrap();
        let vis = home.path().join("vis");
        let hid = home.path().join("hid");
        std::fs::create_dir_all(&vis).unwrap();
        std::fs::write(vis.join("f.txt"), b"x").unwrap();
        let guard = RootGuard::from_specs(vec![
            RootSpec {
                name: "资料".into(),
                path: vis.to_string_lossy().into_owned(),
                source: RootSource::Fs,
                visible: true,
            },
            RootSpec {
                name: "暂存".into(),
                path: hid.to_string_lossy().into_owned(),
                source: RootSource::Fs,
                visible: false,
            },
        ]);
        assert!(guard.check(&hid.join("x")).is_ok());
        assert!(guard.check(&vis.join("x")).is_ok());
        let listing = guard.roots_listing().unwrap();
        let got: Vec<(String, String)> = listing
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.path.clone()))
            .collect();
        assert_eq!(
            got,
            vec![("资料".into(), vis.to_string_lossy().into_owned())],
            "隐藏根不出现在对端根列表;名字取 spec"
        );
    }

    /// 空集语义按输入区分(全局约束 + 评审 Important 修):**显式空集 =
    /// 直通 None**(`roots.set []` = 清回默认合法);**非空集但非法根全
    /// 丢弃(NUL 留痕,沿 new 的 fail-closed 语义)= 启用零根守卫(全面
    /// 拒绝)**——意在授权却传坏集的调用方绝不落全盘直通(fail-open 角)。
    #[test]
    fn roots_specs_empty_passthrough_all_dropped_deny_all() {
        // 显式空集:直通
        let empty = RootGuard::from_specs(vec![]);
        assert!(!empty.enabled());
        assert!(empty.check(Path::new("/anything")).is_ok());
        // 非空集全丢:启用零根 deny-all(列表空但有 = 启用形态)
        let dropped = RootGuard::from_specs(vec![RootSpec {
            name: String::new(),
            path: "bad\0root".into(),
            source: RootSource::Fs,
            visible: true,
        }]);
        assert!(
            dropped.enabled(),
            "非法根全丢应保持启用形态(fail-closed),不落直通"
        );
        assert!(dropped.check(Path::new("/anything")).is_err(), "零根守卫全面拒绝");
        let listing = dropped.roots_listing().unwrap();
        assert_eq!(listing.entries.len(), 0, "零根守卫列表为空(启用形态)");
    }

    /// bridge 根词法前缀矩阵(§4.1):合成 vpath 命名空间纯词法判定——
    /// 集内(根自身/深层/噪声分隔符)放行,`..` 逃逸/字节前缀坑/集外
    /// vpath 拒。桥臂先于 fs 管线:`/照片图库` 在 Windows 过不了本机平台
    /// 绝对形态关,先 gate 会整体拒——本用例双平台可跑即钉此序;列表名字
    /// 取 spec(路径串形态平台分歧,unix 臂内钉)。
    #[test]
    fn roots_specs_bridge_lexical_prefix_matrix() {
        let guard = RootGuard::from_specs(vec![RootSpec {
            name: "照片图库".into(),
            path: "/照片图库".into(),
            source: RootSource::Bridge,
            visible: true,
        }]);
        assert!(guard.enabled());
        assert!(guard.check(Path::new("/照片图库")).is_ok());
        assert!(guard.check(Path::new("/照片图库/相册/IMG_1.JPG")).is_ok());
        assert!(guard.check(Path::new("/照片图库//./x")).is_ok());
        // `..` 逃逸:规范化后出 vpath → 桥臂不中,fs 管线亦不中 → 拒
        assert!(guard.check(Path::new("/照片图库/../evil")).is_err());
        // 字节前缀坑(整组件判定)
        assert!(guard.check(Path::new("/照片图库-x")).is_err());
        // 集外 vpath
        assert!(guard.check(Path::new("/其他根")).is_err());
        // 列表:名字取 spec;条目恰一(可见)
        let listing = guard.roots_listing().unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "照片图库");
        #[cfg(unix)]
        assert_eq!(listing.entries[0].path, "/照片图库");
    }

    /// 跨源重复根去重保首见序:同路径不同源属病理配置——列表两行比丢一
    /// 行更糟,首见胜出(与同源去重同口径)。
    #[test]
    fn roots_specs_dedup_cross_source_keeps_first() {
        let guard = RootGuard::from_specs(vec![
            RootSpec {
                name: "a".into(),
                path: "/共享".into(),
                source: RootSource::Bridge,
                visible: true,
            },
            RootSpec {
                name: "b".into(),
                path: "/共享".into(),
                source: RootSource::Fs,
                visible: true,
            },
        ]);
        assert!(guard.enabled());
        let listing = guard.roots_listing().unwrap();
        let names: Vec<String> = listing.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a".to_string()], "重复根只出一行,首见胜出");
    }

    /// 桥根路由矩阵(V3-4 §4.1 服务桥,bridge_route):命中 = (root, sub)
    /// 拆分;判定与 check 桥臂同口径——`..` 逃逸/字节前缀坑/集外 vpath/
    /// 相对与空 → None;fs 根不参与;嵌套桥根最长匹配;隐藏桥根照常路由
    /// (check 放行同款)。字符串断言钉 unix 形(生产面 Android;Windows
    /// 词法规范化 RootDir → `\`,路径串形态平台分歧)。
    #[test]
    fn bridge_route_matrix() {
        let guard = RootGuard::from_specs(vec![
            RootSpec {
                name: "收件箱".into(),
                path: "/收件箱".into(),
                source: RootSource::Bridge,
                visible: true,
            },
            RootSpec {
                name: "暂存".into(),
                path: "/暂存区".into(),
                source: RootSource::Bridge,
                visible: false,
            },
        ]);
        let hit = |raw: &str| guard.bridge_route(raw);
        // 未配置 = 无路由(直通形态整体无桥面)
        assert!(RootGuard::new(None).bridge_route("/收件箱").is_none());
        // 根自身 / 深层 / 噪声分隔符(与 check 同口径消解)
        assert!(hit("/收件箱").is_some());
        assert!(hit("/收件箱/子目录/报告.pdf").is_some());
        assert!(hit("/收件箱//./a").is_some());
        // `..` 逃逸:规范化后出 vpath → None(fs 面守卫按集外拒)
        assert!(hit("/收件箱/../evil").is_none());
        // 字节前缀坑(整组件判定)
        assert!(hit("/收件箱-x").is_none());
        // 集外 vpath / 相对 / 空 / NUL
        assert!(hit("/其他根").is_none());
        assert!(hit("收件箱").is_none());
        assert!(hit("").is_none());
        assert!(hit("/a\0b").is_none());
        // 隐藏桥根照常路由(可授权而不可见)
        assert!(hit("/暂存区/x.pdf").is_some());
        // (root, sub, name) 拆分钉 unix 形(生产面 Android;Windows 词法
        // 规范化 RootDir → `\`,路径串形态平台分歧——见 normalize_lexical)
        #[cfg(unix)]
        {
            let h = hit("/收件箱").unwrap();
            assert_eq!(h.root, "/收件箱");
            assert_eq!(h.sub, "");
            assert_eq!(h.name, "收件箱");
            assert_eq!(hit("/收件箱/子目录/报告.pdf").unwrap().sub, "子目录/报告.pdf");
            assert_eq!(hit("/收件箱//./a").unwrap().sub, "a");
            let h = hit("/暂存区/x.pdf").unwrap();
            assert_eq!(h.root, "/暂存区");
            assert_eq!(h.sub, "x.pdf");
        }
    }

    /// 嵌套桥根最长匹配(mount-point 语义):`/a` 与 `/a/b` 并存时
    /// `/a/b/c` 路由到深者;`/a/x` 仍路由浅者。
    #[test]
    fn bridge_route_nested_longest_match() {
        let guard = RootGuard::from_specs(vec![
            RootSpec {
                name: "浅".into(),
                path: "/mnt".into(),
                source: RootSource::Bridge,
                visible: true,
            },
            RootSpec {
                name: "深".into(),
                path: "/mnt/DCIM".into(),
                source: RootSource::Bridge,
                visible: true,
            },
        ]);
        assert!(guard.bridge_route("/mnt/DCIM/IMG_1.JPG").is_some());
        assert!(guard.bridge_route("/mnt/其他/x").is_some());
        #[cfg(unix)]
        {
            let h = guard.bridge_route("/mnt/DCIM/IMG_1.JPG").unwrap();
            assert_eq!(h.root, "/mnt/DCIM");
            assert_eq!(h.sub, "IMG_1.JPG");
            let h = guard.bridge_route("/mnt/其他/x").unwrap();
            assert_eq!(h.root, "/mnt");
            assert_eq!(h.sub, "其他/x");
        }
    }

    /// 桥根列表条目 = 合成(children 惰性 None,不 stat 本机——合成 vpath
    /// 非本机路径面);fs 根条目照旧 stat(children 实数)。本机真有同名字
    /// 目录也不误采(stat 面对桥根整体不进)。
    #[test]
    fn bridge_root_listing_entry_is_synthetic() {
        let home = tempfile::tempdir().unwrap();
        let docs = home.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("f.txt"), b"x").unwrap();
        let guard = RootGuard::from_specs(vec![
            RootSpec {
                name: "资料".into(),
                path: docs.to_string_lossy().into_owned(),
                source: RootSource::Fs,
                visible: true,
            },
            RootSpec {
                name: "收件箱".into(),
                path: "/收件箱".into(),
                source: RootSource::Bridge,
                visible: true,
            },
        ]);
        let listing = guard.roots_listing().unwrap();
        let by_name = |n: &str| {
            listing
                .entries
                .iter()
                .find(|e| e.name == n)
                .unwrap_or_else(|| panic!("列根应含 {n}: {:?}", listing.entries))
        };
        assert_eq!(by_name("资料").children, Some(1), "fs 根 children 实数");
        let bridge = by_name("收件箱");
        assert!(bridge.is_dir);
        assert_eq!(bridge.children, None, "桥根 children 惰性 None");
        assert_eq!(bridge.size, 0);
        assert_eq!(bridge.mtime_ms, 0);
    }
}
