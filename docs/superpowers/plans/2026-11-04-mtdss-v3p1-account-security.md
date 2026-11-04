# MTDSS V3-1 账号安全与服务端管理 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地受信终端账号安全体系(邮箱真实投递/审批/扫码/上限踢选/限流)+ admin 三命令 + 账号数据 KV + 任务命名两端统一。

**Architecture:** 服务端(signaling,axum+rusqlite+proto WS)先行:登录状态机三路径同一收口;proto 与 IPC 契约增量(缺省字段/新帧,旧端无感);daemon 登录面重构后三端 UI/CLI 消费。引擎传输面仅加 display_name 下发,信令/打洞/传输零改动。

**Tech Stack:** Rust workspace 既有 + axum/rusqlite/hmac/sha1(新,DM 签名)/qrcode(新,CLI ASCII 码);React(Tauri UI)/Kotlin Compose/SwiftUI 增量面。

**Spec:** `docs/superpowers/specs/2026-11-04-mtdss-v3p1-account-security-design.md`(本计划一切语义以 spec 为准,冲突以 spec 为准;伞形 `2026-11-04-mtdss-v3-design.md` §3 亦参)

## Global Constraints

- 桌面回归零:既有全量测试面(Windows cargo test --workspace、WSL 面、nat-matrix、ui、android、ios job)不得因本计划变红;既有 op 语义除 spec §4.2 明示(login.verify 形态)外不动;golden 变更一律 REGEN 全变体+Kotlin/Swift 断言同步。
- proto 增量纪律:新消息/新字段带缺省(protoc 3.12 兼容);OrchestrateRequest display_name=**7**(5/6 已占)、ConnectRequest display_name=5。
- 旧客户端加新设备不设回退(spec §7);新客户端对旧服务器:verify 响应缺 `status` 字段按 ok 解析(容错,发版顺序的保险丝)。
- 服务端凭据/配置只经环境注入不落库:`MTDSS_DM_ACCESS_KEY_ID/SECRET/ACCOUNT/ALIAS`;DM 未配置且非 dev 门=request-code 500 fail-closed。
- 本机 bash 门禁先 `unset MTDSS_UPDATE_URL`;cargo 走 rsproxy;gh CLI bash 需 `export PATH="/c/Program Files/GitHub CLI:$PATH"`;WSL 测试口径 `MSYS_NO_PATHCONV=1 wsl.exe env CARGO_TARGET_DIR=/root/t2-target /root/.cargo/bin/cargo t …`;adb 一律 MSYS_NO_PATHCONV=1。
- 分支:`worktree-mtdss-v3p1-account`(EnterWorktree);每任务一提交起,测试与实现同 commit;每任务尾 Windows 全量+clippy -D warnings(涉服务端/WSL 面时加跑)。
- 服务端 UI 无关,中文文案只进邮件模板/CLI/三端词表。

## Review Focus

1. **presence 快照 fail-closed**:verify 时刻有在线受信终端、随后全掉线 → 本会话仍必须被裁决或超时,绝不降级直进 → T4 测试「快照后全员掉线→approve 前提不消失」。
2. **码暴力**:verify 验证码与审批码各自错 5 次即作废(重新领码/重新登录),作废后原码再对也不放行 → T2/T4 计数测试。
3. **换绑绕行**:已属账号 A 的 pubkey 以账号 B 邮箱 verify → 直入分支必拒、强制走 B 的审批 → T4 测试。
4. **踢后凭据残留**:evict/unbind 后,被踢设备的 refresh token 必续签失败、活 WS 必断(code=9/重连拒)→ T5 测试(refresh 401 + 重连 code1 双断言)。
5. **QR 单次性**:同 token 二次 claim/approve、approve 后重放、过期后使用 → 全拒 → T6 测试。

---

### Task 1: 库表迁移 + admin 三命令

**Files:**
- Modify: `server/signaling/src/db.rs`(迁移小节:幂等 ALTER/CREATE)
- Modify: `server/signaling/src/admin.rs`(三函数)、`server/signaling/src/admin_main.rs`(三子命令)
- Test: `server/signaling/tests/admin_cli.rs`(追加)

