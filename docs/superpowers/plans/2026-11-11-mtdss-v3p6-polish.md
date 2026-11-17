# MTDSS V3-6 Windows 打磨与收尾 + 接收并发/中继回切 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 V3-6 spec 四大块——引擎两特性(接收轮并发化;中继三层压缩=并行预备+armed 记忆+探洞回切)、Windows UI 六 bug+右键快速发送、移动端/跨端六票、引擎五小票与 1.3.0 收尾。

**Architecture:** 引擎面全部收在 daemon/core/server 三 crate(fallback.rs 单缝消费记忆与预备;flow.rs 轮结构重构为派发器;warm.rs 承探洞豁免与回切);协议仅增 `RelayRelease`(signal.proto,旧端互通无害);UI 面 ui/ 纯前端(Tauri 桥零改);移动端 Android 为主+iOS 一票审计。

**Tech Stack:** Rust workspace(proto/daemon/core)+ Tauri/React(vitest)+ Kotlin(Gradle JVM)+ Swift(Mac mini 构建盒)+ NSIS/TestFlight/APK 发布链。

**Spec:** `docs/superpowers/specs/2026-11-11-mtdss-v3p6-polish-design.md`(已批准 2026-11-11;本计划与 spec 同读)

## Global Constraints

- 调优值全走 env 旋钮族纪律(同 `ctrl.rs` CTRL_TUNING 族):接收并发 3、预备延迟 3s、TTL 15min、探洞节奏 30s、armed 等推窗 45s(spec §11 裁量 1/3/4/5/7)。
- **红线**:XferSlots 按设备单槽语义不动;Stage B 两臂收口段(`warm_xfer_round`/`recv_handshake` 既有舞步)打洞路径逐行不动,新面只加不改;`INBOX_VPATH` 路由资格判定(inbox_publish 在场)前置秩序不动。
- 协议增量纪律:消息包裹/缺省值(protoc 3.12);`RelayRelease` 为 signal.proto 新 oneof 臂,旧服务端解出空臂无害、旧客户端永不发送——无发版顺序约束。
- 更新链构建纪律:凡本地出 Windows 更新包必带 `MTDSS_UPDATE_URL`+`MTDSS_UPDATE_PUBKEY` 双 env,产物 mtdssd.exe 双串 grep 验证。
- 版本联动面:根 `Cargo.toml:9`、`ui/src-tauri/Cargo.toml:3`、`ui/package.json:4`、`ui/src-tauri/tauri.conf.json:4`、`android/app/build.gradle:77-78`(versionCode 4→5)+ Cargo.lock×2 + contract golden REGEN + Kotlin 断言×5(grep `1\.2\.1` android/app/src 定位)+ iOS project.yml/测试内版本串。
- 四套件测试绿 = cargo workspace(三平台)+ ui(vitest+tsc+build)+ gradlew + Mac swift;CI 六绿是 merge 线。
- 每任务 TDD:先红后绿;逐任务独立评审;修复波走同闸。

## Review Focus

1. **并发轮同设备双来件**:第二笔暖资格失败须落打洞臂继续,XferSlots 单槽不被 drop 流(预期:两笔都成)→ T5 e2e 钉。
2. **单侧 armed 失步**:仅一侧 armed 时,armed 响应方 45s 窗须吸收对端打洞穷尽期,不产空败轮 → T6 单测钉。
3. **预备会话泄漏**:打洞成功后并行分配的中继会话必须显式释放;release 帧到不了旧服务器时退化为既有 120s 空闲回收,不放大失败 → T7(释放面)+ T8(预备消费)测试钉。
4. **持久化匿名卡 id 撞号**:重启后新 incoming id 不得复用已持久化记录的 id(裸计数器归零撞号)→ T11 测试钉。
5. **回切后直连再死**:轮换一发后直连死亡须重新走穷尽计数再 armed,中继兜底不断供(不乒乓)→ T10 单测钉。

---

## Block A|引擎

### Task 1: PunchMemory 状态机(纯模块)

