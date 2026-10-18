# MTDSS M3b(浏览)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 M3 的浏览能力:daemon 间控制通道 + 远端 FS `list`/`stat` + RPC/CLI 列表命令 + UI 三视图/排序/搜索/状态栏/离线态,并清偿 M3a 遗留的 M3b 种子债务(信令连接收敛、托盘唤起主窗、SDDL 管道收紧等)。

**Architecture:** 先把 daemon 的三条信令连接(在线循环/常驻接收/发送流)收敛为单连接 SignalHub(消除 presence 槽互抢——服务器同设备「后连者胜」,发送流连接关闭会把本机从 presence 摘除,后续来件被丢弃,是正确性问题);控制通道复用既有打洞机制(CtrlConnect/CtrlAccept + 同一 Candidates 消息),QUIC 连接按需建立、60s 空闲自灭(无 keep-alive),FS 服务(卷枚举/目录列表/stat)进 core 供 daemon 承载;本机设备的 list/stat 在 RPC 层直调 core 短路。UI 从「设备列表壳」长成浏览器:Loc 导航 + 懒加载树 + 三视图。

**Tech Stack:** 既有栈不动;新增 proto `ctrl.proto`(prost);UI 新增 `tauri-plugin-single-instance`(唤起主窗去重);ipc crate Windows 侧加 `windows-sys`(SDDL)。

**Spec:** `docs/superpowers/specs/2026-10-17-mtdss-m3-explorer-design.md`(M3 设计;本计划实现 §11 M3b 切片 + §5.2 控制通道/远端 FS + 终审 triage 与 M3a 冒烟发现的 M3b 种子债务,见项目记忆)

## Global Constraints

- 门禁 = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 在 Windows 与 Linux(WSL/CI)双平台全绿 + nat-matrix CI job + ui job 全绿。**每个任务提交前门禁必须全绿。**
- 本机 bash 环境:`export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`(新终端 PATH 陈旧时先执行前半句自救)。
- cargo 走 rsproxy.cn;npm 慢时临时用 npmmirror,**不得把 .npmrc 提交进仓库**;git 走 SSH。
- **服务器代码(`server/`)与 `e2e.proto`(即 `mtdss.ws` 包)零改动**。`signal.proto` 是 peer 间信封面,增量加变体服务器不可见(Relay.payload 对服务器不透明);`manifest.proto` 不动。
- IPC 契约 verbatim:新方法 `list {device:String, path:String}` → `{"entries":[{name,path,size,mtime_ms,is_dir}],"truncated":bool}`;`stat {device,path}` → `{"entry":{…同上五字段…}}`;新事件 `event.login {"logged_in":bool}`;新错误码 **3 = 对端离线**(既有:1 业务失败、2 忙、-32601/-32602)。其余方法/事件签名不变。
- 控制通道常量 verbatim:`mtdss_proto::CTRL_VERSION = 1`;调优 `keep_alive_interval: None, max_idle_timeout: 60s`(懒驱逐:空闲控制连接由 QUIC 空闲超时自然关闭);`LIST_ENTRY_CAP = 10_000`(超限截断 + `truncated:true`);ctrl 打洞 `PunchConfig { budget: 10s, handshake_timeout: 2s, tuning: 上者 }`。
- FS 隐藏规则沿 intake:`.mtdss` 与 `.mtdss-part` 名字条目一律不列出(精确匹配;大小写债务归 M3c 隐藏暂存夹重做)。
- CLI 依赖白名单不变(新命令不加依赖);UI 文案全部中文入 `zh.json`;提交信息 `feat:`/`fix:`/`docs:`/`test:`/`refactor:` 前缀。
- 仓库根 `smoke/` 是 M3a 冒烟本机残留,不入库(Task 11 加 .gitignore)。

## Review Focus

1. **对端离线/瞬断的 list**:不挂死、不空烧打洞预算——目录快照判离线立即 code 3;对端 daemon 被杀后(list 时快照仍在线)有界失败并可恢复。→ Task 4 `list_peer_offline_is_fast_error` + `peer_daemon_killed_list_fails_bounded_and_recovers`。
2. **大目录与怪输入**:>10000 条截断且带 truncated 标志;非 UTF-8 名 lossy 不 panic;空串=根卷;相对路径/内嵌 NUL → INVALID_PATH;不存在 → NOT_FOUND。→ Task 3 fs 测试组(截断用可注入小 cap)+ Task 4 e2e。
3. **信令代际更替**:Hub 重连期间,等待中的 wait_signal 快速失败(不悬等超时),等待方按错误收口重试;控制连接死(对端重启)后池驱逐重打洞。→ Task 1 Closed 收口路径(以双向回归测试兜底)+ Task 4 killed-peer 用例。
4. **双向并发**:A↔B 同时互发/互浏览时 presence 槽不互抢(旧 bug:发送流抢槽并关闭后本机从 presence 消失,对端来件丢失)。→ Task 1 `send_both_directions_back_to_back` + Task 4 `list_bidirectional_concurrent`。
5. **UI 导航竞态与离线进入**:快速连点/←→ 时只有最新 Loc 的响应生效(旧响应丢弃);点击离线设备/离线后进入子目录被阻止且有提示。→ Task 6 纯函数(nav 应用响应的 seq 守卫)+ Task 8 `canEnter` 门控。

---

