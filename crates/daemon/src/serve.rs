//! 服务桥(V3-4 §4.1 安卓服务桥;spec《2026-11-08-mtdss-v3p4-android-design》
//! §4.1「安卓服务桥 = 照片桥模式的泛化」):对端发起的浏览/下载/删除安卓
//! SAF 内容,路由 App 进程实现——引擎读不了 content://,对端到达的
//! bridge vpath(roots.set 登记的 `RootSpec{source:Bridge}` 命名空间)经
//! 本桥逐请求实问 App。
//!
//! 架构(photo.rs 逐段对照泛化):
//! - **列表**(0x08):引擎广播 `fs.serve_list {"root","sub","request_id"}`
//!   事件,App 回 `{entries, ok, request_id, truncated}`(entries 逐项
//!   `{children?, is_dir, mtime_ms, name, size}`;SAF 树任意深度)——
//!   root = 注册桥根 vpath(规范化形),sub = 根下子路径("" = 根自身);
//! - **导出**(0x09):引擎广播 `fs.serve_export {"root","sub","request_id"}`
//!   事件,App 导出**原始字节**到自有临时目录(`{home}/serve-tmp/`,在
//!   `.mtdss` 记账区命名空间之外——photo `serve_dir` R8 终审 I-2 同款
//!   先例)后回 `{ok, path, request_id}`——引擎按普通文件读取/发送,传输
//!   终态后 unlink(App 侧另有启动清扫);
//! - **删除**(0x0A):引擎广播 `fs.serve_delete {"root","sub","request_id"}`
//!   事件,App 侧 SAF 递归整树删除(一次调用 = 整树语义)后回
//!   `{deleted, ok, request_id}`;引擎把 ctrl delete 的 bridge vpath 逐条
//!   映射到本事件(删除安全门在客户端 UI,引擎侧 delete op 照常执行)。
//!
//! **路由判定**:路径命中任一注册桥根 vpath 前缀(词法规范化后整组件判定)
//! = 桥面;判定与守卫桥臂**同一口径**(`crate::roots::RootGuard::
//! bridge_route`,不另造判定)——`/收件箱/../x` 规范化为 `/x` 出集、
//! `/收件箱-x` 字节前缀不匹配,均回落 fs 面按集外拒。
//!
//! **绑定语义**(全局约束):写/移/复制到 bridge vpath 一律拒(只读来源,
//! ctrl 面收口);delete 是唯一例外。桥不可达(无应答/超时)→ 桥根从对端
//! 根列表隐藏(桥级负缓存 TTL,photo `ROOT_HIDDEN_TTL` 同款)、浏览/下载/
//! 删除诚实报错,不挂死;每次访问都是新请求,重试自然重探。
//!
//! **缓存不设**(与照片桥的分歧,简报钉死):列表每次实问——桥根集可经
//! roots.set 运行时热换,缓存会跨换根陈旧;stat 由「列父目录找条目」现答。
//!
//! 通道纪律与 [`crate::jobs::DestQueries`] 同款:登记先于广播、id 单调、
//! 超时即摘(迟到应答按未知 request_id 拒,ack ok:false,表不积陈项)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use mtdss_ipc::EventFrame;
use serde_json::{json, Value};

/// 列表请求事件(daemon → App;App 经 0x08 应答)。
pub(crate) const EV_SERVE_LIST: &str = "fs.serve_list";
/// 导出请求事件(daemon → App;App 经 0x09 应答)。
pub(crate) const EV_SERVE_EXPORT: &str = "fs.serve_export";
/// 删除请求事件(daemon → App;App 经 0x0A 应答)。
pub(crate) const EV_SERVE_DELETE: &str = "fs.serve_delete";

/// 列表应答等待上限(冷桥/大 SAF 树查询的合理余量)。
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// 导出应答等待上限(原始字节导出可能,photo 同款宽裕收口)。
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);
/// 删除应答等待上限(SAF deleteRow 批删除的合理余量)。
const DELETE_TIMEOUT: Duration = Duration::from_secs(10);

/// 测试注入(`MTDSS_TEST_SERVE_TIMEOUT_MS`,沿 MTDSS_TEST_* 家族口径):
/// 注入值同时覆盖列表/导出/删除三窗(e2e 不真等 10s)。
fn injected_timeout() -> Option<Duration> {
    std::env::var("MTDSS_TEST_SERVE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
}

/// App 服务列表条目(0x08 entries 逐项的引擎侧投影;无 photo_id 对位——
/// 导出凭据即 root/sub 本身)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServeEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: u64,
    /// 目录项的直接子项数(``N 项``语义;文件项 None)。
    pub children: Option<u32>,
}