**Files:**
- Create: `crates/daemon/src/relaymem.rs`
- Modify: `crates/daemon/src/lib.rs`(mod 注册)、`crates/daemon/src/state.rs`(DaemonState 增 `punch_memory` 字段,getter 同 `xfer_slots` 房风 state.rs:449)

**Interfaces:**
- Produces: `pub(crate) struct PunchMemory`(内部 Mutex 表);方法:
  - `note_exhaust(&self, peer: &str) -> u64`(计数+1,armed 代际=计数;返回代际号)
  - `note_direct_success(&self, peer: &str)`(清零+撤 armed)
  - `is_armed(&self, peer: &str) -> bool`(惰性 TTL:`last_event` 距今 > 15min 先衰减归零再判)
  - `register_relay_round(&self, peer: &str, conn: quinn::Connection)` / `clear_relay_round(&self, peer: &str)`(活跃中继轮登记,conn 存克隆)
  - `relay_round_active(&self, peer: &str) -> bool`
  - `drain_relay_conns(&self, peer: &str) -> Vec<quinn::Connection>`
  - `take_rotation_shot(&self, peer: &str) -> bool`(一次 armed 代际一发;未 armed 或已发过 → false;发过标记随 note_direct_success 重置)
  - `probe_floor(&self, peer: &str) -> Duration`(活跃中继轮 → 30s,否则 `Duration::MAX`)
- 常量:`PUNCH_MEM_TTL = 15min`、`PROBE_FLOOR = 30s`(env 旋钮族可调,沿 `warm_debounce_from` 房风 warm.rs:49)

- [ ] **Step 1: 写失败单测**(同文件 tests 模块):单次 exhaust 即 armed;direct_success 清;TTL 过期后 is_armed=false 且再 note_exhaust 重新计;take_rotation_shot 同代际恰一次、清后可再生;register/drain/clear_relay_round 生命周期;probe_floor 两态。
- [ ] **Step 2: 跑测红**(cargo t -p mtdss-daemon relaymem)
- [ ] **Step 3: 实现** `PunchMemory`(std Mutex 不跨 await,房风同 XferSlots mux.rs:55;TTL 惰性判定在读路径,无后台任务)+ state.rs 接线。
- [ ] **Step 4: 跑测绿 + commit**(`feat(v3p6): punch 失败记忆状态机`)

### Task 2: mux serve 队头阻塞(read_tag spawn 化)

**Files:**
- Modify: `crates/daemon/src/mux.rs:85-93`(accept_bi 循环)
- Test: `crates/daemon/src/mux.rs`(tests 模块)

**Interfaces:** 无新接口;行为变更:accept 循环不再内联 await `read_tag`。

- [ ] **Step 1: 写失败测试**:mux 连接上先开一条**不写首字节**的流并 stall,随后开第二条 TAG_CTRL 流——现状第二条被堵(超时断言),修后秒达。
- [ ] **Step 2: 红**;**Step 3: 实现**:read_tag 连同流两半收进 `tokio::spawn`(读完标签再原地分派;TAG_TRANSFER 移交逻辑随任务体搬移,槽 take 语义不变);**Step 4: 绿 + commit**(`fix(v3p6): mux serve 每流 spawn 化,消队头阻塞`)

### Task 3: 暖臂驱逐仅连接级失败

**Files:**
- Modify: `crates/daemon/src/flow.rs`(send_round 暖臂失败收口,Stage B T5 面;grep `evict_warm` 定位)
- Test: `crates/daemon/src/flow.rs` 或 ctrl.rs tests

**Interfaces:** 无新接口。

- [ ] **Step 1: 写失败测试**:暖臂传输失败但错误为**非连接级**(超时/取消/登录翻转类)→ 池条目不被驱逐(下一轮仍可暖);连接级(QUIC conn 错误)→ 驱逐照旧。
- [ ] **Step 2: 红**;**Step 3: 实现**:失败收口按错误分类门控 evict(错误分类沿 `SendError` 形态判 Network-连接 vs 其余);**Step 4: 绿 + commit**(`fix(v3p6): 暖条目驱逐仅连接级失败`)

