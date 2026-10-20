# MTDSS M4a(清债+硬化)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 清偿 M3c 终审裁定的全部债务并收口打包前硬化项:jobs 终态保留上限、毒锁恢复/驱动监督 join、ctrl copy/move 冲突策略(引擎+CLI+UI)、devices 单飞、RPC 错误码类型化、对话框焦点陷阱、spawn_blocking 清扫、put `--to`、CSP 硬化,并执行 SDDL 跨用户 §E 与托盘唤起 D2/D3 两项手工验证。

**Architecture:** 全部为既有面内改动、零新依赖:jobs 注册表加终态淘汰(插入时驱逐);锁获取统一走中毒恢复助手、驱动 future 包 catch_unwind 监督(panic → failed,不再悬死);fs copy/move 增策略参数、语义对齐传输面 TransferPolicy,经 ctrl 面(proto 缺省兼容)打通到 CLI 与 UI;devices RPC 加 1s 单飞记忆;`RpcError` 增结构化 `data.kind` 替代标记串匹配;UI 对话框焦点圈 = 纯函数 + 薄 hook(不引 testing-library);阻塞 fs 扫描统一 `spawn_blocking`(沿用 ctrl.rs 既有惯例);tauri CSP 收紧(devCsp 保 HMR)。

**Tech Stack:** 既有栈不动;`proto/ctrl.proto` Copy/Move 增字段(旧端解码缺省 = legacy 行为,双端互通不断);UI 不新增 npm 依赖。

**Spec:** `docs/superpowers/specs/2026-10-20-mtdss-m4-release-design.md`(§7.1 清债面、§7.5 前两项验证、§8 M4a 切片;D6–D15 裁定)

## Global Constraints

