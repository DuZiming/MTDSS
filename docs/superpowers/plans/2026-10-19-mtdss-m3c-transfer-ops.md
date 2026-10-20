# MTDSS M3c(传输与操作)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 M3 的操作面:上传/下载到本机/粘贴写入/删除/剪切/冲突对话框/A→B/隐藏暂存夹收编(含 .mtdss 大小写债务)/通知/传输面板/拖出落点解析,并清偿 M3b 终审携带清单(登出拆卸 ctrl、fs spawn_blocking、C:foo、ctrl_e2e stderr、code3 文案、nav.ts 告诫、StatusBar 加载态、快照陈旧、EOF 竞速重估、UI 打磨)。

**Architecture:** 冲突决策在**操作端**(远端无人值守不弹确认):操作端预扫目标目录(本机直查/远端经 ctrl list)→ 跳过/覆盖策略随**定向传输请求**下发。定向 = `ConnectRequest` 携带 `target_dir + job_id + policy`(密封信封内加字段:服务器零改动、旧端互通不断——旧收端忽略新字段落默认目录、旧发端无字段走 M2 默认语义);接收侧 `reconcile_with_policy` 按策略出 Skip/覆盖(覆盖=计划期移除既有文件)计划,引擎发送端**零改动**。A→B/下载 = 经既有 ctrl 通道发 `orchestrate` 指令(数据不经操作端落地);cut = 操作端在传输成功后对源端发 ctrl delete。传输作业化:daemon `jobs.rs` 注册表 + `transfer.*` RPC + `event.transfer` 进度(节流)+ 双侧 pause 门(发送端波间/块前查、接收端停读靠 QUIC 流控背压)+ cancel(本端 abort 任务 + 经 ctrl 传播)。拖出落点解析独立薄模块(UIA),先 spike 定去留。

**Tech Stack:** 既有栈不动;新增 `trash`(core,Windows 走 IFileOperation+ALLOWUNDO)、`notify-rust`(daemon 通知)、`tauri-plugin-dialog` + `windows-sys`(UI 选择器/UIA/剪贴板)。

**Spec:** `docs/superpowers/specs/2026-10-17-mtdss-m3-explorer-design.md`(M3 设计 §4 交互矩阵/§4.3 冲突删除暂存/§6 传输管理 + 2026-10-19 brainstorm 定稿:目录恒合并、CLI put/get/cp/rm/mv、暂停=停发保活断链转续传、传输面板=窗内卡片、通知 daemon 侧发、拖出 spike 前置、携带件全收)

## Global Constraints

- 门禁 = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 双平台(本机 Windows + WSL)全绿 + nat-matrix + ui job 全绿。**每个任务提交前门禁必须全绿。**
- 本机 bash 环境:`export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`。
- cargo 走 rsproxy.cn;npm 慢时临时用 npmmirror(**不提交 .npmrc**);git 走 SSH;gh 在 bash 需 `export PATH="/c/Program Files/GitHub CLI:$PATH"`。
- **服务器代码与服务器可见协议零改动**:ws 帧与 `signal.proto`(mtdss.ws)不动;`e2e.proto` 的 `ConnectRequest` 在密封信封内加字段(服务器不透明);`manifest.proto` **一字不动**(策略不经清单,签名域不变);`ctrl.proto` `CTRL_VERSION` 升 **2**(旧端对未知 oneof 变体解 body=None → 忽略/报错,互通不破;混版本互通非门禁)。
- IPC 契约 verbatim(新方法,既有方法/事件签名不变):
  - `transfer.start`:upload `{"kind":"upload","files":[String],"to":dev_id,"dir":abs,"skip":[],"overwrite":[],"cut":false}`;download `{"kind":"download","device":dev_id,"paths":[abs@远端],"dir":abs本机,"skip":[],"overwrite":[],"cut":false}`;remote_copy `{"kind":"remote_copy","from":dev_id,"paths":[],"to":dev_id,"dir":abs@to,"skip":[],"overwrite":[],"cut":false}`;local_copy `{"kind":"local_copy","paths":[本机],"dir":abs,"cut":false}` → 一律 `{"job_id":"<u64 字符串>"}`(终审注记:download/remote_copy 实际返回操作端命名空间 id `"<本机dev>:<seq>"`——T7 修复,回显进源端注册表后与源端自增面/其他操作端永不撞号;upload/local_copy 仍为裸 u64 序号。契约补注,原文不重写)。
  - `transfer.list {}` → `{"jobs":[{id,kind,state("queued|running|paused|done|failed|cancelled"),files_done,files_total,bytes_done,bytes_total,speed_bps,src,dst,error}]}`;`transfer.pause|resume|cancel {id}` → `{}`;`transfer.wait {id}` 长轮询 → 终态报告 `{state,error?,files_done,files_total,bytes_total}`。
  - `delete {device,paths:[],force:bool}` → `{"deleted":n,"recycled":bool,"failed":[{path,code,message}]}`;`move {device,pairs:[{from,to}]}` → `{"moved":n,"failed":[…]}`;`copy {device,paths:[],dir}` → `{"copied":n,"failed":[…]}`(move/copy 仅同设备;跨设备报 code 1 提示用 cp)。
  - `devices` 改现拉:`{"devices":[…],"stale":bool}`(REST 失败回缓存且 stale=true)。
  - 事件:`event.transfer {id,kind,state,files_done,files_total,bytes_done,bytes_total,speed_bps,error?}`(状态翻转即发;进度节流 ≥400ms);`event.notify {"title","body"}`;`event.send_log` 不变。
  - 错误码:1/2/3 语义不变(2 仅遗留 send;jobs 并发不再互斥);回收站不可用 = code 1 且 message 含标记串 `回收站不可用`(CLI/UI 据此升级为永久删除确认)。
- 冲突语义钉死:**目录恒合并**(create_dir_all),冲突判定只做文件级(同名双双为文件,或同名类型不匹配);策略条目按 rel 精确或**目录前缀**(`p` 匹配 `p` 与 `p/…`)命中;预扫未覆盖的嵌套同名 → 维持现状自动改名 `name (1)`(安全网,冒烟文档记录)。策略上限:skip+overwrite 合计 ≤2048 条、单条 ≤4096 字节(超限在操作端预扫即拒)。
- 删除语义钉死:可回收 → **两端完全静默**(操作侧不弹);回收不可用 → 操作端确认后 `force:true` 永久删;Windows 走 `trash` crate(IFileOperation+ALLOWUNDO),Unix 走 freedesktop/macOS Trash。
- 暂停语义钉死:暂停=停发新块、连接保活(传输调优默认 keep-alive 10s);暂停期间断链 → job 转 running-等待,resume 后由既有重试循环重打洞续传(intake RESUME 语义天然成立);接收侧暂停=停读 uni 流靠 QUIC 流控背压。
- 已知 V1 限制(计划内记录,非缺陷):同一接收方串行收件(resident loop 单轮),并发同向传输第二传在 ~50s Accept 窗口外失败可重试;A→B/下载的远端源冲突预扫只到**顶层**(操作端无对端全树),嵌套同名走自动改名安全网。
- CLI 依赖白名单不变(rm/mv/put/get/cp 零新依赖);UI 文案全中文入 `zh.json`;提交信息 `feat:`/`fix:`/`docs:`/`test:`/`refactor:` 前缀;`smoke/` 不入库。