/// 服务桥错误(两臂语义分歧进消费面):**Unreachable** = 桥不在(零订阅/
/// 超时/应答畸形)——进桥级负缓存,根列表整面隐藏 TTL;**Refused** = App
/// 在而拒答(ok:false + error 文案)——单请求失败,桥仍在,其余根照常。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServeErr {
    Unreachable(String),
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 构造面 = 应答解析(unix fdpass/宿主单测)
    Refused(String),
}

impl std::fmt::Display for ServeErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeErr::Unreachable(m) => write!(f, "{m}"),
            ServeErr::Refused(m) => write!(f, "{m}"),
        }
    }
}

/// 条目名防御(photo `name_ok` 同款口径):名字含分隔符/冒号/`.`/`..`/
/// `.mtdss` 保留段的条目跳过(留痕)——清单 rel 校验的前置过滤,不因单
/// 条坏名整单失败。jobs 的 rel 基段名判定同源复用。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 消费面 = parse_list_reply + jobs rel 基段(unix fdpass/宿主单测)
pub(crate) fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case(".mtdss")
}

/// 服务桥(daemon 侧请求方;每引擎至多一份,`state.serve` 槽持
/// `Option<Arc<_>>`——任一 bridge 根经 roots.set 注册即建,全 fs 根集撤)。
pub(crate) struct ServeBridge {
    /// 事件广播出口(fs.serve_* 与作业事件同一 channel,已连 IPC 客户端 =
    /// App 的订阅面)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
    /// 列表等待面:request_id → 等待方(oneshot,携解析结果)。
    pending_list: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ListReply>>>,
    /// 导出等待面:request_id → 等待方。
    pending_export: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<ExportReply>>>,
    /// 删除等待面:request_id → 等待方。
    pending_delete: Mutex<HashMap<u64, tokio::sync::oneshot::Sender<DeleteReply>>>,
    /// request_id 单调源(与 0x01/0x04/0x05/0x06/0x07 的空间独立,通道实例各异)。
    next_id: AtomicU64,
    /// 列表窗(测试注入小值)。
    list_timeout: Duration,
    /// 导出窗(测试注入小值)。
    export_timeout: Duration,
    /// 删除窗(测试注入小值)。
    delete_timeout: Duration,
    /// 桥不可达负缓存(V3-4 §4.1):Unreachable(零订阅/超时/畸形)结果在
    /// TTL 内不再串行重探——根列表探针直接隐藏(photo ROOT_HIDDEN_TTL 同
    /// 款)。App 拒答(Refused)**不落**此缓存:桥在,只是该请求被拒。
    unreachable_until: Mutex<Option<std::time::Instant>>,
}

/// 列表应答形:entries + truncated 标志(0x08 回包解析面)。
type ListReply = Result<(Vec<ServeEntry>, bool), ServeErr>;
/// 导出应答形:引擎可读的临时件路径(0x09 回包解析面)。
type ExportReply = Result<PathBuf, ServeErr>;
/// 删除应答形:目标是否真被删除(0x0A 回包解析面)。
type DeleteReply = Result<bool, ServeErr>;

/// 桥不可达负缓存 TTL(photo ROOT_HIDDEN_TTL 同值):远大于 App 重启/
/// 授权翻转的瞬时窗,远小于用户感知的一次刷新周期——缺席后 30s 内不再
/// 探,30s 后下一次访问自然重试。
pub(crate) const BRIDGE_UNREACHABLE_TTL: Duration = Duration::from_secs(30);

