//! 照片图库虚拟根(R7,Ruling P:照片库对其他设备只读可浏览/可下载,
//! App 内零照片 UI)。
//!
//! 架构(iOS 引擎形态):App 侧常驻**照片桥应答器**(Kit `PhotoResponder`,
//! 经 fdpass 通道 `{home}/fds.sock` 回帧),引擎侧 [`PhotoBridge`] 为请求方:
//! - **列表**(0x06):引擎广播 `fs.photo_list {"path","request_id"}` 事件,
//!   App 回 `{entries, ok, request_id, truncated}`(字母序;entries 逐项
//!   `{children?, is_dir, mtime_ms, name, photo_id?, size}`)——path = 虚拟
//!   子路径("" = 根 = 相册列表;相册名 = 相册内照片);
//! - **导出**(0x07):引擎广播 `fs.photo_export {"photo_id","request_id"}`
//!   事件,App 导出**原始字节**(PHAssetResource,R5c 导出代码复用)到
//!   App 自有临时目录(`{home}/photo-serve/`,在 `.mtdss` 暂存区命名空间
//!   之外——R8 终审 I-2,见 [`serve_dir`])后回
//!   `{ok, path, request_id}`——引擎按普通文件读取/发送,传输终态后
//!   unlink(App 侧另有启动清扫)。
//!
//! **虚拟根路由**(ctrl/jobs 面消费):对端路径以 [`PHOTO_ROOT`]
//! (`/照片图库`)为首段 = 照片虚拟路径,不经授权根守卫/文件系统(桥可用
//! 性即授权——App 只在照片权限 authorized/limited 时应答);对 photo 根的
//! 写/删/移/复制一律拒(只读来源)。[`photo_vpath`] 做词法规范化后的前缀
//! 判定:`/照片图库/../x` 规范化为 `/x` 不再是照片路径(回落 fs 面,守卫
//! 拒绝),`/照片图库foo` 整段不匹配——无逃逸面。
//!
//! **门控**:状态内 `Option<PhotoBridge>`(`DaemonOpts::photo_serve`,
//! iOS/android 引擎进程注入,bin 形态恒关)——桌面缺省 None,ctrl/jobs 的
//! 照片臂整体不进(行为逐字节不变)。android 侧权限即数据面:App 未授权
//! 时不应答(Kotlin PhotoResponder 权限门)→ 等待超时 → 根隐藏。桥不可用
//! (App 未授权/应答器未跑/App 已死)→
//! 请求超时/ok:false → 虚拟根隐藏(ctrl 列根不追加)/浏览报错,重试自然
//! 重探(每次访问都是新请求)。
//!
//! **缓存**:列表应答整表落 `path → DirEntryData` 缓存(完整虚拟路径为
//! 键),stat 面命中即回、未命中重列父目录再查(简报口径:stat 走缓存
//! 列表数据,miss 重列)。缓存随查询滚动刷新,无失效面(照片元数据变化
//! 由下次列表自然覆盖)。
//!
//! 通道纪律与 [`crate::jobs::DestQueries`] 同款:登记先于广播、id 单调、
//! 超时即摘(迟到应答按未知 request_id 拒,ack ok:false,表不积陈项)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use mtdss_core::fs::DirEntryData;
use mtdss_ipc::EventFrame;
use serde_json::{json, Value};

/// 虚拟根目录名(对端根列表里的显示名/路径段;与全 App 语言一致)。
// 临时 allow:R7 ctrl/jobs 集成(紧随的下一提交)即消费;fdpass 交付面
// 本就 unix 门,Windows 编译单元在集成前为 dead(DestQueries 先案)。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const PHOTO_ROOT_NAME: &str = "照片图库";
/// 虚拟根完整路径(ctrl/jobs 面的路径前缀判定锚)。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const PHOTO_ROOT: &str = "/照片图库";
/// 列表请求事件(daemon → App;App 经 0x06 应答)。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const EV_PHOTO_LIST: &str = "fs.photo_list";
/// 导出请求事件(daemon → App;App 经 0x07 应答)。
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const EV_PHOTO_EXPORT: &str = "fs.photo_export";

/// 列表应答等待上限(冷桥/大相册 App 侧 fetch 的合理余量)。
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// 导出应答等待上限(原始字节导出含 iCloud 拉取可能,宽裕收口)。
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);

