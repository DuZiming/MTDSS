# MTDSS P4a 收件私有暂存+完成发布 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 安卓收件传输改为「App 私有暂存(FsDestIo 复用)→ 完成发布进 MediaStore」:跨进程死亡真续传,根除 F3/F4/"(1)"。

**Architecture:** 引擎把收件桥路由到 FsDestIo(recv_dir={home}/inbox-staging),fdpass 0x02/fs.request_dest/BridgeDestIo 退役;新发布协议:finalize 后引擎发 `fs.publish` 事件,App 发布进 MediaStore 后经 0x05 应答,协调器驱动任务 publishing→done/failed「保存失败」;重发按暂存记录分区(完整→纯本地 republish,缺失→网络)。

**Tech Stack:** Rust workspace(既有)+ Kotlin(既有);协议沿 fdpass/IPC 事件;无新依赖。

**Spec:** [docs/superpowers/specs/2026-10-27-mtdss-p4a-inbox-staging-design.md](../specs/2026-10-27-mtdss-p4a-inbox-staging-design.md)(§6 五裁定绑定)。

## Global Constraints

- 桌面零漂移:FsDestIo 本体零改动(只许包装);桌面 golden 逐字节不变;桌面任务态不含 publishing。
- 通道纪律改二:0x01(源)、查询通道(0x04 在场 + 0x05 发布应答)**统一 id 计数器**(spec §6.5)。
- 收件 final_state=「记录门(done+published)→0x04 查询」,绝不 stat 暂存路径(spec §6.2)。
- 发布名与 0x04 键 = 记录 final_rel;发布撞名沿任务 TransferPolicy,僵尸驱逐复用 MediaFace(F11 机器)(spec §6.4)。
- republish 资格=暂存记录 done,不限任务显示态;混合 rel 集分区,一个新 job(spec §6.3)。
- RELATIVE_PATH 恒带尾杠(F12 形态延续);512 护栏与发送侧零改动。
- 门禁:双平台 cargo test+clippy -D warnings、nat-matrix、ui、android job;本机 bash 先 unset MTDSS_UPDATE_URL;WSL 口径 `MSYS_NO_PATHCONV=1 wsl.exe env CARGO_TARGET_DIR=/root/t2-target /root/.cargo/bin/cargo t …`;adb 一律 MSYS_NO_PATHCONV=1。
- 分支:`worktree-mtdss-p4a-staging`;每任务门禁绿才提交。

## Review Focus

1. **staged-未发布被误 Skip**(final_state 若 stat 暂存,重发会把已收妥未发布当终件跳过)→T1 e2e 断言「暂存 done 未发布 → 重发走网络不 Skip」。
2. **发布窗内宽限误杀**(download 源端 10s 宽限侵入 30s 发布窗→假失败)→T1 测试「源端完成后进入 publishing,宽限不触发」。
3. **重发分区泄漏**(引擎 Full 路径截断待发布暂存件)→T2 e2e「混合集:staged 件字节零变化」。
4. **发布中 App 被杀**(任务落 interrupted,重发应 republish 不重传)→T2 e2e「publishing 态重启→重发→零网络字节」。
5. **桌面 golden 漂移**(任务态/事件新增泄漏进桌面面)→T1/T4 全量 golden 比对零漂移。

---

### Task 1: 引擎——暂存路由 + 发布协议 + 协调器(含 0x02 退役)

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(路由:inbox 接收改 FsDestIo({home}/inbox-staging)包装;DestQueries 统一 id 兼 0x04/0x05;发布协调器;终态合成;0x02 分派/FdBridgeHandler 臂/request_dest resolver 退役)
- Modify: `crates/daemon/src/bridge_dest.rs` → 删除(FsDestIo 包装新居 `crates/daemon/src/inbox_dest.rs`:写/对账复用 FsDestIo,final_state 覆写「记录门→0x04」)
- Modify: `crates/ipc/src/fdpass.rs`(KIND_PUBLISH_RESULT=0x05 无 fd 帧;线格式表更新)
- Modify: `crates/daemon/src/flow.rs`(fs.publish 事件 `{"request_id","rel","staging_path","size"}`;终态钩子)
- Test: `crates/daemon/tests/inbox_staging_e2e.rs`(替代 inbox_bridge_e2e.rs)

**Interfaces:**
- Produces: 事件 `EV_PUBLISH="fs.publish"`;kind `KIND_PUBLISH_RESULT: u8 = 0x05`(payload `{"request_id":u64,"ok":bool,"error"?:str}` 字母序);任务态字符串 `"publishing"`(非终态,重启沿既有语义映射「中断」);`MTDSS_TEST_PUBLISH_TIMEOUT_MS` 注入缝(缺省 30000)。
- Produces: `InboxDestIo::new(home: PathBuf, queries: Arc<DestQueries>) -> Self`(DestIo 包装)。
- 退役:fs.request_dest 事件、0x02、BridgeDestIo、`MTDSS_TEST_REQUEST_DEST_TIMEOUT_MS`。