impl ServeBridge {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<EventFrame>) -> Self {
        Self {
            events,
            pending_list: Mutex::new(HashMap::new()),
            pending_export: Mutex::new(HashMap::new()),
            pending_delete: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            list_timeout: injected_timeout().unwrap_or(LIST_TIMEOUT),
            export_timeout: injected_timeout().unwrap_or(EXPORT_TIMEOUT),
            delete_timeout: injected_timeout().unwrap_or(DELETE_TIMEOUT),
            unreachable_until: Mutex::new(None),
        }
    }

    /// 快失败(photo 同款):零订阅者 = App 必然不在(无任何已连 IPC 客户
    /// 端),广播必无人接——不等应答窗,立即收口。
    fn fast_fail(&self) -> Option<ServeErr> {
        (self.events.receiver_count() == 0)
            .then(|| ServeErr::Unreachable("服务桥不可用(应用未连接)".into()))
    }

    /// 桥不可达负缓存落账/读取。
    fn note_unreachable(&self) {
        tracing::info!(
            ttl = ?BRIDGE_UNREACHABLE_TTL,
            "服务桥不可达,桥根隐藏(负缓存 TTL 内不再重探)"
        );
        *self.unreachable_until.lock().unwrap() =
            Some(std::time::Instant::now() + BRIDGE_UNREACHABLE_TTL);
    }

    fn unreachable_cached(&self) -> bool {
        self.unreachable_until
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|until| std::time::Instant::now() < *until)
    }

    /// 列 bridge vpath 子路径(sub "" = 根自身)。Err = 桥不可用/拒答
    /// (Unreachable 进负缓存)。
    pub(crate) async fn list(&self, root: &str, sub: &str) -> Result<(Vec<ServeEntry>, bool), ServeErr> {
        if let Some(e) = self.fast_fail() {
            self.note_unreachable();
            return Err(e);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // 登记先于广播:事件落地即必有等待方,无丢失窗口
        self.pending_list.lock().unwrap().insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_SERVE_LIST.into(),
            params: json!({ "request_id": request_id, "root": root, "sub": sub }),
        });
        let outcome = tokio::time::timeout(self.list_timeout, rx).await;
        // 先摘 pending(超时/异常路径):迟到应答按未知 request_id 拒,不积陈项
        self.pending_list.lock().unwrap().remove(&request_id);
        let out = match outcome {
            Ok(Ok(Ok(v))) => {
                // 应答即活:清桥级负缓存(下一次根探针免缓存直探)
                *self.unreachable_until.lock().unwrap() = None;
                return Ok(v);
            }
            Ok(Ok(Err(e))) => e,
            // 等待方通道异常关闭(理论不可达:sender 恒先 send 或随 remove Drop)
            Ok(Err(_)) => ServeErr::Unreachable("服务桥应答通道异常关闭".into()),
            Err(_) => ServeErr::Unreachable(format!(
                "等待 App 服务列表应答超时({:?};应用未运行或未应答)",
                self.list_timeout
            )),
        };
        if matches!(out, ServeErr::Unreachable(_)) {
            self.note_unreachable();
        }
        Err(out)
    }

    /// 导出一件(原始字节落 App 临时目录 `{home}/serve-tmp/`,回其绝对路
    /// 径)。Err 文案直达作业 failed(驱动消费)。
    pub(crate) async fn export(&self, root: &str, sub: &str) -> Result<PathBuf, ServeErr> {
        if let Some(e) = self.fast_fail() {
            self.note_unreachable();
            return Err(e);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending_export.lock().unwrap().insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_SERVE_EXPORT.into(),
            params: json!({ "request_id": request_id, "root": root, "sub": sub }),
        });
        let outcome = tokio::time::timeout(self.export_timeout, rx).await;
        self.pending_export.lock().unwrap().remove(&request_id);
        let out = match outcome {
            Ok(Ok(Ok(v))) => {
                *self.unreachable_until.lock().unwrap() = None;
                return Ok(v);
            }
            Ok(Ok(Err(e))) => e,
            Ok(Err(_)) => ServeErr::Unreachable("服务桥应答通道异常关闭".into()),
            Err(_) => ServeErr::Unreachable(format!(
                "等待 App 服务导出应答超时({:?};应用未运行或未应答)",
                self.export_timeout
            )),
        };
        if matches!(out, ServeErr::Unreachable(_)) {
            self.note_unreachable();
        }
        Err(out)
    }

    /// 删一件(整树语义:App 侧 SAF 递归删,一次调用)。Ok(false) = 目标
    /// 缺席(App 明言未删);Err = 桥不可用/拒答。
    pub(crate) async fn delete(&self, root: &str, sub: &str) -> Result<bool, ServeErr> {
        if let Some(e) = self.fast_fail() {
            self.note_unreachable();
            return Err(e);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending_delete.lock().unwrap().insert(request_id, tx);
        let _ = self.events.send(EventFrame {
            method: EV_SERVE_DELETE.into(),
            params: json!({ "request_id": request_id, "root": root, "sub": sub }),
        });
        let outcome = tokio::time::timeout(self.delete_timeout, rx).await;
        self.pending_delete.lock().unwrap().remove(&request_id);
        let out = match outcome {
            Ok(Ok(Ok(v))) => {
                *self.unreachable_until.lock().unwrap() = None;
                return Ok(v);
            }
            Ok(Ok(Err(e))) => e,
            Ok(Err(_)) => ServeErr::Unreachable("服务桥应答通道异常关闭".into()),
            Err(_) => ServeErr::Unreachable(format!(
                "等待 App 服务删除应答超时({:?};应用未运行或未应答)",
                self.delete_timeout
            )),
        };
        if matches!(out, ServeErr::Unreachable(_)) {
            self.note_unreachable();
        }
        Err(out)
    }

    /// fdpass 0x08 落点(FdBridgeHandler 分派):解析 payload 交付等待方。
    /// false = 无此等待方(已超时/已应答),调用方回 ack ok:false。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);宿主单测直驱
    pub(crate) fn deliver_list(&self, request_id: u64, payload: Value) -> bool {
        let parsed = parse_list_reply(&payload);
        match self.pending_list.lock().unwrap().remove(&request_id) {
            Some(tx) => tx.send(parsed).is_ok(),
            None => false,
        }
    }

    /// fdpass 0x09 落点(同 [`Self::deliver_list`])。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);宿主单测直驱
    pub(crate) fn deliver_export(&self, request_id: u64, payload: Value) -> bool {
        let parsed = parse_export_reply(&payload);
        match self.pending_export.lock().unwrap().remove(&request_id) {
            Some(tx) => tx.send(parsed).is_ok(),
            None => false,
        }
    }

    /// fdpass 0x0A 落点(同 [`Self::deliver_list`])。
    #[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 正式面经 FdBridgeHandler(unix 门);宿主单测直驱
    pub(crate) fn deliver_delete(&self, request_id: u64, payload: Value) -> bool {
        let parsed = parse_delete_reply(&payload);
        match self.pending_delete.lock().unwrap().remove(&request_id) {
            Some(tx) => tx.send(parsed).is_ok(),
            None => false,
        }
    }

    /// 桥根探针(根列表可见性收口,photo `root_listing_entry` 同款哲学):
    /// 桥级负缓存命中 → 隐藏(不重探);实问列表成功 → 可见(负缓存清);
    /// Unreachable → 隐藏 + 负缓存落 TTL;Refused → 仅该根隐藏(桥仍在,
    /// 其余根照常探)。
    pub(crate) async fn root_probe(&self, root: &str) -> bool {
        if self.unreachable_cached() {
            return false;
        }
        match self.list(root, "").await {
            Ok(_) => {
                *self.unreachable_until.lock().unwrap() = None;
                true
            }
            Err(ServeErr::Unreachable(_)) => {
                // list 内部已落负缓存
                false
            }
            Err(ServeErr::Refused(err)) => {
                tracing::info!(root, %err, "服务桥拒答,该根隐藏(其余根照常)");
                false
            }
        }
    }

    /// 单查(缓存不设——列表每次实问):根自身 → 探活成功即合成根条目
    /// (children 惰性 None,与根列表条目同形);子路径 → 列父目录找同名
    /// 条目。未命中/桥不可用 → FsError(ctrl Stat 臂原码透传)。`display` =
    /// 注册名(根条目 name;BridgeHit.name)。
    pub(crate) async fn stat(
        &self,
        root: &str,
        sub: &str,
        display: &str,
    ) -> Result<mtdss_core::fs::DirEntryData, mtdss_core::fs::FsError> {
        let unavailable =
            |e: ServeErr| mtdss_core::fs::FsError::NotFound(format!("服务目录不可用: {e}"));
        if sub.is_empty() {
            // 根自身:探活(list 成功)即合成条目——不另造 stat 协议面
            return self
                .list(root, "")
                .await
                .map(|_| mtdss_core::fs::DirEntryData {
                    name: display.to_string(),
                    path: root.to_string(),
                    size: 0,
                    mtime_ms: 0,
                    is_dir: true,
                    children: None,
                })
                .map_err(unavailable);
        }
        let parent = parent_sub(sub);
        let name = sub.rsplit('/').next().unwrap_or(sub);
        let (entries, _) = self.list(root, &parent).await.map_err(unavailable)?;
        entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| mtdss_core::fs::DirEntryData {
                name: e.name.clone(),
                path: join_vpath(root, &parent, &e.name),
                size: e.size,
                mtime_ms: e.mtime_ms,
                is_dir: e.is_dir,
                children: e.children,
            })
            .ok_or_else(|| mtdss_core::fs::FsError::NotFound(format!("{root}/{sub}")))
    }
}