## Review Focus

1. **冲突策略边界**:rel 前缀命中不得误伤(`ab` 不匹配 `abc`/`ab-c`);类型不匹配(文件↔目录)不 panic、按覆盖语义清障;skip 条目发 FileDone 计入完成;未扫嵌套同名落自动改名安全网。→ Task 3 `policy_prefix_requires_separator` / `policy_overwrite_type_mismatch` / `policy_skip_is_counted_done` + Task 6 `scan_conflicts_*`。
2. **取消/暂停/断链竞速**:cancel 必须**不复活**(abort 任务即杀 run_send_with 重试循环);pause 期间对端死亡 → resume 后重连续传不丢已落盘;download 取消经 ctrl 传播到发送端 job(发送端不得重试到天荒地老)。→ Task 6 `cancel_aborts_retry_loop`、`pause_survives_peer_death` + Task 8 `download_cancel_propagates`。
3. **定向接收与绑定**:target_dir 相对/NUL/UNC → 拒用并回退默认收件目录(warn,不静默落错盘);旧发端(无字段)/旧收端(忽略字段)互通;job_id 绑定进度路由,legacy 无 job_id 来件生成匿名 incoming 卡片。→ Task 5 `target_dir_invalid_falls_back` / `legacy_send_still_lands_default_dir` + Task 7 `download_progress_routes_to_job`。
4. **删除不可逆面**:回收不可用必须显式 force 才永久;批量部分失败逐条报告不半途弃批;trash 调用失败不 panic。→ Task 2 `delete_no_recycle_needs_force` / `delete_partial_failure_report` + Task 4 e2e。
5. **拖出解析误判**:非文件系统视图(库/此电脑/回收站/`::{GUID}`/UNC)一律 Unresolvable;UIA/COM 失败安全降级不 panic;解析出的路径必须是已存在的目录。→ Task 10 `parse_address_bar_rejects_non_fs` 系列 + 手动验证矩阵。
6. **UI 操作竞速**:粘贴目标离线/传输面板事件乱序重复不炸渲染;框选与 Ctrl/Shift 选择互不污染。→ Task 11 `rubber_band_*` + Task 13 `applyTransferEvents_dedupes`。

---

### Task 1: fs 路径硬化 + 保留名大小写不敏感(清偿 C:foo 与 .mtdss 大小写债务)

**Files:**
- Modify: `crates/core/src/fs.rs`(normalize/RESERVED_NAMES)
- Modify: `crates/core/src/protocol/manifest.rs:10`(`validate_rel_path` 首段保留名大小写)
- Modify: `crates/daemon/src/flow.rs`(collect_items/walk_dir 保留名比较)
- Test: `crates/core/tests/fs.rs`(增例)、`crates/core/tests/manifest.rs`(增例)、`crates/daemon/tests/flow_unit.rs`(增例)