### Task 4: 重试轮重复来件卡幂等重绑

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(register_incoming:2535 增 `src_job: &str` 参数并入 JobMeta 新字段;新增 `find_live_incoming(&self, src_job: &str, from: &str) -> Option<String>`)、`crates/daemon/src/flow.rs:1862-1872`(Miss 分支先查重绑)
- Test: `crates/daemon/src/jobs.rs` tests

**Interfaces:**
- Produces: `find_live_incoming(src_job, from) -> Option<job_id>`(非终态 incoming、`(meta.src_job, meta.src)` 双键匹配——spec §7-3)
- `register_incoming` 签名变化: callers = flow.rs:1871 一处。

- [ ] **Step 1: 写失败测试**:同 (job_id, from) 两笔来件 → 复用同卡(同 id,进度续流);不同 from 同 job_id → 各建新卡;终态卡 → 新卡;src_job 空(legacy)→ 恒新卡。
- [ ] **Step 2: 红**;**Step 3: 实现** JobMeta.src_job 字段 + 查询 + flow 接线;**Step 4: 绿 + commit**(`fix(v3p6): 重试轮来件幂等重绑(双键)`)

### Task 5: 接收轮并发化(spec §3)

**Files:**
- Modify: `crates/daemon/src/flow.rs:1661-2066`(resident_recv_loop 重构)
- Test: `crates/daemon/src/flow.rs` tests + `crates/daemon/tests/daemon_e2e.rs`(新增并发用例)

**Interfaces:**
- Produces: 轮循环=纯取件派发器;`RECV_ROUND_CONCURRENCY` 默认 3(env 旋钮 `MTDSS_RECV_CONCURRENCY`);每轮任务自有 subscribe/relay_events/login_rx/shutdown 克隆;`me` 提升为 `Arc<DeviceIdentity>` 每代际一份。
- Consumes: Task 1 无关;XferSlots/两臂/收口段调用面不变。

- [ ] **Step 1: 写失败 e2e**:双对端(两 daemon)几乎同时向第三端发小文件——断言接收端两轮时间线重叠(进度事件交错或总墙钟 < 串行和;e2e 房沿 daemon_e2e 现有两端对用例扩三端)。
- [ ] **Step 2: 红**;**Step 3: 实现重构**:内层轮循环取 req 后整体 spawn `round_task`(IncomingRequest 各字段 + Arc 身份 + opts 快照 + 许可证 acquired 后搬入);每轮任务内自订 broadcast/relay;失败 pacing 移入任务尾且不持 permit(RECV_ROUND_PACE 仅失败后小歇);关停/登出经克隆 watch;收口/通知/inbox 发布原样搬移。
- [ ] **Step 4: 跑新 e2e 绿 + 全量 daemon_e2e 既有面回归**(inbox 五用例等,行为等价性靠既有断言);**Step 5: commit**(`feat(v3p6): 接收轮并发化(N=3)`)

### Task 6: armed 消费 + 响应方 45s 窗(fallback 单缝)

**Files:**
- Modify: `crates/daemon/src/fallback.rs:123-158`(connect_with_fallback + relay_via_push 等推窗参数化)、`crates/daemon/src/flow.rs`(send_round:1034 与 recv 握手两调用点传 `Some(&state.punch_memory())`)、`crates/daemon/src/state.rs`(若无则补 getter)
- Test: `crates/daemon/src/fallback.rs` tests

**Interfaces:**
- `connect_with_fallback` 追加参数 `punch_mem: Option<&PunchMemory>`(None=测试直驱面,行为零改)。
- 常量 `ARMED_PUSH_WAIT = 45s`(env 族)。
- 语义:发起方 armed∧资格在手∧!force_relay → 跳过 punch 直分配;punch Ok → note_direct_success;punch Err → note_exhaust;armed 轮 RelayFailed → 反转试 punch。响应方 armed → 跳互拨 + 等推窗 45s。