### Task 1: SignalHub——单信令连接收敛(daemon)

**Files:**
- Create: `crates/daemon/src/hub.rs`(吸收并取代 `online.rs`;含 backoff 纯函数与单测锚点)
- Delete: `crates/daemon/src/online.rs`
- Modify: `crates/daemon/src/state.rs`(`hub: SignalHub` 字段;删 `recv_restart_tx/rx` 与 `mark_online` 的重启通知)
- Modify: `crates/daemon/src/flow.rs`(send_round/recv_handshake/resident_recv_loop 改走 hub;删 `wait_opened`/`wait_hello_stun`/presence 钩子与 `RecvRound.presence`;`setup_punch_endpoint`/`parse_candidates`/`peer_pk_from` 升 `pub(crate)`)
- Modify: `crates/daemon/src/lib.rs`(在线任务改 `hub::run`;删 recv_restart 通道)
- Test: `crates/daemon/tests/online.rs`(应全数保留通过)、`crates/daemon/tests/daemon_e2e.rs`(新增双向回归)、`crates/daemon/src/hub.rs` 内嵌单测

**Interfaces:**
- Consumes: `SignalingClient`/`ClientEvent`、`signaling_connect`、`account::{fetch_devices, refresh}`、`seal/open`、`tofu_pin`。
- Produces(Task 4/5 依赖,签名 verbatim):

```rust
// hub.rs
pub(crate) type SignalEvt = (String /* from */, SignalBody);   // 已解封验签的信令

pub(crate) struct ConnHandle {           // 一代连接:relay 句柄 + 信令广播
    pub(crate) client: mtdss_core::signaling::client::SignalingClient,   // Clone 廉价
    pub(crate) signals: tokio::sync::broadcast::Sender<SignalEvt>,       // drop → 订阅方收 Closed
}
pub(crate) struct SignalHub {
    conn: std::sync::RwLock<Option<std::sync::Arc<ConnHandle>>>,
    pks:  std::sync::RwLock<HashMap<String, VerifyingKey>>,   // 设备→公钥(目录刷新维护)
}
impl SignalHub {
    pub(crate) fn new() -> Self;
    pub(crate) fn current(&self) -> Option<std::sync::Arc<ConnHandle>>;
    pub(crate) fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<SignalEvt>>;  // 离线 None
    pub(crate) fn stun(&self) -> Vec<String>;                 // 当前连接的 STUN 通告;离线空
    pub(crate) fn peer_pk(&self, device_id: &str) -> Option<VerifyingKey>;
    pub(crate) async fn relay(&self, to: &str, payload: &[u8]) -> anyhow::Result<()>;  // RELAY_TIMEOUT 有界;离线即 Err
}
/// 在订阅流上等来自 `from` 且满足 `pred` 的信令体;SIGNAL_TIMEOUT 有界;
/// 广播 Lagged(本订阅者落后丢帧)→ warn 后继续等(丢失即随轮超时失败);
/// 广播 Closed(连接代际更替)→ 立即 Err(调用方按轮错误收口,不等超时)。
pub(crate) async fn wait_signal(
    rx: &mut tokio::sync::broadcast::Receiver<SignalEvt>,
    from: &str,
    pred: impl Fn(&SignalBody) -> bool,
) -> anyhow::Result<SignalBody>;
/// 常驻主循环(原 online::run 职责 + 集中信封开箱):未登录挂起;登录即连;
/// HelloOk(stun 缓存)→ 目录全量快照(pk 缓存随刷新更新)→ event.online true;
/// Relayed → 查 pk(未命中先全量刷新一次)→ tofu_pin → open 验签 → 广播 SignalEvt;
/// Presence 增量维护快照;断连/鉴权路径与原 online::run 相同(backoff_for/refresh 原样搬)。
pub(crate) async fn run(state: std::sync::Arc<DaemonState>, events: tokio::sync::broadcast::Sender<EventFrame>);
```

关键行为约定:信封开箱失败(验签失败/TOFU 变更/未知设备刷新后仍缺)→ `warn` 并丢弃该帧,循环不死;`refresh_snapshot`/`emit_current`/`build_snapshot` 随迁 `hub.rs` 且 `refresh_snapshot` 同时更新 pk 缓存;`mark_online` 不再发 recv_restart(该机制整体删除——接收循环不再持有信令连接,`set_recv_dir` 对下一来件自然生效)。

