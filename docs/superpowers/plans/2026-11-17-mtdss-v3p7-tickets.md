# MTDSS V3-7 票池修复实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 HANDOFF 优先序收掉 V3-6 挂账五票:ctrl 流截断家族、推送会话定界路由、finalize 占位孤儿、跨作业同名 intake + 键空间对、慢机预算票余员。

**Architecture:** 五票相互独立、按 HANDOFF 序串行派发;①⑤是引擎健壮性/测试工艺,②是 wire 级三端( proto+server+daemon )扩展,③④是 core intake / daemon 收件面的边角收口。全部走既有房风:小步 TDD、逐任务 review、CI 六绿为 merge 线。

**Tech Stack:** Rust(workspace:core/daemon/cli/ipc/mtdss-sign/server)、prost/protobuf、tokio、quinn;测试面 cargo test + 既有 e2e 家族。

**Spec:** 无独立 spec——票源与根因链全部在案:`.claude/sdd-archive/2026-11-11-mtdss-v3p6-polish/progress.md`(V3-6 台账,下称「台账」)+ 本计划各任务内的锚点引文。裁定已录的事项不再重开。

## Global Constraints

- 基座:worktree `.claude/worktrees/v3p7` 分支 `v3p7` @ `49a19e5`(main,V3-6 merge 后全量);PR 目标 main。
- 每任务收口线:局部套件绿 + `cargo clippy --workspace --all-targets -- -D warnings` 净才 commit;**改握手/信令面必须本地全 e2e 家族过一遍**(daemon_e2e/ctrl_e2e/relay_e2e/jobs_e2e/inbox_*——V3-6c 漏跑 jobs_e2e 教训,台账 141 行)。
- 环境纪律:全量套件前 `unset http_proxy https_proxy MTDSS_UPDATE_URL MTDSS_SIGNALING_URL`(台账 35/83 行);本机重负载 `-j 2`(台账 84 行)。
- golden 契约:非增量字段不动;任何帧面增量走 REGEN+复验;**新增字段不进排序/确定性面**(created_ms 教训,台账 191 行)。
- 并发写纪律:任何 tmp 文件名必须唯一(pid+进程内单调计数——write_atomic 并发碰撞教训,台账 207-209 行)。
- 兼容红线:新 wire 字段对旧端(旧服务端/旧客户端)必须空值回退现行为,不劣化(proto 房风:只增不改号;空 purpose/空 rendezvous 不过滤)。
- 文案房风:中文注释/日志;错误码语义不漂移;既有测试断言文案逐字稳定(除非任务明说改)。
- 慢机 flake 家族(CI 抽签)与本波无关时:重跑定性,同码两红才停手升级真查(既有规矩)。

## Review Focus

1. **活连接上的真超时不得被截断分类放大成反复驱逐**:T1 只把「空 body 的 JSON EOF」判连接级疑似;慢盘大目录枚举超时(detail 非空、连接仍活)必须留在验证缝原语义——T1 测试钉两臂。
2. **旧端互通不劣化**:T2 新 rendezvous 字段,旧发起端(空)推送必须照旧被消费;旧响应端不过滤——T2 兼容测试两例。
3. **绝不误删用户文件**:T3 占位清理与 T5 .part 后缀清扫只动自家产物(后缀精确匹配 `rel + ".part"`,身份判据在记录面已终审)——各钉「旁人素材不动」断言。
4. **同名并发第二作业锁等待后不得覆写**:T4 第二作业拿到锁后看到 done 记录须 Skip(用户文件安全设计),只有内容不同才 Full——T4 测试钉 Skip 臂与 Full 臂两脸。
5. **测试健壮化不得弱化产品语义**:T6 small-files-first 的调度契约与 reinitiate 的预算耗尽→failed 映射断言必须原样保留,只换观测面(时序→事件序)——T6 每改一断言在报告列新旧对照。

---

### Task 1: ctrl 流截断家族——判定与重试分类真修