**Interfaces:**
- Produces(行为契约,供 Task 3/5 依赖):
  - Windows 绝对形态收紧:`C:foo`(第三字符非 `\`/`/` 且长度>2)→ `InvalidPath`;`C:`(裸盘符)与 `C:\…` 语义不变。Unix 不变。
  - `RESERVED_NAMES` 匹配改 `eq_ignore_ascii_case`(fs 列表、manifest rel 首段、collect_items/walk_dir 三处同口径):`.MTDSS`/`.Mtdss-part` 等变体一律滤除/拒绝/跳过。
  - 其余签名零变化(`list_dir`/`stat_path`/`DirEntryData` 不动)。

- [ ] **Step 1: 写失败测试** — fs:`list_dir("C:foo")` → InvalidPath(Windows);tempdir 建 `.MTDSS` 目录与 `x.MTDSS-part`… 只断言 `.MTDSS` 变体被滤、`x.mtdss-part` 后缀名不误伤(精确匹配语义保留);manifest:`validate_rel_path(".MTDSS/a")` = false(Unix 也为 false——策略面统一);flow_unit:collect_items 对目录内 `.MTDSS` 条目跳过(构造目录树纯断言)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core --test fs --test manifest && cargo test -p mtdss-daemon --test flow_unit` Expected: FAIL
- [ ] **Step 3: 实现** — normalize 加第三字符判定;三处保留名比较换 `eq_ignore_ascii_case`;fs.rs 模块文档同步(删「债务归 M3c」注)。
- [ ] **Step 4: 跑测试 + 门禁** — 同 Step 2 命令 + `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` Expected: PASS(存量断言 `.mtdss` 精确行为的用例如有需同步)
- [ ] **Step 5: Commit** — `fix(core+daemon): reject drive-relative paths; reserved names now case-insensitive`

### Task 2: core fs 删除/移动/本地暂存复制

**Files:**
- Modify: `crates/core/src/fs.rs`(delete_paths/move_entries/copy_paths + FsError 增变体)
- Modify: `crates/core/Cargo.toml`(+`trash`)
- Test: `crates/core/tests/fs.rs`(增组)

**Interfaces:**
- Consumes: Task 1 的 normalize 口径。
- Produces(Task 4/7 依赖,签名 verbatim):

```rust
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FsError {
    // …既有四变体不动…
    #[error("回收站不可用: {0}")] RecycleUnavailable(String),   // code() = "RECYCLE_UNAVAILABLE"
}
/// 删除一批绝对路径(文件或目录整树)。force=false:先试回收(trash crate;
/// Windows=IFileOperation+ALLOWUNDO),不可用 → RecycleUnavailable;force=true:
/// 永久删除。逐条执行,返回 (成功数, 是否走了回收, 逐条失败表)。
pub fn delete_paths(paths: &[String], force: bool)
    -> Result<(usize, bool, Vec<PathOpFailure>), FsError>;
pub struct PathOpFailure { pub path: String, pub code: String, pub message: String }
/// 同设备移动/改名(rename;跨卷回退 暂存复制+删除,复制走 .mtdss 暂存)。
pub fn move_entries(pairs: &[(String, String)])
    -> Result<(usize, Vec<PathOpFailure>), FsError>;
/// 本机复制(整树,走目标目录 .mtdss 暂存:拷入 .part → 原子改名落位;
/// 目标同名由 unique_target 语义处理,策略判定在调用方)。返回落盘路径。
pub fn copy_paths(sources: &[String], target_dir: &str) -> Result<Vec<String>, FsError>;
```

语义钉死:跨卷 move 的回退复制对**调用方透明**(成功即移动完成,源删除为永久——数据已在目的地);`copy_paths` 复用 `transport::intake::unique_target` 的占位独占逻辑(该 fn 升 `pub(crate)` → core 内可见即可,或复制窄化版);暂存键 = `.mtdss/loc-<uuid>.part`,完成后清理;`.mtdss` 暂存目录确保存在沿 `intake::ensure_intake_dir`(升 `pub(crate)`)。

- [ ] **Step 1: 写失败测试** — tempdir:delete 永久(force=true,文件与目录树消失);delete 回收路径(trash 真调用,断言原路径消失且 Not NotFound 错误;CI 上 freedesktop trash 可用性如差则 `#[cfg]` 放行跳过并记 warn,Windows 必测);无回收模拟:略(真实不可回收难注入——`RecycleUnavailable` 分支经 `delete_paths_inner(trash_impl)` 注入面测,生产传 `trash::delete`);部分失败(一条不存在+一条正常 → failed 表含 NOT_FOUND、成功数=1);move 同卷改名/整树、跨卷回退(Windows C:→D: 或 Unix 跨 tmpfs 挂载不可行则注入面);copy_paths 单文件/嵌套树/暂存清理(目标 `.mtdss` 落空)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core --test fs` Expected: FAIL
- [ ] **Step 3: 实现** — 上述签名;trash 调用包在 `delete_paths_inner(trash: impl Fn(&Path) -> std::io::Result<()>)` 注入面;跨卷检测 rename 报 EXDEV。
- [ ] **Step 4: 跑测试 + 双平台** — 本机 + WSL `cargo test -p mtdss-core` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(core): fs delete (recycle-first) / move / staged local copy`

### Task 3: 传输引擎策略 + 接收进度 + 双侧暂停门

**Files:**
- Modify: `crates/core/src/transport/intake.rs`(`reconcile_with_policy`/`prepare_overwrite`)
- Modify: `crates/core/src/transport/transfer.rs`(RecvProgress/pause/RecvOpts 内聚)
- Test: `crates/core/tests/transfer_v2.rs`(增组)

**Interfaces:**
- Produces(Task 5/6/7 依赖,签名 verbatim):

```rust
// intake.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferPolicy { pub skip: Vec<String>, pub overwrite: Vec<String> }  // rel 或顶层前缀
/// reconcile 的策略版:先算既有计划,再叠加策略——
/// skip 命中(rel 精确或 entry rel 以 "p/" 前缀)→ Skip(无论记录状态);
/// overwrite 命中 → prepare_overwrite(移除目标处既有**文件**,及 rel 各级
/// 祖先上存在的文件型占位)后按 Full/Resume 计划(有匹配记录则续传);
/// 未命中条目维持既有语义(记录匹配 Skip / 无记录 Full / 自动改名安全网)。
pub fn reconcile_with_policy(out_dir: &Path, man: &Manifest, policy: &TransferPolicy) -> Vec<FilePlan>;

// transfer.rs
#[derive(Debug)]
pub enum RecvProgress<'a> { FileDone { rel: &'a str }, Bytes { rel: &'a str, done: u64, total: u64 } }
/// 共享暂停门:load=false 直通;=true 挂起直至翻转(Notify 唤醒,轮询 ≤200ms 兜底)。
pub struct PauseGate { /* AtomicBool + Notify */ }
impl PauseGate { pub fn new() -> Arc<Self>; pub fn set(&self, paused: bool); pub fn paused(&self) -> bool; pub async fn wait(&self); }
/// SendOptions 增:pub pause: Option<std::sync::Arc<PauseGate>>
///   —— send_rounds 每波前、push_file 每块 open_uni 前调 wait()。
/// RecvCtl(新):pub struct RecvCtl { pub policy: TransferPolicy, pub progress: Option<Box<dyn Fn(&RecvProgress) + Send + Sync>>, pub pause: Option<Arc<PauseGate>> }
///   —— recv 主循环 biased select 增「pause 门」臂(挂起即停 accept_uni/停读,
///      在途块任务自然收尾;背压由 QUIC 流控建立);progress 在 write_chunk 后
///      按 FileState.remaining(bytes)差值发 Bytes,finalize 后发 FileDone。
#[doc(hidden)]
pub async fn recv_files_ctl(conn: &Connection, out_dir: &Path, ctl: &RecvCtl) -> RecvOutcome;
```

`recv_files`/`recv_files_fault` 保留原签名(内部走空 ctl 转发,既有测试零改动);`paths` 穿出语义不变;remaining 初值 = 记录已收字节的补(Resume)或整文件大小(Full),Skip 不发进度。

- [ ] **Step 1: 写失败测试** — transfer_v2 增(沿既有真实 QUIC 对样板):`policy_skip_is_counted_done`(接收端 ctl.policy.skip=[a.txt] → 发送端收 FileDone、bytes_sent 不含该文件、目标盘无该文件);`policy_overwrite_replaces_existing`(目标预置同名旧内容文件 → 传输后内容=新、**无** ` (1)` 改名);`policy_prefix_requires_separator`(skip=["ab"] 不命中 "abc" 与 "ab-c",命中 "ab/x");`policy_overwrite_type_mismatch`(目标预置**文件** "d"、清单 rel "d/e" → 传输后 d 为目录含 e);`pause_resume_mid_transfer`(4MiB×8 文件、max_concurrent=2 注入,发送 1s 后 pause → 断言 bytes 停走(采样两次进度差≈0)→ resume → 完成校验哈希);`recv_progress_counts`(ctl.progress 累计 Bytes.done 单调、终值=total、FileDone 每文件一次);`paused_receiver_backpressures`(收端 pause → 发端进度停走,resume 后完成)。intake 单测:`reconcile_with_policy` 表驱动(记录三态 × 策略命中矩阵,不拉 QUIC)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core --test transfer_v2` Expected: FAIL
- [ ] **Step 3: 实现** — intake 策略函数 + prepare_overwrite;transfer PauseGate/RecvCtl/进度/暂停臂;模块文档补协议注(策略不经 wire 签名域,由信令层携带——见 Task 5)。
- [ ] **Step 4: 跑测试 + 门禁** — `cargo test -p mtdss-core && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` Expected: PASS(transfer_v2 既有用例零回归)
- [ ] **Step 5: Commit** — `feat(core): transfer policy (skip/overwrite), recv progress, pause gates`

### Task 4: ctrl proto v2 + daemon delete/move + spawn_blocking + 登出拆卸

**Files:**
- Modify: `proto/proto/ctrl.proto`(v2 增量)、`proto/src/lib.rs`(`CTRL_VERSION = 2`)
- Modify: `crates/core/src/signaling/envelope.rs`(无改动——本任务不动信令体)
- Modify: `crates/daemon/src/ctrl.rs`(dispatch 增 Delete/Move;handler 全量 spawn_blocking;`CtrlPool::reset()`)
- Modify: `crates/daemon/src/api.rs`(`delete`/`move`/`copy` RPC:本机短路 + CtrlPool 透传)
- Modify: `crates/daemon/src/state.rs`(logout 成功路径调 `ctrl.reset()`)
- Test: `crates/daemon/tests/ctrl_e2e.rs`(增例 + stderr 双抽干修)

**Interfaces:**
- Produces(proto 增量 verbatim;Task 7/8 依赖):

```protobuf
// ctrl.proto 追加
message PathOpFailure { string path = 1; string code = 2; string message = 3; }
message DeleteRequest { repeated string paths = 1; bool force_permanent = 2; }
message DeleteResponse { uint32 deleted = 1; bool recycled = 2; repeated PathOpFailure failed = 3; }
message MovePair { string from = 1; string to = 2; }
message MoveRequest  { repeated MovePair pairs = 1; }
message MoveResponse { uint32 moved = 1; repeated PathOpFailure failed = 2; }
message CopyRequest  { repeated string paths = 1; string dir = 2; }   // 同设备远端复制(无网络传输)
message CopyResponse { uint32 copied = 1; repeated PathOpFailure failed = 2; }
message CtrlRequest  { uint32 version = 1; oneof body { …既有 list=2/stat=3… DeleteRequest delete = 4; MoveRequest move = 5; CopyRequest copy = 8; } }
message CtrlResponse { uint32 version = 1; bool ok = 2; oneof body { …既有… DeleteResponse delete = 6; MoveResponse move = 7; CopyResponse copy = 9; CtrlError error = 5; } }
```

```rust
// ctrl.rs
impl CtrlPool {
    pub(crate) async fn delete(&self, state:&Arc<DaemonState>, device:&str, paths:&[String], force: bool)
        -> Result<mtdss_proto::ctrl::DeleteResponse, CtrlErr>;   // 本机短路同 list 口径
    pub(crate) async fn move_(&self, state:&Arc<DaemonState>, device:&str, pairs:&[(String,String)])
        -> Result<mtdss_proto::ctrl::MoveResponse, CtrlErr>;
    pub(crate) async fn copy(&self, state:&Arc<DaemonState>, device:&str, paths:&[String], dir:&str)
        -> Result<mtdss_proto::ctrl::CopyResponse, CtrlErr>;     // 同上;对端走 fs::copy_paths
    pub(crate) fn reset(&self);   // 清空 conns 池(登出拆卸;锁内即返)
}
/// dispatch 侧 fs 调用一律 tokio::task::spawn_blocking(清偿 M3b 携带);
/// 同步 handler 签名不变(spawn_blocking 在 serve_conn handler 包装层做,
/// handle_ctrl_request 改 async 或包一层——取 serve_conn 泛型 handler 支持 async 的最小改法)。
```

`RECYCLE_UNAVAILABLE` 沿 CtrlError 透传;RPC 面 `delete`/`move`/`copy` 参数校验沿 `need_str`/数组助手;`move`/`copy` 目标含 `to_device ≠ device` → 直接 code 1(「跨设备请用复制/传输」)。ctrl_e2e 存量子进程样板的 stderr 收尾 helper 修双抽干:正常退出与 kill 两条路径都**先并发 drain stderr 再 wait**(管道满缓冲会卡 wait)。

- [ ] **Step 1: 写失败测试** — ctrl_e2e 增:`delete_remote_roundtrip`(A 经 RPC delete B 的 tempdir 文件/目录 → B 盘上消失,recycled=true;部分不存在 → failed 表);`delete_force_permanent`;`move_remote_same_volume`(改名+整树);`copy_remote_same_device`(B 内 tempdir A → tempdir B:内容一致、源仍在——**不经网络传输面**,同 daemon 内);`move_cross_device_rejected`;`ctrl_version_mismatch_still_reports`(手造 version=1 请求 → UNSUPPORTED_VERSION);`logout_evicts_ctrl_pool`(A 建立一条 ctrl 连接后 logout → 池空[经内部状态或下次请求重新打洞断言] — 经 `delete` 后 logout 后再 login 再 delete 仍工作回归)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test ctrl_e2e` Expected: FAIL
- [ ] **Step 3: 实现** — proto/lib/envelope 无 envelope 改动;serve_conn 泛型改 `F: Fn(CtrlRequest) -> Fut`(async handler)或 handler 内 spawn_blocking 收口;api.rs 两方法;state.logout 接 reset。
- [ ] **Step 4: 跑测试 + WSL 核** — 本机 + WSL `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(proto+daemon): ctrl v2 delete/move, blocking fs dispatch, logout pool teardown`

### Task 5: 定向接收(target_dir/job_id/policy 信令扩展)+ 接收完成通知

**Files:**
- Modify: `proto/proto/e2e.proto`(ConnectRequest 加字段)、`proto/build.rs` 无改动
- Modify: `crates/core/src/signaling/envelope.rs`(`SignalBody::ConnectRequest { session_id, target_dir: String, job_id: String, policy: Option<TransferPolicySig> }`——空/None = legacy)
- Modify: `crates/daemon/src/flow.rs`(run_send_with 增 `SendSpec { target_dir, job_id, policy }` 参数;send_round 封 ConnectRequest;recv 侧 RecvRound 增 spec 与 RecvCtl 装配)
- Modify: `crates/daemon/src/notify.rs`(新:notify_one(title, body) → notify-rust,失败 warn;headless 无总线即静默)
- Modify: `crates/daemon/src/api.rs`(`send` RPC 增可选 `dir` 参数——遗留 CLI send 定向化)、`crates/daemon/src/lib.rs`(mod notify)
- Modify: `crates/daemon/Cargo.toml`(+`notify-rust`)
- Test: `crates/core/tests/envelope.rs`(新变体字段往返)、`crates/daemon/tests/daemon_e2e.rs`(定向落盘/回退/通知事件)

**Interfaces:**
- Produces(Task 6/7/8 依赖):

```protobuf
// e2e.proto(密封信封内,服务器不透明)
message ConnectRequest { bytes session_id = 1; string target_dir = 2; string job_id = 3; TransferPolicy policy = 4; }
message TransferPolicy { repeated string skip = 1; repeated string overwrite = 2; }
```

```rust
// flow.rs
pub(crate) struct SendSpec { pub target_dir: String /*空=默认*/, pub job_id: String /*空=legacy*/, pub policy: mtdss_core::transport::intake::TransferPolicy }
// run_send_with(... , spec: &SendSpec, ...) —— 老调用以空 spec 转发;
// resident_recv_loop:ConnectRequest 带非空 target_dir 且合法(Task 1 normalize 口径)→ 该轮 out_dir=它,
//   非法 → warn + 回退 state.recv_dir();spec.policy 装配进 RecvCtl(本任务 pause/progress 留空——
//   进度路由与取消绑定在 Task 6/7 的 jobs 接线时补);轮完成(干净或有落盘)→
//   notify::notify_one("MTDSS", "«对端名» 向 {dir} 写入 N 个文件") + event.notify。
// notify.rs
pub(crate) fn notify_one(title: &str, body: &str);
```

- [ ] **Step 1: 写失败测试** — envelope:ConnectRequest 新字段 seal→open 往返(含 policy 嵌套、legacy 全空);daemon_e2e 增:`send_with_target_dir_lands_there`(A daemon send RPC 带 dir=B tempdir → B 该目录见文件、默认收件目录无);`send_target_invalid_falls_back`(dir="C:foo" → 回退默认目录 + B 日志含 warn——日志断言沿 daemon_e2e 日志 grep 样板);`legacy_semantics_unchanged`(不带 dir 的 send → 默认收件目录,既有用例天然覆盖,补一条显式);`receive_completion_emits_notify_event`(B 侧 IPC 事件流收到 event.notify,body 含目录与数量)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core --test envelope && cargo test -p mtdss-daemon --test daemon_e2e` Expected: FAIL
- [ ] **Step 3: 实现** — proto/envelope/flow/notify/api 按 Interfaces;send RPC 兼容(缺 dir = 旧行为)。
- [ ] **Step 4: 跑测试 + 门禁** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`(WSL 核 daemon/core)Expected: PASS
- [ ] **Step 5: Commit** — `feat(daemon): targeted transfers via ConnectRequest (dir/job/policy) + receive notifications`

### Task 6: 传输作业注册表 + upload 变体 + transfer.* RPC + 事件 + 暂停/取消

**Files:**
- Create: `crates/daemon/src/jobs.rs`(注册表/驱动/速度 EMA/事件节流)
- Create: `crates/daemon/src/ops.rs`(冲突预扫纯函数 + 本地源展开)
- Modify: `crates/daemon/src/api.rs`(transfer.start/list/pause/resume/cancel/wait)、`crates/daemon/src/lib.rs`、`crates/daemon/src/flow.rs`(run_send_with 增 ctl:pause 门 + cancel watch)
- Test: `crates/daemon/tests/jobs_e2e.rs`(新)、`crates/daemon/tests/ops_unit.rs`(新,或并入 flow_unit)

**Interfaces:**
- Consumes: Task 3 PauseGate/RecvCtl、Task 5 SendSpec。
- Produces(Task 7/8/11/12 依赖,签名 verbatim):

```rust
// jobs.rs
pub(crate) struct Jobs { /* next_id: AtomicU64, map: Mutex<HashMap<String, JobHandle>> */ }
impl Jobs {
    pub(crate) fn new() -> Self;
    /// 起一个 upload 作业:spawn 驱动任务跑 run_send_with(带 SendSpec{dir,job_id,policy} + ctl),
    /// 进度回调 → 快照/事件(节流 400ms + 状态翻转即发);返回 job_id。
    pub(crate) fn start_upload(&self, state: &Arc<DaemonState>, files: Vec<PathBuf>, to: &str,
        dir: &str, policy: TransferPolicy, cut_src: bool) -> Result<String, String>;
    pub(crate) fn snapshot(&self) -> Vec<JobSnap>;          // transfer.list 的数据源,按 id 升序
    pub(crate) fn set_paused(&self, id: &str, paused: bool) -> bool;
    pub(crate) fn cancel(&self, id: &str) -> bool;          // 置 cancel watch + abort 驱动任务
    pub(crate) async fn wait(&self, id: &str) -> JobSnap;   // 终态长轮询(notify 唤醒)
    /// resident 接收轮与作业的双向句柄(进度入注册表 + 作业侧取消/暂停直达该轮)。
    /// Task 6 只定义类型与注册表槽位;resident loop 的实际接线在 Task 7(download 需要它)。
    pub(crate) fn bind_incoming(&self, job_id: &str, round: RoundHandle) -> bool;
}
pub(crate) struct RoundHandle { pub cancel: tokio::sync::watch::Sender<bool>,
    pub pause: std::sync::Arc<mtdss_core::transport::transfer::PauseGate>,
    pub progress: std::sync::Arc<dyn Fn(&mtdss_core::transport::transfer::RecvProgress<'_>) + Send + Sync> }
pub(crate) struct JobSnap { pub id: String, pub kind: &'static str, pub state: String,
    pub files_done: u64, pub files_total: u64, pub bytes_done: u64, pub bytes_total: u64,
    pub speed_bps: u64, pub src: String, pub dst: String, pub error: Option<String> }
// 速度 = 采样 EMA:pub(crate) fn ema_speed(samples: &[(ms, bytes)], window_ms: u64) -> u64  // 纯函数
// ops.rs
/// 目标侧条目(顶层粒度或本地全树 rel)与目标目录列表比对:
/// 同名双文件 / 同名类型不匹配 → 冲突;目录对目录 → 无(合并)。
pub fn scan_conflicts(sources: &[(String /*rel*/, bool /*is_dir*/, u64 /*size*/)],
                      dst_entries: &[(String /*name*/, bool, u64)]) -> Vec<Conflict>;
pub struct Conflict { pub rel: String, pub src_is_dir: bool, pub src_size: u64,
                      pub dst_size: u64, pub dst_mtime_ms: u64 }
/// 本地源展开为全树 rel(供 upload 全粒度预扫):沿 collect_items 口径但只读元数据不报错跳过。
pub fn local_tree_rels(paths: &[PathBuf]) -> Vec<(String, bool, u64)>;
```

取消语义:cancel = 置 watch + **abort 驱动任务**(连 run_send_with 重试循环一并杀,不复活);cut=true 的 upload 在驱动成功后经 Task 2 本地 `delete_paths(force=false)` 删源(尽力回收);`transfer.start` 校验:files 非空、dir 合法(Task 1 口径)、policy 上限(全局约束)。

- [ ] **Step 1: 写失败测试** — ops_unit:`scan_conflicts_file_vs_file/type_mismatch/dir_merge_none/empty_dst`;`local_tree_rels_nested_and_sorted`;`ema_speed_window`。jobs_e2e(进程内双 Daemon + IpcClient):`upload_job_completes`(A 起 upload 至 B 定向目录 → wait 终态 done、B 盘校验内容);`upload_conflict_policy_applied`(B 预置同名旧文件,A 覆盖策略 → 内容翻新无 (1));`pause_resume_job`(大文件 pause → 进度停走 → resume → done);`cancel_aborts_retry_loop`(起作业即 cancel + 杀 B daemon → 驱动任务秒退、state=cancelled、不再有重试[事件流断言无后续]);`cut_upload_deletes_sources`(cut=true → A 源进回收、目标在);`transfer_list_and_events`(list 快照字段齐;事件含状态翻转与节流进度)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test jobs_e2e --test ops_unit` Expected: FAIL
- [ ] **Step 3: 实现** — jobs.rs/ops.rs/api 接线/flow ctl 扩展(流内 select cancel watch;abort 由注册表持 JoinHandle)。
- [ ] **Step 4: 跑测试 + daemon 全量回归** — `cargo test -p mtdss-daemon` Expected: PASS(send RPC 既有用例不回归)
- [ ] **Step 5: 门禁 + Commit** — `feat(daemon): transfer jobs — upload variant, pause/cancel/wait, events, conflict pre-scan`

### Task 7: download / local_copy 变体 + 剪切收尾 + CLI put/get + devices 现拉

**Files:**
- Modify: `proto/proto/ctrl.proto`(orchestrate/cancel/job_query 三消息并入 Task 4 之后的第二拨——本任务加)、`proto/src/lib.rs`
- Modify: `crates/daemon/src/ctrl.rs`(`CtrlPool::orchestrate/cancel_job/job_query` + responder 三分支)
- Modify: `crates/daemon/src/jobs.rs`(download/local_copy 驱动 + bind_incoming + incoming 匿名卡)、`crates/daemon/src/api.rs`(download/local_copy kind + `devices` 现拉)
- Modify: `crates/cli/src/main.rs`(Put/Get 子命令 + 冲突交互段)、`crates/cli/src/client.rs`(put_cmd/get_cmd/wait_job/冲突问询数据)
- Test: `crates/daemon/tests/jobs_e2e.rs`(增例)、`crates/cli/tests/ipc_cli.rs`(纯函数)

**Interfaces:**
- Produces(proto 增量 verbatim;Task 8/11/12 依赖):

```protobuf
message OrchestrateRequest  { string job_id = 1; string to_device = 2; repeated string paths = 3;
                              string target_dir = 4; TransferPolicy policy = 5; }  // ctrl.proto 自有同名 TransferPolicy{skip,overwrite}
message OrchestrateResponse { bool accepted = 1; string detail = 2; }
message CancelJobRequest  { string job_id = 1; }
message CancelJobResponse { bool cancelled = 1; }
message JobQueryRequest   { string job_id = 1; }
message JobQueryResponse  { string state = 1; uint64 files_done = 2; uint64 files_total = 3;
                            uint64 bytes_done = 4; uint64 bytes_total = 5; string error = 6; }
// CtrlRequest oneof += orchestrate=10 cancel_job=11 job_query=12;
// CtrlResponse oneof += orchestrate=10 cancel_job=11 job_query=12(6/7/8/9 已被 Task 4 的 delete/move/error/copy 占用)
```

行为钉死:download = 操作端注册 incoming job(含 RecvCtl 装配:pause 门绑定 job)→ ctrl `orchestrate` 发给源设备(to_device=自己)→ 对端起 upload 驱动(同 job_id)→ ConnectRequest 带 job_id 回来 → resident loop `bind_incoming` 路由进度/取消;cut 收尾 = 成功后对源端 `delete`(Task 4 面)。local_copy = spawn_blocking 跑 `fs::copy_paths`/`move_entries`(cut=move 快路)。`devices` RPC:现拉 REST(5s 超时)成功 → 更新缓存返回 stale=false;失败 → 返回缓存 stale=true。CLI:`mtdss put <本地…> <设备>:<远端目录> [--conflict ask|skip|overwrite]`(ask = 逐条列出冲突问 全部跳过/全部覆盖/取消);`mtdss get <设备>:<远端路径…> --to <本机目录> [--conflict …]`;两命令 `transfer.wait` 阻塞至终态,退出码按 state。

- [ ] **Step 1: 写失败测试** — jobs_e2e 增:`download_job_roundtrip`(A 的 RPC download{device:B, paths, dir:A tempdir} → A 侧 wait done、文件内容校验、B 侧源仍在);`download_progress_routes_to_job`(download 作业进度事件来自收端 RecvProgress[断言 bytes_done 单调]);`download_pause_pauses_sender`(A pause → B 发端进度停[经 job_query 断言]→ resume → 完成);`local_copy_job`(local_copy 落盘 + 暂存清理;cut=true → 源移走);`devices_rpc_now_pulls`(现拉命中:改服务器侧名字?回环服务器难改 → 断言 stale=false 且与缓存路径不同的调用计数——经注入 fake fetch?采用最小:daemon 内 devices 现拉失败场景杀服务器 → stale=true 回缓存)。ipc_cli:`parse_scp_target`(`dev_b:D:\x` → (dev, `D:\x`);裸路径/多冒号报错)、`conflict_prompt_data`(问询文案纯函数)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test jobs_e2e && cargo test -p mtdss-cli` Expected: FAIL
- [ ] **Step 3: 实现** — proto 三消息 + ctrl 双端 + jobs 两驱动 + api 两 kind + devices 现拉 + CLI 两命令(main.rs 交互段留 main,lib 出数据)。
- [ ] **Step 4: 跑测试 + 门禁** — `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` Expected: PASS
- [ ] **Step 5: Commit** — `feat(daemon+cli): download/local-copy jobs, cut finalization, put/get, fresh devices RPC`

### Task 8: A→B 编排 + 取消传播 + CLI cp

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(remote_copy 驱动:orchestrate 到 from 设备 + 轮询 job_query)、`crates/daemon/src/api.rs`(remote_copy kind)
- Modify: `crates/cli/src/main.rs`(Cp 子命令)、`crates/cli/src/client.rs`(cp_cmd)
- Test: `crates/daemon/tests/jobs_e2e.rs`(三 daemon 增例)

**Interfaces:**
- Produces:`remote_copy` = 预扫(to 设备目标目录经 ctrl list[截断则按顶层可见集+警示] × from 顶层条目)→ policy → orchestrate 发给 **from 设备**(其起 upload 驱动、ConnectRequest 定向到 to)→ 本端 job 轮询 from 的 job_query(1.5s 间隔,卡片可见期间)至终态;cut 收尾 = 成功后对 from 端 delete。cancel = ctrl cancel_job 发 from + 本端卡片终态。
- CLI:`mtdss cp <源设备>:<路径…> --to <目标设备>:<目录> [--conflict …]`(`--to` 本机目录形态 = download 糖、源本机 = upload 糖、源=目标同设备 = 走 `copy` RPC 糖——cli 层分发到四态)。

- [ ] **Step 1: 写失败测试** — jobs_e2e 增(三 Daemon X/A/B 同账号):`a2b_transfer_lands_on_b`(X 起 remote_copy A→B 定向目录 → wait done → B 盘校验、A 源仍在、cut=false;X 自身 home 目录无数据落地[目录计数断言]);`a2b_cut_deletes_a_sources`;`download_cancel_propagates`(X cancel 一进行中的 download → 源端 job 短窗内转 cancelled[经 job_query 断言],B 侧无后续重试事件);`a2b_conflict_policy_flows`(B 预置同名 → X 覆盖策略 → B 内容翻新)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test jobs_e2e a2b` Expected: FAIL
- [ ] **Step 3: 实现** — jobs 驱动 + api + cli。
- [ ] **Step 4: 跑测试 + 门禁** — `cargo test --workspace && cargo clippy …` Expected: PASS
- [ ] **Step 5: Commit** — `feat(daemon+cli): A→B orchestrated transfers with cancel propagation + cp`

### Task 9: CLI rm/mv + 永久删除确认流

**Files:**
- Modify: `crates/cli/src/main.rs`(Rm/Mv 子命令)、`crates/cli/src/client.rs`(rm_cmd/mv_cmd/need_force_confirm 纯函数)
- Test: `crates/cli/tests/ipc_cli.rs`(纯函数增例)

**Interfaces:**
- Produces:`mtdss rm <设备>:<路径…> [--permanent]` / `mtdss mv <设备>:<源…> --to <设备>:<目录>`(同设备);`--permanent` 或收到「回收站不可用」错误后确认(y/N)再 force 重试;`need_force_confirm(resp) -> bool`(message 含标记串判定,纯函数);mv 跨设备 → 直接指引 cp。

- [ ] **Step 1: 写失败测试** — ipc_cli:`need_force_confirm_matches_marker`(含/不含「回收站不可用」)、`rm_mv_arg_shapes`(clap 形状冒烟或参数解析纯函数)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-cli` Expected: FAIL
- [ ] **Step 3: 实现** — 两命令 + 确认流;ctrl_e2e 已覆盖 RPC 面,此处 CLI 薄壳 + 手动冒烟留 Task 14。
- [ ] **Step 4: 门禁 + Commit** — `feat(cli): rm/mv with permanent-delete confirmation ladder`

### Task 10: 拖出落点解析 spike(dropresolve 薄模块)

**Files:**
- Create: `ui/src-tauri/src/dropresolve.rs`(Windows 专属;`#[cfg(windows)]` 全模块,他平台空实现)
- Modify: `ui/src-tauri/Cargo.toml`(+`windows-sys` features:Win32_UI_Accessibility/Win32_UI_WindowsAndMessaging/Win32_Foundation/Win32_System_Com)、`ui/src-tauri/src/lib.rs`(mod + 命令注册)
- Test: `ui/src-tauri/src/dropresolve.rs` 内嵌单测(纯函数)+ `#[ignore]` 手动探针测试

**Interfaces:**
- Produces(Task 12 集成依赖):

```rust
pub enum DropTarget { Desktop(PathBuf), ExplorerDir(PathBuf), Unresolvable(String) }
/// 鼠标落点 → 目标:WindowFromPoint 取 hwnd → 类名 "Progman"/"WorkerW" = 桌面
/// (SHGetKnownFolderPath Desktop);"CabinetWClass" = UIA 走地址栏值;其余一律
/// Unresolvable。UIA 任何失败 → Unresolvable(不 panic)。
pub fn resolve_drop_at_cursor() -> DropTarget;
/// 地址栏文本 → 文件系统路径(纯函数,单测面):盘符绝对路径 Ok;
/// "::{GUID}"/"shell:"/空/"此电脑"/"回收站"/UNC → None。尾缀分隔符规整。
pub fn parse_address_bar(text: &str) -> Option<PathBuf>;
#[tauri::command] pub async fn drop_resolve_at_cursor() -> Result<serde_json::Value, String>;
// {"kind":"desktop"|"dir"|"unresolvable","path":String?,"reason":String?}
```

Spike 判据(写入冒烟手册):真实资源管理器普通窗口 10 次拖放 ≥9 次解析正确即留 V1;不稳 → 模块保留但 Task 12 拖出仅桌面路径,窗口路径全走降级提示(降级开关 = dropresolve 内部 UIA 探测失败自然降级,无需配置面)。

- [ ] **Step 1: 写失败测试** — 内嵌单测:`parse_address_bar_rejects_non_fs`(六类输入表驱动)、`parse_address_bar_accepts_plain_paths`(大小写盘符/正反斜杠/中文/空格)。`#[ignore]` 探针 `manual_probe_prints_targets`(显式 `cargo test -p ui --lib manual_probe -- --ignored` 跑真实窗口,输出人核)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` 不适用(本任务 Rust 侧):`cargo test --manifest-path ui/src-tauri/Cargo.toml dropresolve` Expected: FAIL(纯函数)
- [ ] **Step 3: 实现** — Win32 + UIA FFI(unsafe 面收口在本模块;UIA 初始化 CoInitializeEx 按线程)。
- [ ] **Step 4: 构建 + 手动探针** — `cargo check --manifest-path ui/src-tauri/Cargo.toml` + `#[ignore]` 探针对真实桌面/窗口/库视图/此电脑各跑一轮,结果记入本任务提交信息与冒烟手册草稿;**结论(留/降)回填本计划同 Task 12 步骤**。
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): drop-point resolution module (UIA) with manual probe`

### Task 11: UI 操作面 I——右键菜单/框选/工具栏/删除 + code3 文案

**Files:**
- Create: `ui/src/components/ContextMenu.tsx`
- Modify: `ui/src/lib/select.ts`(框选纯函数)、`ui/src/components/{Toolbar,FileDetails,FileList,FileTiles}.tsx`、`ui/src/App.tsx`、`ui/src/App.css`、`ui/src/i18n/zh.json`
- Modify: `ui/src/lib/daemon.ts`(`daemonDelete`/RpcError code3 映射 `friendlyFsError`)、`ui/src/lib/nav.ts`(分隔符告诫注释)
- Test: `ui/src/lib/select.test.ts`(增例)、`ui/src/lib/daemon.test.ts`(新增,若纯函数抽出)

**Interfaces:**
- Consumes: Task 4 delete RPC。
- Produces(Task 12/13 依赖):右键菜单(空白处:粘贴/刷新;条目上:打开[仅目录]/剪切 Ctrl+X/复制 Ctrl+C/删除 Del——粘贴项本任务渲染置灰,Task 12 接线);框选:三视图空白 mousedown 拖出选框 → `rubberBandSelect(anchorXY, curXY, rowRects, prev) -> Set<path>`(纯函数,与 Ctrl/Shift 锚点互斥:框选启动清锚点);工具栏 剪切/复制/粘贴/删除 按选中态启用(剪切/复制/删除需非空选择;粘贴恒可点,Task 12 前置灰);Del 键触发删除;删除 = 直接 RPC(静默,spec §4.3),RECYCLE 标记串错误 → 弹确认(force 重试),code3 → 「设备离线」文案;`friendlyFsError(code, message) -> string`(纯函数:3 → i18n key;含标记串 → 确认升级信号)。
- [ ] **Step 1: 写失败测试** — select:`rubber_band_intersects_rows`/`rubber_band_empty_clears`/`rubber_band_does_not_disturb_anchor`(锚点被清);daemon:`friendly_fs_error_maps_code3_and_recycle_marker`。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL
- [ ] **Step 3: 实现** — 组件/纯函数/样式/文案;`npm run build` 过 tsc。
- [ ] **Step 4: 构建 + 单测回归 + 手动冒烟** — vitest + build;`npm run tauri dev` 双 daemon:右键菜单四项、框选、Del 删除远端文件、离线删除报友好文案。
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): context menu, rubber-band select, toolbar ops, delete with friendly errors`