- [ ] **Step 1: 写失败测试** — `daemon_e2e.rs` 新增 `send_both_directions_back_to_back`:双 Daemon 同账号在线;A→B send 完成后**立即** B→A send;断言第二轮成功落盘(旧代码:A 的发送连接关闭时把 A 从 presence 摘除、A 的接收循环成僵尸,B 的 ConnectRequest 被丢,重试耗尽失败——本测试是收敛的回归锚点)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test daemon_e2e send_both_directions`
  Expected: FAIL(B→A 轮失败/超时)
- [ ] **Step 3: 实现 hub.rs + 改造 flow/lib/state** — hub 按上述接口;`send_round` 改为:`hub.relay` 发信令、`hub.subscribe()`+`wait_signal` 等 Accept/Candidates、`hub.stun()` 建打洞端点;`resident_recv_loop` 改为:在线后 `hub.subscribe()` 一次跨轮持有(轮间到达的信令在广播通道排队),内层循环 `wait_signal(ConnectRequest)` → `hub.peer_pk` → Accept/Candidates → punch → 传输;删 presence 钩子、`recv_handshake` 的信令连接段与轮首 `refresh_snapshot`;`lib.rs` 在线任务换 `hub::run`。
- [ ] **Step 4: 既有测试全数回归** — Run: `cargo test -p mtdss-daemon`
  Expected: PASS(online.rs 三例、daemon_e2e 全部、login/daemon_basic/tray_logic 不回归)
- [ ] **Step 5: 门禁 + Commit** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Commit: `refactor(daemon): converge on single signaling connection (SignalHub)`

### Task 2: ctrl.proto + 信令体扩展 + 控制帧编解码(proto + core)

**Files:**
- Create: `proto/proto/ctrl.proto`
- Modify: `proto/proto/signal.proto`(BodyProto oneof 增两变体)
- Modify: `proto/build.rs`、`proto/src/lib.rs`(`pub mod ctrl` + `pub const CTRL_VERSION: u32 = 1;`)
- Modify: `crates/core/src/signaling/envelope.rs`(`SignalBody` 增 `CtrlConnect`/`CtrlAccept`)
- Create: `crates/core/src/transport/ctrl.rs`(`pub mod ctrl` 挂进 transport)
- Test: `crates/core/tests/envelope.rs`(增变体往返)、`crates/core/src/transport/ctrl.rs` 内嵌单测(duplex 流)+ `crates/core/tests/ctrl.rs`(真实 QUIC 对)

**Interfaces:**
- Consumes: prost、quinn 既有。
- Produces(Task 3/4 依赖):

```protobuf
// ctrl.proto(全文 verbatim)
syntax = "proto3";
package mtdss.ctrl;
message DirEntry {
  string name = 1;      // 显示名(无路径;根卷条目 Windows="C:"、Unix="/")
  string path = 2;      // 供后续 list/stat 的绝对路径(Windows 根="C:\\")
  uint64 size = 3;      // 目录=0
  uint64 mtime_ms = 4;  // Unix 毫秒;不可得=0
  bool is_dir = 5;
}
message ListRequest  { string path = 1; }                 // "" = 根卷列表
message StatRequest  { string path = 1; }
message ListResponse { repeated DirEntry entries = 1; bool truncated = 2; }
message StatResponse { DirEntry entry = 1; }
message CtrlError { string code = 1; string message = 2; } // NOT_FOUND|PERMISSION_DENIED|INVALID_PATH|UNSUPPORTED_VERSION|INTERNAL
message CtrlRequest  { uint32 version = 1; oneof body { ListRequest list = 2; StatRequest stat = 3; } }
message CtrlResponse { uint32 version = 1; bool ok = 2; oneof body { ListResponse list = 3; StatResponse stat = 4; CtrlError error = 5; } }
```

```rust
// signal.proto BodyProto oneof 追加(增量:旧端解到未知变体 → body None → open() Malformed → 忽略,互通不破)
//   CtrlConnect ctrl_connect = 4;  CtrlAccept ctrl_accept = 5;   // 字段均 { bytes session_id = 1; }
// envelope.rs SignalBody 追加:CtrlConnect { session_id: Vec<u8> } / CtrlAccept { session_id: Vec<u8> }

// transport/ctrl.rs
pub const FRAME_CAP: usize = 16 * 1024 * 1024;   // 单帧上限(读侧超限报错)
/// u32 LE 长度前缀 + prost 帧。内部 pub 供单测,业务走 send_request/serve。
pub(crate) async fn write_frame(w: &mut quinn::SendStream, msg: &impl prost::Message) -> anyhow::Result<()>;
pub(crate) async fn read_frame<T: prost::Message + Default>(r: &mut quinn::RecvStream) -> anyhow::Result<T>;
/// 打开 bi 流,写请求读响应(timeout 有界);响应 version 不符 → Err(UNSUPPORTED_VERSION 语义)。
pub async fn send_request(conn: &quinn::Connection, req: &mtdss_proto::ctrl::CtrlRequest,
                          timeout: std::time::Duration) -> anyhow::Result<mtdss_proto::ctrl::CtrlResponse>;
/// 响应端:accept_bi 循环,每流 spawn 一个任务(读请求 → handler → 写响应);连接关闭即返回。
pub async fn serve_conn<F>(conn: quinn::Connection, handler: F)
where F: Fn(mtdss_proto::ctrl::CtrlRequest) -> mtdss_proto::ctrl::CtrlResponse + Send + Sync + 'static;
```

- [ ] **Step 1: 写失败测试** — envelope.rs 测试文件增 `ctrl_bodies_roundtrip`(seal→open 两新变体,字段保真);`core/tests/ctrl.rs`:沿 `dual_endpoint.rs` 自连样板建 QUIC 对,一端 `serve_conn`(echo handler:list 请求回 3 个固定 DirEntry),另一端 `send_request` 断言字段往返 + 版本不符路径(手造 version=0 响应端 → send 侧 Err)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core --test envelope && cargo test -p mtdss-core --test ctrl`
  Expected: FAIL(变体/模块不存在)