/// 测试注入(`MTDSS_TEST_PHOTO_TIMEOUT_MS`,沿 MTDSS_TEST_* 家族口径):
/// 注入值同时覆盖列表/导出两窗(e2e 不真等 10s)。
fn injected_timeout() -> Option<Duration> {
    std::env::var("MTDSS_TEST_PHOTO_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
}

fn list_timeout() -> Duration {
    injected_timeout().unwrap_or(LIST_TIMEOUT)
}

fn export_timeout() -> Duration {
    injected_timeout().unwrap_or(EXPORT_TIMEOUT)
}

/// App 照片列表条目(0x06 entries 逐项的引擎侧投影;`photo_id` 仅文件项
/// ——0x07 导出凭据,目录项无)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhotoEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: u64,
    /// 目录项的直接子项数(相册 = 照片数,T5b「N 项」语义;文件项 None)。
    pub children: Option<u32>,
    /// 文件项的 App 侧资产标识(0x07 导出凭据;目录项 None)。
    pub photo_id: Option<String>,
}

#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // unix 交付面(parse 链);Windows 编译单元在 fdpass 门外
/// 条目名防御(清单 rel 校验的前置过滤):名字含分隔符/冒号/`.`/`..`/
/// `.mtdss` 保留段的条目跳过(留痕)——清单 rel 校验(拒 `:`/`\`)与
/// intake 保留段不因单条坏名整单失败。
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case(".mtdss")
}

/// 照片桥(daemon 侧请求方;每引擎一份,状态持有 `Option<Arc<_>>`)。
#[cfg_attr(not(unix), allow(dead_code))]
/// 列表应答形:entries + truncated 标志(0x06 回包解析面)。
type ListReply = Result<(Vec<PhotoEntry>, bool), String>;
/// 导出应答形:引擎可读的临时件路径(0x07 回包解析面)。
type ExportReply = Result<PathBuf, String>;

pub(crate) struct PhotoBridge {
    /// 事件广播出口(fs.photo_list/fs.photo_export 与作业事件同一 channel,
    /// 已连 IPC 客户端 = App 的订阅面)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// 列表等待面:request_id → 等待方(oneshot,携解析结果)。
    pending_list: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ListReply>>>,
    /// 导出等待面:request_id → 等待方。
    pending_export: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ExportReply>>>,
    /// request_id 单调源(与 0x04/0x05 的空间独立,通道实例各异)。
    next_id: AtomicU64,
    /// 列表窗(测试注入小值)。
    list_timeout: Duration,
    /// 导出窗(测试注入小值)。
    export_timeout: Duration,
    /// 列表缓存:完整虚拟路径 → 条目(+文件项 photo_id 凭据;stat 面数据
    /// 源,miss 重列父目录)。
    cache: Mutex<HashMap<String, CachedEntry>>,
    /// 根探针负缓存(R7 评审 I-1b):桥缺席(超时/ok:false/无订阅)结果
    /// 在 TTL 内不再串行重探——列根追加面直接返回 None(根隐藏),防对端
    /// 反复刷新被探针串行拖住。成功探到即清。
    root_hidden_until: Mutex<Option<std::time::Instant>>,
}

/// 根探针负缓存 TTL:远大于 App 重启/授权翻转的瞬时窗,远小于用户感知
/// 的一次刷新周期——缺席后 30s 内不再探,30s 后下一次访问自然重试。
pub(crate) const ROOT_HIDDEN_TTL: Duration = Duration::from_secs(30);

/// 缓存条目:ctrl stat 面的 DirEntryData + 照片件导出凭据(0x06 文件项
/// 透传;目录项 None)。
#[derive(Debug, Clone)]
pub(crate) struct CachedEntry {
    pub data: DirEntryData,
    pub photo_id: Option<String>,
}