/// 子路径 → 父子路径("" = 根;`a/b/c` → `a/b`)。
pub(crate) fn parent_sub(sub: &str) -> String {
    match sub.rfind('/') {
        Some(idx) => sub[..idx].to_string(),
        None => String::new(),
    }
}

/// 根 + 当前目录 sub + 条目名 → 条目完整虚拟路径。
pub(crate) fn join_vpath(root: &str, sub: &str, name: &str) -> String {
    if sub.is_empty() {
        format!("{root}/{name}")
    } else {
        format!("{root}/{sub}/{name}")
    }
}

/// 当前目录 sub + 条目名 → 子项的 sub(导出/下钻寻址)。
pub(crate) fn child_sub(sub: &str, name: &str) -> String {
    if sub.is_empty() {
        name.to_string()
    } else {
        format!("{sub}/{name}")
    }
}

/// 导出临时目录锚(`{home}/serve-tmp`;与 App 侧布局同构,两侧不得漂移)。
/// **在 `.mtdss` 记账区命名空间之外**(photo `serve_dir` R8 终审 I-2 同款
/// 先例):收件暂存/记账不与导出中间态同命名空间,App 挂载清扫/逐请求目录
/// 建删不会摧毁在途导出件;home 为 App 私有目录,外间不可见。
pub(crate) fn serve_dir(home: &Path) -> PathBuf {
    home.join("serve-tmp")
}

