# MTDSS M3a(地基)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 M3 的地基:三平台常驻 daemon(独占服务器连接与身份)+ IPC + 托盘/登录/设备在线 + CLI 改造为纯 IPC 客户端 + 测试形态迁移(daemon+CLI)+ UI 壳,并修复收端 EOF 舞步无界债务。

**Architecture:** 新增两个 crate:`mtdss-ipc`(JSON-RPC 2.0 over NDJSON,Windows 命名管道 / Unix domain socket,含 mtdss_home 与 socket 派生)与 `mtdss-daemon`(lib + bin `mtdssd`,吸收 cli 的 session/flow/REST 逻辑并常驻化)。CLI 瘦身为 IPC 客户端(期间用临时 re-export shim 保 gate 全绿)。信令服务器零改动。Tauri v2 UI 壳作为独立 npm 工程(`ui/`,src-tauri 不入 workspace)。

**Tech Stack:** Rust(tokio/quinn/axum 既有)、`interprocess` 2(本地套接字)、`tray-icon`(Windows 托盘)、`winreg`(开机自启)、`gethostname`;UI:Tauri v2 + React + TS + Vite + react-i18next(仅中文)。

**Spec:** `docs/superpowers/specs/2026-10-17-mtdss-m3-explorer-design.md`(M3 设计稿;本计划实现其 §11 的 M3a 切片 + §10 债务①)

## Global Constraints

- 门禁 = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 在 Windows 与 Linux 双平台全绿,外加 nat-matrix CI job(Linux)全绿;Task 11 起新增 ui job 后同为门禁。**每个任务提交前门禁必须全绿。**
- 本机 bash 环境:`export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`
- cargo 走 rsproxy.cn(已配好);新依赖用 `cargo add <name>` 加;npm 慢时本机临时 `npm config set registry https://registry.npmmirror.com`,**不得把 .npmrc 提交进仓库**。
- workspace 依赖风格:各 crate 用 `dep.workspace = true`;根 `Cargo.toml` `[workspace.dependencies]` 补新共享依赖(`interprocess`、`gethostname`、`winreg`、`tray-icon`);新 crate 版本随 workspace 0.1.0、edition 2021。
- 提交信息风格:`feat:` / `fix:` / `docs:` / `test:` 前缀(见 git log)。
- CLI 最终依赖白名单(反目标:CLI/UI 不得直连服务器):`mtdss-ipc`、clap、tokio、anyhow、serde_json、tracing、tracing-subscriber。
- 新二进制名:`mtdssd`(daemon);CLI 仍叫 `mtdss`。
- IPC 方法名与事件名按本计划 **verbatim**;RPC 错误码:1=业务失败(message 含失败报告全文),2=忙(传输进行中),-32601=未知方法,-32602=参数错误。
- UI 文案全部中文。
- 服务器代码(`server/`)与既有 proto 本切片**零改动**。

## Review Focus

1. **daemon 未运行且找不到 mtdssd** → CLI 必须报可行动错误(指明 MTDSSD_BIN/安装),不得挂死或 panic。→ Task 8 `find_daemon_bin` None 分支测试。
2. **同机并行多 daemon(不同 MTDSS_HOME,测试并行依赖)** → 管道/套接字名不得冲突。→ Task 2 `socket_path` 区分度测试 + 双 server 并发互通测试。
3. **信令服务器重启期间 daemon 不死、恢复后自动回在线** → 在线循环退避有上限、Closed/Error 必重连。→ Task 6 固定端口 server 杀/重启用例 + backoff 纯函数单测。
4. **JWT 失效(常驻 daemon 长跑必遇)** → 在线循环用 refresh 轮换并更新 session.json,不要求用户重登。→ Task 6 「垃圾 jwt + 有效 refresh → 自动恢复在线」用例。
5. **send 期间对端 daemon 被硬杀** → RPC 有界返回失败报告(重试耗尽),不永久挂起。→ Task 7 杀对端子进程用例。

---

### Task 1: 收端 EOF 舞步 2s 有界化(core 债务①)

**Files:**
- Modify: `crates/core/src/transport/transfer.rs`(`SendOptions` 加 fault 钩子;终局与拒绝路径的 EOF 等待改有界)
- Test: `crates/core/tests/transfer_v2.rs`(recv_files 所在测试文件;执行者以 `cargo test -p mtdss-core recv` 确认落点)

**Interfaces:**
- Consumes: 既有 `transfer::SendOptions`、`recv_files_fault`。
- Produces: `SendOptions` 新字段 `#[doc(hidden)] pub fault_no_eof_after_final: bool`(默认 false);`transfer.rs` 内私有常量 `EOF_DANCE_GRACE: Duration = Duration::from_secs(2)`;私有异步 `fn wait_peer_eof_grace(ctl_rx: &mut quinn::RecvStream)`。