- [ ] **Step 1: 写失败单测**(fallback 现有 push/allocate 测试房扩):①armed 发起方零 punch 调用直进 allocate;②punch Ok 清记忆;③armed 轮 RelayFailed 反转走 punch;④armed 响应方等推窗参数=45s(非 armed=5s)。
- [ ] **Step 2: 红**;**Step 3: 实现**(punch 可注入桩以断言调用缺席;等推窗经参传入 relay_via_push);**Step 4: 绿 + flow 两调用点接线 + relay_e2e 回归 + commit**(`feat(v3p6): armed 记忆消费+45s 等推窗`)

### Task 7: RelayRelease 协议面 + 服务端释放

**Files:**
- Modify: `proto/proto/signal.proto`(SignalFromClient 增 `RelayRelease relay_release = 4;` + `message RelayRelease { uint64 sid = 1; }`)、`server/signaling/src/lib.rs`(WS 帧分派)+ `server/signaling/src/relay/mod.rs`(会话杀面复用 patrol 既有终止路径 mod.rs:439)、`crates/core/src/signaling/`(客户端发送面)+ `crates/daemon/src/hub.rs`(暴露 `release_relay(sid)`)
- Test: `server/signaling/src/relay/mod.rs` tests + core signaling tests

**Interfaces:**
- Produces: `SignalHub::release_relay(&self, sid: u64)`(daemon 面;封装 WS 帧);服务端收帧 → 杀会话 + 双方通告(复用既有 reason 面新 reason="released")。
- proto 重生成:按仓内既定 protoc 3.12 流程(golden 零涉,signal 面无 golden)。

- [ ] **Step 1: 写失败服务端测试**:在册会话收 release → 即刻移除 + 通告 alive=false(reason=released);未知 sid → 静默忽略。
- [ ] **Step 2: 红**;**Step 3: 实现** proto+服务端+core+hub 面;**Step 4: 服务端套件绿 + commit**(`feat(v3p6): RelayRelease 中继显式释放面`)

### Task 8: 并行预备——打洞与中继分配赛跑(spec §4.2)

**Files:**
- Modify: `crates/daemon/src/fallback.rs`(initiator 非 armed 路径重构)
- Test: `crates/daemon/src/fallback.rs` tests + `crates/daemon/tests/relay_e2e.rs`

**Interfaces:**
- Consumes: Task 6 `punch_mem`、Task 7 `hub.release_relay`。
- 常量 `HEDGE_DELAY = 3s`(env 族)。
- 语义:punch 与 3s 计时赛跑;计时先到 ∧ 资格在手 → spawn `relay_via_allocate` 任务(**跑到完成为止不 abort**,结果经 oneshot 回);punch Ok → note_direct_success + 若分配已成 → `release_relay(sid)` 弃单;punch 穷尽 → note_exhaust + await 分配结果(已毕即用)即切;资格不在手不预备(零改);分配被拒 → 依 punch 结果走既有错误面。

- [ ] **Step 1: 写失败单测**:①punch 1s 成 + 分配被预备 → 断言 release_relay 收到 sid、直连胜出;②punch 10s 穷尽 + 分配 4s 起跑 → 总耗时不含穷尽后再分配段(时钟桩断言立即切换);③无资格 → 无预备任务。relay_e2e:真链路预备-弃单-释放-配额回收复用。
- [ ] **Step 2: 红**;**Step 3: 实现**(tokio::select + sleep 桩可注入;oneshot 信道收分配任务);**Step 4: 绿 + relay_e2e 回归 + commit**(`feat(v3p6): 中继并行预备,穷尽即切`)

### Task 9: 探洞豁免 + 回切轮换(spec §4.4)

**Files:**
- Modify: `crates/daemon/src/warm.rs:315-389`(warm_device 退避钳制+成功回切)、`crates/daemon/src/flow.rs`(send/recv 两轮 relay_path 落定处 register_relay_round,轮终局 clear;grep `中继兜底已建立`/`mark_relay` 定位收点位)
- Test: `crates/daemon/src/warm.rs` tests + `crates/daemon/tests/relay_e2e.rs`(回切编舞)