**Interfaces:**
- Produces: 迁移落定——`accounts.device_limit INTEGER NOT NULL DEFAULT 3`(0=不限)、`devices.last_seen INTEGER NOT NULL DEFAULT 0`、新表 `approvals(id TEXT PK, account_id, pubkey, name, platform, code_hash, attempts INTEGER DEFAULT 0, state TEXT CHECK(state IN('pending','approved','denied','expired')), expires_at INTEGER)`、`enroll_tokens(token TEXT PK, account_id, pubkey, name, platform, expires_at)`、`qr_tokens(token TEXT PK, account_id, claimed_by TEXT, candidate_pubkey, candidate_name, candidate_platform, state TEXT, expires_at)`、`account_kv(account_id TEXT, key TEXT, value TEXT, updated_at INTEGER, PRIMARY KEY(account_id,key))`、`codes` 增列 `attempts INTEGER NOT NULL DEFAULT 0`;`user_remove` 级联清 approvals/enroll_tokens/qr_tokens/account_kv(随 devices)。
- Produces: `pub fn user_set_device_limit(conn, email, n: u32) -> Result<()>`;`pub struct DeviceRow { device_id, name, platform, created_at, last_seen }`;`pub fn device_list(conn, email) -> Result<Vec<DeviceRow>>`;`pub fn device_unbind(conn, email, device_id) -> Result<bool>`。
- CLI:`user set-device-limit <email> <n>`、`device list <email>`(在线列=last_seen≤70s,表头注明「近似」)、`device unbind <email> <device_id>`;`user list` 增列。

- [ ] **Step 1: 失败测试**:三命令 happy path + set-device-limit 边界(0/改回)+ unbind 不存在设备 Err + 迁移幂等(连跑两次 init 无错)+ user_remove 级联断言。
- [ ] **Step 2: 跑红 → 实现(db.rs 迁移小节参照 `CREATE TABLE IF NOT EXISTS` 风格,ALTER 用 `PRAGMA table_info` 探列幂等)→ 跑绿**。
- [ ] **Step 3: Windows `cargo test -p mtdss-signaling` + clippy;Commit** `feat(server): v3p1 库表迁移+admin 终端管理三命令`

### Task 2: 邮件传输层 + request-code 改造 + 限流基建

**Files:**
- Create: `server/signaling/src/mail.rs`(CodeMailer trait + AliyunDm + DevEcho)
- Modify: `server/signaling/src/lib.rs`(AppState.mailer + 装配 + dev 门参数)、`main.rs`(`--dev-echo-code`)
- Modify: `server/signaling/src/api.rs`(request_code 改造)、`auth.rs`(codes.attempts 计数)
- Create: `server/signaling/src/throttle.rs`(进程内限流器)
- Test: `server/signaling/tests/`(新 mail/throttle 面)

**Interfaces:**
- Produces: `pub(crate) trait CodeMailer: Send + Sync { fn send(&self, email: &str, code: &str) -> Result<()>; }`;`AliyunDm::from_env() -> Result<Self>`(RPC v1 签名 HMAC-SHA1,SingleSendMail;依赖加 hmac/sha1);`DevEcho`(send 恒 Ok,不发信)。装配:env 四键齐→AliyunDm;`--dev-echo-code`→DevEcho(优先);两者皆无→启动 tracing::warn + request_code 500。
- Produces: `pub(crate) struct Throttle { … }`(DashMap<key, Vec<Instant>>);`fn allow(&self, key: &str, max: u32, window: Duration) -> bool`。
- 语义:**发信成功才落库**(先 mailer.send 后 INSERT;失败 500 不产半态);响应 dev 门开=`{code}`、关=`{}`;request-code 限速 per-email 1/min+5/h、per-IP 10/h(超限 429+Retry-After);verify 码错 5 次 DELETE codes 行。
- 邮件模板(中文,spec §3.5 逐字):`你的验证码是 {code},10 分钟内有效。若非你本人操作,请忽略。`