- [ ] **Step 1: 写失败测试 `recv_returns_within_grace_when_sender_withholds_eof`**

沿该文件既有「双端点对接」样板搭一对连接;发送侧 `send_files` 以 `SendOptions { fault_no_eof_after_final: true, ..Default::default() }` 驱动(`tokio::time::timeout` 包裹防挂死),接收侧 `recv_files_fault` 计时:必须返回 `Ok`(终局帧已发出、文件已落盘),总耗时 ≤ `EOF_DANCE_GRACE + 3s`,且 ≥ 2s(证明确实等过宽限再放行)。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test transfer_v2 recv_returns_within_grace`
Expected: FAIL(现状无界:测试以 idle 超时或外层 timeout 失败/超时)

- [ ] **Step 3: 实现**

①`SendOptions` 加 `fault_no_eof_after_final`;`ControlMsg::Final` 处理分支(line ~419)在 finish 之前:若钩子为真,`tracing::warn!("fault: withholding EOF after Final")` 后 `std::future::pending::<()>().await`(扣住连接不断、不发 FIN)。②新增 `wait_peer_eof_grace`(把 `ctl_rx.read` 包进 `tokio::time::timeout(EOF_DANCE_GRACE, ..)`,结果丢弃——该等待只为给终局/拒绝帧留投递窗口)。③终局路径(line ~954 的 `let _ = ctl_rx.read(&mut tag).await;`)与 `reject_exit`(line ~1242 同款)都改调 `wait_peer_eof_grace`。

- [ ] **Step 4: 跑测试确认通过 + 既有测试不回归**

Run: `cargo test -p mtdss-core --test transfer_v2 && cargo test -p mtdss-core --test transfer`
Expected: PASS(全部)

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/transport/transfer.rs crates/core/tests/transfer_v2.rs
git commit -m "fix(core): bound receiver EOF dance with 2s grace (M3 debt #1)"
```

### Task 2: `crates/ipc` — mtdss-ipc 线协议 + 服务端/客户端

**Files:**
- Create: `crates/ipc/Cargo.toml`、`crates/ipc/src/lib.rs`
- Modify: 根 `Cargo.toml`(members + `[workspace.dependencies]` 增 `interprocess = "2"`(features `["tokio"]`)、`gethostname = "0.5"`)
- Test: `crates/ipc/tests/ipc.rs`

**Interfaces:**
- Consumes: 无。
- Produces(Task 3/7/10 依赖,签名 verbatim):

```rust
pub fn mtdss_home() -> std::path::PathBuf;   // 从 cli/session.rs 原样搬来($MTDSS_HOME 覆盖 → $HOME/.mtdss)
pub fn socket_path(home: &std::path::Path) -> String;
// Windows: r"\\.\pipe\mtdss-" + hex(sha256(home 的绝对路径))[..8]
// Unix:    {home}/daemon.sock

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcError { pub code: i64, pub message: String }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventFrame { pub method: String, pub params: serde_json::Value }

pub trait Dispatcher: Send + Sync {
    fn dispatch(&self, method: String, params: serde_json::Value)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, RpcError>> + Send>>;
}

pub async fn serve(home: &std::path::Path, dispatcher: std::sync::Arc<dyn Dispatcher>,
                   event_tx: tokio::sync::broadcast::Sender<EventFrame>) -> anyhow::Result<()>;
// NDJSON:一行一帧。请求 {"jsonrpc":"2.0","id":N,"method","params"};响应 {"id":N,"result"} 或
// {"id":N,"error":{code,message}};事件 = 通知帧 {"jsonrpc":"2.0","method","params"}(无 id)。
// 连接逐个 accept,每连接双半工任务;事件向所有连接广播。Unix 下 socket 文件 0600。

pub struct IpcClient { /* ... */ }
impl IpcClient {
    pub async fn connect(home: &std::path::Path) -> anyhow::Result<Self>;   // 连不上 → Err(快速失败)
    pub async fn call(&self, method: &str, params: serde_json::Value)
        -> Result<serde_json::Value, RpcError>;                              // id 自增,并发调用按 id 路由
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<EventFrame>;    // 订阅推送
}

pub fn find_daemon_bin() -> Option<std::path::PathBuf>;
// 解析序:env MTDSSD_BIN → current_exe 同目录的 mtdssd[.exe] → cfg(debug_assertions) 下
// 从 CARGO_MANIFEST_DIR 逐级向上找 target/{debug,release}/mtdssd[.exe]
```