**Interfaces:**
- Consumes: Task 1 全部方法。
- 语义:①退避钳制:`delay = min(delay, probe_floor(device))`(活跃中继轮期间 30s 上限,兼破 300s 冷却);②ensure_warm 成功且 `relay_round_active` → `take_rotation_shot` 过闸 → note_direct_success + `drain_relay_conns` 逐 conn `close`(应用层码 VARIABLE_ERROr/0x?? 沿 quinn close 房风)+ 日志「回切:关闭中继半连,重试轮落直连」;③轮两端各自 register/clear(发送轮 relay conn、接收轮 relay conn)。

- [ ] **Step 1: 写失败单测**:①probe_floor 钳制(冷却中 300s → 活跃轮 30s);②rotation 恰一发(第二次 warm 成功不再关);③drain 后 register 可再入。relay_e2e 回切编舞:force_relay 起中继轮 → 注入直连可达 → 断言中继 conn 关闭 + 重试轮落直连(若 e2e 环境不可造,降级为单测 + smoke §A 实机项,记偏离)。
- [ ] **Step 2: 红**;**Step 3: 实现**;**Step 4: 绿 + commit**(`feat(v3p6): 探洞豁免+armed 回切轮换`)

### Task 10: 匿名卡持久化 + 计数器持久(spec §5.1-4 daemon 面)

**Files:**
- Modify: `crates/daemon/src/taskstore.rs`(schema 增 `next_incoming_id`;TaskRec 增 kind 区分或复用现有)、`crates/daemon/src/jobs.rs`(register_incoming store=Some + 终态写持久 + 退役上限 100 含 incoming;重启「中断」标记沿用)、`crates/daemon/src/state.rs`(装配)
- Test: `crates/daemon/tests/daemon_e2e.rs`

**Interfaces:** 无对外新接口;tasks.json 向后兼容(旧文件无新字段 → 缺省)。

- [ ] **Step 1: 写失败 e2e**:接收一轮完成 → 重启 daemon → transfer.list 含该 incoming 记录(终态 done);重启后新来件 id 与持久记录不撞号(Review Focus 4);终态超 100 退役含 incoming。
- [ ] **Step 2: 红**;**Step 3: 实现**;**Step 4: 绿 + golden 校验零漂(taskstore 无 golden 则跳)+ commit**(`feat(v3p6): 匿名收件卡持久化+计数器持久`)

### Task 11: 续传进度累计口径(spec §7-4)

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(build_round_handle/轮进度种子:per-job `landed_base`,轮终局 += 本轮落盘字节;round_progress 事件面加 base)、`crates/daemon/src/flow.rs`(recv 轮进度回调包装)
- Test: `crates/daemon/tests/daemon_e2e.rs`(kill-resume 族旁增)

**Interfaces:** 无新对外接口;JobSnapUI/事件进度数值语义=累计。

- [ ] **Step 1: 定口径再写测试**:先读 `transfer.rs` RecvProgressBridge drain 面(1301/1370)确认 skip 补报现状;e2e:大件中断→重发→断言进度事件序列单调不回落(累计口径)。
- [ ] **Step 2: 红**;**Step 3: 实现**(优先 jobs 层 base+会话值换算,引擎零改;若 drain 已可推导累计则收口在桥层);**Step 4: 绿 + kill-resume 族回归 + commit**(`fix(v3p6): 续传进度累计口径`)

### Task 12: %e→{e:#} 全仓清扫

**Files:**
- Modify: `crates/daemon/src/*.rs`、`crates/core/src/**/*.rs`(grep `%e`/`err = %e`/`, e)` warn!/error! 面;info/debug 不动——只清吞错误链的 warn/error)

**Interfaces:** 无。

- [ ] **Step 1: 盘点**:`grep -rn "warn!\|error!" crates/daemon/src crates/core/src | grep -c "%e\|% `e`\|, e)"` 列清单入任务台账;**Step 2: 批量替换**(`{e:#}` 形,anyhow 错误才适用;非 anyhow(如 std io)保持);**Step 3: cargo 全测绿 + clippy 净 + commit**(`chore(v3p6): 日志错误链全显清扫`)

## Block B|Windows UI(ui/,vitest)

### Task 13: Ctrl+C 全局拦防 + user-select 收敛

