# MTDSS v2-P2 传输任务管理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 传输任务记录持久化(只在发起端)+ 重启后「中断」标记 + 重新发起(部分预检/对账续传/dropped 清理)+ 清除(跨设备带回执)+ 底部抽屉 UI;搭车修两张 P1 移交债。

**Architecture:** daemon 新增 `taskstore.rs` 持久层(tasks.json 原子写),jobs.rs 三个建卡点接线落盘,`transfer.list` 改「持久层 + live 合并」;重新发起 = 翻新同记录换新 job_id + upload 驱动「部分预检」模式 + ctrl 新操作 `cleanup_outbox`(源端统一清接收端残留);UI 右下浮卡退役,换底部抽屉六列表格。

**Tech Stack:** Rust workspace(proto/core/daemon)+ protobuf 只增字段 + React/TS UI(vitest)。

**Spec:** [docs/superpowers/specs/2026-10-24-mtdss-v2p2-task-manager-design.md](../specs/2026-10-24-mtdss-v2p2-task-manager-design.md) —— 计划从 spec 立论,执行者两份都读。

## Global Constraints

- 门禁:`cargo test --workspace && cargo clippy --workspace --all-targets -D warnings` 双平台(本机 Windows;WSL `~/mtdss-ci` 可跑 Linux 侧)+ nat-matrix + ui job。
- proto 只增字段不改号;QUIC 流帧格式零改动。
- 本机 bash 环境:`export PROTOC`(winget protoc)与 gh PATH 自救;cargo 走 rsproxy.cn。
- 注释与用户文案中文,沿既有风格;作业生命周期不变量(jobs.rs 头注)改动前必读。
- 每任务一提交,信息风格沿既有(`feat(daemon): …` / `test(relay): …` / `chore(ci): …`)。
- 重新发起仅对 failed/interrupted;已完成不开;记录不自动删除。

## Review Focus

1. **tasks.json 损坏/半写** → daemon 照常起、列表空、原文件备份 → Task 3 `corrupt_json_backs_up_and_starts_empty`。
2. **混合版本 purpose 为空**(老发起端/老响应端)→ 不过滤、不劣化为现行为 → Task 1 `wait_relay_push` 单测两例。
3. **重新发起撞号**(对端注册表仍持旧 job_id 的重试驱动)→ 新 job_id 直接过 → Task 6 e2e `reinitiate_upload_survives_stale_peer_job_id`。
4. **cleanup 重复/半清**(record 删了 part 没删,或 rel 命中他人记录)→ 幂等、身份吻合才删 → Task 5 `cleanup_rels` 单测。
5. **清除时对端离线** → RPC 失败、任务记录保留可重试 → Task 7 e2e `clear_upload_fails_and_keeps_record_when_peer_offline`。

---

### Task 1: 票① RelayAllocate purpose 判别字段(开工前置债)

**Files:**
- Modify: `proto/proto/signal.proto:40-63`(RelayAllocate/RelayAllocated/RelaySession)
- Modify: `crates/core/src/signaling/client.rs:324`(`relay_allocate`)
- Modify: `server/signaling/src/ws.rs:175,295`(分配处理与推送)
- Modify: `crates/daemon/src/fallback.rs:187-234`(`relay_via_push`/`wait_relay_push`)及 `connect_with_fallback` purpose 穿参(grep `connect_with_fallback` 定位 ctrl.rs / hub.rs 调用点)
- Test: `server/signaling/tests/relay.rs`(echo)、`crates/daemon/src/fallback.rs` 内单测、`crates/daemon/tests/relay_e2e.rs`(双面并发组合)

**Interfaces:**
- Produces: `signal.proto`:`RelayAllocate { string to_device = 1; string purpose = 2; }`;`RelayAllocated`/`RelaySession` 各增 `string purpose = 8;`
- Produces: `SignalHub::relay_allocate(&self, to_device: &str, purpose: &str) -> Result<AllocOutcome>`(原签名增 purpose)
- Produces: daemon `fallback.rs`:`pub(crate) enum RelayPurpose { Ctrl, Transport }` + `fn as_str(&self) -> &'static str`;`wait_relay_push(relay_rx, peer_id, purpose: &str)`,过滤条件 `evt.purpose == purpose || evt.purpose.is_empty()`(空 = 老客户端,不劣化)
- 服务端 ws.rs:请求方应答帧与对端推送帧均回显 `ra.purpose`