### Task 12: UI 操作面 II——剪贴板/粘贴/拖入/冲突对话框/下载到本机/拖出集成

**Files:**
- Create: `ui/src/lib/clipboard.ts`(内部剪贴板状态机 + 种类判定)、`ui/src/components/ConflictDialog.tsx`
- Modify: `ui/src-tauri/src/ipc_bridge.rs`(`clipboard_files` 命令[CF_HDROP 直读,Windows] + `pick_folder`[tauri-plugin-dialog 包装] + 注册 `drop_resolve_at_cursor`)、`ui/src-tauri/Cargo.toml`(+`tauri-plugin-dialog`)
- Modify: `ui/src/lib/daemon.ts`(transfer.start 封装)、`ui/src/App.tsx`、`ui/src/components/*.tsx`、`ui/src/App.css`、`ui/src/i18n/zh.json`
- Test: `ui/src/lib/clipboard.test.ts`(新)、`ui/src/lib/conflict.test.ts`(新,纯函数)

**Interfaces:**
- Produces:粘贴分派(纯函数 `pastePlan(clip, targetDevice) -> {kind:"upload"|"download"|"remote_copy"|"device_copy"|"local_copy"|"none", …}`):内部剪贴板 {device, paths, cut} → 跨设备路由 remote_copy / 同**远端**设备 device_copy(ctrl copy/move,无网络传输)/ 目标本机 = local_copy(源本机)或 download(源远端);空内部剪贴板 → 桥 `clipboard_files()` 读 CF_HDROP → upload;冲突对话框(4 键:跳过/覆盖/批量时加全部跳过/全部覆盖;`groupConflicts(conflicts) -> {single:boolean, rows:[…]}` 纯函数);「下载到本机…」= 右键/菜单项 → pick_folder → download job;拖入 = Tauri `onDragDropEvent` paths → upload(冲突走同对话框);**窗口内拖拽**(spec §4.2):视图行 draggable → 拖到树节点/当前目录/面包屑段(HTML5 DnD,webview 内原生可用)→ 放下分派与 pastePlan 同一纯函数(跨设备拖=复制、同设备拖=移动、任何设备→本机目录=落盘,修饰键不改义);拖出 = 行 mousedown 拖出窗口(mouseup 于窗外)→ `drop_resolve_at_cursor` → Desktop/dir → download job 落该目录,unresolvable → 提示条「请拖到桌面或使用下载到本机…」(按 Task 10 结论,窗口路径不稳则仅桌面生效)。**[Task 10 spike 结论已回填:留 V1——普通资源管理器窗口落点保留;真实桌面探针 9 类目标 × 10 次 = 90/90 正确(含驱动器根显示名回退),主路 UIA 全路径 ≈70ms;库/此电脑/回收站/第三方窗按设计 Unresolvable。]**
- [ ] **Step 1: 写失败测试** — clipboard:`paste_plan_routes_local_to_remote/remote_to_local/remote_to_remote/same_remote_device_copy/self_copy`、`drag_plan_matches_paste_semantics`(同设备拖=移动标志)、`cut_vs_copy_flag`;conflict:`group_conflicts_single_vs_batch`、`apply_all_decisions`(全部键只作用剩余未决项)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL
- [ ] **Step 3: 实现** — 桥两命令 + 前端全接线(粘贴/窗口内拖/拖入共用分派)+ 对话框 + 拖出(含 Task 10 结论落地)。
- [ ] **Step 4: 构建 + 单测 + 手动冒烟** — vitest + build;双 daemon:Explorer 复制 → MTDSS 粘贴上传;MTDSS 复制远端 → 粘贴本机/另一远端/同设备目录;冲突对话框四键;下载到本机选择器直落;窗口内拖 A 目录条目到 B 设备树节点(数据不经本机);拖出到桌面落盘;拖出到窗口(按 spike 结论)。
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): paste/drag-in/drag-out, conflict dialog, download-to-local`

### Task 13: 传输面板 + 状态栏速率/加载态 + 托盘计数 + 通知接线 + UI 打磨四小件

**Files:**
- Create: `ui/src/components/TransferCards.tsx`
- Modify: `ui/src-tauri/src/ipc_bridge.rs`(转发 `event.transfer`/`event.notify` → `daemon://transfer`/`daemon://notify`)、`ui/src/lib/daemon.ts`(listenTransfer/applyTransferEvents 纯 reducer)、`ui/src/components/StatusBar.tsx`、`crates/daemon/src/tray.rs`(tooltip 进行中计数)、`crates/daemon/src/jobs.rs`(计数 watch)、`ui/src/App.tsx`、`ui/src/App.css`、`ui/src/i18n/zh.json`
- Test: `ui/src/lib/transfer.test.ts`(新,reducer/格式化纯函数)、`crates/daemon/tests/tray_logic.rs`(如需计数纯函数注入)