- [ ] **Step 1: 失败测试**:mock mailer 成功→落库+响应无 code;失败→500+库无行;dev 门→响应含 code 且零发信;未配置(无 env 非 dev)→500;限流 1/min 第 2 次 429;码错 5 次后正确码也 401。
- [ ] **Step 2: 跑红 → 实现(mail.rs/throttle.rs/api 接线)→ 跑绿 + clippy**。
- [ ] **Step 3: Commit** `feat(server): 阿里云邮件投递+dev 回显门+request-code 限流+码错作废`

### Task 3: proto 增量(AuthNotice 帧 + display_name 双字段)

**Files:**
- Modify: `proto/proto/ws.proto`(AuthNotice + WsServerFrame oneof 增notice)、`proto/proto/ctrl.proto`(OrchestrateRequest display_name=7)、`proto/proto/e2e.proto`(ConnectRequest display_name=5)
- Test: `proto/tests/`(既有兼容面跑绿即证;无新断言)

**Interfaces:**
- Produces: `AuthNotice { kind, session_id, code, candidate_name, candidate_platform, expires_at, data_key }` 全缺省;`WsServerFrame.frame` 增 `AuthNoticeNotice notice = <下一空闲号>`(以 ws.proto 现状为准);两 display_name 缺省空=旧端回退。
- 本任务零行为(服务端未发、引擎未读),全量回归绿即证。

- [ ] **Step 1: 改 proto → `cargo check --workspace` 双平台绿 + 全量 test 绿(桌面零回归自证)**。
- [ ] **Step 2: Commit** `feat(proto): AuthNotice 推送帧+传输 display_name(V3-1)`

### Task 4: 审批状态机(服务端核心)

**Files:**
- Modify: `server/signaling/src/auth.rs`(verify_code 三分支重构)、`api.rs`(verify 新形态 + approve-code/approval-respond/login-status)
- Modify: `server/signaling/src/ws.rs`(join_request 推送 helper)
- Test: `server/signaling/tests/`(审批面集成)

**Interfaces:**
- Produces: verify 响应 JSON 三形态:`{status:"ok", jwt, refresh, device_id, account_id}` / `{status:"approval", session_id, expires_at}` / `{status:"limit", enroll_token, devices:[{device_id,name,platform,online}]}`(limit 形态 T5 起真值,本任务占位直通 ok)。
- Produces: `POST /auth/approve-code {session_id, code}`(免认证;对上 approvals.code_hash→approved;错 5 次作废;per-IP 30/min)、`POST /auth/approval-respond {session_id, allow}`(Bearer+须为该账号在线受信设备;allow=false→denied)、`POST /auth/login-status {session_id}`(免认证轮询→`{status:"ok|approval|denied|expired"}`,ok 带票;per-IP 120/min)。
- 语义钉死(spec §3.1):①老终端(device 已在本账号 devices)直入;②新终端 verify 时刻 presence 快照(经 `state.presence.account_peers(account_id)` 非空)**含本账号在线受信终端**→建 approvals 行(6 位码,300s)+对全端 `tx.try_send(AuthNotice join_request)`;③快照空→自动受信直插。**已属他账号的 pubkey 恒走②**(直入分支加「row 不存在 OR account_id==本账号」守卫)。会话单次有效;**快照后全掉线不降级**(状态机只认 approvals 行,不重查 presence)。TTL 300s;同 pubkey 重复 verify=作废旧会话建新会话(重推 join_request)。
- ws.rs helper:`pub(crate) fn push_notice(state, device_id, notice: AuthNotice)`(try_send 尽力)。

- [ ] **Step 1: 失败测试**(集成,起 in-process router+双 WS 模拟受信端):三分支各一;审批两 settle 路(respond 同意/approve-code 输码)+拒绝一票否决+300s 过期(expire 注入)+单次有效(approved 后 login-status 再轮询=终态不重发票);**快照后断开受信端 WS→仍须 approve-code/approval-respond 才放行**;**换绑:pubkey 挂账号 A、以 B 邮箱 verify→approval 形态**;码错 5 次作废。
- [ ] **Step 2: 跑红 → 实现 → 跑绿 + signaling 全套 + clippy**。
- [ ] **Step 3: Commit** `feat(server): 新终端审批状态机(先到先裁/快照 fail-closed/换绑守卫)`