- [ ] **Step 1: 写失败的单测**(fallback.rs tests 模块):构造 broadcast channel,喂三帧 `{peer, purpose:"transport"}`、`{peer, purpose:""}`(老客户端)、`{other_peer, purpose:"ctrl"}`,断言 `wait_relay_push(rx, peer, "ctrl")` 跳过第一帧、接受第二帧、忽略第三帧
- [ ] **Step 2: `cargo test -p mtdss-daemon wait_relay_push` 确认编译失败**(签名无 purpose)
- [ ] **Step 3: 实现**:proto 三消息增字段 → `cargo build -p mtdss-proto`;core `relay_allocate` 增参;server ws.rs 回显;fallback.rs `RelayPurpose` + 过滤;ctrl/传输调用点分别传 `RelayPurpose::Ctrl` / `RelayPurpose::Transport`
- [ ] **Step 4: server 测试** `cargo test -p signaling relay`(echo 断言)+ daemon 单测过
- [ ] **Step 5: e2e**:`relay_e2e.rs` 新增 `dual_plane_concurrent_fallback`——同对端 ctrl 与传输同时走中继兜底,断言两条连接各自建立、互不错绑(标签各归各)
- [ ] **Step 6: 门禁 + 提交** `cargo test -p mtdss-daemon -p mtdss-core -p signaling && cargo clippy --workspace --all-targets -D warnings`;commit `fix(relay): RelayAllocate purpose 判别字段,双面并发兜底不错绑`

### Task 2: 票② 上传旗舰 1MiB 摊薄(小债)

**Files:**
- Modify: `crates/daemon/tests/relay_e2e.rs:572`(`transfer_via_relay_full`)

**Interfaces:** 无(测试内常量与注释)

- [ ] **Step 1:** 载荷 128KiB → 1MiB(沿 `download_via_relay_full` 的摊薄裁定,容差仍 1.25× 不放宽),同步修失真的「回环开销≈5%」注释(实测口径:双会话固定开销 ctrl ~25KB + 传输 ~23KB)
- [ ] **Step 2:** `cargo test -p mtdss-daemon transfer_via_relay_full` 过
- [ ] **Step 3:** commit `test(relay): transfer_via_relay_full 1MiB 摊薄(128KiB 下固定开销 1.37× 失真)`

### Task 3: TaskStore 持久层

**Files:**
- Create: `crates/daemon/src/taskstore.rs`(含底部 `#[cfg(test)]` 单测,house 风格)
- Modify: `crates/daemon/src/lib.rs`(模块声明)

**Interfaces:**
- Produces: `pub(crate) struct TaskRec { pub id: String, pub kind: String, pub name: String, pub from_dev: String, pub to_dev: String, pub src_paths: Vec<String>, pub target_dir: String, pub policy_skip: Vec<String>, pub policy_overwrite: Vec<String>, pub cut: bool, pub rels: Vec<String>, pub files_done: u64, pub files_total: u64, pub bytes_done: u64, pub bytes_total: u64, pub state: String, pub error: Option<String>, pub created_ms: u64, pub finished_ms: Option<u64> }`(全字段 `#[serde(default)]`,旧记录前向兼容)
- Produces: `pub(crate) struct TaskStore`(内部 `Mutex<HashMap<String, TaskRec>>` + `home: PathBuf`);`load(home: &Path) -> Self` / `upsert(&self, rec: &TaskRec)` / `remove(&self, id: &str) -> bool` / `get(&self, id: &str) -> Option<TaskRec>` / `list(&self) -> Vec<TaskRec>`(id 升序)
- 落盘:`{home}/tasks.json`,复用 `session.rs` 的 `write_atomic`;损坏(解析失败)→ 改名 `tasks.json.bad-<ts>` 备份后从空开始,tracing::warn

- [ ] **Step 1: 写失败单测**:①`roundtrip_upsert_remove_list`(升序);②`load_marks_nonterminal_interrupted`(running/paused/queued → interrupted,done/failed 不动);③`corrupt_json_backs_up_and_starts_empty`(写垃圾 json → load 空、.bad 备份在场);④`partial_old_record_defaults_name_empty`
- [ ] **Step 2: `cargo test -p mtdss-daemon taskstore` 失败**(模块不存在)
- [ ] **Step 3: 实现 taskstore.rs**;lib.rs 挂模块
- [ ] **Step 4: 单测全过**
- [ ] **Step 5: commit** `feat(daemon): 任务记录持久层 TaskStore(tasks.json 原子写/损坏备份/中断标记)`