**背景(台账 160-163 行):** 统一签名「EOF while parsing a value at line 1 column 0」= ctrl 面(QUIC+JSON)流被截断;warm_xfer_concurrent_second_punches 中 job2 落 failed 且 error 存该串。机制假说:池连接并发多流(TAG_CTRL mux)下连接级 close 截断在飞流;CtrlPool::request 的驱逐/验证缝应兜住,「为何 job2 落 failed」待查(重试预算耗尽 or 驱动层 fail-fast 无重试)。

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(classify_failure 249 行 / request_failure_action 258 行 / request 池命中臂 893-1020 行)
- 读定位:`crates/core/src/transport/ctrl.rs`(send_request/send_request_tagged 的响应读取与 serde 解析点)、`crates/daemon/src/flow.rs:1208`(retryable 只认 Network)、`crates/daemon/src/jobs.rs:4848-4851`(remote_copy 预扫 ctrl 错误直接 finish failed——fail-fast 面)、`crates/daemon/tests/daemon_e2e.rs` 的 `warm_xfer_concurrent_second_punches`
- Test: `crates/daemon/src/ctrl.rs` tests 模块 + daemon e2e 注入用例

**Interfaces:**
- Consumes: 既有 `conn_alive()`(ctrl.rs:1469)、`FailureAction`(Surface/Evict)。
- Produces: `fn is_stream_truncation(e: &anyhow::Error) -> bool`(或同类谓词;判定=响应读回 0 字节/空 body 的 serde EOF——**不是任何 parse 错误**);`request()` 对截断类错误的分派改动(连接级疑似:驱逐+新拨重发恰一次,不走 Surface 验证缝);后续任务无消费。

- [ ] **Step 1: 复现与定性(先查后修)**
  - 读 e2e `warm_xfer_concurrent_second_punches` 全文,标出 job2 的 ctrl 调用路径(预扫/orchestrate/轮询哪一支)。
  - 写最小单测:池内 mux 连接,serve 端接受 ctrl 流后**不开写直接关流**(空 body)→ 断言当前行为(request 返回何错、走哪臂、是否重试)。此步允许结论为「现状已兜住,洞在驱动层」——把证据写进任务报告再定修复面。
- [ ] **Step 2: 写失败测试(修复面按 Step 1 结论落)**
  - 单测两臂:①截断错误(空 body EOF)→ 期望驱逐重试后成功;②慢响应超时(detail 非空/连接活)→ 期望维持验证缝原语义(不驱逐暖连接)。
  - 若 Step 1 定性洞在驱动层(jobs 预扫 fail-fast 无重试):补驱动层裁定——预扫/轮询类 ctrl 错误对「截断/连接级」类引入有限重试(恰一次,与池内语义对齐),其余错误维持快败;测试钉「截断后重试一次成功→作业不 failed」。
- [ ] **Step 3: 实现**——按上述两臂落码;谓词只认空 body EOF(serde_json `EOF while parsing a value at line 1 column 0` 或读回 0 字节),注释写明与慢超时的分界。
- [ ] **Step 4: 验证**——`cargo test -p mtdss-daemon --lib ctrl`(新例绿);`unset` 代理后 `cargo test -p mtdss-daemon --test daemon_e2e warm_xfer`(或对应套件)绿;clippy 净。
- [ ] **Step 5: Commit**——`fix(v3p7): ctrl 流截断分类——空 body EOF 连接级疑似驱逐重试,慢超时留验证缝`

### Task 2: 推送会话定界路由(wire 三端)

**背景(台账 139-141 行):** wait_relay_push 按 (peer,purpose) 匹配不分会话——ctrl 面同对端多握手并存(warmer/浏览/重试)时 armed 响应方推送窗会偷走其他握手的推送。V3-6c 定稿响应方撤回 None;本票做 sid/tag 级定界,**做完才安全开响应方 armed 消费**(后续波)。

**Files:**
- Modify: `proto/proto/signal.proto`(RelayAllocate 增 `string rendezvous = 3`;RelayAllocated 增 `string rendezvous = 9`)
- Modify: `server/signaling/src/`(relay 分配路径:存请求 rendezvous 并在双帧回显;沿 purpose 字段 42-45/67 行同款管线)
- Modify: `crates/core/src/signaling/client.rs`(relay_allocate 签名增参;RelayAllocated 解析面)
- Modify: `crates/daemon/src/hub.rs`(RelayEvt::Push 增 rendezvous 字段)
- Modify: `crates/daemon/src/fallback.rs`(发起方:每次 connect_with_fallback 生成 nonce 传入 allocate;响应方:wait_relay_push 过滤条件加 rendezvous)
- Modify: ConnectRequest 载荷(proto ctrl 面,grep `display_name` 定位 V3-1 增量先例)——发起方把 nonce 随握手带给响应方
- Test: `server/signaling/tests/relay.rs`、`crates/daemon/src/fallback.rs` tests、`crates/core/tests/signaling_client.rs`