**Files:**
- Modify: `ui/src/App.tsx:2273-2309`(X/C/V handler:空选 C 也 preventDefault;非 dir/favdir 位 C preventDefault)、`ui/src/App.css`(全局 `user-select:none` + input/textarea/[contenteditable] 豁免)
- Test: `ui/src/App.test.tsx` 或就近键盘测试文件(grep `ctrlKey` 定位)

- [ ] **Step 1: 写失败测试**:①空选 Ctrl+C → preventDefault 被调且不触发浏览器复制;②有选 → clipFromSelection 照旧;③输入框聚焦 → 不拦截(文本复制可用)。
- [ ] **Step 2: 红**;**Step 3: 实现**;**Step 4: 绿 + commit**(`fix(v3p6): Ctrl+C 误全选+快捷键拦防`)

### Task 14: 拖拽回归修复

**Files:**
- Modify: `ui/src/App.tsx`(指针拖拽机制 1486/1572/2655/2700 一带;P4b ghost 面)、按排查定位的行属性行
- Test: 既有 P4b 拖拽单测(grep `ghost` ui/src)复活/扩展

- [ ] **Step 1: 复现定位**:按 P4b smoke 三路(内部行拖/Explorer 拖入/粘贴)逐路走查,定位 V3-3 §14 单表重构后断点(疑:行 data 属性/命中测试);记台账。
- [ ] **Step 2: 写失败测试**(断点最小面:行拾起→ghost+N 角标;落点分派);**Step 3: 修**;**Step 4: 绿 + 三路手动冒烟记档 + commit**(`fix(v3p6): 拖拽回归`)

### Task 15: 列表表头固定 + 去悬停高亮

**Files:**
- Modify: `ui/src/components/FileList.tsx`(设备浏览列表 thead)+ 对应 css(App.css/themes.css)
- Test: `ui/src/components/FileList.test.tsx`

- [ ] **Step 1: 写失败测试**:thead 具粘性类(`position:sticky` 断言/类名锚)+ 悬停高亮仅 th 行;**Step 2: 红**;**Step 3: 实现**(对齐 V3-3 §14 已交付面口径);**Step 4: 绿 + commit**(`fix(v3p6): 表头固定+悬停收敛`)

### Task 16: 任务抽屉最新在上

**Files:**
- Modify: `ui/src/lib/transfer.ts`(seedJobs/reducer 排序)、`ui/src/components/TransferDrawer.tsx`(列表排序口径)
- Test: `ui/src/lib/transfer.test.ts` + `TransferDrawer.test.tsx`

- [ ] **Step 1: 写失败测试**:种子与增量事件后列表按 created/时间戳降序(最新在上);**Step 2: 红**;**Step 3: 实现**(JobSnapUI 无时间戳则补 created 序号面,grep JobSnapUI 定);**Step 4: 绿 + commit**(`fix(v3p6): 抽屉最新在上`)

### Task 17: 主题收敛白黑两套默认白

**Files:**
- Modify: `ui/src/lib/theme.ts`(THEMES=["light","dark"]、DEFAULT="light"、loadTheme 迁移:dark→dark 其余→light)、`ui/src/themes.css`(token 收敛:workbench 并入 light 或删;三处逐主题图标/阴影面随删)、`ui/src/App.tsx`(循环钮两态)、i18n 词条
- Test: `ui/src/lib/theme.test.ts`

- [ ] **Step 1: 写失败测试**:迁移三态(dark 保留、fluent/workbench→light、非法→light);循环序两态;**Step 2: 红**;**Step 3: 实现**(css 逐 token 对表 r2h 存档,勿凭记忆);**Step 4: 绿 + 三主题残留 grep 门(`data-theme="fluent"` 零命中)+ commit**(`feat(v3p6): 主题收敛两套默认白`)

### Task 18: 右键快速发送(spec §5.2)

**Files:**
- Modify: `ui/src/components/ContextMenu.tsx`(本地行增「快速发送到」子菜单:全部 devices 除本机、离线置灰)、`ui/src/App.tsx`(devices/onQuickSend 供给;发送管线复用 toolbar 发送,target_dir=收件箱哨兵)、i18n(zh/en 词条)
- Test: `ui/src/components/ContextMenu.test.tsx`