impl PhotoBridge {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        Self {
            events,
            pending_list: Mutex::new(HashMap::new()),
            pending_export: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            list_timeout: list_timeout(),
            export_timeout: export_timeout(),
            cache: Mutex::new(HashMap::new()),
            root_hidden_until: Mutex::new(None),
        }
    }

    /// 列虚拟子路径("":根 = 相册列表;相册名:相册内照片)。成功即整表
    /// 落缓存。Err = 桥不可用(超时/ok:false/应答畸形)。
    pub(crate) async fn list(&self, sub: &str) -> Result<(Vec<PhotoEntry>, bool), String> {
        // 快失败(R7 评审 I-1a):零订阅者 = App 必然不在(无任何已连 IPC
        // 客户端),广播必无人接——不等应答窗,立即收口。
        if self.events.receiver_count() == 0 {
            return Err("照片桥不可用(应用未连接)".into());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // 登记先于广播:事件落地即必有等待方,无丢失窗口
        self.pending_list.lock().unwrap().insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_PHOTO_LIST.into(),
            params: json!({ "request_id": request_id, "path": sub }),
        });
        let outcome = tokio::time::timeout(self.list_timeout, rx).await;
        // 先摘 pending(超时/异常路径):迟到应答按未知 request_id 拒,不积陈项
        self.pending_list.lock().unwrap().remove(&request_id);
        match outcome {
            Ok(Ok(Ok((entries, truncated)))) => {
                self.cache_entries(sub, &entries);
                Ok((entries, truncated))
            }
            Ok(Ok(Err(m))) => Err(m),
            // 等待方通道异常关闭(理论不可达:sender 恒先 send 或随 remove Drop)
            Ok(Err(_)) => Err("照片桥应答通道异常关闭".into()),
            Err(_) => Err(format!(
                "等待 App 照片列表应答超时({:?};照片权限未授予或应用未运行)",
                self.list_timeout
            )),
        }
    }

    /// 导出一张照片(原始字节落 App 临时目录,回其绝对路径)。Err 文案
    /// 直达作业 failed(驱动消费)。
    pub(crate) async fn export(&self, photo_id: &str) -> Result<PathBuf, String> {
        // 快失败同 [`Self::list`] 注。
        if self.events.receiver_count() == 0 {
            return Err("照片桥不可用(应用未连接)".into());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending_export.lock().unwrap().insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_PHOTO_EXPORT.into(),
            params: json!({ "photo_id": photo_id, "request_id": request_id }),
        });
        let outcome = tokio::time::timeout(self.export_timeout, rx).await;
        self.pending_export.lock().unwrap().remove(&request_id);
        match outcome {
            Ok(Ok(Ok(path))) => Ok(path),
            Ok(Ok(Err(m))) => Err(m),
            Ok(Err(_)) => Err("照片桥应答通道异常关闭".into()),
            Err(_) => Err(format!(
                "等待 App 照片导出应答超时({:?};照片权限未授予或应用未运行)",
                self.export_timeout
            )),
        }
    }

    /// fdpass 0x06 落点(FdBridgeHandler 分派):解析 payload 交付等待方。
    /// false = 无此等待方(已超时/已应答),调用方回 ack ok:false。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);宿主单测直驱
    pub(crate) fn deliver_list(&self, request_id: u64, payload: Value) -> bool {
        let parsed = parse_list_reply(&payload);
        match self.pending_list.lock().unwrap().remove(&request_id) {
            Some(tx) => tx.send(parsed).is_ok(),
            None => false,
        }
    }

    /// fdpass 0x07 落点(同 [`Self::deliver_list`])。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);宿主单测直驱
    pub(crate) fn deliver_export(&self, request_id: u64, payload: Value) -> bool {
        let parsed = parse_export_reply(&payload);
        match self.pending_export.lock().unwrap().remove(&request_id) {
            Some(tx) => tx.send(parsed).is_ok(),
            None => false,
        }
    }

    /// 虚拟根的目录条目(列根追加面):相册列表成功 → 根条目(children =
    /// 相册数,负缓存清);桥不可用 → None(根隐藏,负缓存落 TTL——TTL 内
    /// 的重复列根不再串行重探,R7 评审 I-1b)。
    pub(crate) async fn root_listing_entry(&self) -> Option<DirEntryData> {
        if let Some(until) = self.root_hidden_until.lock().unwrap().as_ref() {
            if std::time::Instant::now() < *until {
                return None;
            }
        }
        match self.list("").await {
            Ok((entries, _)) => {
                *self.root_hidden_until.lock().unwrap() = None;
                let entry = DirEntryData {
                    name: PHOTO_ROOT_NAME.into(),
                    path: PHOTO_ROOT.into(),
                    size: 0,
                    mtime_ms: 0,
                    is_dir: true,
                    children: Some(u32::try_from(entries.len()).unwrap_or(u32::MAX)),
                };
                self.cache.lock().unwrap().insert(
                    PHOTO_ROOT.into(),
                    CachedEntry {
                        data: entry.clone(),
                        photo_id: None,
                    },
                );
                Some(entry)
            }
            Err(err) => {
                tracing::info!(%err, "照片桥不可用,虚拟根隐藏({ROOT_HIDDEN_TTL:?} 内不再重探)");
                *self.root_hidden_until.lock().unwrap() =
                    Some(std::time::Instant::now() + ROOT_HIDDEN_TTL);
                None
            }
        }
    }

    /// 虚拟路径单查:缓存命中即回;未命中重列父目录后复查(简报口径);
    /// 根自身 children 来自根列表。桥不可用/条目不存在 → FsError(ctrl
    /// Stat 臂原码透传)。
    pub(crate) async fn stat(&self, vpath: &str) -> Result<DirEntryData, mtdss_core::fs::FsError> {
        if let Some(hit) = self.cached(vpath) {
            return Ok(hit);
        }
        // miss:根 → 重列根;子项 → 重列父目录(列表应答整表落缓存)
        let parent = parent_vpath(vpath);
        let list_sub = match parent.as_deref() {
            None => Err(mtdss_core::fs::FsError::NotFound(vpath.to_string())),
            Some(PHOTO_ROOT) => {
                // 父 = 根:列根("")
                self.list("")
                    .await
                    .map(|_| ())
                    .map_err(|e| mtdss_core::fs::FsError::NotFound(format!("照片图库不可用: {e}")))
            }
            Some(p) => {
                let sub = p[PHOTO_ROOT.len()..].trim_start_matches('/').to_string();
                self.list(&sub)
                    .await
                    .map(|_| ())
                    .map_err(|e| mtdss_core::fs::FsError::NotFound(format!("照片图库不可用: {e}")))
            }
        };
        list_sub?;
        self.cached(vpath)
            .ok_or_else(|| mtdss_core::fs::FsError::NotFound(vpath.to_string()))
    }

    /// 单查 + 导出凭据(照片源解析面;jobs):条目数据与 photo_id 一并
    /// 返回(文件项 Some;目录/根 None)。
    pub(crate) async fn stat_with_id(
        &self,
        vpath: &str,
    ) -> Result<(DirEntryData, Option<String>), mtdss_core::fs::FsError> {
        self.stat(vpath).await?;
        let hit = self.cache.lock().unwrap().get(vpath).cloned();
        match hit {
            Some(c) => Ok((c.data, c.photo_id)),
            None => Err(mtdss_core::fs::FsError::NotFound(vpath.to_string())),
        }
    }

    fn cached(&self, vpath: &str) -> Option<DirEntryData> {
        self.cache.lock().unwrap().get(vpath).map(|c| c.data.clone())
    }

    /// 列表应答整表落缓存(键 = 完整虚拟路径;坏名条目 parse 期已滤)。
    fn cache_entries(&self, sub: &str, entries: &[PhotoEntry]) {
        let mut cache = self.cache.lock().unwrap();
        for e in entries {
            let full = join_vpath(sub, &e.name);
            cache.insert(
                full.clone(),
                CachedEntry {
                    data: DirEntryData {
                        name: e.name.clone(),
                        path: full,
                        size: e.size,
                        mtime_ms: e.mtime_ms,
                        is_dir: e.is_dir,
                        children: e.children,
                    },
                    photo_id: e.photo_id.clone(),
                },
            );
        }
    }
}