- [ ] **Step 1: 写失败测试**(逐条,一个 `#[tokio::test]` 一断言主题)
  `socket_path_distinct_per_home`(两个不同 home → 两个不同管道名/socket;同 home 稳定)、`codec_roundtrip`(请求/响应/错误/事件四种帧序列化→逐行解析还原)、`unknown_method_returns_minus_32601`、`echo_call_and_concurrent_calls`(两个客户端 × 各 8 个并发 call,id 路由不错串)、`event_broadcast_reaches_all_clients`、`missing_socket_fails_fast`(connect 对不存在的 socket 在 2s 内返回 Err)、`large_params_roundtrip`(1 MiB params)。
- [ ] **Step 2: 跑测试确认失败**
  Run: `cargo test -p mtdss-ipc`
  Expected: FAIL(crate 未实现)
- [ ] **Step 3: 实现 `lib.rs`** — `interprocess` tokio LocalSocket 监听/连接;`serve` accept 循环 + 每连接读循环(行缓冲 8 MiB 上限,超限断连)+ 事件写广播(慢消费者用 `try_send`,满则跳帧断连防一个卡端拖死全部);`IpcClient` 读任务把响应按 id 分发 oneshot、通知帧进 broadcast。Unix 落盘 `daemon.sock` 权限 0600(Windows 命名管道默认 ACL 即本用户)。
- [ ] **Step 4: 跑测试确认通过**
  Run: `cargo test -p mtdss-ipc`
  Expected: PASS
- [ ] **Step 5: Commit** — `feat(ipc): JSON-RPC over NDJSON local-socket server/client (mtdss-ipc)`

### Task 3: `crates/daemon` 骨架 + session 迁移 + `mtdssd` 二进制

**Files:**
- Create: `crates/daemon/Cargo.toml`(lib `mtdss_daemon` + bin `mtdssd`;deps:mtdss-core、mtdss-proto、mtdss-ipc、tokio、serde、serde_json、anyhow、tracing、tracing-subscriber、gethostname;dev:tempfile、signaling、stun-server、reqwest)
- Create: `crates/daemon/src/state.rs`(`DaemonState`:home、logged_in、session:Option、settings、事件通道;内部锁)
- Create: `crates/daemon/src/api.rs`(方法分派,实现 `mtdss_ipc::Dispatcher`)
- Create: `crates/daemon/src/main.rs`(flags:`--recv-dir <DIR>` 默认 `{home}/inbox`、`--log-file <PATH>` 默认 stderr、`--no-tray`)
- Create: `crates/daemon/src/settings.rs`(`settings.json` {autostart:bool, recv_dir:Option<String>} 的 load/save;Task 7 用 recv_dir、Task 9 用 autostart)
- Move: `git mv crates/cli/src/session.rs crates/daemon/src/session.rs`(`Session`/`PeersCache`/`write_private` 原样;`mtdss_home` 已在 Task 2 搬走,此处删)
- Modify: `crates/cli/src/lib.rs`(加临时 shim `pub use mtdss_daemon::{session, flow};` —— flow 尚未搬,本任务只 shim session:`pub use mtdss_daemon::session;`,flow 的 shim Task 5 再加)
- Modify: `crates/cli/Cargo.toml`(临时加 `mtdss-daemon` path dep;Task 8 撤)
- Modify: 根 `Cargo.toml` members += `crates/daemon`
- Test: `crates/daemon/tests/daemon_basic.rs`

**Interfaces:**
- Consumes: `mtdss_ipc::{serve, IpcClient, Dispatcher, RpcError}`。
- Produces(Task 4-9 依赖):

```rust
pub struct DaemonOpts { pub recv_dir: std::path::PathBuf, pub tray: bool }
impl Default for DaemonOpts { /* recv_dir = mtdss_home()/inbox, tray = false */ }

pub struct Daemon { pub events: tokio::sync::broadcast::Sender<mtdss_ipc::EventFrame>, /* ... */ }
impl Daemon {
    pub async fn start(home: &std::path::Path, opts: DaemonOpts) -> anyhow::Result<Daemon>;
    pub async fn shutdown(self) -> anyhow::Result<()>;
}
```

本任务实现的方法:`hello` → `{"version":"0.1.0","logged_in":bool,"device":null|{"device_id","name"}}`;`status` → `{"logged_in":bool,"online":false,"server":null|String,"device":null|{...},"recv_dir":String}`。未知方法 → -32601。`DaemonState` 提供 `pub(crate)` 读接口:`logged_in()`、`session()`、`recv_dir()`、`home()`。