/// 回传路径是否落在导出临时目录内(photo `path_under_serve_dir` 同款防御:
/// 读/清理双面的约束——App 侧 bug 不得诱使引擎读/删前缀外文件)。词法面:
/// 规范化后整组件前缀;非 UTF-8/非法形态一律不在(fail-closed)。
pub(crate) fn path_under_serve_dir(home: &Path, p: &Path) -> bool {
    match (
        crate::roots::normalize_lexical(&serve_dir(home)),
        crate::roots::normalize_lexical(p),
    ) {
        (Ok(dir), Ok(path)) => path.starts_with(&dir),
        _ => false,
    }
}

/// 服务导出临时件回收(尽力而为):逐个 unlink,失败仅留痕(App 侧有启动
/// 清扫兜底,photo 同哲学)。删除面约束在导出临时目录前缀内(入表路径虽
/// 已验,清理面二次把门——前缀外路径拒删留痕)。
pub(crate) async fn cleanup_serve_temps(home: &Path, temps: &[PathBuf]) {
    if temps.is_empty() {
        return;
    }
    let temps = temps.to_vec();
    let serve = serve_dir(home);
    let _ = tokio::task::spawn_blocking(move || {
        for t in &temps {
            if !t.starts_with(&serve) {
                tracing::warn!(path = %t.display(), "服务导出临时件越出服务目录,拒删");
                continue;
            }
            if let Err(err) = std::fs::remove_file(t) {
                tracing::warn!(path = %t.display(), %err, "服务导出临时件清理失败(App 清扫兜底)");
            }
        }
    })
    .await;
}

/// 0x08 应答解析:`{entries, ok, request_id, truncated}`(ok:false + error
/// = App 拒答;畸形 = 桥异常)。坏名条目跳过留痕(不整单失败)。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 消费面 = 0x08/0x09/0x0A 交付(unix 门)
fn parse_list_reply(payload: &Value) -> ListReply {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| ServeErr::Unreachable("serve_list 应答缺 ok 字段".into()))?;
    if !ok {
        let err = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("服务桥拒答(根不可用)");
        return Err(ServeErr::Refused(format!("App 服务桥拒绝: {err}")));
    }
    let arr = payload
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| ServeErr::Unreachable("serve_list 应答缺 entries 数组".into()))?;
    let mut entries = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            tracing::warn!(entry = %item, "serve_list 条目缺 name,跳过");
            continue;
        };
        if !name_ok(name) {
            tracing::warn!(name = %name, "serve_list 条目名非法,跳过");
            continue;
        }
        entries.push(ServeEntry {
            name: name.to_string(),
            is_dir: item.get("is_dir").and_then(Value::as_bool).unwrap_or(false),
            size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
            mtime_ms: item.get("mtime_ms").and_then(Value::as_u64).unwrap_or(0),
            children: item
                .get("children")
                .and_then(Value::as_u64)
                .and_then(|c| u32::try_from(c).ok()),
        });
    }
    let truncated = payload
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok((entries, truncated))
}

/// 0x09 应答解析:`{ok, path, request_id}` / `{error, ok:false, …}`
/// (photo `parse_export_reply` 同款)。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 消费面 = 0x08/0x09/0x0A 交付(unix 门)
fn parse_export_reply(payload: &Value) -> ExportReply {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| ServeErr::Unreachable("serve_export 应答缺 ok 字段".into()))?;
    if !ok {
        let err = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("服务导出被拒");
        return Err(ServeErr::Refused(format!("App 服务导出失败: {err}")));
    }
    let path = payload
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| ServeErr::Unreachable("serve_export 应答缺 path".into()))?;
    if path.is_empty() {
        return Err(ServeErr::Unreachable("App 服务导出失败: 空 path".into()));
    }
    Ok(PathBuf::from(path))
}