/// 原始对端路径 → 照片子路径(词法规范化后的前缀判定;路由/守卫共用):
/// - `/照片图库` → `Some("")`(根);
/// - `/照片图库/相册/x` → `Some("相册/x")`;
/// - 规范化后越出根(`/照片图库/../x` = `/x`)、非前缀形态、相对/NUL/
///   空 → `None`(回落 fs 面,授权守卫按集外拒)。
pub(crate) fn photo_vpath(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let norm = crate::roots::normalize_lexical(Path::new(raw)).ok()?;
    let s = norm.to_string_lossy().into_owned();
    if s == PHOTO_ROOT {
        return Some(String::new());
    }
    s.strip_prefix(&format!("{PHOTO_ROOT}/"))
        .map(|rest| rest.trim_end_matches('/').to_string())
        .filter(|rest| !rest.is_empty())
}

/// 子路径 → 完整虚拟路径("":根;相册:x;相册内:a/b)。
pub(crate) fn full_vpath(sub: &str) -> String {
    if sub.is_empty() {
        PHOTO_ROOT.to_string()
    } else {
        format!("{PHOTO_ROOT}/{sub}")
    }
}

/// 子路径 + 条目名 → 条目完整虚拟路径。
pub(crate) fn join_vpath(sub: &str, name: &str) -> String {
    if sub.is_empty() {
        format!("{PHOTO_ROOT}/{name}")
    } else {
        format!("{PHOTO_ROOT}/{sub}/{name}")
    }
}