- 门禁 = `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 在 Windows 与 Linux(WSL/CI)双平台全绿 + nat-matrix CI job + ui job 全绿。**每个任务提交前门禁必须全绿。**
- UI 门禁:`cd ui && npx vitest run && npm run build`。
- 本机 bash 环境:`export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`(新终端 PATH 陈旧时先执行前半句自救)。
- cargo 走 rsproxy.cn;npm 慢时临时用 npmmirror,**不得把 .npmrc 提交进仓库**;git 走 SSH。
- 执行在 worktree(`EnterWorktree` 分支 `worktree-mtdss-m4a`,前缀由工具自带)。
- 任务顺序 = 终审债序,勿重排:jobs 上限 → 毒锁/join → ctrl 冲突(引擎+CLI → UI)→ devices 单飞 → 错误码 → 焦点陷阱 → spawn_blocking → put --to → CSP → 手工验证。

## Review Focus

1. **驱逐 vs wait 悬挂**:被上限驱逐的终态作业,`transfer.wait` 必须立即报错而非长轮询挂死 → Task 1 测试钉死。
2. **panic 后 cancel 仍可达**:驱动 panic 毒化 `JobCore.m` 后,`Jobs::cancel`/`transition` 必须仍能落 cancelled/failed(锁恢复 + 监督兜底)→ Task 2 测试。
3. **同设备 copy 目录撞目录**:策略未命中/目录目标不得误伤既有「目录恒合并、嵌套自动改名」安全网 → Task 3 fs 单测。
4. **devices 单飞慢拉取并发超时**:并发 RPC 遇 fetch 失败/超时必须全部走 stale 缓存返回,不死锁不串台 → Task 5 测试。
5. **dev 模式 HMR 不断**:CSP 收紧后 `tauri dev`(vite ws://localhost:1421)必须照常 → Task 10 devCsp + 手动验证步。

---

### Task 1: jobs 注册表终态保留上限

**Files:**
- Modify: `crates/daemon/src/jobs.rs`( Jobs 结构 :192、六个插入点 :645/:797/:916/:1001/:1047/:1063、snapshot :1086、wait :1163)
- Test: `crates/daemon/src/jobs.rs`(模块内单测区 :1795-2414)

**Interfaces:**
- Produces: `const TERMINAL_RETAIN_CAP: usize = 100;`;`fn retain_terminal_cap(map: &mut HashMap<String, JobHandle>)`(按 id seq 升序保留最新 100 条终态,活跃作业永不驱逐;复用 snapshot 的 seq 比较器);`Jobs::wait` 对不存在/已驱逐 id 立即返回 `Err`(报文「作业不存在: <id>」),不得长轮询。

- [ ] **Step 1: 写失败测试** — 模块内新增 `terminal_cap_evicts_oldest`:灌入 `TERMINAL_RETAIN_CAP + 2` 个终态作业(复用现有单测的 Jobs 构造法)→ `snapshot()` 条数 == CAP 且最旧两条不在;再加 `wait_on_evicted_errors_fast`:对已驱逐 id 调 `wait`,断言立即 Err(非挂起;用 `tokio::time::timeout(1s)` 包裹断言不超时路径之外先到 Err)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --lib jobs::tests::terminal_cap` Expected: FAIL(编译错或断言挂/计数不符)
- [ ] **Step 3: 实现** — 六个 `map.lock().unwrap().insert(...)` 临界区改为经私有 `Jobs::insert_job(&self, id, h)`(lock+insert+`retain_terminal_cap`);`wait` 入口先查 map,无此 id → `Err`。模块文档 :186-187 的「终态不摘除」不变量句改为「终态保留上限 100,驱逐只发生在插入新作业时」。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon`(全部,含 jobs_e2e 的 list/wait 面)Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — 双平台门禁;`fix(daemon): jobs terminal retention cap (evict oldest beyond 100; wait on evicted errors not hangs)`

### Task 2: 毒锁恢复 + 驱动监督 join

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(unwrap 面 :363/:388/:433/:484/:496/:501/:600/:645…/:1087/:1108/:1138/:1164/:1211/:1123/:1149/:1223;驱动 spawn :632/:764/:883/:990)
- Modify: `crates/daemon/src/flow.rs`(SendTracker :647/:658/:777)
- Test: `crates/daemon/src/jobs.rs`(模块内单测区)

**Interfaces:**
- Produces: `pub(crate) fn lock_ok<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T>`(Err(poisoned) → `poison.into_inner()`,jobs.rs 定义、flow.rs 复用);`async fn supervise_driver(core: Arc<JobCore>, fut: impl Future<Output = ()>)`(`AssertUnwindSafe(fut).catch_unwind().await`,Err(panic) 且 `!core.is_terminal()` → `core.transition("failed", Some("内部错误: 传输驱动异常退出"))`;四个驱动 spawn 点统一包此函数,JoinHandle 语义不变,abort 路径不受影响)。

- [ ] **Step 1: 写失败测试** — ①`lock_ok_recovers_poison`:std Mutex 持锁 panic 毒化后 `lock_ok` 仍取到守卫并可写;②`panicking_driver_marks_failed`:构造 JobCore(沿现有单测构造法),`supervise_driver(core.clone(), async { panic!("boom") }).await` → 轮询/notify 等 `core.snap()` state == "failed"。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --lib -- lock_ok panicking_driver` Expected: FAIL(函数不存在)
- [ ] **Step 3: 实现** — jobs.rs 全部 `.lock().unwrap()`(JobCore.m / Jobs.map / JobHandle.task / JobHandle.round)与 flow.rs SendTracker 三处换 `lock_ok`;四处 `tokio::spawn(driver…)` 换 `tokio::spawn(supervise_driver(core.clone(), async move { driver(…).await }))`。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon`(含 cancel/pause e2e 回归面)Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `fix(daemon): poison-lock recovery + supervised drivers (panic -> failed; locks recover via into_inner)`

### Task 3: ctrl copy/move 冲突策略 — 引擎 + ctrl 面 + CLI

**Files:**
- Modify: `proto/proto/ctrl.proto`(CopyRequest/MoveRequest 增 `TransferPolicy policy = <下一可用字段号>`;CopyResponse/MoveResponse 增 `repeated string skipped = <下一可用字段号>`;读现值后取号)
- Modify: `crates/core/src/fs.rs`(copy_paths :418-468、move_entries :329-401)
- Modify: `crates/daemon/src/ctrl.rs`(CtrlPool::copy :286 / move_ :245、responder Req::Copy :998 / Req::Move :991、policy_proto/policy_engine :705-720)
- Modify: `crates/daemon/src/api.rs`(ctrl_copy :711 / ctrl_move :690:payload 增 `"skip"/"overwrite"` 数组,经 `check_policy_limits` jobs.rs:138 校验)
- Modify: `crates/cli/src/main.rs`(cp 同设备糖 :332-346、mv 同设备路径)
- Test: `crates/core/src/fs.rs`(模块内)、`crates/daemon/tests/ctrl_e2e.rs`、`crates/cli/tests/ipc_cli.rs`

**Interfaces:**
- Consumes: `TransferPolicy { skip: Vec<String>, overwrite: Vec<String> }`(core intake.rs:190,精确或段边界前缀命中)。
- Produces: `pub fn copy_paths_with(paths: &[PathBuf], dir: &Path, policy: &TransferPolicy) -> Result<CopyOutcome>`、`pub fn move_entries_with(pairs: &[(PathBuf, PathBuf)], policy: &TransferPolicy) -> Result<MoveOutcome>`,`CopyOutcome/MoveOutcome { copied/moved: Vec<String>, failed: …(沿现失败表形态), skipped: Vec<String> }`;原 `copy_paths/move_entries` 改为空策略委托保留旧签名。策略语义(设计 §7.1-4):skip 命中 → 整项跳过计入 skipped;overwrite 命中 → copy 先清文件型占用再落位、move 直接 rename 覆盖;未命中 → legacy(copy 自动改名 `name (1)`、move 文件覆盖目录撞名逐对失败)。proto 缺省(旧端/无策略)→ legacy,双端互通不断。
- Produces(CLI):cp ③ 与 mv 同设备改为「`scan_cross_conflicts`(client.rs:618)→ `resolve_conflicts` ask 流程 → copy/move RPC 带 skip/overwrite 列表」;`--conflict skip/overwrite` 直发;删除「同机复制冲突由接收端自动改名(--conflict 不适用)」提示。

- [ ] **Step 1: 写失败测试** — ①fs 单测:`copy_policy_skip_overwrite_legacy`(三态各一:skip 命中不出现在目标、overwrite 命中内容翻新无 ` (1)`、未命中自动改名)、`move_policy_skip`(skip 命中源仍在)、`copy_dir_target_policy_no_clobber`(目录撞目录未命中 → 沿 legacy 合并/改名,不报错——Review Focus 3);②ctrl_e2e:`copy_remote_with_policy_skip_overwrite`、`copy_remote_absent_policy_legacy_renames`(旧式无策略请求 → 自动改名,互通面);③ipc_cli:cp ③ ask 流程输入 s/o 的管道测试(沿现有交互测试形态)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-core -- copy_policy move_policy copy_dir_target` 与 `cargo test -p mtdss-daemon --test ctrl_e2e -- policy legacy` Expected: FAIL
- [ ] **Step 3: 实现** — proto 加字段(注:无需动 CTRL_VERSION,缺省即 legacy);fs 增 `*_with` 与 outcome 结构;ctrl.rs 发起/响应两侧贯通 `Option<TransferPolicy>`;api.rs 两个 RPC 收 skip/overwrite 并限流校验;CLI cp/mv 同设备接 ask。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-core -p mtdss-daemon -p mtdss-cli` Expected: PASS(含既有 `copy_remote_same_device`/`move_remote_same_volume`/`duplicate_conflict_renames_uniquely` 不破坏)
- [ ] **Step 5: 门禁 + Commit** — `feat(core+daemon+cli): policy-aware same-device copy/move (TransferPolicy semantics; legacy default for old peers)`

### Task 4: ctrl copy/move 冲突策略 — UI 同设备对话框

**Files:**
- Modify: `ui/src/App.tsx`(粘贴分派 :986 附近与窗内拖拽同设备臂、`pendingConflict` :229)
- Modify: `ui/src/lib/daemon.ts`(copy/move invoke 增 policy 参数)
- Test: `ui/src/lib/transfer.test.ts` 或新 `paste.test.ts`(纯函数面)

**Interfaces:**
- Consumes: Task 3 的 RPC payload(`skip`/`overwrite` 数组);既有 `scanConflicts`(lib/conflict.ts:48)、`ConflictDialog`、`pendingConflict` 流(App.tsx:821-858 runTransfer 同款)。
- Produces: `ui/src/lib` 纯函数 `planSameDeviceOp(op: "copy"|"move", sources: FsEntry[], targetDir: string)` 返回 `{ conflicts: Conflict[]; exec: (policy) => void }` 的材料(或等价纯分派);daemon.ts `copyEntries(device, paths, dir, policy?)` / `moveEntries(device, pairs, toDevice?, policy?)`。

- [ ] **Step 1: 写失败测试** — vitest:同设备粘贴含同名 → `planSameDeviceOp` 产出 conflicts(scanConflicts 语义复用);无同名 → 空 conflicts 直发 exec(policy 为空表)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL
- [ ] **Step 3: 实现** — 同设备粘贴/拖拽臂改为:`daemonList`(目标目录)→ `scanConflicts` → 有冲突 `setPendingConflict({conflicts, exec: policy => ctrlCopyMove(…, policy)})`,复用既有 ConflictDialog 四键;无冲突直发。模态门(modality.ts)已覆盖 pendingConflict,无需改。
- [ ] **Step 4: 跑测试确认通过 + build** — Run: `cd ui && npx vitest run && npm run build` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): same-device paste/drag conflict dialog (scan -> ConflictDialog -> policy)`