/// 0x0A 应答解析:`{deleted, ok, request_id}`(deleted = 目标是否真被删;
/// 契约两字段钉死,缺 deleted 按畸形收口)。
#[cfg_attr(all(not(unix), not(test)), allow(dead_code))] // 消费面 = 0x08/0x09/0x0A 交付(unix 门)
fn parse_delete_reply(payload: &Value) -> DeleteReply {
    let ok = payload
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| ServeErr::Unreachable("serve_delete 应答缺 ok 字段".into()))?;
    if !ok {
        let err = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("服务删除被拒");
        return Err(ServeErr::Refused(format!("App 服务删除失败: {err}")));
    }
    let deleted = payload
        .get("deleted")
        .and_then(Value::as_bool)
        .ok_or_else(|| ServeErr::Unreachable("serve_delete 应答缺 deleted 字段".into()))?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 测试超时注入(本模块所有用例共用小窗;不 remove——同二进制内并发
    /// 用例都要求小窗,photo 单测的 set/remove 竞态面在此直接规避)。
    fn inject() {
        std::env::set_var("MTDSS_TEST_SERVE_TIMEOUT_MS", "300");
    }

    /// 条目名防御(photo name_defense 同款矩阵)。
    #[test]
    fn name_defense() {
        assert!(name_ok("报告.pdf"));
        assert!(name_ok("子目录"));
        assert!(name_ok("旅行 🏔.heic"));
        assert!(!name_ok(""));
        assert!(!name_ok("a/b"));
        assert!(!name_ok("a\\b"));
        assert!(!name_ok("a:b"));
        assert!(!name_ok("."));
        assert!(!name_ok(".."));
        assert!(!name_ok(".mtdss"));
        assert!(!name_ok(".MTDSS"));
    }

    /// 路径小工具:parent_sub / join_vpath / child_sub。
    #[test]
    fn vpath_helpers() {
        assert_eq!(parent_sub(""), "");
        assert_eq!(parent_sub("a"), "");
        assert_eq!(parent_sub("a/b"), "a");
        assert_eq!(parent_sub("a/b/c"), "a/b");
        assert_eq!(join_vpath("/收件箱", "", "x.pdf"), "/收件箱/x.pdf");
        assert_eq!(join_vpath("/收件箱", "a", "x.pdf"), "/收件箱/a/x.pdf");
        assert_eq!(child_sub("", "a"), "a");
        assert_eq!(child_sub("a", "b"), "a/b");
    }

    /// 0x08/0x09/0x0A 应答解析:成功/拒答/畸形三臂(纯函数直测)。
    #[test]
    fn reply_parsing() {
        let ok_list = serde_json::json!({
            "entries": [
                {"name": "子目录", "is_dir": true, "size": 0, "mtime_ms": 0, "children": 2},
                {"name": "报告.pdf", "is_dir": false, "size": 123, "mtime_ms": 456},
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
        assert_eq!(entries[1].children, None);

        let refused = serde_json::json!({"error": "URI 授权已失效", "ok": false, "request_id": 7});
        assert_eq!(
            parse_list_reply(&refused).unwrap_err(),
            ServeErr::Refused("App 服务桥拒绝: URI 授权已失效".into()),
            "ok:false = Refused(单请求失败,不进桥级负缓存)"
        );
        let malformed = parse_list_reply(&serde_json::json!({"ok": true})).unwrap_err();
        assert!(
            matches!(malformed, ServeErr::Unreachable(_)),
            "缺 entries = 畸形 → Unreachable: {malformed:?}"
        );

        assert!(
            parse_export_reply(&serde_json::json!({"ok": true, "path": "/tmp/x", "request_id": 1}))
                .is_ok()
        );
        assert!(matches!(
            parse_export_reply(&serde_json::json!({"ok": true, "request_id": 1})).unwrap_err(),
            ServeErr::Unreachable(_)
        ));
        assert_eq!(
            parse_export_reply(&serde_json::json!({"ok": false, "error": "源不可用"}))
                .unwrap_err(),
            ServeErr::Refused("App 服务导出失败: 源不可用".into())
        );

        assert!(
            parse_delete_reply(&serde_json::json!({"deleted": true, "ok": true, "request_id": 2}))
                .unwrap()
        );
        assert!(
            !parse_delete_reply(&serde_json::json!({"deleted": false, "ok": true, "request_id": 2}))
                .unwrap(),
            "目标缺席 = ok+deleted:false(NOT_FOUND 面)"
        );
        assert_eq!(
            parse_delete_reply(&serde_json::json!({"error": "SAF 拒绝", "ok": false}))
                .unwrap_err(),
            ServeErr::Refused("App 服务删除失败: SAF 拒绝".into())
        );
        let malformed = parse_delete_reply(&serde_json::json!({"ok": true})).unwrap_err();
        assert!(
            matches!(malformed, ServeErr::Unreachable(_)),
            "缺 deleted = 畸形(契约两字段钉死): {malformed:?}"
        );
    }

    /// 状态机(伪事件通道,无 fdpass):登记 → 广播 → 交付 → 等待落定;
    /// 超时摘除后迟到应答按未知 request_id 拒(false);三操作同一纪律。
    #[tokio::test]
    async fn bridge_roundtrip_timeout_and_late_reply() {
        inject();
        let (events, mut rx) = tokio::sync::broadcast::channel(16);
        let bridge = Arc::new(ServeBridge::new(events));

        // ① 超时 + 迟到应答按未知 id 拒(list)
        let b = bridge.clone();
        let wait = tokio::spawn(async move { b.list("/收件箱", "a").await });
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.method, EV_SERVE_LIST);
        assert_eq!(frame.params["root"], "/收件箱");
        assert_eq!(frame.params["sub"], "a");
        let id = frame.params["request_id"].as_u64().unwrap();
        let out = wait.await.unwrap();
        assert!(out.is_err(), "无应答应超时: {out:?}");
        assert!(
            !bridge.deliver_list(id, serde_json::json!({"entries": [], "ok": true, "truncated": false})),
            "迟到应答按未知 request_id 拒"
        );

        // ② 正常往返 + 条目投影
        let b2 = bridge.clone();
        let wait = tokio::spawn(async move { b2.list("/收件箱", "").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({
                "entries": [
                    {"name": "子目录", "is_dir": true, "children": 1},
                    {"name": "报告.pdf", "is_dir": false, "size": 9, "mtime_ms": 5},
                ],
                "ok": true,
                "request_id": id,
                "truncated": false,
            }),
        ));
        let (entries, _) = wait.await.unwrap().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].size, 9);

        // ③ 导出往返
        let b3 = bridge.clone();
        let wait = tokio::spawn(async move { b3.export("/收件箱", "报告.pdf").await });
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.method, EV_SERVE_EXPORT);
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_export(
            id,
            serde_json::json!({"ok": true, "path": "/tmp/serve-tmp/1-报告.pdf", "request_id": id}),
        ));
        assert_eq!(
            wait.await.unwrap().unwrap(),
            PathBuf::from("/tmp/serve-tmp/1-报告.pdf")
        );

        // ④ 删除往返 + 拒答臂(Refused 不进桥级负缓存,见 ⑥)
        let b4 = bridge.clone();
        let wait = tokio::spawn(async move { b4.delete("/收件箱", "子目录").await });
        let frame = rx.recv().await.unwrap();
        assert_eq!(frame.method, EV_SERVE_DELETE);
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_delete(
            id,
            serde_json::json!({"deleted": true, "ok": true, "request_id": id}),
        ));
        assert!(wait.await.unwrap().unwrap(), "删除应答 deleted:true");

        // ⑤ stat:子路径 = 列父目录找条目;根自身 = 探活合成(children
        //    None);未命中 = NOT_FOUND(父列表应答在案而名字缺席,非桥不
        //    可达——不落负缓存)。
        let bstat = bridge.clone();
        let wait = tokio::spawn(async move { bstat.stat("/收件箱", "报告.pdf", "收件箱").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert_eq!(frame.params["sub"], "", "stat 子路径列的是父目录(此处 = 根)");
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({
                "entries": [
                    {"name": "子目录", "is_dir": true, "children": 1},
                    {"name": "报告.pdf", "is_dir": false, "size": 9, "mtime_ms": 5},
                ],
                "ok": true,
                "request_id": id,
                "truncated": false,
            }),
        ));
        let st = wait.await.unwrap().unwrap();
        assert_eq!(st.size, 9);
        assert_eq!(st.path, "/收件箱/报告.pdf");
        // 未命中:空列表应答 → NOT_FOUND
        let bmiss = bridge.clone();
        let miss = tokio::spawn(async move { bmiss.stat("/收件箱", "无此件.pdf", "收件箱").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({"entries": [], "ok": true, "request_id": id, "truncated": false}),
        ));
        assert!(miss.await.unwrap().is_err(), "未命中 → NOT_FOUND");
        assert!(!bridge.unreachable_cached(), "NOT_FOUND 不落负缓存");
        // 根自身:探活成功 → 合成条目(children 惰性 None)
        let broot = bridge.clone();
        let rootst = tokio::spawn(async move { broot.stat("/收件箱", "", "收件箱").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({"entries": [], "ok": true, "request_id": id, "truncated": false}),
        ));
        let st_root = rootst.await.unwrap().unwrap();
        assert!(st_root.is_dir);
        assert_eq!(st_root.children, None, "根条目 children 惰性 None");
        assert_eq!(st_root.name, "收件箱");

        // ⑥ 根探针负缓存:Refused 只藏该根、不落桥级缓存(探针可再发);
        //    Unreachable 落桥级缓存,缓存命中即回(零事件),强翻过期后
        //    重探成功即清。(此前 ① 的超时已落缓存,但 ②-⑤ 的成功应答
        //    已清——此处从干净态起。)
        // Refused 臂:App 回 ok:false → false 且无桥级缓存(下一探针仍发事件)
        let b5 = bridge.clone();
        let probe = tokio::spawn(async move { b5.root_probe("/收件箱").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({"error": "授权失效", "ok": false, "request_id": id}),
        ));
        assert!(!probe.await.unwrap(), "拒答 → 该根隐藏");
        assert!(
            !bridge.unreachable_cached(),
            "Refused 不进桥级负缓存"
        );
        // Unreachable 臂:超时 → false + 桥级缓存落 TTL;缓存命中即回(零事件)
        let b6 = bridge.clone();
        let probe = tokio::spawn(async move { b6.root_probe("/收件箱").await });
        let _frame = rx.recv().await.unwrap(); // 探针事件,不答 → 超时
        assert!(!probe.await.unwrap(), "无应答 → 隐藏");
        assert!(bridge.unreachable_cached(), "超时落桥级负缓存");
        let started = std::time::Instant::now();
        assert!(!bridge.root_probe("/收件箱").await, "缓存命中即隐藏");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "负缓存命中不得再发事件"
        );
        assert!(rx.try_recv().is_err(), "负缓存命中零事件");
        // 强翻过期 → 重探成功 → 可见 + 缓存清
        *bridge.unreachable_until.lock().unwrap() =
            Some(std::time::Instant::now() - Duration::from_secs(1));
        let b7 = bridge.clone();
        let probe = tokio::spawn(async move { b7.root_probe("/收件箱").await });
        let frame = rx.recv().await.unwrap();
        let id = frame.params["request_id"].as_u64().unwrap();
        assert!(bridge.deliver_list(
            id,
            serde_json::json!({"entries": [], "ok": true, "request_id": id, "truncated": false}),
        ));
        assert!(probe.await.unwrap(), "过期重探成功 → 可见");
        assert!(!bridge.unreachable_cached(), "成功即清负缓存");
    }

    /// 快失败(photo 同款):零订阅者 = App 必然不在,list/export/delete
    /// 立即 Unreachable,不等应答窗;且落桥级负缓存(根探针免串行重探)。
    #[tokio::test]
    async fn fast_fail_with_zero_receivers() {
        inject();
        // receiver 用 `_` 立即弃(留绑定会让 receiver_count==1,快失败不触发)
        let (events, _) = tokio::sync::broadcast::channel::<EventFrame>(16);
        let bridge = Arc::new(ServeBridge::new(events));
        let started = std::time::Instant::now();
        for (r, m) in [
            (bridge.list("/收件箱", "").await.is_err(), "list"),
            (bridge.export("/收件箱", "x").await.is_err(), "export"),
            (bridge.delete("/收件箱", "x").await.is_err(), "delete"),
        ] {
            assert!(r, "{m} 零订阅须即错");
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "零订阅须即回: {:?}",
            started.elapsed()
        );
        assert!(bridge.unreachable_cached(), "快失败落桥级负缓存");
    }

    /// 导出目录前缀判定(photo serve_dir_prefix_matrix 同款):前缀内(含
    /// 自身/深层)在;越出/兄弟目录/前缀字节撞(serve-tmp-x)/非法形态不在;
    /// 暂存区同名面不在(命名空间隔离)。
    #[cfg(unix)]
    #[test]
    fn serve_dir_prefix_matrix() {
        let home = Path::new("/home/u/h");
        let under = |p: &str| path_under_serve_dir(home, Path::new(p));
        assert!(under("/home/u/h/serve-tmp/1-报告.pdf"));
        assert!(under("/home/u/h/serve-tmp/1/d/a.pdf"));
        assert!(under("/home/u/h/serve-tmp/"));
        // 字节前缀撞:serve-tmp-x 不是 serve-tmp 内
        assert!(!under("/home/u/h/serve-tmp-x/f"));
        assert!(!under("/home/u/h/.mtdss/serve-tmp/f"), "记账区同名面不是服务目录内");
        assert!(!under("/home/u/h/photo-serve/f"), "photo 服务目录不是 serve-tmp 内");
        assert!(!under("/home/u/h/other/f"));
        assert!(!under("/home/u/h/evil.bin"));
        assert!(!under("/etc/passwd"));
        // 噪声规范化后仍在(./ 消解)
        assert!(under("/home/u/h/serve-tmp/./a/../b.pdf"));
        // 越出根:规范化后出前缀
        assert!(!under("/home/u/h/serve-tmp/../../evil"));
        assert_eq!(serve_dir(home), PathBuf::from("/home/u/h/serve-tmp"));
    }
}