- [ ] **Step 1: 失败 e2e(伪 App:订阅 fs.publish,应答 0x05)**:①全链:收→finalize→publishing→发布 ok→暂存件删除、记录留 done+published、任务 done;②发布超时(注入 1500ms)→failed「保存失败」、暂存保留;③**staged-未发布误 Skip 守卫**:发布无应答落 failed 后,重发→走网络(接收字节>0)且**不 Skip**;④源端完成后 10s 宽限窗内进入 publishing 不误杀(注入紧宽限常量证不触发);⑤多文件并发发布、单文件失败=N 项失败其余发布保留。
- [ ] **Step 2: 跑红**(WSL 口径)。
- [ ] **Step 3: 实现**(顺序:fdpass 0x05 → DestQueries 统一 id → InboxDestIo 包装 → 路由/退役 → 协调器/终态/宽限作用域)。发布协调器逐文件 request_id 独立、并发、超时独立;taskstore publishing 落盘语义沿既有非终态节流。
- [ ] **Step 4: 跑绿 + Windows 全量 + clippy 双端 + golden 零漂移核验**
- [ ] **Step 5: Commit** `feat(staging): 私有暂存路由+发布协议+协调器,0x02 退役(P4a)`

### Task 2: 引擎——重发分区与 republish

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(reinitiate 对 download 记录:预扫暂存记录→分区)
- Test: `crates/daemon/tests/inbox_staging_e2e.rs`(追加)

**Interfaces:**
- Consumes: T1 全部。
- Produces: republish 分支(纯本地:对暂存 done 件重发 fs.publish,免登录/免对端;与网络分支同 job 翻新);taskstore 记录携带分区语义(缺省沿 P2 字段)。

- [ ] **Step 1: 失败 e2e 三支**:①跨进程续传:传中杀 daemon→重发→位图续传(跳块断言)→publishing→done(**headline,首次可 e2e**);②republish-only:publishing/failed「保存失败」/cancelled 任意态、暂存 done→重发→零网络字节→发布→done;③混合分区:暂存 2 件 done+1 件缺失→重发→2 件 republish+1 件网络,staged 件字节零变化(文件 mtime/内容断言)。
- [ ] **Step 2: 跑红 → 实现(reinitiate 预扫+分区+republish 驱动)→ 跑绿 + 双端门禁**
- [ ] **Step 3: Commit** `feat(staging): 重发分区与 republish——跨进程续传+纯本地重发布(P4a)`

### Task 3: Kotlin——发布面与任务态渲染

**Files:**
- Modify: `android/app/src/main/java/com/mtdss/android/inbox/InboxProvider.kt`(改造为发布器:订阅 fs.publish→发布→0x05 应答;0x02 请求面删除)
- Modify: `android/app/src/main/java/com/mtdss/android/inbox/MediaStoreInboxStore.kt`(发布流程:尾杠 insert+流拷贝+揭开;撞名沿 policy:skip 弃件记 skipped/overwrite 驱逐+插入;F11 驱逐复用;拷贝失败清 pending 行)
- Modify: `android/app/src/main/java/com/mtdss/android/AppGraph.kt`(通道三→二:0x02 通道退役,查询通道承担 0x04+0x05——统一计数器在 daemon 侧,App 单通道单表)
- Modify: `android/app/src/main/java/com/mtdss/android/ui/tasks/TasksReducer.kt`+`TasksScreen.kt`(publishing 态徽章「保存中」;failed 文案「保存失败」)
- Modify: `android/app/src/main/java/com/mtdss/android/fdpass/FdWire.kt`+`FdPassChannel.kt`(0x05 帧;sendPublishResult;纪律文档改二)
- Test: JVM(InboxLogic 发布决策/FdWire 0x05 字节/TasksReducer publishing)

**Interfaces:**
- Consumes: T1 协议(事件/帧/常量)。
- Produces: `InboxStore.publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult`(枚举 Published/Skipped/Failed(reason));App 端不再有 request_dest 应答面。

- [ ] **Step 1: 失败 JVM 测试**(发布决策矩阵:成功/skip 弃件/overwrite 驱逐/僵尸驱逐/拷贝失败清行/0x05 帧字节/publishing 渲染)
- [ ] **Step 2: 跑红 → 实现 → 跑绿(:app:testDebugUnitTest :app:assembleDebug)**
- [ ] **Step 3: 实机装机**(MSYS_NO_PATHCONV=1 adb install -r;失败走 /sdcard 手动)
- [ ] **Step 4: Commit** `feat(android): 发布面+保存中渲染,0x02 通道退役(P4a)`

### Task 4: 契约、冒烟与收官

**Files:**
- Modify: `crates/ipc/contract/`(评估:fs.publish 事件/0x05 不入 op golden(同 request_dest 例);transfer.list 若含 publishing 场景→android 变体 golden,REGEN 幂等)
- Create: `docs/smoke-p4a.md`(实机清单:跨进程续传 headline/无“(1)”/无孤儿/删件重发/发布失败注入/短熄屏)
- Modify: spec 状态行收官回填

- [ ] **Step 1: golden 评估与落地**(桌面零漂移全量比对;如需变体,REGEN 幂等)
- [ ] **Step 2: smoke-p4a.md 编制**(含 T1-T3 各 e2e 对应实机步骤与预期)
- [ ] **Step 3: 双端全量门禁 + Commit + 整支终审(requesting-code-review)→ PR**
- [ ] **Step 4: 实机场(用户在场)**:按 smoke-p4a 逐项;重点=force-stop 续传跳块日志、删终件重发、发布中杀 App→重发 republish。

---

## 执行注

- 顺序刚性:T1→T2→T3→T4;T1 为最大单刀(引擎核心,允许实现者内部分多次提交但门禁以任务尾一次全绿为准)。
- 实机步骤需用户协作;子代理遇阻即暂停征召。
- 旧 pending 残留(仅测试机)不迁移:新版自然接管(装新版首启清理 inbox-staging 外的旧 inbox-records——实现者在 T1 加一行启动清扫,注释钉 spec §2 非目标)。