- [ ] **Step 3: 实现** — proto 两文件 + build.rs + lib.rs;envelope 转换两分支;ctrl.rs 读写帧(`read_frame` 拒超 FRAME_CAP)+ send_request/serve_conn。
- [ ] **Step 4: 跑测试确认通过 + 门禁** — Run: `cargo test -p mtdss-core && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Expected: PASS(全量;服务器 crate 不受 proto 增量影响)
- [ ] **Step 5: Commit** — `feat(proto+core): ctrl wire protocol + CtrlConnect/CtrlAccept signal bodies`

### Task 3: core FS 服务(根卷/列表/stat/过滤/截断)

**Files:**
- Create: `crates/core/src/fs.rs`(`pub mod fs` 挂 lib.rs)
- Test: `crates/core/tests/fs.rs`

**Interfaces:**
- Consumes: 无(纯 std)。
- Produces(Task 4/5 依赖,签名 verbatim):

```rust
pub const LIST_ENTRY_CAP: usize = 10_000;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryData { pub name: String, pub path: String, pub size: u64, pub mtime_ms: u64, pub is_dir: bool }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOut { pub entries: Vec<DirEntryData>, pub truncated: bool }
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FsError {
    #[error("路径不存在: {0}")] NotFound(String),
    #[error("无权限访问: {0}")] PermissionDenied(String),
    #[error("非法路径: {0}")]  InvalidPath(String),   // 相对路径/空(stat)/内嵌 NUL
    #[error("{0}")]            Internal(String),
}
impl FsError { pub fn code(&self) -> &'static str; }   // NOT_FOUND / PERMISSION_DENIED / INVALID_PATH / INTERNAL

/// path "" = 根卷列表(Windows = A:..Z: 探测在位盘符;Unix = 单条 "/")。
/// 结果序:目录优先,组内名称字节升序;过滤 .mtdss/.mtdss-part;超 LIST_ENTRY_CAP 截断。
pub fn list_dir(path: &str) -> Result<ListOut, FsError>;
/// 与 list_dir 同判定的单条查询("" → InvalidPath;盘根如 "C:\\" 合法)。
pub fn stat_path(path: &str) -> Result<DirEntryData, FsError>;
#[cfg(test)] pub(crate) fn list_dir_capped(path: &str, cap: usize) -> Result<ListOut, FsError>;  // 截断测试注入面
```

语义钉死:符号链接条目按 `entry.file_type()` 不跟随(目录链接显示为文件,V1 简化,注释记录);mtime 取 `metadata().modified()`,失败置 0;名称非 UTF-8 用 `to_string_lossy`(V1 限制,注释记录);`list_dir` 路径要求绝对(Windows 以 `X:` 开头、Unix 以 `/` 开头),Windows 两字符裸盘符 `C:` 规范化为 `C:\`;io 错误按 kind 映射 FsError。

- [ ] **Step 1: 写失败测试** — `fs.rs` 测试:tempdir 混合文件/目录 + `.mtdss` 目录与 `.mtdss-part` 文件(均被滤除);顺序断言(目录优先、组内名升);mtime 非零;size 文件=内容长、目录=0;`path` 字段为可再入的绝对路径(list 结果逐条 stat 成功);`list_dir_capped`(7 条 cap=5 → truncated + 恰 5 条);stat 文件/目录/盘根;错误:不存在(父在子缺)、相对路径、内嵌 `\0`、stat("")、`list_dir("")` 各平台根(Windows 断言至少含系统盘 `C:`;Unix 断言恰一条 `/`);Unix-only:符号链接条目 is_dir=false、chmod 000 目录 → PermissionDenied。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core --test fs` Expected: FAIL
- [ ] **Step 3: 实现** — `fs.rs` 按接口;盘符探测 `std::fs::metadata("{L}:\\")` 26 次试错(亚毫秒本地调用)。
- [ ] **Step 4: 跑测试确认通过 + 双平台核** — Run: `cargo test -p mtdss-core` 本机 + WSL `cargo test -p mtdss-core --test fs` Expected: PASS
- [ ] **Step 5: Commit** — `feat(core): remote-fs listing service (roots/list/stat, .mtdss filtering, cap)`

### Task 4: daemon 控制通道(响应端 + 发起端连接池)

**Files:**
- Create: `crates/daemon/src/ctrl.rs`
- Modify: `crates/daemon/src/api.rs`(`ApiDispatcher::new(state, ctrl: Arc<CtrlPool>)` 签名变更)、`crates/daemon/src/lib.rs`(建 `CtrlPool`、spawn `ctrl::responder_loop`,Daemon 增任务句柄随 shutdown abort)
- Test: `crates/daemon/tests/ctrl_e2e.rs`

**Interfaces:**
- Consumes: Task 1 hub(`subscribe/relay/wait_signal/peer_pk/stun`)、Task 2 `ctrl::send_request/serve_conn` + proto、Task 3 `fs::{list_dir, stat_path}`、flow 的 `setup_punch_endpoint/parse_candidates/peer_pk_from/tofu_pin/fetch_devices`。
- Produces(Task 5 依赖):

```rust
// ctrl.rs
pub(crate) const CTRL_TUNING: mtdss_core::transport::endpoint::TransportTuning =
    mtdss_core::transport::endpoint::TransportTuning { keep_alive_interval: None, max_idle_timeout: Duration::from_secs(60) };

#[derive(Debug)]
pub(crate) enum CtrlErr {
    PeerOffline(String),                                   // → RPC code 3
    Punch(String),                                         // punch_err 诊断全文 → code 1
    Peer { code: String, message: String },                // 对端 FsError → code 1,message 前缀「远端: 」
    Timeout(String), Internal(String),                     // → code 1
}

pub(crate) struct CtrlPool { /* per-device 单飞锁 + 连接缓存 */ }
impl CtrlPool {
    pub(crate) fn new() -> Self;
    /// device == 本机 device_id → core fs 直调(短路,不建连接);离线(目录快照 false/无快照)→ PeerOffline;
    /// 池命中且 conn.closed().is_none() → 复用;否则打洞:fetch_devices 解 pk + tofu_pin →
    /// CtrlConnect/CtrlAccept + 同一 Candidates 消息交换(经 hub)→ punch_connect(CTRL_TUNING)→ 入池。
    /// 请求失败且连接已死 → 驱逐后整程重试恰一次。
    pub(crate) async fn list(&self, state: &Arc<DaemonState>, device: &str, path: &str)
        -> Result<mtdss_core::fs::ListOut, CtrlErr>;
    pub(crate) async fn stat(&self, state: &Arc<DaemonState>, device: &str, path: &str)
        -> Result<mtdss_core::fs::DirEntryData, CtrlErr>;
}
/// 响应端常驻任务:hub 订阅 CtrlConnect → spawn 会话(punch_via_hub 响应侧 → serve_conn,
/// handler 把 List/Stat 映射 core fs,FsError → CtrlError code)。
pub(crate) async fn responder_loop(state: Arc<DaemonState>);
```

打洞编舞(双向共用,initiator/role 由消息方向天然定):发起端 `CtrlConnect{sid}` → 等 `CtrlAccept{sid}` → 双方各自建端点(`hub.stun()`)互发 `Candidates{sid}` → `punch_connect`。`send_request` 超时取 15s(打洞+往返上界)。响应端请求分发:`req.version != CTRL_VERSION` → `CtrlError{code:"UNSUPPORTED_VERSION"}`(ok=false)。

- [ ] **Step 1: 写失败测试** — `ctrl_e2e.rs`(进程内双 Daemon + IpcClient,沿 daemon_e2e 登录样板):
  `list_remote_roundtrip` — B 侧 tempdir 建 `b1.txt`/`子目录/c.txt`/`.mtdss`/`x.mtdss-part`;A `list{device:B, path:""}` 得根(Windows 断言含 `C:`;Unix 恰 `/`)→ 取 B 的 tempdir 路径再 list:条目/顺序/过滤/`path` 可再入全对;A `stat` b1.txt → size 正确;
  `list_self_shortcircuit` — A `list{device:A, path:B 同款 tempdir}` 不经网络即成(B 离线亦成);
  `list_peer_offline_is_fast_error` — B 仅 REST 注册(无 daemon,快照 online=false)→ A list B → code 3,且调用 < 2s;
  `peer_daemon_killed_list_fails_bounded_and_recovers` — 真实 `mtdssd` 子进程当 B(独立 MTDSS_HOME,沿 daemon_e2e 的既有子进程样板)→ list 成功 → kill -9 → 快照尚在线期间 list 有界失败(code 1,≤ 30s)→ 等 presence 转离线 → code 3 → 重启 B 子进程并等在线 → list 复通;
  `list_bidirectional_concurrent` — A/B 同时互相 list 各自 tempdir,双双成功;
  `stat_remote_missing_is_peer_not_found` — stat 不存在路径 → code 1 且 message 含 `远端: NOT_FOUND`。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test ctrl_e2e` Expected: FAIL(方法不存在)
- [ ] **Step 3: 实现** — ctrl.rs + lib.rs 接线 + api.rs 分派器签名(api 层 list/stat 方法体本任务先占位透传 CtrlErr,正式 RPC 面在 Task 5;本任务测试经 CtrlPool 直调或占位方法二选一,取占位方法最简)。
- [ ] **Step 4: 跑测试确认通过 + daemon 全量回归** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Commit: `feat(daemon): control channel — remote list/stat over punched QUIC with idle eviction`

### Task 5: RPC `list`/`stat` 正式面 + CLI `ls`

**Files:**
- Modify: `crates/daemon/src/api.rs`(两方法完整参数校验与错误映射)、`crates/daemon/src/lib.rs`
- Modify: `crates/cli/src/main.rs`(`Ls { device: String, path: Option<String> }`)、`crates/cli/src/client.rs`(`resolve_device`/`ls_cmd`/`format_ls`)
- Test: `crates/cli/tests/ipc_cli.rs`、`crates/daemon/tests/ctrl_e2e.rs`(补 RPC 面断言)

**Interfaces:**
- Consumes: Task 4 `CtrlPool`。
- Produces:

```rust
// client.rs
/// 设备定位:id 精确命中优先;否则按 name 精确匹配:0 台 → 列出可用设备的 Err;
/// >1 台 → 提示歧义并列 device_id 的 Err;恰 1 台 → 其 id。纯函数。
pub fn resolve_device(devices: &[serde_json::Value], name_or_id: &str) -> std::result::Result<String, CliError>;
pub async fn ls_cmd(c: &DaemonClient, device: &str, path: &str) -> std::result::Result<serde_json::Value, CliError>;
/// 行格式(稳定可 grep):`{d|-}\t{name}\t{size}\t{mtime_ms}`;truncated → 追加末行提示。纯函数。
pub fn format_ls(v: &serde_json::Value) -> String;
```

RPC 错误映射(Task 4 CtrlErr → RpcError):`PeerOffline` → `code 3, "设备离线: {device}"`;`Peer` → `code 1, "远端: {code} {message}"`;其余 → `code 1, 诊断全文`。参数:`need_str` device/path(path 允许空串=根,改用可空变体)。

- [ ] **Step 1: 写失败测试** — `ipc_cli.rs`:`resolve_device_id_exact` / `resolve_device_unique_name` / `resolve_device_ambiguous_lists_ids` / `resolve_device_none_lists_available` / `format_ls_rows_and_truncated`;`ctrl_e2e.rs` 补:`list_rpc_offline_code3`(经 IpcClient 断言 code 3)、`ls_end_to_end`(进程内 Daemon + `ls_cmd(self 设备, tempdir)` → format_ls 含预期行)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-cli` Expected: FAIL
- [ ] **Step 3: 实现** — api.rs 两方法 + main.rs `Ls`(path 缺省 `""`;先 `devices` RPC 取快照 → `resolve_device` → `ls_cmd` → 打印)。
- [ ] **Step 4: 跑测试 + 门禁** — `cargo test -p mtdss-daemon -p mtdss-cli && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Expected: PASS
- [ ] **Step 5: Commit** — `feat(ipc+cli): list/stat RPC + mtdss ls (device by id or name)`

### Task 6: UI 浏览逻辑层(vitest 纯函数)

**Files:**
- Create: `ui/src/lib/nav.ts`、`ui/src/lib/collate.ts`、`ui/src/lib/browse.ts`
- Modify: `ui/src/lib/derived.ts`(`sortDevices` 接受注入 cmp,默认 `zhCmp()`——清偿拼音排序种子)
- Test: `ui/src/lib/nav.test.ts`、`ui/src/lib/collate.test.ts`、`ui/src/lib/browse.test.ts`、`ui/src/lib/derived.test.ts`(增补)

**Interfaces:**
- Produces(Task 7/8 依赖):

```ts
export type Loc = { kind: "devices" } | { kind: "dir"; device: string; path: string };
export interface NavState { back: Loc[]; fwd: Loc[]; current: Loc }
export function navGo(h: NavState, next: Loc): NavState;          // push + 清 fwd;同 Loc 重复点击仍 push
export function navBack(h: NavState): NavState | null;             // 无后退 null
export function navForward(h: NavState): NavState | null;
export function parentLoc(loc: Loc): Loc;                          // dir 根 → devices;dir → 父目录
export function parentPath(p: string): string | null;              // "C:\\a\\b"→"C:\\a";"C:\\"→null;"/a/b"→"/a";"/"→null
export function pathSegments(p: string): string[];                 // "C:\\a\\b"→["C:","a","b"];"/a/b"→["/","a","b"]
export function locAtSegment(dev: string, p: string, segCount: number): Loc;  // 面包屑第 k 段 → Loc

export type Cmp = (a: string, b: string) => number;
export const codePointCmp: Cmp;                                    // 跨环境确定(测试基准)
export function zhCmp(): Cmp;                                      // Intl.Collator("zh",{numeric:true});构造异常 → codePointCmp

export interface FsEntry { name: string; path: string; size: number; mtime_ms: number; is_dir: boolean }
export interface Listing { entries: FsEntry[]; truncated: boolean }   // list RPC 结果切片
export function sortEntries(es: FsEntry[], dir: "asc" | "desc", cmp: Cmp): FsEntry[];  // 目录恒优先,组内名称 cmp;desc 组内反序
export function filterEntries(es: FsEntry[], q: string): FsEntry[];  // 名称小写包含;q 空 → 原样
export function fmtSize(n: number, isDir: boolean): string;          // 目录 "";0="0 B";1024 进制,一位小数
export function fmtMtime(ms: number): string;                        // 0 → "";本地时区 `YYYY/M/D H:MM`
export function canEnterDir(targetOnline: boolean, browsingOffline: boolean): boolean; // 离线阻断门控
export function applyListing(seq: number, latestSeq: number, listing: Listing | null): Listing | null; // seq 守卫:旧响应丢弃
```

- [ ] **Step 1: 写失败测试** — nav(各函数含盘根/POSIX 根边界、navGo 清 fwd、back/fwd 栈序)、collate(codePointCmp 序;zhCmp 至少满足自反/反对称、ASCII 有序)、browse(排序升降+目录优先恒成立、过滤、fmtSize/fmtMtime 边界、canEnterDir 三态、applyListing seq 新旧)、derived(注入 cmp 断言被采用 + 本机置顶不回归)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run src/lib` Expected: FAIL
- [ ] **Step 3: 实现** — 三文件 + derived 改造。
- [ ] **Step 4: 跑测试确认通过** — Run: `cd ui && npx vitest run src/lib` Expected: PASS
- [ ] **Step 5: Commit** — `feat(ui): navigation/collation/browse logic layer (+zh collator device sort)`

### Task 7: UI 浏览界面(桥命令 + 导航 + 目录视图 + 树 + 地址栏 + 状态栏)

**Files:**
- Modify: `crates/daemon/src/state.rs`(login/logout 成功路径发 `event.login {"logged_in":bool}`)、`crates/daemon/src/api.rs`(文档错误码表补 3)
- Modify: `ui/src-tauri/src/ipc_bridge.rs`(命令 `list(device,path)`/`stat(device,path)`;转发 `event.login` → `daemon://login`)
- Modify: `ui/src/lib/daemon.ts`(invoke 封装 + `listenLogin`)、`ui/src/App.tsx`、`ui/src/components/{Sidebar,AddressBar,StatusBar}.tsx`、`ui/src/components/FileDetails.tsx`(新)、`ui/src/App.css`、`ui/src/i18n/zh.json`
- Test: 手动冒烟(`npm run tauri dev` 双 daemon 回环)+ `cd ui && npm run build`

**Interfaces:**
- Consumes: Task 5 RPC、Task 6 全部。
- Produces: App 状态机——`NavState`(Loc 导航)+ `listing {loading, entries, truncated, error, seq}`(`applyListing` 守卫);默认视图=详细信息(FileDetails:名称/修改日期/类型/大小四列,数据源 `sortEntries(entries, sortDir, zhCmp())`);Sidebar 树:设备节点(本机置顶+「本机」标注)caret 懒展开(list "" → 目录子节点逐级懒加载),当前 Loc 高亮,离线灰且禁点禁展开;AddressBar:←/→/↑ 按栈启用、面包屑段可点、搜索框启用(过滤在 Task 8 接线,本任务先渲染);StatusBar:目录视图「{n} 个项目{,已选 m}」+ 信令 + 设备数;`daemon://login` → 全量 refresh(登出即回引导态);设备列表行双击 → 进入该设备根;内容区双击目录 → `navGo` 父→子;加载/空/错误三态;truncated 提示行。

- [ ] **Step 1: daemon 事件 + 桥命令先行**(state 两处 emit;ipc_bridge 两命令 + 转发;daemon.ts 封装)。
- [ ] **Step 2: App/组件实现**(上述状态机与组件;zh.json 增全部新文案)。
- [ ] **Step 3: 构建 + 单测回归** — `cd ui && npx vitest run && npm run build`;`cargo check --manifest-path ui/src-tauri/Cargo.toml` Expected: PASS
- [ ] **Step 4: 手动冒烟** — `npm run tauri dev`:双 MTDSS_HOME daemon 回环,树展开本机/远端设备、导航 ←→↑、面包屑跳转、登出回引导态、托盘登录后 UI 联动。
- [ ] **Step 5: 门禁 + Commit** — workspace 门禁;Commit: `feat(ui): browse — lazy tree, breadcrumb navigation, details view, status bar`

### Task 8: UI 三视图/排序/搜索/多选/离线态收尾

**Files:**
- Modify: `ui/src/App.tsx`、`ui/src/components/Toolbar.tsx`(排序▾/查看▾ 启用,其余保持置灰)、`ui/src/components/FileGrid.tsx`(新)、`ui/src/components/FileList.tsx`(新)、`ui/src/App.css`、`ui/src/i18n/zh.json`
- Test: `ui/src/lib/browse.test.ts`(补 UI 所需纯逻辑若有)、手动冒烟、`npm run build`

**Interfaces:**
- Consumes: Task 6 `sortEntries/filterEntries/canEnterDir`、Task 7 状态机。
- Produces: 工具栏「排序」菜单(名称递增|递减,勾选态)与「查看」菜单(平铺|列表|详细信息,默认详细信息)生效;三视图渲染(平铺=图标+名称网格;列表=单列名称;详细信息=Task 7 表格);搜索框接 `filterEntries`(状态栏计数随过滤后);选择:单击单选、Ctrl 点增减、Shift 点范围(anchor 记忆)、空白处清空,状态栏已选数;双击文件无操作;离线态:当前浏览设备转离线 → 内容保留 + 顶部提示条 + `canEnterDir` 阻断进入子目录(点击给提示),离开后树节点禁用;查看/排序选择仅内存态(不持久化,V1)。

- [ ] **Step 1: 实现菜单与三视图** + 选择交互 + 离线门控接线。
- [ ] **Step 2: 构建 + 单测回归** — `cd ui && npx vitest run && npm run build` Expected: PASS
- [ ] **Step 3: 手动冒烟** — 三视图切换、排序升降、搜索过滤即时、Ctrl/Shift 多选、杀远端 daemon 后页面保持+阻断+恢复。
- [ ] **Step 4: 门禁 + Commit** — `feat(ui): three views, sort menu, search filter, multi-select, offline gating`

### Task 9: 托盘唤起主窗 + UI 单实例

**Files:**
- Create: `crates/daemon/src/uilaunch.rs`(`find_ui_bin` + `open_main_window`,Windows 专属行为跨平台空实现)
- Modify: `crates/daemon/src/tray.rs`(「打开主窗口」「登录/登出」启用;open→`open_main_window`;login 项:已登录 → `state.logout()`(图标随 watch 自动换),未登录 → `open_main_window()`)、`crates/daemon/src/lib.rs`(mod 挂载)
- Modify: `ui/src-tauri/Cargo.toml`(+`tauri-plugin-single-instance`)、`ui/src-tauri/src/lib.rs`(插件初始化:第二实例回调 → 主窗 `show + set_focus`)
- Test: `crates/daemon/tests/tray_logic.rs`(find_ui_bin 注入单测)、构建 + 手动冒烟

**Interfaces:**
- Produces:

```rust
// uilaunch.rs(镜像 find_daemon_bin 的解析序,纯函数可注入 env 便于测试)
/// 解析序:env MTDSS_UI_BIN → current_exe 同目录 ui[.exe] → debug 下
/// CARGO_MANIFEST_DIR 逐级向上找 ui/src-tauri/target/{debug,release}/ui[.exe]。
pub fn find_ui_bin() -> Option<std::path::PathBuf>;
/// 找到即后台 spawn(stdout/stderr null,不等待);找不到/失败 → warn,不 panic。
pub fn open_main_window();
```

- [ ] **Step 1: 写失败测试** — `tray_logic.rs`:`find_ui_bin_env_override`(MTDSS_UI_BIN 指向临时假文件 → 恰返回该路径)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test tray_logic` Expected: FAIL
- [ ] **Step 3: 实现** — uilaunch.rs + tray 两项启用与分流 + UI 单实例插件。
- [ ] **Step 4: 构建 + 手动冒烟** — 托盘「打开主窗口」拉起/聚焦主窗(二次点击聚焦既有窗);未登录点「登录」→ 主窗引导态;已登录「登出」→ 直接登出且 UI 联动;`cargo check -p mtdss-daemon`(WSL,Linux 无 tray 依赖面)。
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon+ui): tray opens/focuses main window via single-instance; tray logout`

### Task 10: 硬化包(CLI 自适应拉起 / ipc serve 监督 / set_recv_dir 写序 / 管道 SDDL)

**Files:**
- Modify: `crates/cli/src/client.rs`(spawn 后持 Child:退出即败、10s 未就绪提示一次、硬顶 60s)
- Modify: `crates/daemon/src/lib.rs`(serve 任务改监督循环:泛型 `supervise_serve`,失败 warn + 1s 重试,单测 mock 两败一成)
- Modify: `crates/daemon/src/state.rs`(`set_recv_dir` 先持久化成功再换内存值,消除半程态)
- Modify: `crates/ipc/src/lib.rs`(Windows:`pipe_sddl()` = `D:P(A;;GA;;;{当前用户SID})(A;;GA;;;SY)(A;;GA;;;BA)`,经 interprocess 的 security_descriptor 接口挂到 ListenerOptions;`[target.'cfg(windows)'.dependencies]` 增 `windows-sys` 所需 feature)
- Test: `crates/daemon/tests/daemon_basic.rs`(supervise 单测)、`crates/ipc/tests/ipc.rs`(SDDL 下同用户全量回归)、`crates/cli/tests/ipc_cli.rs`(unix:MTDSSD_BIN 指向立即退出脚本 → 有界失败且文案可行动;Windows 手册验证)

**Interfaces:**
- Produces: `supervise_serve<S, F>(...)`(私有,F: Fn... → Future<Result<()>>;循环调 F,Ok 亦视为异常退出同样重试——serve 正常不返回);`pub(crate) fn pipe_sddl() -> String`(ipc,`#[cfg(windows)]`,当前用户 SID 经 OpenProcessToken→GetTokenInformation(TokenUser)→ConvertSidToStringSidW)。

- [ ] **Step 1: 写失败测试** — supervise:mock serve 前两次 Err 第三次挂起成功 → 断言被调 3 次且未退出;ipc 回归:既有 ipc.rs 套件在 SDDL 生效后全绿(同用户不受影响即证明未破坏);unix 拉起失败测试。
- [ ] **Step 2: 跑测试确认失败/回归** — 对应命令 Expected: FAIL(新逻辑)/PASS(回归面)
- [ ] **Step 3: 实现四项**(逐项小步,SDDL 的 interprocess 挂接 API 名以 2.4.6 文档为准:ListenerOptions 的 security_descriptor 面)。
- [ ] **Step 4: 跑测试 + 双平台门禁** — WSL `cargo test -p mtdss-ipc -p mtdss-daemon -p mtdss-cli` + 本机全量门禁 Expected: PASS
- [ ] **Step 5: Commit** — `fix(daemon+ipc+cli): adaptive spawn wait, supervised ipc serve, write-order, pipe SDDL lockdown`

### Task 11: 文档 + 冒烟手册 + 终门禁

**Files:**
- Modify: `.gitignore`(`smoke/`)
- Create: `docs/smoke-m3b.md`(手动冒烟手册:回环双 daemon 浏览/三视图/排序/搜索/离线、托盘唤起、CLI ls、SDDL 跨用户手工验证步骤)
- Modify: `docs/release-checklist.md`(「M3b 手动冒烟」段,含 SDDL 验证与 deferred 复核项)

**Deferred(本计划不做,记录依据):** EOF 2s 竞速重估(M3c 隐藏暂存夹重做传输终局路径时一并);CSP 硬化(M4 打包前);框选/拖拽/右键菜单/传输速率汇总(M3c);设备重命名 API(需服务器端点,后置);`.mtdss` 大小写(M3c);公网真机拓扑(M3 后,沿 M3a 既有清单)。

- [ ] **Step 1: 手动冒烟执行并记录**(沿 smoke-m3b.md;结果回填 checklist 勾选)。
- [ ] **Step 2: 终门禁** — 本机 `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` + `cd ui && npx vitest run && npm run build`。
- [ ] **Step 3: Commit + 推送确认 CI 三 job 全绿** — `docs: m3b smoke manual + checklist (loopback pass; public deferred)`