**Interfaces:**
- Consumes: 既有 purpose 管线(signal.proto 42-45 行注释 = 本任务同款做法的模板)。
- Produces: `wait_relay_push(relay_rx, peer_id, purpose, rendezvous: &str, window)`——匹配条件 `(evt.purpose == purpose || evt.purpose.is_empty()) && (evt.rendezvous == rendezvous || evt.rendezvous.is_empty())`;nonce 生成 `fn rendezvous_nonce() -> String`(每握手一份,进程内唯一即可,如 `format!("{:016x}", counter)` + pid 前缀,不进持久面)。
- 兼容矩阵(测试钉全):新发起+旧服务端(无回显→空→响应方不过滤);旧发起+新服务端(请求无字段→回显空);新+新(精确路由,他握手推送不偷)。

- [ ] **Step 1: 写失败测试**——fallback tests:broadcast 喂 `{peer, purpose:"ctrl", rendezvous:"A"}` 与 `{peer, purpose:"ctrl", rendezvous:"B"}` 与 `{peer, purpose:"ctrl", rendezvous:""}`(旧发起),断言 `wait_relay_push(rx,"peer","ctrl","B",…)`:只收 B 帧;空帧(旧端)在无匹配时兜底可收。server tests:分配回显 rendezvous 原样、旧请求回显空。
- [ ] **Step 2: 跑测确认失败**(签名无参/字段不存在,编译红)。
- [ ] **Step 3: 实现**——proto 只增不改号;server 存储+双帧回显(照 purpose 模板);core client 透传;hub 事件面;fallback 两端接线;ConnectRequest 携 nonce(可选字段,旧端忽略)。**改的是信令/握手面:收口前全 e2e 家族本地过一遍。**
- [ ] **Step 4: 验证**——`cargo test -p mtdss-signaling -p mtdss-core --test signaling_client`、`cargo test -p mtdss-daemon --lib fallback`、server relay 套件;全 e2e 家族;clippy 净。golden 契约(signal.proto 不在其面)确认零改。
- [ ] **Step 5: Commit**——`feat(v3p7): 推送会话定界路由——rendezvous nonce 三端贯通,空值回退旧行为`

### Task 3: finalize 占位孤儿——rename 失败路径收口

**背景(台账 126-130 行):** intake.rs commit() 377-387 行:rename 失败时占位清理是 `let _` 尽力而为;CI 载荷下 rename+清理双败 → 0 字节壳占基名 → 重试轮 unique_target 见占用落 (1)。数据不丢但观感坏。

**Files:**
- Modify: `crates/core/src/transport/intake.rs:376-387`(commit 的 rename 失败臂)
- Test: `crates/core/src/transport/intake.rs` tests(或 `crates/core/tests/transfer.rs` 既有 intake 测试面)

**Interfaces:**
- Produces: `fn remove_placeholder_with_retry(path: &Path, attempts: u32, delay: Duration) -> std::io::Result<()>`(私函即可;有限重试,如 3 次×50ms 退避;失败返回 Err 供调用方拼警示日志)。
- 语义红线:占位是本调用刚 create_new 的空壳,重试删除失败绝不能升级为「删除既有同名文件」;错误文案携带 part 与终名双路径(排障面,台账 130 行要求)。

- [ ] **Step 1: 写失败测试**——单测直驱 `remove_placeholder_with_retry`:①在场文件删除成功;②删除失败注入(只读目录/占用句柄,Windows 下开独占句柄即可)→ 有限重试后如实 Err;③重试间隔内文件被释放(注入:首删失败后异步释放——若注入面太脆,退化为断言重试次数与延迟下界)。另钉:commit rename 失败臂调用本函数且错误 context 含双路径。
- [ ] **Step 2: 跑测确认失败**(函数不存在,编译红)。
- [ ] **Step 3: 实现**——失败臂换 `match remove_placeholder_with_retry(&final_path, 3, 50ms)`:Ok 继续原错误返回;Err 追加 warn 日志(双路径+错误串)后原样返回错误。清理失败不改变错误语义(仍是 rename 失败上抛)。
- [ ] **Step 4: 验证**——`cargo test -p mtdss-core --lib transport::intake`(或对应路径);`cargo test -p mtdss-core --test transfer` 绿;clippy 净。
- [ ] **Step 5: Commit**——`fix(v3p7): finalize 占位清理有限重试+双路径警示——0 字节壳孤儿收口`