- [ ] **Step 1: 写失败测试** — `daemon/tests/daemon_basic.rs`:`hello_before_login`(临时 home 起 Daemon → IpcClient `hello` → logged_in=false、version 有值)、`status_reflects_opts`(recv_dir 回显)、`second_daemon_same_home_fails`(同 home 第二个 `Daemon::start` 返回 Err——socket 占用)、`shutdown_closes_socket`(shutdown 后 connect Err)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon` Expected: FAIL
- [ ] **Step 3: 实现** — session.rs git mv;cli shim;`Daemon::start` = 加载 session/settings(不存在则默认)→ 起 `mtdss_ipc::serve` → 返回句柄;`mtdssd` main:解析 flags/MTDSS_HOME、tracing(文件则按行 JSON 落盘)、`Daemon::start` + `ctrl_c` 等待后 shutdown(tray 标志本任务仅接受不生效,Task 9 接线)。
- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 全 workspace 门禁(shim 保住 CLI 兼容)** — Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Expected: PASS(CLI 走 shim 行为不变)
- [ ] **Step 6: Commit** — `feat(daemon): resident daemon skeleton with IPC + session takeover (mtdssd)`

### Task 4: 登录链路迁移(account REST → daemon)+ login.* RPC

**Files:**
- Create: `crates/daemon/src/account.rs`
- Modify: `crates/daemon/src/api.rs`(login.request_code / login.verify / login.logout)、`crates/daemon/src/state.rs`
- Test: `crates/daemon/tests/login.rs`

**Interfaces:**
- Consumes: `signaling::spawn_test_server`(测试);`flow.rs` 里的 REST 段落(搬移源,cli/flow.rs 在 Task 5 才整体搬)。
- Produces(account.rs,Task 6 复用):

```rust
pub(crate) fn http_client(ca: Option<&Path>) -> anyhow::Result<reqwest::Client>;   // 原 flow.rs 原样搬
pub(crate) async fn request_code(server:&str, email:&str, ca:Option<&Path>) -> Result<String>;
pub(crate) async fn verify(server:&str, email:&str, code:&str, pubkey_hex:&str, name:&str,
                           ca:Option<&Path>) -> Result<(String /*jwt*/, String /*refresh*/, String /*device_id*/)>;
pub(crate) async fn refresh(server:&str, refresh:&str, ca:Option<&Path>) -> Result<(String, String)>;  // POST /auth/refresh 轮换
pub(crate) async fn fetch_devices(s:&crate::session::Session) -> Result<Vec<serde_json::Value>>;
pub(crate) fn peer_pk_from(devices:&[serde_json::Value], device_id:&str) -> Result<ed25519_dalek::VerifyingKey>;
```

RPC 方法:`login.request_code {server, email, ca?}` → `{"code":String}`(dev 码回显透传;server/email/ca 存入 state 作 pending);`login.verify {email, code, name?}` → `{"device_id","name"}`(name 缺省 `gethostname()`;身份走 `load_identity`(从 flow.rs 搬到 daemon/src/flow.rs 的事 Task 5,本任务先在 account.rs 内联同逻辑:identity.seed 不存在则生成落盘 0600);verify 成功写 session.json、置 logged_in);`login.logout {}` → `{}`(删 session.json、置 logged_out)。

- [ ] **Step 1: 写失败测试** — `login.rs`(spawn_test_server + Daemon::start + IpcClient):`request_code_returns_six_digits`;`verify_wrong_code_is_rpc_error`;`verify_ok_saves_session_and_hello_shows_device`(hello.logged_in=true、device_id 回显、`session.json` 存在);`logout_clears_session`;`relogin_same_email_ok`。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test login` Expected: FAIL
- [ ] **Step 3: 实现** — account.rs 搬 REST 四函数(+refresh);api.rs 三方法;state 加 pending-login 与登录态翻转(翻转时向 events 发 `event.devices` 空/全量与 `event.online {online:false}`?在线循环 Task 6 才有,本任务登录态变化只影响 hello/status)。
- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: Commit** — `feat(daemon): account REST migration + login/logout over IPC`

### Task 5: flow 编排整体迁入 daemon(机械搬移)

**Files:**
- Move: `git mv crates/cli/src/flow.rs crates/daemon/src/flow.rs`(改 `use crate::session::…`;`load_identity` 变 `pub(crate)`;`pub` 面(apply_test_env/SendOpts/RecvOpts 等)保持)
- Modify: `crates/daemon/src/lib.rs`(mod flow)、`crates/cli/src/lib.rs`(shim 扩为 `pub use mtdss_daemon::{session, flow};`)、`crates/daemon/Cargo.toml`(补 quinn、rand、hex、ed25519-dalek、reqwest、futures-util、sha2 等实际所需)、`crates/cli/Cargo.toml`(如暂缺则补,Task 8 一并撤)
- Test: 搬 `git mv crates/cli/tests/flow.rs crates/daemon/tests/flow_unit.rs`(`mtdss_cli::` 引用改 `mtdss_daemon::`)