### Task 4: 建卡/状态/进度接线 + 重启恢复 + transfer.list 合并

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(JobMeta/JobSnap/JobCore/transition/track_common/insert_job/三个建卡点;删 `retain_terminal_cap` + `TERMINAL_RETAIN_CAP` 及其单测)
- Modify: `crates/daemon/src/state.rs`(DaemonState 增字段)、`crates/daemon/src/lib.rs`(装配 `TaskStore::load`)
- Modify: `crates/daemon/src/api.rs:579`(`transfer_list`)
- Test: `crates/daemon/tests/daemon_e2e.rs`(重启恢复)

**Interfaces:**
- Consumes: Task 3 的 `TaskStore` 全接口
- Produces: `DaemonState.tasks: Arc<TaskStore>`;`JobMeta` 增 `name: String`、`rels: Vec<String>`(预扫 rel 全集);`JobSnap` 增 `pub name: String`;`JobCore` 增 `store: Option<Arc<TaskStore>>`(incoming/local_copy 建卡传 None;三类网络作业传 Some)
- Produces: 落盘点:`transition()` 即时 upsert(state/error/finished_ms);`track_common()` 节流 upsert(≥5s,`JobMut` 增 `last_store_write: Option<Instant>`);三个建卡点(start_upload_internal / accept_orchestration / start_download / start_remote_copy)建卡即 upsert 全字段(name=首项名,rels=预扫集)
- Produces: `transfer_list` = `store.list()` 打底,live `snapshot()` 按 id 覆盖(name/kind/src/dst 字段并入 JSON)