### Task 4: 跨作业同名 intake——per-rel 串行锁

**背景(台账 38 行,V3-6 T5 票池新增):** 跨作业同名文件并发写同目录,intake 层锁缺;hash 门兜底无静默损坏(失败轮+重试自愈),但浪费一轮且 error 观感差。

**Files:**
- Modify: `crates/core/src/transport/intake.rs`(FsDestIo 或其上层 recv 编排——锁的落位在 Step 1 定:须覆盖同 rel 的 open_writer→commit 全跨度)
- Test: `crates/core/tests/transfer.rs`(或 intake 单测面)并发同名用例

**Interfaces:**
- Produces: 进程内 per-(staging_root, rel) 异步互斥(实现建议:全局 `OnceLock<Mutex<HashMap<(PathBuf,String), Arc<TokioMutex<()>>>>>`,弱引用或计数回收防泄漏——实现面任务自裁,接口语义钉死:同键串行、异键零影响);第二等待者获得锁后**必须重读记录面**再落笔(done+内容吻合 → Skip;内容不同 → Full 覆写记录——既有对账语义,不得绕过)。
- 红线:锁不得跨 await 持有 std 锁(房风);不得引入跨进程锁(单 daemon 内并发即可)。

- [ ] **Step 1: 写失败测试**——core 级:两个并发 recv 会话同目标目录同 rel(内容不同或同),断言:①无一轮因 interleaved .part 失败(hash 门不再被触发);②后落笔者按重读后的记录面行事(同内容→Skip 计 done,异内容→Full 覆写)。(现状此测试应红:交错写触发 hash 门失败轮。)
- [ ] **Step 2: 跑测确认失败。**
- [ ] **Step 3: 实现**——锁粒度=rel 级(不是目录级,不伤 V3-5 并发轮吞吐);等待后重读记录面的接线点在 open_writer(resume 判定)与 plan 之间按 Step 1 定性落。
- [ ] **Step 4: 验证**——新例绿;`cargo test -p mtdss-core --test transfer` 全绿(既有续传/对账语义零回归);`cargo test -p mtdss-daemon --test jobs_e2e`(daemon 侧并发轮消费面)绿;clippy 净。
- [ ] **Step 5: Commit**——`fix(v3p7): 跨作业同名 intake per-rel 串行锁——锁后重读记录面,hash 门兜底退为最后防线`

### Task 5: 键空间对——根向守卫 + .part 清除盲区

**背景(台账 73 行,V3-6 T20 记档;inbox_dest.rs 127-141/496-501 行如实在案):** ①前缀轮中断后 cleanup_inbox 按平铺 rel 清 .part,而前缀轮 .part 键带前缀 → 残留;②子目录轮记录落平铺键位(final_rel 带前缀),根轮同 rel 对账时 final_state 无守卫 → 误 Skip(内容已在树,永不落根位)。

**Files:**
- Modify: `crates/daemon/src/inbox_dest.rs`(final_state 根向守卫 127-159 行;cleanup_inbox 496-507 行)
- Test: `crates/daemon/src/inbox_dest.rs` tests 模块(既有 `final_state_scope_guard_rejects_other_prefix_finals` 920-953 行是模板)

**Interfaces:**
- Produces: ①`final_state` 守卫扩展:任何轮(含空前缀/根轮)要求 `dirname(final_rel) == dirname(rel)`(冲突改名 `名 (n).ext` 不改 dirname,同域恒过;他前缀 `前缀/rel` 的 dirname 恒不等于 rel 的 dirname,恒拒)——语义:根轮 rel="a.bin" 不再吃 `子/a.bin` 记录的 Skip;根轮嵌套 rel="docs/a.bin" 与其冲突改名同域照常 Skip。②`cleanup_inbox` 增 .part 后缀清扫:对每个 rel,扫暂存树删除**路径恰以 `<sep> + rel + ".part"` 结尾**的文件(精确后缀,rel 含自身内部目录段;记录面身份终审已由 cleanup_rels 完成,.part 单体是惰性垃圾,后缀匹配即安全)。
- 红线:清扫绝不匹配他人 rel(后缀含完整 rel,`x/a.part` 不匹配 rel="a");不动正式终件与记录。