**Interfaces:**
- Consumes: Task 3 的 `session`。
- Produces: `mtdss_daemon::flow::{load_identity, run_send_with, run_recv_with, apply_test_env, SendOpts, RecvOpts, …}`(全部原名;Task 6/7 依赖 `run_send_with` 与 `apply_test_env`)。

- [ ] **Step 1: 搬移 + 修引用** — git mv 两文件;cli/daemon lib.rs mod 与 shim;daemon Cargo.toml 补依赖。
- [ ] **Step 2: 验证行为不变** — Run: `cargo test -p mtdss-daemon --test flow_unit && cargo test -p mtdss-cli --test cli_e2e`
  Expected: PASS(旧 e2e 走 shim 仍绿——搬移零行为变化)
- [ ] **Step 3: 门禁 + Commit** — Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
  Commit: `refactor(daemon): move send/recv orchestration from cli (behavior unchanged)`

### Task 6: 在线循环 + devices RPC + 在线事件

**Files:**
- Create: `crates/daemon/src/online.rs`
- Modify: `crates/daemon/src/api.rs`(`devices` 方法)、`state.rs`(devices 缓存 + online 标志)、`main.rs`(启动时若 logged_in 则拉起在线循环;Task 4 的登录成功同样触发)、`lib.rs`(Daemon::start 起在线任务,shutdown 通知)
- Test: `crates/daemon/tests/online.rs`

**Interfaces:**
- Consumes: `flow::signaling_connect`(改 `pub(crate)`)、`account::{fetch_devices, refresh}`、`SignalingClient` 事件流。
- Produces: RPC `devices {}` → `{"devices":[{"device_id","name","platform","online"}]}`(快照,含本机,本机恒 online=自身连接态);事件 `event.online {"online":bool}`、`event.devices {"devices":[…全量快照…]}`(连接建立取目录全量,此后 Presence 增量更新即发新快照);`online.rs` 私有 `fn backoff_for(attempt:u32)->Duration`(1s×2^(n-1),封顶 30s——纯函数供单测)。

- [ ] **Step 1: 写失败测试** — `online.rs`:
  `backoff_caps_at_30s`(纯函数:1,2,4,…,30,30);
  `login_then_online_and_devices_snapshot`(测试服务器 + daemon 登录 → `status.online=true`;用 `SignalingClient` 以第二个已登记身份直连服务器制造 presence → daemon 的 `event.devices` 快照含该设备 online=true;断开 → 快照转 false;`devices` RPC 同步一致);
  `dead_jwt_refreshes_and_recovers`(登录后手动把 `session.json` 的 jwt 换成垃圾串、refresh 保持有效 → 重启 Daemon → 在线循环收 Error → 走 account::refresh → session.json 的 jwt 已更新且 `status.online=true`);
  `server_restart_daemon_recovers`(测试服务器监听固定端口:drop 监听 → daemon `event.online {online:false}`;同端口重启 → 最终 `online:true`,期间 daemon 进程不退)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test online` Expected: FAIL
- [ ] **Step 3: 实现** — online.rs 主循环:未登录则等 state 通知;登录后 `signaling_connect` → HelloOk(取 stun 忽略,本任务不用)→ fetch_devices 全量 + 发快照 → 循环收事件:Presence 增量更新缓存重发快照;Closed/Error → 退避重连,遇鉴权错误先 `refresh` 一次(写回 session)再重连;state.online 翻转发 `event.online`。固定端口测试服务器:测试内用 `axum::serve(TcpListener::bind(127.0.0.1:P))` + `signaling::build_router` 自建(注意库导出以 server/signaling/src/lib.rs 实际 pub 为准,`spawn_test_server` 无法固定端口时可仿其内部实现)。
- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): resident signaling online loop, device snapshot + events, jwt refresh`

### Task 7: 传输经 daemon(send RPC、常驻接收、daemon_e2e)

**Files:**
- Modify: `crates/daemon/src/flow.rs`(新增 `pub(crate) async fn resident_recv_loop(state, shutdown)`——`run_recv_with` 的常驻版:deadline 逻辑全部移除,轮错误 → `RECV_ROUND_PACE` 小歇重进,干净轮 → 继续等下一来件,永不返回 Ok/Err 直到 shutdown)
- Modify: `crates/daemon/src/api.rs`(`send`、`set_recv_dir`)、`state.rs`(recv_dir 可更新并持久化进 `settings.json` 的 `recv_dir` 字段;发送互斥)、`lib.rs`(Daemon::start 且 logged_in 时拉起常驻接收;`apply_test_env` 在 daemon 启动时以 `|k| std::env::var(k).ok()` 注入两侧 opts)
- Test: `crates/daemon/tests/daemon_e2e.rs`(新)