### Task 5: 配额门 + evict + 懒踢 + last_seen + 移除刷新推送

**Files:**
- Modify: `server/signaling/src/auth.rs`(enroll 收口)、`api.rs`(/auth/evict)、`ws.rs`(keepalive 绑定复核 + last_seen 写 + code=9 + devices_changed/device_evicted 推送)、`admin.rs`(unbind 复用同一删除路径)
- Test: 追加集成面

**Interfaces:**
- Produces: `fn enroll_device(state, account_id, pubkey, name, platform) -> EnrollOutcome`,其中 `enum EnrollOutcome { Ok(VerifyOk), Limit { enroll_token: String, devices: Vec<DeviceInfo> } }`——**email 直入(T4)/审批 approved(T4)/扫码 approved(T6)三路径唯一收口**;超额(含新行 count+1>device_limit,0=不限)→不插行、铸 enroll_tokens 行(600s)。
- Produces: `POST /auth/evict {enroll_token, device_id}`(免认证;删该 devices 行→**即时踢**(presence 查柄 tx.send(Error code=9)+remove_if_current)→插新行铸票→ok 形态;token 单次;自踢(删自己不可能——自己尚无行);per-IP 30/min)。
- 懒踢:keepalive ping tick 加绑定复核(`SELECT account_id FROM devices WHERE device_id=?`,归属变/行失→Error code=9 断;admin unbind/user_remove 复用删行,≤60s 生效);presence 注册/注销/tick 写 `devices.last_seen`。
- 推送:devices 表**任何增删**(直插/审批插/evict/unbind/换绑)→全账号在线端 `AuthNotice{kind:"devices_changed"}`;被踢端临别 `AuthNotice{kind:"device_evicted"}`(先 notice 后断)。

- [ ] **Step 1: 失败测试**:配额三路径同函数一致(参数化:email 直入/审批后/占位扫码路径各跑 limit→evict→ok 全链);evict 即时踢断在线 WS(被踢端收 code9)+enroll_token 单次+过期;**被踢设备 refresh→401、重连→code1 device not bound**;懒踢:admin unbind 后 ≤2 tick 内 WS 断(压缩 ping 间隔注入);last_seen 滚动;devices_changed 在 evict 后广播至同账号他端;user_remove 级联懒踢。
- [ ] **Step 2: 跑红 → 实现 → 跑绿 + clippy + WSL 面加跑**。
- [ ] **Step 3: Commit** `feat(server): 终端数上限+evict 即时踢+admin 懒踢+devices_changed`

### Task 6: 扫码登录(服务端)

**Files:**
- Modify: `server/signaling/src/api.rs`(qr-begin/qr-claim/qr-approve/qr-status)
- Test: 追加集成面

**Interfaces:**
- Produces(免认证组,spec §3.2 逐字):`POST /auth/qr-begin {pubkey,name,platform} → {qr_token, expires_at}`(120s;per-IP 10/h);`POST /auth/qr-claim {qr_token}`(Bearer;首认领者钉死 claimed_by,幂等同认领者;他者 409)→`{candidate:{name,platform}, expires_at}`;`POST /auth/qr-approve {qr_token, allow}`(Bearer;须=claimant;allow→**enroll_device 收口**→状态 ok|limit;拒→denied;单次);`GET /auth/qr-status?t=`(轮询 2s 档;→`{status:"pending|ok|denied|expired|limit", jwt?,…}` ok 带票、limit 带 enroll_token+devices;per-IP 120/min)。
- qr_begin 的 server 侧不参与(二维码内容=客户端拼 `https://www.mtdss.top/qr?t=<token>`);token 128bit hex。