- [ ] **Step 1: 写失败测试**——①根向:`put_record(rel="a.bin", final_rel="子/a.bin", published)` + 伪 App 答在场 → 根轮 dest 的 `final_state("a.bin")` 必须 None(现红:守卫只护子前缀轮);同域对照 `final_rel="a (1).bin"` 照常 Some。②清扫:暂存树手植 `子/rel.bin.part` 与旁人 `子/other.bin.part`,cleanup 后前者删、后者在;平铺 `rel.bin.part` 照旧由既有路径删。
- [ ] **Step 2: 跑测确认失败。**
- [ ] **Step 3: 实现**——守卫条件改两轮通用(dirname 等式);清扫实现 walk 暂存树(浅层即可?否——前缀可嵌套 `子/孙`,须递归;暂存树有界,阻塞 fs 面房风 spawn_blocking 调用方既有)。注释更新 496-501 行「已知边界」段(边界已闭合,如实改写)。
- [ ] **Step 4: 验证**——`cargo test -p mtdss-daemon --lib inbox_dest` 全绿(既有守卫/清扫/发布族零回归);`cargo test -p mtdss-daemon --test inbox_staging_e2e` 绿;clippy 净。
- [ ] **Step 5: Commit**——`fix(v3p7): 收件键空间两洞——根向 dirname 守卫 + 前缀轮 .part 后缀清扫`

### Task 6: 慢机预算票余员——reinitiate/scheduling 事件序化

**背景(台账 174-177 行):** 夜载荷三员各不同,update_state_stream 已治;余 reinitiate_remote_copy(杀门预算)与 scheduling_small_files_first(调度序竞态断言)。真治方向已裁:测试编舞载荷健壮化——时序断言改事件序/预算去拟合。

**Files:**
- Modify: `crates/daemon/tests/daemon_e2e.rs:3355`(reinitiate_remote_copy_orchestrated)
- Modify: `crates/core/tests/transfer_v2.rs:654`(scheduling_small_files_first)
- 读定位:core send 调度器(grep `max_concurrent_files` 消费点)的选件序与 SendProgress 变体面(FileStart 类事件是否存在;无则评估加进程内回调变体——**不进 wire 帧,golden 零改**)

**Interfaces:**
- Produces: 无产品面产出(纯测试工艺);若 SendProgress 需增变体,签名 `SendProgress::FileBegin { rel: String }`(进程内进度回调面,wire/golden 不动)。
- 语义红线(Review Focus 5):small-files-first 调度契约与「预算耗尽→failed 映射」断言原样保留,只换观测面;报告列每条断言的新旧对照。

- [ ] **Step 1: 定性两测的竞窗**——reinitiate:标出杀门时刻/预算耗尽等待的时序假设(哪些 sleep/轮询拍在竞窗上);scheduling:确证断言依赖 FileDone 完成序而竞窗在调度交接(两槽同时空闲时选件)。写进任务报告。
- [ ] **Step 2: 改断言**——scheduling:断言面从 FileDone 序改为调度决策序(FileBegin/选件事件序:small1、small2 先于 big);reinitiate:杀门后状态推进改事件驱动等待(job_query 轮询已有,去掉对固定时刻的假设;预算断言改「终态 failed 且 attempts 面=预算值」)。
- [ ] **Step 3: 验证**——两测各本地 5×绿(其中至少一次与全量同跑载荷下);`cargo test -p mtdss-core --test transfer_v2` 与 `cargo test -p mtdss-daemon --test daemon_e2e reinitiate` 绿;clippy 净。
- [ ] **Step 4: Commit**——`test(v3p7): 慢机预算余员事件序化——scheduling 断言面换调度序,reinitiate 杀门断言去时序拟合`

---

## 波基线(控制器项,派发前)

- `cargo build --workspace` 绿(已验,worktree 首建)。
- `unset` 代理/env 后 `cargo test -p mtdss-daemon` 全量退码 0(红则停,先报告)。

## 依赖与派发序

按 HANDOFF 序串行:T1→T2→T3→T4→T5→T6。T1/T2 同触 daemon ctrl/fallback 面(T2 消费 T1 落定后的分派现状,串行无冲突);T3/T4 同触 core intake(串行);T5 daemon 收件面独立;T6 纯测试。每任务完成即台账逐行记录(SDD 房风)。