**Interfaces:**
- Consumes: Task 5 `flow`、Task 6 在线循环(event.devices 供测试等对端在线)。
- Produces: RPC `send {files:[String], to:String}` → `{}` 成功;失败 → `RpcError{code:1,message:<failure_report 全文>}`;并发第二个 → `RpcError{code:2}`(「传输进行中」)。RPC `set_recv_dir {dir}` → `{}`。事件 `event.send_log {"line":String}`(flow 的 out_report 每行一条)。

- [ ] **Step 1: 写失败测试** — `daemon_e2e.rs`(进程内双 Daemon + 双 IpcClient,对端在线等 `event.devices`):`send_files_and_dirs_land_in_recv_dir`(混合文件/目录 → `{recv_dir}` 下按 manifest rel 落盘、逐文件内容一致);`send_log_events_observed`(至少收到清单/完成各一行);`set_recv_dir_persists`(改目录 → 新传输落新目录;重启 Daemon 仍用新目录——settings.json);`send_rejected_surfaces_report`(对端 `recv_dir` 指向不可写位置 → code=1 且 message 非空、含失败分栏文案,不 brittle 匹配具体字串);`concurrent_send_is_code_2`;`peer_daemon_killed_send_fails_bounded`(真实 `mtdssd` 子进程当对端——`env!("CARGO_BIN_EXE_mtdssd")` + 独立 MTDSS_HOME + IPC 登录;传输中途 kill -9 → 发送侧 RPC 在 `max_reconnects×backoff + 60s` 内返回 code=1)。
  另:移植 `crates/cli/tests/cli_e2e.rs` 其余既有用例语义到本文件(静默断链跨进程续传、manifest 超限 reject、坏块重传等——逐用例对号,断言保持;`register_rest` 辅助函数一并搬)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test daemon_e2e` Expected: FAIL
- [ ] **Step 3: 实现** — resident_recv_loop;api 两方法(send 里 tokio::spawn 跑 `run_send_with`,out_report → event.send_log,结束经 oneshot 回 RPC;state 发送互斥锁);启动接线。
- [ ] **Step 4: 跑测试确认通过** — Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): transfers over IPC + resident receiver + daemon e2e`

### Task 8: CLI 换轨为 IPC 客户端 + nat-matrix/chaos 迁移 + 旧直连测试删除

**Files:**
- Modify: `crates/cli/src/main.rs`(命令重构:`login --server --email [--ca] [--name]`(request_code RPC → 终端回显 dev 码 + stdin 读码 → verify RPC)、`logout`、`devices`、`status`、`send <PATHS...> --to`、`recv --out <DIR>`(→ set_recv_dir);全局 `--ca` 旗标取消,ca 是 login 参数)
- Create: `crates/cli/src/client.rs`(`DaemonClient::connect_or_spawn(home)`:先 1s 试连;失败 → `mtdss_ipc::find_daemon_bin()`(None → 报「找不到 mtdssd:请安装或设置 MTDSSD_BIN」退出码 2);spawn 后台(Windows `CREATE_NO_WINDOW` 0x08000000,Unix 正常 spawn,`--no-tray` 不传——CLI 拉起即默认带托盘;Linux 无托盘编译)→ 最多 10s 轮询重连;连上后 `hello`:!logged_in 且命令非 login/status → 打印「未登录:请先 `mtdss login`」退出码 2)
- Modify: `crates/cli/Cargo.toml`(依赖收敛到白名单;删 mtdss-daemon 依赖)、`crates/cli/src/lib.rs`(删 shim)
- Delete: `crates/cli/tests/cli_e2e.rs`(语义已由 daemon_e2e + 矩阵接管)
- Create: `crates/cli/tests/ipc_cli.rs`(dev-dep `mtdss-daemon` 进程内起 daemon)
- Modify: `scripts/nat-matrix/lib.sh`(nm_register 改走 CLI login;新增 `nm_start_daemon <ns> <home> <log> <recv_dir> [env...]`:起 `mtdssd`;`nm_build` 同时构建 `mtdss`+`mtdssd`)、`run_case.sh`(③ 双侧起 daemon(日志即 LOG_A/LOG_B;`MTDSS_TEST_*` 注入 env 移到 daemon 启动)→ ④「b 侧 recv 后台」删除(daemon 常驻)→ presence 等待改轮询 `mtdss devices` → ⑤ send 走 CLI → ok 例断言改:轮询 OUT_B 文件齐 + LOG_B 出现 finalize/终局日志行(替代旧「recv 进程自退」等待);srflx grep 改查 daemon 日志(文案行不变)、`README.md` 同步)
- Modify: `scripts/nat-matrix/chaos.sh`(同型迁移:netem 作用于 netns,daemon 常驻、send 经 CLI;断言从 recv 进程退出改为文件落地 + daemon 日志)
- Test: `crates/cli/tests/ipc_cli.rs`