### Task 5: devices 单飞

**Files:**
- Modify: `crates/daemon/src/state.rs`(DaemonState 增字段;结构 :315 附近)
- Modify: `crates/daemon/src/api.rs`(devices() :137-165)
- Test: `crates/daemon/tests/jobs_e2e.rs`(沿 `devices_rpc_now_pulls` :1119 的测试服形态)

**Interfaces:**
- Produces: `DaemonState.devices_memo: Arc<tokio::sync::Mutex<Option<(std::time::Instant, Vec<flow::RawDevice>)>>>`(类型以 flow.rs:59-70 `fetch_devices` 实际返回为准);`const DEVICES_MEMO_TTL: Duration = 1s`。devices() 逻辑:未登录原样;锁内查 memo——`age < TTL` 直接复用,否则持锁执行 `fetch_devices`(单飞:并发调用者在锁上排队,醒来即见新 memo);fetch Err/外层 5s 超时 → 沿现路径回 stale 缓存。stale 快照/hub load_pks 行为不变。

- [ ] **Step 1: 写失败测试** — `devices_single_flight_shares_one_pull`:测试服 `/devices` 路由加原子计数;并发 5 个 `devices` RPC → 断言 REST 命中数 == 1 且 5 个响应均 `stale == false`;再叠加 `devices_pull_failure_all_stale_no_deadlock`:DROP TABLE 触发 500 → 并发 N 个全部 `stale == true` 返回、`timeout` 包裹不挂(Review Focus 4)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-daemon --test jobs_e2e devices_single_flight` Expected: FAIL
- [ ] **Step 3: 实现** — memo 字段 + devices() 重排;注意持锁 await 期间外层 timeout 触发的取消安全性(锁随 await 释放)。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-daemon --test jobs_e2e devices` Expected: PASS(含既有 `devices_rpc_now_pulls`)
- [ ] **Step 5: 门禁 + Commit** — `fix(daemon): devices single-flight memo (1s TTL; concurrent RPCs share one REST pull)`