**Interfaces:**
- Produces:`applyTransferEvents(prev, evt) -> Map<id,JobSnapUI>`(乱序/重复/终态后迟到进度全部收敛,reducer 纯函数);`fmtSpeed(bps) -> string`(纯函数,`1.2 MB/s` 形态);`aggRate(jobs) -> {down_bps, active}`(状态栏汇总);传输卡片(每作业一张:图标/方向/src→dst 摘要/聚合进度条/速度/暂停⇄继续/取消;终态 5s 自隐,失败留卡片带错误);状态栏增「↓ 32 MB/s(N 个传输中)」与目录加载态「加载中…」;托盘 tooltip = `MTDSS — N 个传输中`(jobs 计数 watch 驱动,沿 tray 既有 watch 样板);UI 打磨:树节点加载 spinner + 窗口 focus 重拉 devices(快照陈旧配套)、树节点文本超界省略、三视图行距/图标样式、Shift 范围蓝底与选中底色区分。
- [ ] **Step 1: 写失败测试** — transfer.test:`apply_events_dedupes_and_orders`、`terminal_state_wins_over_late_progress`、`fmt_speed_units`、`agg_rate_sums_active`。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL
- [ ] **Step 3: 实现** — 桥转发 + reducer + 卡片/状态栏/托盘/打磨。
- [ ] **Step 4: 构建 + 单测 + 手动冒烟** — vitest + build;双 daemon:起两个传输看卡片聚合、暂停/取消、托盘计数、通知出现(写入方+被写方)、状态栏速率、离线 focus 重拉。
- [ ] **Step 5: 门禁 + Commit** — `feat(ui+daemon): transfer cards, status-bar rates, tray count, notifications wiring, UI polish`