**Interfaces:** Consumes: 既有发送管线(`transfer_start` 面,grep `requestSend`/发送分派定位);`onQuickSend(deviceId: string)`。

- [ ] **Step 1: 写失败测试**:子菜单列全部远端设备;离线 disabled+title;点击触发 onQuickSend(deviceId) 不弹确认;多选态入口在。
- [ ] **Step 2: 红**;**Step 3: 实现**;**Step 4: 绿 + commit**(`feat(v3p6): 右键快速发送`)

### Task 19: 任务名 PC 侧清偿

**Files:**
- Modify: 按盘点定位(jobs.rs 各建卡点已见 1354/1425/1483/1997 + `jobs.rs:1997` register 下载卡名;UI 展示面 TransferDrawer)
- Test: `crates/daemon/src/jobs.rs` tests(卡型名断言补齐)

- [ ] **Step 1: 盘点写测试**:逐卡型(upload/download/orchestrate/incoming/local_copy/遗留 send)断言代表名规则(单文件本名/单目录顶层名/多项首项等 N 项);**Step 2: 红(暴露欠账)**;**Step 3: 补齐**;**Step 4: 绿 + commit**(`fix(v3p6): 任务名 PC 侧清偿`)

## Block C|移动端与跨端

### Task 20: `/收件箱` 子目录直发(spec §6-3)

**Files:**
- Modify: `crates/daemon/src/flow.rs:1787-1829`(路由臂:`INBOX_VPATH` 前缀+`/` → 哨兵 + rel 前缀)、`crates/daemon/src/inbox_dest.rs`(InboxDestIo 增 `rel_prefix: String`,create_dest 前置拼接;发布树落点同前缀)
- Test: `crates/daemon/tests/daemon_e2e.rs`(inbox 族旁)

**Interfaces:** `InboxDestIo::new` 签名增参(内部构造点两处:flow.rs 1792 + 测试)。

- [ ] **Step 1: 写失败 e2e**:target_dir=`/收件箱/子目录` → 落发布树 `子目录/` 下(非字面路径失败);根哨兵行为逐字节回归。
- [ ] **Step 2: 红**;**Step 3: 实现**(前缀路由先于 absolute_form,同根 vpath 前置纪律;照片 vpath 回退臂序不动);**Step 4: 绿 + inbox 族回归 + commit**(`fix(v3p6): 收件箱子目录直发路由`)

### Task 21: 照片图库树只见目录

**Files:**
- Modify: 排查定位(`crates/daemon/src/photo.rs` photo ls 面 / `android/.../favorites/FavoritesTree.kt` / `RootTree.kt`)
- Test: Android JVM 测试 + 必要处 Rust 测试

- [ ] **Step 1: 定位**(MIX3 现象:树成员节点只列目录;着陆页正常 → 树构建层过滤或 photo ls 树形参数差异;台架复现优先);**Step 2: 写失败测试钉根因**;**Step 3: 修**;**Step 4: 绿 + MIX3 实机验(冒烟 §A 记档)+ commit**(`fix(v3p6): 照片图库树含文件`)

### Task 22: 树展开左滑下载报「dir 非法」

**Files:**
- Modify: `android/app/src/main/java/com/mtdss/android/favorites/FavoritesTree.kt`(左滑下载动作:节点 (device, 真实路径) 解析后下发;同着陆页口径)
- Test: Android JVM 测试

- [ ] **Step 1: 写失败测试**:树节点(含快捷命名空间/收件箱 vpath)左滑 → 下发的 dir 为解析后真实路径(或收件箱哨兵),非原始 vpath;**Step 2: 红**;**Step 3: 实现**(复用着陆页解析单源,勿复制);**Step 4: 绿 + commit**(`fix(v3p6): 树行左滑 dir 解析`)

### Task 23: 收件箱内选文件发送报「原设备不是本机」

**Files:**
- Modify: 排查定位(`android/.../ui/browse/BrowseScreen.kt` 或 send 分档面)
- Test: Android JVM 测试