/// 导出临时目录锚(`{home}/photo-serve`;与 App 侧
/// EngineLayout.photoServeDirectory 同布局,两侧不得漂移)。**在 `.mtdss`
/// 暂存区命名空间之外**(R8 终审 I-2 重定位):iOS 收件暂存区注入
/// `{home}/.mtdss`,对端 sender 自选顶层 rel "photo-serve" 的收件中间态
/// 会暂存/落位于 `<staging>/photo-serve`——serve 目录若落暂存区内,App
/// 挂载清扫/逐请求目录建删会在中断窗摧毁已暂存终件;home 为 App 私有
/// 目录(Files App 不可见),移出即断开同命名空间(协议侧再把
/// "photo-serve" 列入 rel 保留首段,纵深防御)。
pub(crate) fn serve_dir(home: &Path) -> PathBuf {
    home.join("photo-serve")
}

/// 回传路径是否落在导出临时目录内(R7 评审 cheap-a,读/清理双面的防御
/// 约束:App 侧 bug 不得诱使引擎读/删前缀外文件)。词法面:规范化后整
/// 组件前缀(`Path::starts_with`);非 UTF-8/非法形态一律不在(fail-closed)。
pub(crate) fn path_under_serve_dir(home: &Path, p: &Path) -> bool {
    match (
        crate::roots::normalize_lexical(&serve_dir(home)),
        crate::roots::normalize_lexical(p),
    ) {
        (Ok(dir), Ok(path)) => path.starts_with(&dir),
        _ => false,
    }
}

/// 完整虚拟路径 → 父完整路径(根自身 → None)。
pub(crate) fn parent_vpath(vpath: &str) -> Option<String> {
    debug_assert!(vpath.starts_with(PHOTO_ROOT));
    if vpath == PHOTO_ROOT {
        return None;
    }
    let parent = match vpath.rfind('/') {
        Some(idx) if idx > PHOTO_ROOT.len() - 1 => vpath[..idx].to_string(),
        _ => PHOTO_ROOT.to_string(),
    };
    Some(parent)
}

/// 子路径的清单 rel 基段(目录输入的展开前缀,collect_items 语义:rel =
/// 输入名/内部相对路径):`""` → 根名;`最近项目` → `最近项目`;
/// `a/b` → `b`(输入末段)。
pub(crate) fn rel_base(sub: &str) -> String {
    if sub.is_empty() {
        PHOTO_ROOT_NAME.to_string()
    } else {
        sub.rsplit('/').next().unwrap_or(sub).to_string()
    }
}