- [ ] **Step 1: 写失败单测**(jobs.rs tests):①建卡即落盘(name/rels/params 齐);②终态翻转即落盘;③进度节流(注入小窗口,两次 track 只落一次);④`transfer_list_merges_store_over_live`(store 有 rec、live 有同 id 新态 → 取 live;store 独有 → 出现在列表)
- [ ] **Step 2: 失败确认**
- [ ] **Step 3: 实现**接线与合并;删除 retain_terminal_cap 及 `terminal_cap_*` 单测(spec §4.1:持久层即历史面)
- [ ] **Step 4: 单测过** + e2e `restart_marks_interrupted`:daemon 传输中 kill → 新 daemon 起 → `transfer.list` 含该任务 state=interrupted、进度为最后已知值
- [ ] **Step 5: 门禁(clippy 双平台)` + commit** `feat(daemon): 作业记录接线持久层,重启恢复中断态,transfer.list 持久+live 合并`

### Task 5: cleanup_outbox 面(intake + proto + ctrl 双侧)

**Files:**
- Modify: `crates/core/src/transport/intake.rs`(新增 `cleanup_rels` + 单测)
- Modify: `proto/proto/ctrl.proto:30-39`(两消息 + oneof 13)
- Modify: `crates/daemon/src/ctrl.rs`(responder arm + client 包装)
- Test: intake 单测、`crates/daemon/tests/ctrl_e2e.rs`(远端清残留一例)

**Interfaces:**
- Produces: `intake::cleanup_rels(out_dir: &Path, rels: &[String]) -> u32`——逐 rel:`record_path` 读记录,记录 `rel` 字段吻合才删 record + `part_path` 的 `.part`;缺席/身份不符静默跳过;返回清理数(幂等)
- Produces: `CleanupOutboxRequest { string dir = 1; repeated string rels = 2; }` / `CleanupOutboxResponse { uint32 cleaned = 1; }`;`CtrlRequest.body.cleanup_outbox = 13`、`CtrlResponse.body.cleanup_outbox = 13`
- Produces: daemon ctrl.rs client `pub(crate) async fn cleanup_outbox(&self, to_device: &str, dir: &str, rels: &[String]) -> Result<u32, CtrlErr>`;responder:dir 绝对形态校验同 list/stat 口径,执行 `intake::cleanup_rels`

- [ ] **Step 1: 写失败单测**(intake):①`cleanup_rels_removes_matching_identity`(part+record 双删、cleaned 计数);②`cleanup_rels_idempotent_and_identity_checked`(二清=0;同 key 但记录 rel 不符 → 不删)
- [ ] **Step 2: 失败确认**
- [ ] **Step 3: 实现 intake + proto + ctrl 双侧**
- [ ] **Step 4: 单测过 + ctrl_e2e 新例** `remote_cleanup_outbox`(A 传 B 半程 → A 发 cleanup → B 的 .mtdss 记录/part 消失)
- [ ] **Step 5: commit** `feat(ctrl): cleanup_outbox 跨设备残留清理(身份吻合幂等)`

### Task 6: 部分预检 + upload 重新发起 + transfer.reinitiate

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(start_upload_internal partial 模式、`Jobs::reinitiate`)
- Modify: `crates/daemon/src/api.rs`(dispatch 增 `transfer.reinitiate`)
- Test: `crates/daemon/tests/daemon_e2e.rs`

**Interfaces:**
- Consumes: Task 4 store 接线、Task 5 `cleanup_outbox`
- Produces: `start_upload_internal` 增参 `old_rels: &[String]`(非空 = 重发部分预检模式):顶层缺失输入收集进 `missing` 不再整单 Err;`dropped = old_rels − 新预扫 rels`;推流前 `cleanup_outbox(to, dir, &dropped)` 尽力而为(失败 tracing::warn 不阻断);驱动 Ok 后 `missing` 非空 → `finish("failed", Some("N 项源已不存在(已清理对端残留)"))`,否则 done
- Produces: `Jobs::reinitiate(&self, state: &Arc<DaemonState>, id: &str) -> Result<String, String>`:store.get 校验 kind=upload 且 state ∈ {failed,interrupted} → `store.remove(id)` → 以原参数 + 新 job_id 建新 rec(翻新)→ partial 预扫路径;download/remote_copy 分支此任务返回 `Err("Task 8 接线")`
- Produces: api `transfer.reinitiate {id}` → `{}`(Err → code 1)
- 语义红线:不自动复活;对端离线照普通作业退避(可注入预算)

- [ ] **Step 1: 写失败 e2e** `reinitiate_upload_resume_and_missing`:10 文件传 5 → kill 对端耗尽失败 → 删 2 个源文件 → reinitiate → 断言:块流只补缺块(计数)、8 文件交付、终态 failed、error 含「2 项源已不存在」、对端 `.mtdss` 中 2 个被清
- [ ] **Step 2: 失败确认**(RPC 无此方法)
- [ ] **Step 3: 实现** partial 预扫 + reinitiate + RPC
- [ ] **Step 4: e2e 过 + 单测补两例**:①`reinitiate_rejects_done_and_running`;②`reinitiate_upload_survives_stale_peer_job_id`(对端 accept_orchestration 预插旧 id 不拒新 id)
- [ ] **Step 5: 门禁 + commit** `feat(daemon): upload 重新发起(部分预检/dropped 清理/翻新换 id)`

### Task 7: transfer.clear(清除)

**Files:**
- Modify: `crates/daemon/src/api.rs`、`crates/daemon/src/jobs.rs`
- Test: `crates/daemon/tests/daemon_e2e.rs`

**Interfaces:**
- Consumes: Task 5 `cleanup_outbox` / `intake::cleanup_rels`、Task 4 store
- Produces: `Jobs::clear(&self, state, id: &str) -> Result<(), ClearErr>`;`enum ClearErr { PeerOffline(String), Other(String) }` → RPC code `RPC_PEER_OFFLINE` / 1
- 语义:download → 本地 `intake::cleanup_rels(target_dir, rec.rels)` + `store.remove`;upload/remote_copy(含 cancelled)→ `ctrl.cleanup_outbox(to_dev, target_dir, rec.rels)` Ok 才 `store.remove`,PeerOffline → 任务保留;rels 为空 → 直接删记录(无可清面)
- Produces: api `transfer.clear {id}` → `{}`

- [ ] **Step 1: 写失败 e2e** 两例:①`clear_upload_online_acks_then_removes`(回执后记录消失、对端残留净);②`clear_upload_fails_and_keeps_record_when_peer_offline`
- [ ] **Step 2: 失败确认**
- [ ] **Step 3: 实现**
- [ ] **Step 4: e2e 过 + download 本地清一例**
- [ ] **Step 5: 门禁 + commit** `feat(daemon): transfer.clear 各清各的(在线回执/离线失败保留)`

### Task 8: orchestrate old_rels + job_query rels + download/remote_copy 重发

**Files:**
- Modify: `proto/proto/ctrl.proto`(OrchestrateRequest.old_rels=6;JobQueryResponse.rels=7)
- Modify: `crates/daemon/src/ctrl.rs`(两处签名/解析)
- Modify: `crates/daemon/src/jobs.rs`(accept_orchestration partial、job_query 回填 rels、download/remote_copy 轮询回写 store、reinitiate 两分支)
- Test: `crates/daemon/tests/daemon_e2e.rs`(沿 remote-pull e2e 样板)

**Interfaces:**
- Consumes: Task 6 `Jobs::reinitiate` 骨架
- Produces: `OrchestrateRequest { … string target_dir = 4; TransferPolicy policy = 5; repeated string old_rels = 6; }`;`JobQueryResponse { … string error = 6; repeated string rels = 7; }`(源端 upload 驱动从 manifest rel 集回填)
- Produces: accept_orchestration 增 old_rels 透传 partial 模式(dropped 由源端清 to 端,同一机制);download/remote_copy 驱动 job_query 轮询处 `rec.rels` 回写(非空时);reinitiate download/remote_copy 分支:翻新 + 原参数 + `old_rels=rec.rels` 重发编排
- [ ] **Step 1: 写失败 e2e** `reinitiate_download_resume`(B 拉 A,B 中断 → A 端删 1 文件 → B reinitiate → 续传完成、终态 failed 带 1 项缺失、B 的 .mtdss 该 rel 被清)
- [ ] **Step 2: 失败确认**
- [ ] **Step 3: 实现** proto → ctrl → jobs 三段
- [ ] **Step 4: e2e 过 + remote_copy 重发一例(X 编排,A→B,断言 X 端记录翻新、rels 经 job_query 回填)**
- [ ] **Step 5: 门禁 + commit** `feat(daemon): download/remote_copy 重新发起(old_rels 编排/rels 回填/源端统一清残留)`

### Task 9: UI 传输抽屉(浮卡退役)

**Files:**
- Modify: `ui/src/lib/transfer.ts`(JobState/JobSnapUI/过滤助手 + `transfer.test.ts`)
- Create: `ui/src/components/TransferDrawer.tsx`、`ui/src/components/NotifyToasts.tsx`(自 TransferCards.tsx 迁出)
- Delete: `ui/src/components/TransferCards.tsx`
- Modify: `ui/src/App.tsx`(挂抽屉、reinitiate/clear 调用)、`ui/src/lib/daemon.ts`(如需 RPC 通道补口)

**Interfaces:**
- Consumes: daemon `transfer.list`(含 name/interrupted)、`transfer.reinitiate/clear/pause/resume/cancel`
- Produces: `JobState` 增 `"interrupted"`(isTerminalState 含之);`JobSnapUI` 增 `name?: string`;`export function filterByKind(jobs: JobSnapUI[], filter: "all"|"upload"|"download"|"remote_copy"): JobSnapUI[]`
- Produces: `<TransferDrawer jobs onPause onResume onCancel onReinitiate onClear />`:收起条(进行中 N · 速率 · 中断 N · 失败 N)/展开默认 25% 窗高/把手拖 25%–75%/状态存 localStorage(`mtdss.drawer`);六列表格(任务名/类型/方向/状态/传输速度/操作)按 spec §7.2:表头居中、类型三色定宽徽章、方向三段定宽箭头对齐、进度条定宽、操作图标左对齐(⏸ ✕ ↻ 🗑 悬停提示);操作可用性矩阵同 spec(完成无操作;调度无暂停;被动 incoming 行按 kind 显示、会话级)
- 任务名:单文件=文件名,多文件/目录=首项名 + 「· N 项」(name 与 files_total 组合;name 缺省回退 id)

- [ ] **Step 1: 写失败 vitest**(transfer.test.ts):interrupted 终态判定、filterByKind 四档、name 回退
- [ ] **Step 2: `npm test` 失败确认**(ui 目录)
- [ ] **Step 3: 实现** lib + 组件 + App 接线;NotifyToasts 迁出、TransferCards 删除
- [ ] **Step 4: `npm run build && npm test` 过;`mcp getDiagnostics` 无 TS 错**
- [ ] **Step 5: commit** `feat(ui): 传输抽屉替代浮卡(六列表格/筛选/重发/清除/拖高记忆)`

---

## 收尾

- 全量门禁:`cargo test --workspace && cargo clippy --workspace --all-targets -D warnings`(Windows + WSL)+ nat-matrix + ui job。
- `superpowers:requesting-code-review` 终审 → 修复波 → PR 合入 main(分支 `worktree-mtdss-v2p2-task-manager`)。
- 计划尾注(终审后回填):裁定与偏离记录。

## Self-Review 记录

- spec 覆盖:§3 六裁定 → T3/T4(①②)、T6(④)、T5/T7(③)、T6 拒已完成(⑤)、无删除策略(⑥);§4 → T3/T4;§5 → T5/T6/T7/T8;§6 → T6/T7/T8/RPC + T9;§7 → T9;§8 → T1/T2;§10 测试逐任务落;§11 验收项均有对应任务。无缺口。
- 类型一致:TaskStore 接口在 T3 定义、T4/T6/T7/T8 消费同名;cleanup_outbox 在 T5 定义、T6/T7/T8 消费;reinitiate 在 T6 定义骨架、T8 补分支。
- Review Focus 五条均已映射到 owning task 的测试。

## 尾注(终审回填 2026-10-25)

执行:subagent-driven,9 任务+逐任务审查(T4、T7 各一轮修复)+ 整支终审(opus)→ 修复波(2 Important+4 minor)→ scoped 复审全 ADDRESSED;本机 Windows 全量 60 测试面绿+clippy 净。

**裁定与偏离记录:**
1. 环境门禁:本机 shell 全局 `MTDSS_UPDATE_URL` 经 `option_env!` 烧入编译期缺省,ipc 占位测试按设计红——门禁一律 CI 口径(unset 该二变量)。
2. T1 purpose 穿参调用点按 grep 实际(ctrl.rs/flow.rs/lib.rs;计划文 hub.rs 失真);`RelayPurpose` 取 `pub` 而非 `pub(crate)`(private_interfaces × -D warnings,沿 FallbackRole 先例);clippy `-D warnings` 须置于 `--` 后。
3. T4:「三个建卡点」=三类发起端作业建卡即 upsert;`accept_orchestration` 仅穿 store=None(被动收件不持久,spec §2);TaskStore::load 装配落 state.rs(非 lib.rs);节流单测以机制级替代注入缝(两连发 Bytes 只落一次+FileDone 恒写)。
4. T4 修复轮:作业 id 计数器自 store 最大 seq 播种(跨会话撞号,终审补 saturating_add 防畸形 id 哨兵溢出)。
5. T7 裁定:done 拒清「已完成任务无可清除面」;live 态(queued/running/paused)拒清门置于空 rels 短路之前(修复轮,download 空 rels 不可绕);intake `cleaned` 计数改真值(记录删除成功才计,`.part` 尽力)。
6. T8 计划外改动(终审复核通过):下载 done 收口自接收轮 Ok 移至源端态合流(混合后果终态不再被粘滞吞掉,代价 done 可见至迟 +3s);remote_copy 预扫重发态容忍源消失(NOT_FOUND 交源端 partial,首发仍快败);reinitiate 幂等门=store.remove 返回值原子裁决(真线程测试)。
7. T9:Tauri 桥补 `transfer_reinitiate`/`transfer_clear` 命令(「如需 RPC 通道补口」延伸;clear 35s 浏览超时防误杀对端回执)。
8. 终审修复波:重发起动失败回插原记录(全缺失不再蒸发,对端残留不失清理入口);清除/重发后终态 live 行经 `Jobs::evict_terminal` 摘除(幽灵行出列,e2e 断言改单行);非 UTF-8 损坏并入备份面;UI 终态动作按持久 kind(upload/download/remote_copy)资格门。
9. 门禁:双平台以 GH CI 为正式门(ubuntu+windows test+clippy / nat-matrix / ui job);已知环境 flake:download_cancel_propagates、list_remote_roundtrip(打洞超时,重跑均绿,P3 前归因)。PR #18 首轮 CI 又暴露 flake 家族新成员 `transfer_list_and_events`(CI 慢机 翻转→首条进度 240ms)——根因=断言把 queued→running 状态翻转帧计入节流间隔序列,而 400ms 契约只约束进度帧两两;已修测试(b98c374,剔除翻转帧+断言消息带全帧),非 P2 回归(发射结构与 73c5d8c 逐行无差)。
10. 接受面(留观):逐出后迟到 ConnectRequest 降级匿名卡(极窄窗,cancel→立即清除为最紧变体);tasks.json 无界增长与 transfer.list 全量成本(spec 裁定 6,P4);spec §5.1「首发整单拒绝」措辞与既有「部分缺失首发=静默跳过」不符——后续按代码实况校准措辞,勿按字面补拒绝。