- [ ] **Step 1: 失败测试**:全链(claim→确认弹层数据→approve→status ok 带票);二次 claim 异认领者 409/同认领者幂等;approve 后 token 重放=终态;过期;未登录 approve 401;allow=false→denied;limit 路径经同一 enroll 收口。
- [ ] **Step 2: 跑红 → 实现 → 跑绿 + clippy**。
- [ ] **Step 3: Commit** `feat(server): 扫码登录四端点(单次 token+enroll 收口)`

### Task 7: 账号数据 KV(服务端)

**Files:**
- Modify: `server/signaling/src/api.rs`(GET/PUT /data/:key)、`ws.rs`(data_changed 推送复用 push_notice)
- Test: 追加

**Interfaces:**
- Produces:`GET /data/:key → 200 {value, updated_at} | 404`;`PUT /data/:key {value} → {updated_at}`(value ≤256KiB 校验);写后对本账号**其他**在线端 `AuthNotice{kind:"data_changed", data_key}` 尽力推。

- [ ] **Step 1: 失败测试**:CRUD 往返/404 合法态/大小上限 4xx/写后他端收 data_changed(本端不收)。
- [ ] **Step 2: 跑红 → 实现 → 跑绿;Commit** `feat(server): 账号数据 KV+变更通告`

### Task 8: daemon 登录状态机 + IPC 面 + golden REGEN

**Files:**
- Modify: `crates/daemon/src/account.rs`(HTTP 客户端全面:verify 新形态/approve-code/approval-respond/login-status/evict/qr 四端点/kv.get/put)
- Modify: `crates/daemon/src/api.rs`(op 面:login.verify 新形态/login.submit_code/login.status/login.evict/login.cancel/login.qr_begin/login.qr_status/approval.respond/kv.get/kv.put)、`state.rs`/`session.rs`(登录状态机+被踢清理)、`hub.rs`(AuthNotice→IPC 事件+devices_changed 快照刷)
- Modify: `crates/core/src/signaling/ws_connector.rs`(新帧解出上抛;Error code=9 事件化)
- Test: `crates/daemon/tests/`(登录面)+ `crates/ipc/contract/`(REGEN)

**Interfaces:**
- Produces(account.rs):`pub(crate) enum LoginStep { Ok { device_id: String, name: Option<String> }, Approval { session_id: String, expires_at: i64 }, Limit { enroll_token: String, devices: Vec<DeviceBrief> }, Denied, Expired }`(`DeviceBrief { device_id, name, platform, online }`);旧服务器容错:**verify 响应缺 `status` 字段→按 Ok 解析**。
- Produces(IPC,spec §4.2 全表):op 十枚(请求/响应逐字段对表);事件 `event.approval {session_id, candidate_name, candidate_platform, code, expires_at}`、`event.device_evicted {}`、`event.devices_changed {}`;login.request_code 形态不变(dev 回显有则带)。golden:`login.verify.resp.json`(ok)+`.approval`/`.limit` 变体+新 op 各 req/resp,REGEN 幂等;事件 golden 同步。
- 语义:WS Error code=9 → 清会话(identity.seed 保留)+emit event.device_evicted+UI 回登录由客户端消费;login.cancel 作废本地 pending 与服务端会话;qr_begin 参数 `{server, ca?, name}` 镜像 request_code。

- [ ] **Step 1: 失败测试**:daemon e2e(沿既有起服法+dev 门)——新终端审批全链(B 端经 approval.respond 同意 / A 端 submit_code 输码两路);limit→evict→ok;qr 全链(HTTP 直接驱动 claim/approve);被踢端 event.device_evicted+重连拒;Kotlin 断言文件与 Swift 侧由 T12/T13 同步(本任务只 REGEN + Rust 契约测试)。
- [ ] **Step 2: 跑红 → 实现 → 跑绿 + REGEN 幂等 + Windows 全量 + clippy + WSL 面**。
- [ ] **Step 3: Commit** `feat(daemon): 登录状态机(审批/踢选/扫码)+AuthNotice 事件化+KV 透传`