### Task 6: RPC 错误码类型化(清标记串债)

**Files:**
- Modify: `crates/ipc/src/lib.rs`(RpcError :94-99)
- Modify: `crates/daemon/src/api.rs`(错误构造点;`ctrl_rpc_err` :743)
- Modify: `crates/cli/src/client.rs`(need_force_confirm :699、scan_put_conflicts :475、scan_cross_conflicts :636)
- Test: `crates/ipc/tests/ipc.rs`、`crates/cli/tests/ipc_cli.rs`(:477 既有测试改造)

**Interfaces:**
- Produces: `RpcError { code: i64, message: String, #[serde(default, skip_serializing_if = "Option::is_none")] data: Option<serde_json::Value> }`;daemon 在 FsError 驱动的错误响应附 `data = {"kind": "<FsError::code()>"}`(RECYCLE_UNAVAILABLE / NOT_FOUND / INVALID_PATH / …,取 fs.rs:115-127 现有串);offline 仍走 code 3 不变。CLI:`need_force_confirm(e) = e.data.kind == "RECYCLE_UNAVAILABLE"`;scan_put/cross 的「目标目录不存在 = 无冲突」判定改 `e.data.kind == "NOT_FOUND"`;**删除** `RECYCLE_UNAVAILABLE_MARKER` 子串匹配与 `contains("NOT_FOUND")`(同版本随发,不留双轨)。

- [ ] **Step 1: 写失败测试** — ipc:RpcError serde 往返保留 data(kind 序列化/缺省省略);ipc_cli:`need_force_confirm_matches_data_kind`(data.kind 命中 → true;无 data → false);既有 `need_force_confirm_matches_marker` 改造为 data 形态。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-ipc && cargo test -p mtdss-cli --test ipc_cli` Expected: FAIL
- [ ] **Step 3: 实现** — ipc 加字段与构造器;daemon ctrl 错误映射处(err_response → RpcError)附 kind;CLI 三处判定切换。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-ipc -p mtdss-cli -p mtdss-daemon` Expected: PASS(rm 确认梯 CI 注入面回归)
- [ ] **Step 5: 门禁 + Commit** — `feat(ipc+daemon+cli): structured rpc error data.kind (retire marker-string matching)`