**Interfaces:**
- Consumes: Task 2 `IpcClient/find_daemon_bin`、Task 3-7 全部 RPC 方法。
- Produces: CLI 命令面(上列);`DaemonClient { pub ipc: mtdss_ipc::IpcClient, pub hello: serde_json::Value }`。

- [ ] **Step 1: 写失败测试** — `ipc_cli.rs`(进程内 Daemon + 真 `DaemonClient`):`not_logged_in_commands_refuse_with_hint`(devices/send → 退出码 2 + 文案含「未登录」);`login_devices_status_flow`(全链);`set_recv_dir_via_recv_cmd`;`find_daemon_bin_env_override`(设 MTDSSD_BIN 指向临时假文件 → 返回该路径;清掉且无同名兄弟 → None 或 dev 回退,断言不 panic)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-cli` Expected: FAIL
- [ ] **Step 3: 实现 CLI** — main.rs/client.rs 重构;依赖收敛;删 shim 与旧 e2e。
- [ ] **Step 4: 本机门禁** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] **Step 5: 迁移脚本(WSL/Linux 侧人工核一遍)** — 改 lib.sh/run_case.sh/chaos.sh/README;新增**自动拉起门禁用例**:run_case.sh 支持 `NM_AUTO_SPAWN_A=1`(a 侧不预起 daemon,由 CLI send 自动拉起——CLI 自动拉起行为唯一的真实二进制覆盖);matrix.sh 在 14 例之外追加该单例(如 full×full ok)。本机无 netns,跑 `bash -n scripts/nat-matrix/*.sh` 语法检查;推送后看 CI nat-matrix job 全绿(矩阵回归以 CI 为准)。
- [ ] **Step 6: Commit** — `feat(cli)!: cli becomes pure IPC client; nat-matrix/chaos migrate to daemon+cli`

### Task 9: 托盘 + 开机自启(Windows;Linux/macOS 编译不含)

**Files:**
- Create: `crates/daemon/src/tray.rs`(`#[cfg(windows)]`;含纯函数 `fn tray_icon_rgba(colored: bool) -> Vec<u8>`(32×32 RGBA,中心实心圆填充主色/灰色)与 `fn icon_for(logged_in: bool) -> bool`)
- Modify: `crates/daemon/src/settings.rs`(autostart 字段接线;文件已在 Task 3 建立)、`api.rs`(`autostart.get {}` → `{"enabled":bool}`;`autostart.set {enabled}` → `{}`)、`state.rs`(登录态 watch → 托盘换图标)、`main.rs`(`--no-tray` 与 cfg(windows) 下 `tray::spawn_tray(state)`)
- Modify: 根 Cargo.toml(`[workspace.dependencies]` 增 `tray-icon = "0.20"`、`winreg = "0.55"`)+ daemon Cargo.toml(`[target.'cfg(windows)'.dependencies]`)
- Test: `crates/daemon/tests/tray_logic.rs`(cfg(windows) 为主,纯函数跨平台)

**Interfaces:**
- Consumes: Task 3-6 登录态/事件。
- Produces: 托盘行为(设计 §5.1):登录=亮色图标、登出=灰;右键菜单 = 「打开主窗口」(M3a 置灰)、「登录/登出」(动态文案)、「设置 ▸ 开机自启 ✓」、「退出」(daemon 优雅退出);菜单动作直调进程内 daemon API(登录项 = 打开主窗口的动作 M3a 缺 UI,先置灰并 tooltip「请用 mtdss login」)。开机自启 = HKCU `Software\Microsoft\Windows\CurrentVersion\Run` 值名 `MTDSS` = `"…\mtdssd.exe"`(带引号完整路径)。

- [ ] **Step 1: 写失败测试** — `tray_logic.rs`:`icon_for_true_false`;`tray_icon_rgba_shape`(1024×4 字节;中心 alpha=255 且非灰、边缘 alpha=0);`autostart_set_true_writes_run_key_then_false_removes`(cfg(windows):真写 HKCU,断言值存在/等于带引号路径 → set false → 值删除;测试自带清理);`autostart_get_set_roundtrip_rpc`(进程内 Daemon + IpcClient)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test tray_logic` Expected: FAIL
- [ ] **Step 3: 实现** — tray-icon 专属线程 + win32 消息泵(按 tray-icon crate Windows 文档的推荐泵模式);图标 `tray_icon::Icon::from_rgba(tray_icon_rgba(colored),32,32)`;菜单事件 → state 方法;winreg 写删。
- [ ] **Step 4: 跑测试确认通过 + 双平台编译** — `cargo test -p mtdss-daemon`;Linux(WSL/CI)`cargo check -p mtdss-daemon`(cfg 隔离无 tray 依赖)。
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): windows tray with login-state icon + autostart setting`

### Task 10: Tauri v2 UI 壳(树=设备列表、状态栏、登录引导态)

**Files:**
- Create: `ui/`(vite + react-ts + Tauri v2 脚手架;`ui/src-tauri/` 独立 crate,根 Cargo.toml `exclude = ["ui/src-tauri"]` + `ui/` gitignore(node_modules/dist)
- Create: `ui/src-tauri/src/ipc_bridge.rs`(IpcClient 管理:启动时 connect → 失败 `find_daemon_bin` spawn mtdssd → 重试;tauri command 六个:`hello`、`login_request_code(server,email,ca?)`、`login_verify(email,code,name?)`、`logout`、`devices`、`status`;事件转发:`event.online`/`event.devices` → `app_handle.emit`)
- Create: `ui/src/lib/derived.ts`(`sortDevices(devices, selfDeviceId)`——本机置顶;`onlineText(status)`——未登录/离线/在线 中文文案)+ `ui/src/lib/derived.test.ts`(vitest)
- Create: `ui/src/i18n/`(react-i18next + `zh.json`,全部文案入典)
- Create: `ui/src/App.tsx` + 组件(布局按设计 §3:工具栏全 disabled 占位、地址栏占位、侧栏「常用目录」灰占位 + 「MTDSS设备」树(可点击 → 设备列表视图)、状态栏(信令:在线/离线/未登录 · N 台设备)、未登录引导态(说明 + 登录表单:服务器/邮箱 → 取码 → 码/设备名 → 登录))
- Modify: `docs/superpowers/specs/2026-10-17-mtdss-m3-explorer-design.md`(§5.3 一句微调:引导态含登录表单,托盘「登录」唤起主窗——M3a 托盘登录项仍置灰)

**Interfaces:**
- Consumes: `mtdss-ipc`(path dep)、全部 RPC 方法、`event.online`/`event.devices`。
- Produces: 可 `npm run tauri dev` 运行的壳;窗口标题「MTDSS」;dev 下 daemon 自动拉起。

- [ ] **Step 1: 脚手架** — `npm create tauri-app@latest ui -- --template react-ts`(identifier `com.mtdss.ui`);根 exclude;`npm i` + `npm i -D vitest`;加 react-i18next。
- [ ] **Step 2: 写失败测试** — `npx vitest run src/lib/derived.test.ts`(sortDevices 本机置顶/其余保序;onlineText 三态文案)Expected: FAIL
- [ ] **Step 3: 实现** — derived.ts + i18n + ipc_bridge.rs + App 组件树(设备列表为「详细信息」式表格:名称/平台/在线;本机行加「(本机)」);登录表单调 `login_request_code` 后展示 dev 码(开发期)。
- [ ] **Step 4: 构建冒烟** — `npm run build`(vite)PASS;`npm run tauri build -- --no-bundle` 或 `cargo check --manifest-path ui/src-tauri/Cargo.toml` PASS;`npm run tauri dev` 人工冒烟(连接 daemon、设备列表刷新、登录表单可用)。
- [ ] **Step 5: Commit** — `feat(ui): tauri v2 shell — device tree, status bar, login guide`

### Task 11: CI ui job + 文档收尾 + 终门禁

**Files:**
- Modify: `.github/workflows/ci.yml`(新增 `ui` job:windows-latest,setup-node(22)+ `npm ci` + `npm run build` + `cargo check --manifest-path ui/src-tauri/Cargo.toml`;nat-matrix job 不动路径——Task 8 已让脚本自洽)
- Modify: `docs/release-checklist.md`(增「M3a 手动冒烟」段:托盘亮/灰、托盘退出、开机自启开关生效、CLI 自动拉起 daemon、UI 壳连接/登录/设备列表)
- [ ] **Step 1: ci.yml 加 ui job**
- [ ] **Step 2: release-checklist 增段**
- [ ] **Step 3: 终门禁(本机全量)** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`;`cd ui && npm run build`
- [ ] **Step 4: Commit + 推送确认 CI 三平台 job 全绿** — `docs(ci): ui smoke job + m3a manual smoke checklist`