### Task 9: 引擎任务命名两端统一

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(代表名规则)、`flow.rs`/`ctrl responder`(display_name 消费)、`api.rs`(send/orchestrate 携名)
- Test: `crates/daemon/tests/`(e2e 追加)

**Interfaces:**
- Produces:代表名=单文件本名/单目录顶层名/多项「首项名 等 N 项」(≥2 项起后缀);写 `TaskRec.name`(既有字段)+ConnectRequest/OrchestrateRequest `display_name` 下发;**接收端会话行/任务行 name=display_name 非空取之,空回退现状**;transfer.list/事件与 UI 面恒显 name。
- 桌面 CLI(`mtdss status`)任务名同源。

- [ ] **Step 1: 失败测试**:双 daemon e2e 三形态(单文件/单目录/多项)两端 transfer.list name 同串断言(「报告.pdf」「项目备份」「报告.pdf 等 3 项」);旧端不携名(display_name 空)→接收端回退现状不炸。
- [ ] **Step 2: 跑红 → 实现 → 跑绿 + golden 涉名变体 REGEN + 全量**。
- [ ] **Step 3: Commit** `feat(engine): 任务代表名两端同源(display_name 下发)`

### Task 10: Windows UI 登录面 + 审批弹窗 + 乱名收敛

**Files:**
- Modify: `ui/src/components/GuideView.tsx`(QR tab+审批码屏+踢选屏)、`ui/src/App.tsx`(event.approval 模态/event.device_evicted 处理)、`ui/src/lib/login.ts`(状态机纯函数)、`ui/src/components/TransferDrawer.tsx`(恒显 name)
- Test: `ui/src/lib/login.test.ts`(纯函数)+ 既有 vitest 全绿

**Interfaces:**
- Produces:`type LoginUiState = "form" | "qr" | "approval" | "evict"` reducer(纯函数,qr 轮询/过期重生成/取消);审批弹窗=主窗模态(candidate_name/platform/6 位码大字/同意·拒绝);踢选屏=设备清单+「退出登录」取消;QR tab 复用高级选项 server/CA/设备名。TransferDrawer/表格任务名列改 name 直读。
- 文案 zh.json 增量键(审批/踢选/扫码/被移除)。

- [ ] **Step 1: 失败测试**:reducer 全转移(form→qr→approval→evict→form;denied/expired 文案态);**Step 2: 跑红→实现→跑绿+build;目检三态**;**Step 3: Commit** `feat(ui): 登录审批/踢选/扫码面+任务名收敛(V3-1)`

### Task 11: CLI 文本态

**Files:**
- Modify: `crates/cli/src/main.rs`(login 流)+ `Cargo.toml`(dep qrcode)
- Test: `crates/cli/tests/`(IPC 面模拟)

**Interfaces:**
- Produces:`mtdss login` 邮箱流遇 approval→打印「需已登录终端同意,或直接输入对方显示的 6 位审批码:」(stdin 读码/每 3s 轮询 login.status 可 Ctrl+C);遇 limit→编号清单+`输入编号踢出,q 放弃`;`mtdss login --qr`→qrcode crate 渲染 ASCII 码+URL+轮询。CLI 面不新增 IPC op(全走 T8 面)。

- [ ] **Step 1: 失败测试**:伪 daemon(IPC mock,沿既有 cli 测试法)三状态往返+取消;**Step 2: 实现→跑绿;Commit** `feat(cli): 登录文本态(审批码/编号踢选/ASCII 扫码)`

### Task 12: Android 契约同步 + 登录中间态 + 审批弹窗

**Files:**
- Modify: `android/app/src/main/java/com/mtdss/android/ipc/Contract.kt`(+断言)、`ui/login/LoginViewModel.kt`(状态机)、`data/MtdssEventHub.kt`(三事件)、`MainActivity/AppNav`(审批 BottomSheet+踢选屏+被踢回登录)
- Test: `android/app/src/test/…/ContractGoldenTest.kt`、`LoginViewModelTest.kt`