### Task 7: 对话框焦点陷阱(ConfirmDialog + ConflictDialog)

**Files:**
- Create: `ui/src/lib/focustrap.ts`、`ui/src/lib/useFocusTrap.ts`
- Modify: `ui/src/components/ConfirmDialog.tsx`、`ui/src/components/ConflictDialog.tsx`
- Test: `ui/src/lib/focustrap.test.ts`

**Interfaces:**
- Produces: `export function nextFocusIndex(count: number, current: number, shift: boolean): number`(首+Shift → 末、末+前进 → 首,环绕;count ≤ 0 → -1);`export function initialFocusIndex(count: number): number`(0 或 -1);`export function focusablesIn(root: HTMLElement): HTMLElement[]`(button/[href]/input/select/textarea/[tabindex]:not(-1)、:not(:disabled));`export function useFocusTrap(open: boolean): React.RefObject<HTMLDivElement|null>`(open=true:记 `document.activeElement`、聚焦 `focusablesIn(ref)[initialFocusIndex]`;ref div 上 Tab keydown 捕获:焦点将越界时 `preventDefault` + 按nextFocusIndex 聚焦;open=false/卸载:焦点还原)。两对话框内层 `.dialog` div 挂该 ref;Esc/遮罩语义不变。

- [ ] **Step 1: 写失败测试** — focustrap.test.ts:环绕双向、count=0/-1、initial 0;`focusablesIn` 选择器面用最小 stub(仅纯数组语义可测部分,DOM 细节留手动)。
- [ ] **Step 2: 跑测试确认失败** — Run: `cd ui && npx vitest run` Expected: FAIL(模块不存在)
- [ ] **Step 3: 实现** — 纯函数 + hook + 两组件接线(各 3 行:import、`const ref = useFocusTrap(open)`、`<div ref={ref} …>`)。
- [ ] **Step 4: 跑测试确认通过 + build** — Run: `cd ui && npx vitest run && npm run build` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(ui): focus trap for ConfirmDialog/ConflictDialog (pure nextFocusIndex + hook)`

### Task 8: spawn_blocking 清扫

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(:571 start_upload_internal 的 `local_tree_rels`;:945-949 start_local_copy 的 `is_dir`+`local_tree_rels`)
- Modify: `crates/daemon/src/flow.rs`(:563-578 run_send_with 的 load_identity/collect_items/build_manifest/metadata 合计)
- Modify: `crates/core/src/transport/transfer.rs`(:952-959 recv_files_body 的 free_bytes_at + reconcile_with_policy)
- Modify: `crates/cli/src/client.rs`(:510-516 scan_get_conflicts 的 read_dir)
- Test: 无新测试(纯迁移;回归 = 既有 jobs_e2e/ctrl_e2e/e2e/vitest 全量)

**Interfaces:**
- Consumes: 既有惯例 `tokio::task::spawn_blocking`(ctrl.rs:626-635 blocking_fs、jobs.rs:1749)。
- Produces: 上述五点闭包化 `spawn_blocking(...).await`,JoinError 按各点错误类型映射(`anyhow::Error`/`CliError`);对外签名仅 `start_upload_internal`/`start_local_copy` 转 async(调用方 api.rs :425/:456 加 .await),其余为函数体内重排。

- [ ] **Step 1: 逐点迁移 + 每点后跑目标回归** — 顺序:cli read_dir → jobs 两点 → flow 合并块 → transfer 接收块;每点后 Run: `cargo test -p <crate>(对应套件)` Expected: PASS(行为零变化)
- [ ] **Step 2: 全量回归 + clippy** — Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` Expected: PASS/零警告
- [ ] **Step 3: 双平台门禁 + Commit** — `refactor(daemon+core+cli): offload blocking fs scans to spawn_blocking`

### Task 9: put `--to` 别名

**Files:**
- Modify: `crates/cli/src/main.rs`(Put :74-83、dispatch :212-221)
- Test: `crates/cli/tests/ipc_cli.rs`

**Interfaces:**
- Produces: `Cmd::Put { target: Option<String>, #[arg(long)] to: Option<String>, conflict, files }`;`client.rs` 增 `pub fn resolve_put_target(target: Option<String>, to: Option<String>) -> Result<String, CliError>`(恰好其一:0 个 → 「put 需要目标:位置参数 <设备>:<目录> 或 --to <设备>:<目录>」;2 个 → 「目标只能指定一次(位置参数与 --to 二选一)」;1 个 → 原样返回)。dispatch 用其结果走既有 `parse_scp_target` 路径,其余不动。