/// 0x06 应答解析:`{entries, ok, request_id, truncated}`(ok:false + error
/// = 桥不可用;畸形 = 桥异常)。坏名条目跳过留痕(不整单失败)。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 消费面 = 0x06 交付(unix 门)
fn parse_list_reply(payload: &Value) -> Result<(Vec<PhotoEntry>, bool), String> {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| "photo_list 应答缺 ok 字段".to_string())?;
    if !ok {
        let err = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("照片桥不可用(未授权或未运行)");
        return Err(format!("App 照片桥拒绝: {err}"));
    }
    let arr = payload
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "photo_list 应答缺 entries 数组".to_string())?;
    let mut entries = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            tracing::warn!(entry = %item, "photo_list 条目缺 name,跳过");
            continue;
        };
        if !name_ok(name) {
            tracing::warn!(name = %name, "photo_list 条目名非法,跳过");
            continue;
        }
        entries.push(PhotoEntry {
            name: name.to_string(),
            is_dir: item.get("is_dir").and_then(Value::as_bool).unwrap_or(false),
            size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
            mtime_ms: item.get("mtime_ms").and_then(Value::as_u64).unwrap_or(0),
            children: item
                .get("children")
                .and_then(Value::as_u64)
                .and_then(|c| u32::try_from(c).ok()),
            photo_id: item
                .get("photo_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    let truncated = payload
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok((entries, truncated))
}

/// 0x07 应答解析:`{ok, path, request_id}` / `{error, ok:false, …}`。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 消费面 = 0x07 交付(unix 门)
fn parse_export_reply(payload: &Value) -> Result<PathBuf, String> {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| "photo_export 应答缺 ok 字段".to_string())?;
    if !ok {
        let err = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("照片导出被拒");
        return Err(format!("App 照片导出失败: {err}"));
    }
    let path = payload
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "photo_export 应答缺 path".to_string())?;
    if path.is_empty() {
        return Err("App 照片导出失败: 空 path".into());
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 前缀判定矩阵(UNIX 形态;生产面只在 iOS 引擎,测试与 e2e 同 unix):
    /// 根/子路径/深层/噪声分隔符识别;越出根/字节前缀/相对/空不识别。
    #[cfg(unix)]
    #[test]
    fn photo_vpath_matrix() {
        assert_eq!(photo_vpath("/照片图库").as_deref(), Some(""));
        assert_eq!(photo_vpath("/照片图库/").as_deref(), Some(""));
        assert_eq!(photo_vpath("/照片图库//最近项目").as_deref(), Some("最近项目"));
        assert_eq!(
            photo_vpath("/照片图库/最近项目/IMG_1.JPG").as_deref(),
            Some("最近项目/IMG_1.JPG")
        );
        // 词法噪声:./.. 不越出根即在集内
        assert_eq!(
            photo_vpath("/照片图库/最近项目/../最近项目/x").as_deref(),
            Some("最近项目/x")
        );
        // 字节前缀不匹配(整段判定)
        assert_eq!(photo_vpath("/照片图库x"), None);
        assert_eq!(photo_vpath("/照片图库x/y"), None);
        // 越出根:规范化后落 fs 面,不再是照片路径
        assert_eq!(photo_vpath("/照片图库/../etc"), None);
        assert_eq!(photo_vpath("/照片图库/../../x"), None);
        // 相对/空/NUL 不识别
        assert_eq!(photo_vpath("照片图库"), None);
        assert_eq!(photo_vpath(""), None);
        assert_eq!(photo_vpath("/a\0b"), None);
        // 集内但有 fs 面锚问题:规范化失败形态按 None 收口
        assert_eq!(photo_vpath("/照片图库/.."), None); // 规范化 = "/"
    }

    #[cfg(unix)]
    #[test]
    fn vpath_helpers() {
        assert_eq!(full_vpath(""), "/照片图库");
        assert_eq!(full_vpath("最近项目"), "/照片图库/最近项目");
        assert_eq!(join_vpath("", "a.jpg"), "/照片图库/a.jpg");
        assert_eq!(join_vpath("相册", "a.jpg"), "/照片图库/相册/a.jpg");
        assert_eq!(parent_vpath("/照片图库"), None);
        assert_eq!(parent_vpath("/照片图库/相册").as_deref(), Some("/照片图库"));
        assert_eq!(
            parent_vpath("/照片图库/相册/x.jpg").as_deref(),
            Some("/照片图库/相册")
        );
        assert_eq!(rel_base(""), "照片图库");
        assert_eq!(rel_base("最近项目"), "最近项目");
        assert_eq!(rel_base("a/b"), "b");
    }

    /// 条目名防御:分隔符/保留段拒绝,常规中英名与 emoji 通过。
    #[test]
    fn name_defense() {
        assert!(name_ok("IMG_1.JPG"));
        assert!(name_ok("最近项目"));
        assert!(name_ok("旅游 🏔.heic"));
        assert!(!name_ok(""));
        assert!(!name_ok("a/b"));
        assert!(!name_ok("a\\b"));
        assert!(!name_ok("a:b"));
        assert!(!name_ok("."));
        assert!(!name_ok(".."));
        assert!(!name_ok(".mtdss"));
        assert!(!name_ok(".MTDSS"));
    }

    /// 0x06/0x07 应答解析:成功/拒答/畸形三臂(纯函数直测)。
    #[test]
    fn reply_parsing() {
        let ok_list = serde_json::json!({
            "entries": [
                {"name": "最近项目", "is_dir": true, "size": 0, "mtime_ms": 0, "children": 2},
                {"name": "IMG_1.JPG", "is_dir": false, "size": 123, "mtime_ms": 456, "photo_id": "pid-1"},
                {"name": "bad/name", "is_dir": false},
            ],
            "ok": true,
            "request_id": 7,
            "truncated": false,
        });
        let (entries, truncated) = parse_list_reply(&ok_list).unwrap();
        assert!(!truncated);
        assert_eq!(entries.len(), 2, "坏名条目跳过");
        assert_eq!(entries[0].children, Some(2));
        assert_eq!(entries[1].photo_id.as_deref(), Some("pid-1"));

        let refused = serde_json::json!({"error": "未授权", "ok": false, "request_id": 7});
        assert!(parse_list_reply(&refused).unwrap_err().contains("未授权"));

        assert!(parse_list_reply(&serde_json::json!({"ok": true})).is_err());
        assert!(parse_export_reply(&serde_json::json!({"ok": true, "path": "/tmp/x", "request_id": 1})).is_ok());
        assert!(parse_export_reply(&serde_json::json!({"ok": true, "request_id": 1})).is_err());
        assert!(parse_export_reply(&serde_json::json!({"ok": false, "error": "资产不可用"}))
            .unwrap_err()
            .contains("资产不可用"));
    }

    /// 状态机(伪事件通道,无 fdpass):登记 → 广播 → 交付 → 等待落定;
    /// 超时摘除后迟到应答按未知 request_id 拒(false);缓存命中/重列面。
    #[tokio::test]
    async fn bridge_roundtrip_timeout_and_cache() {
        let (events, mut rx) = tokio::sync::broadcast::channel(16);
        // 注入小超时(构造期读定)
        std::env::set_var("MTDSS_TEST_PHOTO_TIMEOUT_MS", "300");
        let bridge = Arc::new(PhotoBridge::new(events));

        // ① 超时 + 迟到应答按未知 id 拒
        let b = bridge.clone();
        let wait = tokio::spawn(async move { b.list("最近项目").await });
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.method, EV_PHOTO_LIST);
        let id = frame.params["request_id"].as_u64().unwrap();
        assert_eq!(frame.params["path"], "最近项目");
        let out = wait.await.unwrap();
        assert!(out.is_err(), "无应答应超时: {out:?}");
        assert!(
            !bridge.deliver_list(id, serde_json::json!({"ok": true, "entries": [], "truncated": false})),
            "迟到应答按未知 request_id 拒"
        );

        // ② 正常往返 + 缓存
        let b2 = bridge.clone();
        let wait = tokio::spawn(async move { b2.list("最近项目").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({
                "entries": [
                    {"name": "IMG_1.JPG", "is_dir": false, "size": 9, "mtime_ms": 5, "photo_id": "pid-1"},
                    {"name": "IMG_2.JPG", "is_dir": false, "size": 11, "mtime_ms": 6, "photo_id": "pid-2"},
                ],
                "ok": true,
                "request_id": id,
                "truncated": false,
            }),
        ));
        let (entries, truncated) = wait.await.unwrap().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(!truncated);
        // stat 命中缓存(不再发事件:事件通道静默即证)
        let hit = bridge.stat("/照片图库/最近项目/IMG_1.JPG").await.unwrap();
        assert_eq!(hit.size, 9);
        assert_eq!(hit.path, "/照片图库/最近项目/IMG_1.JPG");
        // ③ miss → 重列父目录(事件再发)→ 缓存命中(IMG_3 从未列出,
        // 缓存外——IMG_1/IMG_2 已随 ② 的整表落缓存,直查命中不发事件)
        let b3 = bridge.clone();
        let wait = tokio::spawn(async move { b3.stat("/照片图库/最近项目/IMG_3.JPG").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({
                "entries": [
                    {"name": "IMG_3.JPG", "is_dir": false, "size": 13, "mtime_ms": 7, "photo_id": "pid-3"},
                ],
                "ok": true,
                "request_id": id,
                "truncated": false,
            }),
        ));
        let hit = wait.await.unwrap().unwrap();
        assert_eq!(hit.size, 13);
        // photo_id 缓存面(导出凭据)
        let (_, pid) = bridge
            .stat_with_id("/照片图库/最近项目/IMG_1.JPG")
            .await
            .unwrap();
        assert_eq!(pid.as_deref(), Some("pid-1"));

        // ④ 导出往返
        let b4 = bridge.clone();
        let wait = tokio::spawn(async move { b4.export("pid-1").await });
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.method, EV_PHOTO_EXPORT);
        assert_eq!(frame.params["photo_id"], "pid-1");
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_export(
            id,
            serde_json::json!({"ok": true, "path": "/tmp/serve/IMG_1.JPG", "request_id": id}),
        ));
        assert_eq!(wait.await.unwrap().unwrap(), PathBuf::from("/tmp/serve/IMG_1.JPG"));
        std::env::remove_var("MTDSS_TEST_PHOTO_TIMEOUT_MS");
    }

    /// 快失败(R7 评审 I-1a):零订阅者 = App 必然不在,list/export 立即
    /// Err,不等应答窗。
    #[tokio::test]
    async fn list_export_fast_fail_with_zero_receivers() {
        // receiver 用 `_` 立即弃(留绑定会让 receiver_count==1,快失败不触发)
        let (events, _) = tokio::sync::broadcast::channel::<EventFrame>(16);
        let bridge = Arc::new(PhotoBridge::new(events));
        let started = std::time::Instant::now();
        let err = bridge.list("").await.unwrap_err();
        assert!(err.contains("应用未连接"), "快失败文案: {err}");
        assert!(started.elapsed() < Duration::from_secs(2), "零订阅须即回: {:?}", started.elapsed());
        let err = bridge.export("pid").await.unwrap_err();
        assert!(err.contains("应用未连接"), "导出快失败文案: {err}");
    }

    /// 根探针负缓存(R7 评审 I-1b):有订阅者但无应答 → 超时 None 且落
    /// 负缓存;TTL 内重复列根不再发事件(即回 None);强翻过期 → 第三次
    /// 重新探(事件再发)。
    #[tokio::test]
    async fn root_probe_negative_cache_expires() {
        let (events, mut rx) = tokio::sync::broadcast::channel(16);
        std::env::set_var("MTDSS_TEST_PHOTO_TIMEOUT_MS", "300");
        let bridge = Arc::new(PhotoBridge::new(events));

        // ① 首探:有订阅者无应答 → 超时 None
        let b = bridge.clone();
        let first = tokio::spawn(async move { b.root_listing_entry().await });
        let _frame = rx.recv().await.unwrap();
        assert!(first.await.unwrap().is_none(), "无应答 → None");

        // ② TTL 内重复列根:即回 None,零事件(不重探)
        let started = std::time::Instant::now();
        assert!(bridge.root_listing_entry().await.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "负缓存须即回: {:?}",
            started.elapsed()
        );
        assert!(
            rx.try_recv().is_err(),
            "负缓存命中不得再发事件"
        );

        // ③ 强翻过期:重新探(事件再发;交付成功 → Some + 负缓存清)
        *bridge.root_hidden_until.lock().unwrap() =
            Some(std::time::Instant::now() - Duration::from_secs(1));
        let b2 = bridge.clone();
        let third = tokio::spawn(async move { b2.root_listing_entry().await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({
                "entries": [
                    {"children": 2, "is_dir": true, "mtime_ms": 0, "name": "最近项目", "size": 0},
                ],
                "ok": true,
                "request_id": id,
                "truncated": false,
            }),
        ));
        let entry = third.await.unwrap().unwrap();
        assert_eq!(entry.children, Some(1), "children = 应答表长度(交付 1 相册)");
        assert!(bridge.root_hidden_until.lock().unwrap().is_none(), "成功探到清负缓存");
        std::env::remove_var("MTDSS_TEST_PHOTO_TIMEOUT_MS");
    }

    /// 导出目录前缀判定(R7 评审 cheap-a):前缀内(含自身/深层)在;
    /// 越出/兄弟目录/前缀字节撞(serve-x)/非法形态不在。R8 终审 I-2:
    /// serve 目录 = `{home}/photo-serve`(暂存区命名空间外)。
    #[cfg(unix)]
    #[test]
    fn serve_dir_prefix_matrix() {
        let home = Path::new("/home/u/h");
        let under = |p: &str| path_under_serve_dir(home, Path::new(p));
        assert!(under("/home/u/h/photo-serve/1-IMG.jpg"));
        assert!(under("/home/u/h/photo-serve/1/d/a.jpg"));
        assert!(under("/home/u/h/photo-serve/"));
        // 字节前缀撞:photo-serve-x 不是 photo-serve 内
        assert!(!under("/home/u/h/photo-serve-x/f"));
        assert!(!under("/home/u/h/.mtdss/photo-serve/f"), "暂存区同名面不是 serve 目录内");
        assert!(!under("/home/u/h/other/f"));
        assert!(!under("/home/u/h/evil.bin"));
        assert!(!under("/etc/passwd"));
        // 噪声规范化后仍在(./ 消解)
        assert!(under("/home/u/h/photo-serve/./a/../b.jpg"));
        // 越出根:规范化后出前缀
        assert!(!under("/home/u/h/photo-serve/../../evil"));
        assert_eq!(serve_dir(home), PathBuf::from("/home/u/h/photo-serve"));
    }
}