**Interfaces:**
- Consumes: T8 golden 全变体(Gradle 资源挂同一真源)。
- Produces:登录 VM 状态 `Form/Approval/Limit/Done`;审批弹窗 BottomSheet(candidate+码+同意拒绝);踢选屏(清单+退出);event.device_evicted→清会话回登录页。**扫码器不进本任务**(Android UI 随 V3-4 重构统一落,邮箱路径完整可用——计划注记,spec §5 手机扫码器条对 Android 部分顺延至 V3-4)。

- [ ] **Step 1: 失败测试**:ContractGolden 全变体比对+VM 状态转移(含 denied/expired);**Step 2: 实现→gradle testDebugUnitTest 绿+assembleDebug;Commit** `feat(android): 登录审批/踢选中间态+审批弹窗(V3-1)`

### Task 13: iOS 契约同步 + 登录中间态 + 审批弹窗 + 我的页扫码器

**Files:**
- Modify: `ios/package/Sources/MTDSSKit/Ipc/Contract.swift`(+断言)、`Ui/LoginModel.swift`+`SessionModel.swift`(状态机/被踢清理)、`Data/EventHub.swift`(三事件)
- Modify: `ios/app/MTDSS/Profile/{LoginView,ProfileView}.swift`(中间态+审批 Alert;我的页扫码占位点亮=AVFoundation 扫码器)、`ios/app/MTDSS/Browse/DevicePickerSheet.swift`(踢选屏复用其行样式或新 Sheet)
- Test: `ios/package/Tests/MTDSSKitTests/{ContractGoldenTests,LoginFlowTests}.swift`

**Interfaces:**
- Produces:LoginModel 状态同 T12 面;扫码器:相机会话(AVCaptureMetadataOutput,QR)→取 URL `?t=`→repo `qrClaim`→确认 Alert(candidate 名/平台+同意拒绝)→`qrApprove`;权限拒→指引文案(照片权限同款 spirit)。Mac mini `swift test`+模拟器冒烟。

- [ ] **Step 1: 失败测试**:ContractGolden 全变体+LoginModel 状态转移;**Step 2: 实现→swift test 绿;模拟器冒烟(审批弹窗/扫码 claim→approve 链,HTTP 直驱);Commit** `feat(ios): 登录审批/踢选+我的页扫码器(V3-1)`

### Task 14: e2e 全链 + 冒烟/runbook + 终审收官

**Files:**
- Test: `crates/daemon/tests/v3p1_e2e.rs`(跨端编排在 T8/T9 已分置,此处收口复核+补边角:cancel 作废/限流 429 对 daemon 透传文案/qr 过期重生成)
- Create: `docs/smoke-v3p1.md`(§A 服务端限流与邮件 §B 审批链 §C 踢选 §D 扫码 §E 任务名 §F 被踢清理;实机场步含真邮件收件)
- Modify: `docs/runbook-1.1.0.md` 或新 `docs/runbook-v3.md`(§DM 四 env+SPF/DKIM 步、三端先发→服务端后升顺序、`curl request-code` 无回显核验、admin 定值)
- Modify: spec 状态行收官回填

- [ ] **Step 1: 补边角 e2e 全绿 + 三端套件全绿(Windows 全量/WSL/gradle/swift)+ CI 整板绿**。
- [ ] **Step 2: 冒烟/runbook 编制;Commit;整支终审(requesting-code-review)→修复波→复审**。
- [ ] **Step 3: 实机场(用户在场):按 smoke-v3p1 逐项——真邮件收码、双端审批、手机扫码登 PC、踢选、任务名两端一致**。

---

## 执行注

- 顺序刚性:T1→T7 串行(服务端互为前提);T3 必须先于 T4(帧未定不可推);T8 依赖 T4-T7;T9 与 T10-T13 可并行窗口(不同 crate/端);T14 收口。
- 实机步(T13 模拟器/T14)需用户在场协作;子代理到步暂停征召。
- 阿里云 DM 开通/发信地址/SPF/DKIM/AccessKey=运营者前置(T14 实机场前完成,runbook 清单交付)。