- [ ] **Step 1: 写失败测试** — `resolve_put_target_exactly_one`:None/Some → Ok;双 Some → Err 二选一;双 None → Err 用法。
- [ ] **Step 2: 跑测试确认失败** — Run: `cargo test -p mtdss-cli --test ipc_cli resolve_put_target` Expected: FAIL
- [ ] **Step 3: 实现** — clap 结构改造 + helper + dispatch 接线;`--help` 文案同步。
- [ ] **Step 4: 跑测试确认通过 + 回归** — Run: `cargo test -p mtdss-cli` Expected: PASS
- [ ] **Step 5: 门禁 + Commit** — `feat(cli): put --to alias (exactly-one-of target/--to)`

### Task 10: CSP 硬化

**Files:**
- Modify: `ui/src-tauri/tauri.conf.json`(`app.security` :21 附近 csp:null)
- Test: 无自动测试(构建门禁 + 手动 console 验证)

**Interfaces:**
- Produces: `csp: "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self' ipc: http://ipc.localhost"`;`devCsp: "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self' ws://localhost:1421 http://localhost:1420"`。(前端零外部资源已核实:系统字体、无 fetch/WebSocket/CDN;data: 为 vite 小资源内联;unsafe-inline 为 React 样式属性/HMR 注入。)

- [ ] **Step 1: 改配置** — 两键写入。
- [ ] **Step 2: build 验证** — Run: `cd ui && npm run build && npx vitest run` Expected: PASS
- [ ] **Step 3: dev 验证(Review Focus 5)** — Run: `cd ui && npm run tauri dev`(MTDSS_HOME 指向冒烟 home)Expected: 窗口正常、登录表单/浏览可用、DevTools console **无 CSP violation**;HMR 改一处源码热更生效。若 IPC 面报 connect-src 拦截,按 console 实际缺口补 `connect-src`(仅限 ipc/localhost 域),记录最终串。
- [ ] **Step 4: 门禁 + Commit** — `hardening(ui): strict CSP (+devCsp for vite HMR)`

### Task 11: 手工验证(§E + D2/D3)+ 收尾

**Files:**
- Modify: `docs/release-checklist.md`(M3b 段 :63-64 两行勾选回填、现象记录)
- Create: 无(验证执行,不改代码)

**Interfaces:**
- Consumes: `docs/smoke-m3b.md` §E(E1–E4);release-checklist :87 M3c 段 D2/D3 表述。

- [ ] **Step 1: 环境准备** — 本机双 MTDSS_HOME(home_a 带 tray、home_b 无头)+ 回环服务端(沿 smoke-m3a §2.1);`cargo build -p mtdss-daemon -p mtdss-cli`。
- [ ] **Step 2: 执行 §E(E1–E4)** — 管理员 PowerShell 定位管道 → home_a 同用户对照 → `net user mtdsstest <一次性口令> /add` + `runas` 会话内 `NamedPipeClientStream.Connect(3000)` 断言 `UnauthorizedAccessException`/ACCESS_DENIED → `net user mtdsstest /delete` 清理。逐项记录。
- [ ] **Step 3: 执行 D2/D3(release 布局)** — `cd ui && npm run build`,随后 `ui/src-tauri` 下 `cargo build --release`(或 `npm run tauri build -- --no-bundle`)得 release `ui.exe`;复制到 `target\release\`(与 mtdssd.exe 同目录);`cargo build --release -p mtdss-daemon -p mtdss-cli`;release daemon 带 tray 运行(home_a)→ D3:托盘「打开主窗口」拉起主窗、二次点击聚焦既有窗;D2:登出态点托盘「登录」→ 主窗引导态唤起。逐项记录。
- [ ] **Step 4: 回填 checklist + 终门禁** — release-checklist M3b 段两行勾选 + 现象一句话;本机全量门禁 + `cd ui && npx vitest run && npm run build`;Commit `docs: m4a manual verification (SDDL §E, tray D2/D3) + checklist`。
- [ ] **Step 5: 推送确认 CI 三 job 全绿**(push→main 分支或 PR,沿仓库惯例)

## Deferred(本计划不做,记录依据)

Linux/macOS 更新与打包、服务端 Caddy/--plain-http、release workflow、签名体系(= M4b,设计 §7.2–7.4);设备重命名端点(需服务器迭代);jobs 状态串 proto 枚举化(本任务只清 CLI 标记串面);`mtdss send` 遗留 RPC 迁移(兼容面)。