- [ ] **Step 1: 定位**(收件箱=本地,行 source 分档误判远端;台架复现);**Step 2: 写失败测试**;**Step 3: 修**;**Step 4: 绿 + commit**(`fix(v3p6): 收件箱行本机分档`)

### Task 24: MIX3 tasks.json 持久化断

**Files:**
- Modify: 排查定位(`crates/daemon/src/taskstore.rs` 写面 / Android home 装配 `MtdssEngine.kt` / 进程死亡冲刷窗)
- Test: Rust 测试或 Android 集成锚

- [ ] **Step 1: 现场取证**(MIX3 侧 tasks.json 停 11-10 17:23;logcat/RUST_LOG + 文件 mtime 对照;PC 排障接管法沿 v3p5 场记);**Step 2: 写失败测试钉根因**;**Step 3: 修**;**Step 4: 绿 + MIX3 实机验 + commit**(`fix(v3p6): 安卓任务持久化断点`)

### Task 25: 分享任务名数字(display_name 回归审计)

**Files:**
- Modify: 按审计定位(已排除:Android bridge 建卡名 `jobs.rs:1483` 自算正确、`ShareConsumer.kt:111` rel=真名;疑点=iOS 扩展直发 OrchestrateRequest/display_name、或接收端回退面 `jobs.rs:2550` from=设备 id 形态)
- Test: 按落点补锚(接收端 register_incoming 回退面单测必补:空名回退 from 而非 id 串——若根因在此则双保险)

- [ ] **Step 1: 审计**(iPhone 分享→PC 看卡名;Android 分享→PC 看卡名;各定位 display_name 空否);**Step 2: 写失败测试**;**Step 3: 修**(产生面补算下发,协议零改);**Step 4: 绿 + 双端实机验 + commit**(`fix(v3p6): 分享任务代表名`)

## Block D|收尾

### Task 26: 1.3.0 版本 bump

**Files:**
- Modify: Global Constraints 版本联动面全清单(根 Cargo.toml:9、ui/src-tauri/Cargo.toml:3、ui/package.json:4、ui/src-tauri/tauri.conf.json:4、android/app/build.gradle:77-78、iOS project.yml+测试内版本串、Cargo.lock×2)

- [ ] **Step 1: 六处+锁+断言逐位改 1.3.0**(versionCode=5);**Step 2: golden REGEN**(既定流程);**Step 3: 四套件全量绿(cargo 三平台/ui/gradlew/Mac swift)+ CI 六绿**;**Step 4: commit**(`chore(v3p6): 版本 1.3.0`)

### Task 27: smoke-v3p6.md + 三端产物链

**Files:**
- Create: `docs/smoke-v3p6.md`(§A 引擎验收线:并发/armed 直进/预备即切/回切;§B Windows UI 走查;§C 引用 smoke-v3p4 §B-§F;§D MIX3 实机项)
- 产物: `artifacts/`(Win NSIS 双 env 烤入+grep 验证、APK、TestFlight 上传、linux tar 随需)

- [ ] **Step 1: 写 smoke 文档**(沿 smoke-v3p5 体例:能力边界表/环境/逐项操作预期/排查速查/记录格式);**Step 2: 构建三端产物**(Win:tauri bundle 双 env+双串 grep;APK:cargo-ndk `-P 29` 口径;TestFlight:Mac mini 解锁钥匙串+MTDSS_DEVELOPER_TEAM+ASC 直传口径,见 [[mtdss-mac-mini-access]]);**Step 3: 产物就绪记档 + commit**(`docs(v3p6): smoke 清单+产物链`);**Step 4: 实机场挂用户侧**(§A headline=物理不通对第二笔直进中继+回切零重发)。

---

## 执行与收官

- worktree `.claude/worktrees/v3p6`,分支 `v3p6`,基座 main@ea1125a;逐任务 subagent 实施+独立评审,整支终审后开 PR(CI 全矩阵),实机场 smoke-v3p6 过后用户 merge。
- 依赖序:A 块内 T1→T6→T7→T8→T9 记忆链;T5(并发)独立可并行排;B/C 块互不依赖;T26/T27 殿后(T26 前全绿)。
- 偏离记录:计划尾部交接段回填(房风)。