### Task 14: 文档 + 冒烟手册 + EOF 竞速重估 + 终门禁

**Files:**
- Create: `docs/smoke-m3c.md`(手动冒烟手册:CLI put/get/cp/rm/mv 全命令、定向/冲突/剪切、下载选择器、拖出验证矩阵[桌面/普通窗口/库/此电脑,按 Task 10 结论标注生效面]、传输面板/暂停取消、通知双侧、托盘计数、SDDL 复查指引回链)
- Modify: `docs/release-checklist.md`(「M3c 手动冒烟」段 + deferred 更新)、`docs/superpowers/plans/2026-10-19-mtdss-m3c-transfer-ops.md`(Task 10 spike 结论回填如未及时)

**Deferred(本计划不做,记录依据):** CSP 硬化(M4 打包前);设备重命名(需服务器端点);接收端并发多传输(同向串行,~50s Accept 窗口限制已记);远端源嵌套冲突全粒度预扫(自动改名安全网兜底);跨会话断点续传(V2);`mtdss send` 遗留 RPC 保留未迁移 jobs(e2e/nat-matrix 兼容面)。

- [ ] **Step 1: EOF 2s 竞速重估** — 通读 transfer.rs 终局/拒绝路径在定向+策略+暂停新流下的 EOF 窗口(结论一句话记入 checklist:维持 2s 宽限是否仍充分;发现实质竞速才立项修)。
- [ ] **Step 2: 手动冒烟执行并记录**(沿 smoke-m3c.md;结果回填 checklist 勾选)。
- [ ] **Step 3: 终门禁 + 推送确认 CI 全绿** — 本机/WSL `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` + `cd ui && npx vitest run && npm run build`;Commit `docs: m3c smoke manual + checklist`,推 PR 看 CI(job 全绿后按流程收尾)。
