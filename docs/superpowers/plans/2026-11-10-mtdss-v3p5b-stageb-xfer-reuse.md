# MTDSS V3-5 Stage B 传输复用实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 传输作业复用暖连接——同一对设备的第二个传输作业不再打洞(spec §1 Stage B 验收线),在 Stage A 暖池之上给池内直连加 QUIC 多流复用:双向流首字节流类型标签,传输控制流开在暖连接上,数据面 uni 流不动。

**Architecture:** 全部改动落在协商与编排层,传输引擎与 ctrl 帧协议只做「可注入已建流」的等价拆分。**版本闸门走 proto3 只增不改号**(e2e.proto 四帧各加一个 bool,旧端未知字段忽略→缺省 false,与 M3c 定向三字段/V3-1 display_name 同一互通机制):ctrl 编舞(CtrlConnect/CtrlAccept)协商连接级 `mux` 位——两侧都是新端才进标签模式,**tagged 帧仅用于入池的 mux 直连**(中继连接与每作业新拨连接一律裸帧,旧端互通逐字节不变);传输编舞(ConnectRequest/ConnectAccept)协商单次暖复用——发送端有暖直连则置 `warm_reuse`,接收端池内同设备有活 mux 直连则回 `warm_ok` 并在暖连接上等带 `TAG_TRANSFER` 的控制流(经 CtrlPool 旁的槽登记簿 rendezvous),发送端跳过打洞段直接 `send_files` 开流。单连接单活跃传输轮:发送端 per-device try-lock(占用即回退打洞),接收端槽占用回 `warm_ok=false`。

**Tech Stack:** Rust(tokio/quinn/prost),既有测试设施:core 环回 QUIC 对(endpoint.rs 测试房风)+ daemon 进程内 Pair e2e(ctrl_e2e.rs 样板)+ signaling::spawn_test_server。

**Spec:** `docs/superpowers/specs/2026-11-09-mtdss-v3p5-tunnel-design.md`(§1 Stage B 验收线、§3.2 方向稿、§7 留给本计划的三面:流类型字节值/版本闸门形态/serve 统一分派)。Stage A 已合并(main@ce9c49c)。

## Global Constraints

- **验收线(spec §1)**:同一对设备连续两个传输作业,第二个作业无打洞编舞(日志/计数验证)。
- **流类型字节值**:双向流首字节,`TAG_CTRL = 0x01`、`TAG_TRANSFER = 0x02`(transport::mux 单一真源);**tagged 帧仅用于入池的 mux 直连**——中继连接、每作业新拨连接、decline 窗连接上的**发起**流一律裸帧;数据面 `open_uni` 维持传输独占、帧格式不动(spec §3.2)。
- **proto 增量**:e2e.proto 四字段——`ConnectRequest.warm_reuse = 6`、`ConnectAccept.warm_ok = 2`、`CtrlConnect.mux = 2`、`CtrlAccept.mux = 2`,全部 bool 缺省 false;signal.proto(ws 帧)/manifest.proto/ctrl.proto **一字不动**;`contract_golden` 零漂移(IPC 面无改动)。
- **版本闸门语义**:发起端见 accept 缺 warm_ok/mux → 回退现状逐作业打洞;响应端见 connect 缺 warm_reuse/mux → 现状握手;旧端把新字段当未知字段忽略(proto3)。新端永不因协商失败拒绝服务——协商只影响走哪条路,不影响成败。
- **中继兜底不变(spec §3.2)**:暖复用仅 P2P 直连(池条目 `relay=None` 才资格);中继会话按操作建立、无心跳 60s 自灭、不入 warmer——中继路径行为逐字节不变。
- **调优合并(spec §3.2)**:暖连接沿用 ctrl 调优(15s 心跳/60s 空闲/MTU 1452);传输直连打洞默认调优 MTU 同 1452(`punch::PunchConfig::default().tuning.max_mtu`),无冲突——以断言测试钉死,不做任何合并代码。
- **单连接单活跃传输轮**:发送端 per-device try-lock(try 失败即回退打洞,不等待);接收端槽已占用回 `warm_ok=false`;mux serve 收到无槽的 `TAG_TRANSFER` 流 → warn + 关流(不挂死、不崩连接)。
- **有界等待**:接收端暖轮等控制流移交 `WARM_XFER_HANDOFF_TIMEOUT = 15s`(发送端 warm 路径无 STUN/打洞,Accept 后只剩 open_bi+写清单),超时按等待阶段失败收口(匿名卡 failed / 下载绑定轮保持 running),常驻循环绝不挂死。
- **登出/被踢**:`state.logout` 既有 `ctrl.reset` 不动;暖传输轮/槽随代际自然失效(槽登记簿按请求轮生命周期清位)。
- **测试注入面**:`fault_disable_warm_xfer`(SendOpts 字段,沿 `fault_force_relay` 房风,Default=false 即暖复用默认开);真实二进制经 `MTDSS_TEST_DISABLE_WARM_XFER=1` env(main.rs `apply_test_env` 族);进程内 Pair 经 `Daemon::start_with_warm_xfer_disabled`(doc(hidden),沿 `start_with_force_relay` 先例)。DaemonOpts **零新字段**。
- **零 UI/平台代码改动**:Windows/Android/iOS 胶合层零触碰;任务卡片/事件面零改动(relay 标记逻辑:暖路径恒直连,on_relay 不触发)。
- 测试口径:cargo(本机 Windows 全量 + WSL `MSYS_NO_PATHCONV=1 wsl.exe env CARGO_TARGET_DIR=/root/t2-target /root/.cargo/bin/cargo test --workspace`);分支 `v3p5b`(worktree 执行);netns 矩阵若 WSL 可跑则跑,不可跑记冒烟场代跑。

## Review Focus

(spec 隐含但任务测试未全覆盖、最可能咬人的输入类;每条钉到 owning 任务)

1. **中继连接上的 tagged 误发**:新新对端的 ctrl 兜底连接走中继时,协商位 mux=true 但响应端中继臂只挂裸 `serve_conn`——发起端若按协商位发 tagged 帧即互相解析打架。规则「tagged 仅入池直连」钉死:T3 的 `warm_insert` 入池位取 `mux && relay.is_none()`,T3 单测钉该合成式。
2. **decline 窗连接收到 TAG_TRANSFER**:发送端暖检查通过后,接收端池内可能是另一条物理连接(双侧并发拨的竞态分歧窗)——发送端开流的连接在接收端处于 decline 限时服务窗。decline 路径的 serve 必须与入池连接同一 mux 分派(否则 tagged 流被裸 serve 误读)——T3 统一 serve 入口覆盖 decline 臂,T4 槽按**设备**键(不按连接键)接收任一 mux 连接的移交。
3. **warm_ok=true 后流永不到达**(发送端 Accept 后死):接收端轮必须超时失败而非挂死常驻循环——T4 的 handoff select(超时/取消/登录翻转)钉住;后续靠发送端重试自愈。
4. **旧端四帧旧形态**:ConnectRequest/ConnectAccept/CtrlConnect/CtrlAccept 无新字段解出 false → 全部回退现状——T1 的 legacy-default 单测钉解码;端到端回退臂由 T5 的 disable 旋钮 e2e(走完整打洞路径成功)代钉,真旧端互通进冒烟场(T6,新旧混跑)。
5. **同设备并发两作业**:第一作业持暖连接(含暂停挂起时),第二作业 try-lock 失败必须回退打洞且两作业都成功——T5 的并发用例(暂停第一作业制造确定性占用)钉住。

---

### Task 1: proto 四字段 + 信令体扩展(互通映射)

**Files:**
- Modify: `proto/proto/e2e.proto`(四消息各加 bool 字段,字段号见 Global Constraints;注释沿 M3c「只增不改号」体例写明互通语义)
- Modify: `crates/core/src/signaling/envelope.rs`(SignalBody 四变体加字段 + `body_to_proto`/`body_from_proto` 双向映射)
- Modify: 全部穷举匹配/构造点编译修复(grep `SignalBody::CtrlConnect|SignalBody::CtrlAccept|SignalBody::ConnectRequest|SignalBody::ConnectAccept`:`crates/daemon/src/ctrl.rs`、`crates/daemon/src/flow.rs`、`crates/daemon/src/hub.rs`、envelope.rs 自身;**此任务一律填 `false`/`..`,不接线**——接线在 T3/T5)
- Test: `crates/core/src/signaling/envelope.rs` tests

**Interfaces:**
- Produces: `SignalBody::ConnectRequest { session_id, target_dir, job_id, display_name, policy, warm_reuse: bool }`;`ConnectAccept { session_id, warm_ok: bool }`;`CtrlConnect { session_id, mux: bool }`;`CtrlAccept { session_id, mux: bool }`。proto 字段号:`warm_reuse=6`/`warm_ok=2`/`CtrlConnect.mux=2`/`CtrlAccept.mux=2`。
- Consumes: 无(首任务)。

- [x] **Step 1: 写失败测试**(envelope.rs tests,沿既有 seal/open 测试房风)

```rust
// new_signal_fields_roundtrip_and_legacy_default:
// 1) 四体新字段置 true,seal→open(两把固定种子身份)逐字段断言往返保真;
// 2) 旧形态:手工 prost 编码 BodyProto(消息不带新字段,即旧端线上形态)
//    → open 解出 warm_reuse/mux/warm_ok 恒 false——版本闸门的解码地基。
```

- [x] **Step 2: 跑测确认失败** — `cargo test -p mtdss-core signaling` → 编译错(字段不存在)。
- [x] **Step 3: 实现**——e2e.proto 四字段 + 注释;envelope.rs 变体扩字段 + 双向映射;全仓穷举点机械补 `false`/`..`(本任务零行为差:新端发旧形态,收旧形态)。
- [x] **Step 4: 跑测通过** — `cargo test -p mtdss-core` + `cargo test -p mtdss-daemon --lib`(编译修复面回归);`cargo test -p mtdss-daemon --test contract_golden` 零漂移确认。
- [x] **Step 5: Commit** `feat(v3p5b): 信令四帧增暖复用/多流协商字段——proto3 只增不改号,旧端互通无感`

### Task 2: core 传输层标签原语(引擎等价拆分 + tagged 入口)

**Files:**
- Create: `crates/core/src/transport/mux.rs`(标签常量)
- Modify: `crates/core/src/transport/mod.rs`(mod mux)
- Modify: `crates/core/src/transport/transfer.rs`(`send_files` 拆共享体;`send_files_tagged`;`recv_files_body` 拆控制流注入变体)
- Modify: `crates/core/src/transport/ctrl.rs`(pub 帧原语 + `send_request_tagged`)
- Test: `crates/core/src/transport/transfer.rs` tests(环回 QUIC 集成)+ `ctrl.rs` tests

**Interfaces:**
- Produces:
  - `transport::mux`: `pub const TAG_CTRL: u8 = 0x01;` `pub const TAG_TRANSFER: u8 = 0x02;`
  - `transfer::send_files(conn, man, sources, progress, opts)`(签名/行为不变:open_bi 裸流 → 共享体)
  - `transfer::send_files_tagged(conn, man, sources, progress, opts) -> Result<SendReport, SendError>`(open_bi → 写 `TAG_TRANSFER` 一字节 → 同一共享体)
  - `#[doc(hidden)] transfer::recv_files_on_ctl(conn: &Connection, ctl: (SendStream, RecvStream), dest: Arc<dyn DestIo>, ctl: &RecvCtl) -> RecvOutcome`(控制流由调用方移交;`conn` 仍归引擎做 `accept_uni` 与对端证书读取;内部 `recv_files_body` 拆出「accept_bi 之后」的体,既有入口 = accept_bi + 体,逐字节等价)
  - `transport::ctrl::send_request_tagged(conn, req, timeout) -> Result<CtrlResponse>`(open_bi → 写 `TAG_CTRL` → 既有帧舞步)
  - `transport::ctrl::read_request_frame(rx: &mut RecvStream) -> Result<CtrlRequest>` / `write_response_frame(tx: &mut SendStream, resp: &CtrlResponse) -> Result<()>`(read_frame/write_frame 的 pub 薄包,daemon 侧 mux serve 消费)
- Consumes: 无(独立于 T1——标签是传输层常量,不依赖信令)。

- [x] **Step 1: 写失败测试**(transfer.rs tests;环回 QUIC 一条连接,endpoint.rs 测试房风 `make_dual_endpoint` + 固定种子身份)

```rust
// tagged_transfer_roundtrip_on_one_conn: 同一条环回连接上,一侧
// send_files_tagged(单文件、固定字节),另一侧 accept_bi → 读首字节
// 断言 == TAG_TRANSFER → recv_files_on_ctl(移交该流)→ 落盘字节一致、
// RecvOutcome.paths 非空。暖复用的引擎层全链路锚。
// ctrl.rs: send_request_tagged_roundtrip: 同款环回,对端 accept_bi →
// 首字节断言 == TAG_CTRL → read_request_frame → write_response_frame →
// 发起端拿到响应。
// 调优合并锚(spec §3.2): 断言 punch::PunchConfig::default().tuning.max_mtu
//   == ctrl 调优 max_mtu(1452)——暖连接沿用 ctrl 调优无 MTU 冲突的证据面。
```

- [x] **Step 2: 跑测确认失败** — `cargo test -p mtdss-core transport` → 编译错(符号未定义)。
- [x] **Step 3: 实现**——mux.rs 两常量;transfer.rs 把 `send_files` 的 open_bi 之后段提为内部 `send_files_on(ctl_tx, ctl_rx, conn, …)`、控制流接受之后段提为内部 `recv_files_body_on_ctl(conn, ctl, …)`(纯搬移,逻辑零改);ctrl.rs 薄包 + tagged 变体。
- [x] **Step 4: 跑测通过** — `cargo test -p mtdss-core`(新用例 + 既有传输/帧测试全绿)+ `cargo test -p mtdss-daemon --test jobs_e2e`(引擎等价拆分的端到端回归,裸路径)。
- [x] **Step 5: Commit** `refactor(v3p5b): 传输引擎控制流可注入 + 双流类型标签入口(裸路径逐字节等价)`

### Task 3: 池连接 mux 协商 + 统一 serve(含 decline)+ ctrl 按模式走 tagged

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(PooledConn.mux;dial_session 协商与返回;respond_session 协商;warm_insert 签名;request/ensure_warm 入池与发送选择;spawn_initiator_serve 签名;`warm_mux`/`warm_xfer_conn`/`try_xfer_lock`;统一 serve 分流)
- Create: `crates/daemon/src/mux.rs`(此任务先立 `serve_conn_mux` 的 **ctrl 臂**;TAG_TRANSFER 臂 T4 接管,先 warn+关流占位)
- Modify: `crates/daemon/src/lib.rs`(mod mux;`Daemon::ctrl_warm_mux` doc(hidden) 直驱面;`ctrl_dial_fresh` 适配 dial_session 四元组返回)
- Test: `crates/daemon/src/ctrl.rs` tests + `crates/daemon/tests/ctrl_e2e.rs`

**Interfaces:**
- Produces:
  - `PooledConn { _keep, conn, relay, mux: bool }`;`warm_insert(device, ep, conn, relay, mux: bool) -> bool`(入池 mux 位 = **协商位 ∧ relay.is_none()**,合成式单测钉死——Review Focus 1)
  - `dial_session(state, device, allow_relay) -> Result<(quinn::Endpoint, quinn::Connection, Option<fallback::RelayPath>, bool /*协商 mux*/), CtrlErr>`(CtrlConnect 携 `mux: true`;CtrlAccept 解 `mux`——对端旧则 false)
  - `respond_session(state, from, session_id, mux: bool)`(responder_loop 从 CtrlConnect 解出传递);直连入池带 mux;**decline 臂与入池臂同一 serve 分流**(Review Focus 2)
  - `fn serve_pooled(conn, state, device, mux)`(模块内分流:mux → `mux::serve_conn_mux`,false → `transport::ctrl::serve_conn` 既有裸 serve——中继臂/旧端连接走这里,行为逐字节不变)
  - `spawn_initiator_serve(conn, state, device, mux)`
  - `CtrlPool`: `fn warm_mux(&self, device) -> Option<bool>`;`fn warm_xfer_conn(&self, device) -> Option<quinn::Connection>`(活 ∧ mux ∧ 直连 → 克隆;T5 暖闸消费);`fn try_xfer_lock(&self, device) -> Option<tokio::sync::OwnedMutexGuard<()>>`(新 `xfer_locks` 表,与 `locks` 分离——传输占用不阻塞浏览/打洞单飞)
  - `mux::serve_conn_mux(conn, state, device)`:accept_bi 循环读首字节——`TAG_CTRL` → spawn `read_request_frame` → `handle_ctrl_request` → `write_response_frame`;`TAG_TRANSFER` → T3 占位 warn+drop;其余字节 warn+drop
  - `Daemon::ctrl_warm_mux(device) -> Option<bool>` doc(hidden)
- Consumes: T1(SignalBody 字段)、T2(TODO 常量与帧原语)。

- [x] **Step 1: 写失败测试**

```rust
// ctrl.rs tests: pooled_mux_flag_requires_direct:
//   断言合成式——(协商 true, relay=Some) → 入池位 false;(true, None) → true;
//   (false, None) → false。(纯函数化:入池位计算提为 fn pool_mux(negotiated: bool, relay_is_some: bool) -> bool)
// ctrl_e2e.rs: warm_conn_negotiated_mux:
//   Pair 常规(默认调优),A 对 B list 成功后断言
//   A.ctrl_warm_mux(B) == Some(true) 且 B.ctrl_warm_mux(A) == Some(true)
//   (新新对端即 mux;轮询 ≤10s 容纳 warmer 建连,200ms 步距)。
```

- [x] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib pool_mux` 编译错;`--test ctrl_e2e warm_conn_negotiated_mux` FAIL(`ctrl_warm_mux` 不存在)。
- [x] **Step 3: 实现**——dial_session 发 `CtrlConnect{mux:true}`、CtrlAccept 匹配臂解 `mux` 并随返回四元组带出;respond_session/responder_loop 对向接线;`request()` 池命中臂与两处新拨臂按「条目 mux(直连)→ `send_request_tagged`,否则 `send_request`」选择,验证缝重发臂同款;入池点三处(request 新拨收口/ensure_warm/respond_session)全走新 `warm_insert`;serve 挂载点三处(request 收口/ensure_warm/respond_session 含 decline)统一 `serve_pooled`。
- [x] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --lib` + `--test ctrl_e2e` **全量**(新新 ctrl 全走 tagged 的最大回归面:既有 warm/decline/验证缝/硬杀用例必须全绿)+ `--test daemon_e2e`。
- [x] **Step 5: Commit** `feat(v3p5b): ctrl 连接级 mux 协商——池内直连走流类型标签,旧端/中继/decline 统一裸 serve`

### Task 4: 接收端暖传输轮(槽登记簿 + mux serve 分派 + 常驻循环分支)

**Files:**
- Modify: `crates/daemon/src/mux.rs`(`XferSlots` + `XferHandoff` + TAG_TRANSFER 臂 + `WARM_XFER_HANDOFF_TIMEOUT`)
- Modify: `crates/daemon/src/state.rs`(`xfer_slots: mux::XferSlots` 字段 + 访问器)
- Modify: `crates/daemon/src/flow.rs`(IncomingRequest 透传 warm_reuse;resident_recv_loop 暖分支;落地收口共用重构)
- Test: `crates/daemon/src/mux.rs` tests

**Interfaces:**
- Produces:
  - `pub(crate) struct XferHandoff { conn: quinn::Connection, ctl: (quinn::SendStream, quinn::RecvStream) }`
  - `pub(crate) struct XferSlots(Mutex<HashMap<String, oneshot::Sender<XferHandoff>>>)`;`register(&self, device) -> Option<oneshot::Receiver<XferHandoff>>`(已占用 → None);`take(&self, device) -> Option<oneshot::Sender<XferHandoff>>`(serve 侧消费,取走即清位)
  - `pub(crate) const WARM_XFER_HANDOFF_TIMEOUT: Duration = Duration::from_secs(15);`
  - `serve_conn_mux` TAG_TRANSFER 臂:`state.xfer_slots().take(&device)` → Some(tx) → `tx.send(XferHandoff{conn: conn.clone(), ctl})`(接收方已弃 → Err 丢弃+warn);None → warn + drop 流
  - resident_recv_loop 暖分支(轮装配——目录解析/作业绑定/RecvCtl 全部照旧——之后):
    `warm_reuse && state.ctrl().warm_xfer_conn(&from).is_some() && slots.register(&from).is_some()` → 回 `ConnectAccept{warm_ok:true}` → `select! { handoff=rx + timeout(WARM_XFER_HANDOFF_TIMEOUT), 取消沿, 登录翻转 }` → Ok → `recv_files_on_ctl(handoff.conn, handoff.ctl, dest, ctl)` 包进 RoundLanded → **汇入既有 landed 收口段**(通知/卡片/发布/终态分流逐行不动);任一失败/占用 → 回 `ConnectAccept{warm_ok:false}` → 落既有 `recv_handshake` 全套(现状路径)。**重构纪律**:握手 match 两臂只产出 landed,收口段单份共用(Review Focus 3 的超时臂与既有等待失败臂同走「匿名卡 failed / 下载绑定保持 running」)。
- Consumes: T2 `recv_files_on_ctl`、T3 `warm_xfer_conn`/serve 分流。

- [x] **Step 1: 写失败测试**(mux.rs tests,纯同步单测)

```rust
// xfer_slots_register_take_occupy: register("d") Some;再 register("d") None(占用);
// take("d") Some 且取走后再 take None;take 后 register 又 Some(清位复用)。
// handoff_timeout_anchor: WARM_XFER_HANDOFF_TIMEOUT == 15s(改值须过本断言)。
```

- [x] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib mux` → 编译错(模块符号缺失)。
- [x] **Step 3: 实现**——按 Interfaces;resident 分支的 landed 共用按重构纪律执行。
- [x] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --lib` + `--test ctrl_e2e` + `--test jobs_e2e`(此任务无新 e2e 驱动面——发送端 T5 才置位 warm_reuse;回归面=常驻循环重构零行为差)。
- [x] **Step 5: Commit** `feat(v3p5b): 接收端暖传输轮——槽登记簿 rendezvous,暖连接上收带标签控制流(有界等待)`

### Task 5: 发送端暖臂 + 计数观测面 + e2e 收口

**Files:**
- Modify: `crates/daemon/src/flow.rs`(SendCtx 增 state;run_send_with 增 state 参;send_round 暖臂;`fault_disable_warm_xfer` 注入;计数)
- Modify: `crates/daemon/src/api.rs:940` 与 `crates/daemon/src/jobs.rs:4110`(run_send_with 两调用点补参)
- Modify: `crates/daemon/src/state.rs`(`xfer_dials: AtomicU64` + `note_xfer_dial()`)
- Modify: `crates/daemon/src/lib.rs`(`Daemon::start_with_warm_xfer_disabled` doc(hidden);`Daemon::transfer_dial_count()` doc(hidden))
- Modify: `crates/daemon/src/main.rs`(env `MTDSS_TEST_DISABLE_WARM_XFER=1` → send opts 推导,沿 fault_force_relay 注入路径)
- Test: `crates/daemon/tests/ctrl_e2e.rs`(三用例)+ `crates/daemon/tests/relay_e2e.rs`(一用例)

**Interfaces:**
- Produces:
  - `SendOpts { .., #[doc(hidden)] fault_disable_warm_xfer: bool }`(Default=false=暖复用开;沿 fault_force_relay 房风)
  - `run_send_with(state: &Arc<DaemonState>, session, home, hub, …)`(hub 可由 state 派生但保留参数,最小 diff;两调用点补 state)
  - send_round 暖臂(编舞顺序,把既有函数重排为四段):①暖资格判定(`!fault_disable_warm_xfer` ∧ `try_xfer_lock(to)` 拿到 ∧ `warm_xfer_conn(to)` Some)→ 决定 ConnectRequest 携 `warm_reuse: true/false`;②发 ConnectRequest → 等 ConnectAccept(既有段);③accept 解 `warm_ok`:true → `send_files_tagged(conn, …)`(进度/暂停装配照旧,relay 恒 None 无死亡通告臂)→ 返回(guard 随臂 drop);false → **落入④既有打洞尾段**(Accept 后的端点/候选/connect_with_fallback/send_files 原样——**不重发 ConnectRequest**,接收端 warm_ok=false 时本就走既有握手,双方在同一 sid 上对齐);④暖发送失败(Network 类,连接死)→ 同样落④当轮重打(暖臂失败→打洞段即整轮的重打臂,与既有「恰一次」重试纪律合流;Rejected/Protocol 类终局直接上抛,语义不变)。
  - `DaemonState.note_xfer_dial()`:`send_round` 打洞段入口与 `recv_handshake` 入口各 +1(暖臂/池命中不计——计数即「真打过洞」观测面,同 ctrl dial_count 口径)
  - `Daemon::transfer_dial_count() -> u64`、`Daemon::start_with_warm_xfer_disabled(home, opts)`(doc(hidden) 一行包装)
- Consumes: T1 warm_reuse/warm_ok 字段、T2 send_files_tagged、T3 try_xfer_lock/warm_xfer_conn、T4 接收端。

- [x] **Step 1: 写失败 e2e**(ctrl_e2e.rs,Pair 样板;上传驱动沿 jobs_e2e:371 形态)

```rust
// warm_xfer_second_job_no_punch(验收线 e2e 锚): Pair 默认调优,轮询双测
//   ctrl_warm_state == Some(false) 就绪(沿 warmer_establishes 房风);记
//   a0=A.transfer_dial_count();A→B 传 file1(transfer.start + transfer.wait
//   终态 done);再传 file2;断言:B 侧两文件落盘字节一致;断言
//   A.transfer_dial_count() == a0 且 B 侧计数不变(两作业零打洞——第二作业
//   免打洞即验收线)。
// warm_xfer_disabled_falls_back_to_punch: start_with_warm_xfer_disabled 起
//   Pair;两次上传均成功落盘;断言 transfer_dial_count 两侧各 +2
//   (每作业两侧各一次编舞)——回退臂 = 现状逐作业打洞,正确性不依赖暖。
// warm_xfer_concurrent_second_punches: 起 A→B 上传 job1 → 立即
//   transfer.pause(job1)(持锁挂起)→ 起上传 job2 → 断言计数 +≥1(job2
//   try-lock 失败回退打洞)→ resume(job1) → 两作业都 done、两文件落盘。
// relay_e2e.rs: relay_transfers_never_warm: start_with_force_relay Pair,
//   两次上传均成功;断言 transfer_dial_count 每次递增(中继会话按操作建立,
//   暖复用绝不触及中继)。
```

- [x] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --test ctrl_e2e warm_xfer` → FAIL(暖臂未实现,第二作业打洞计数 +1 / 旋钮用例计数为 0)。
- [x] **Step 3: 实现**——按 Interfaces;注意暖臂结构:暖失败(连接死)映射既有 `SendError::Network` 后落入打洞段当轮重打,`Rejected/Protocol` 类暖失败直接上抛(终局语义不变)。
- [x] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --test ctrl_e2e` + `--test relay_e2e` + `--test jobs_e2e` 全绿。
- [x] **Step 5: Commit** `feat(v3p5b): 发送端暖臂——暖直连上开传输流跳过打洞,占用/旧端/中继全回退,验收线 e2e 锚`

### Task 6: 冒烟清单 Stage B 节 + 全绿收口

**Files:**
- Modify: `docs/smoke-v3p5.md`(追加 Stage B 节)
- Modify: 本计划勾选

**Interfaces:**
- Consumes: T1–T5 全部。

- [x] **Step 1: 写冒烟清单**——spec §1 Stage B 验收线逐条落成实机场步骤:同对设备连续两传输作业第二作业无打洞(RUST_LOG 观察编舞缺席 + 计数面)/ 大文件传输中暂停恢复(暖连接 keep-alive 保活,恢复续传免重打)/ 传输中浏览对端目录(同连接混流,ctrl 与数据流互不干扰)/ 传输中断网自愈(暖臂 Network 落打洞段,重试成功)/ 中继路径两作业均按操作建立(force relay 或 NAT 差网场)/ **新旧混跑**:Stage B 端 ↔ 旧端(1.2.x)双向传输+浏览全通(版本闸门回退)、Stage B ↔ Stage A 端传输照旧逐作业打洞(ctrl 已暖)/ 移动端(MIX 3/iPhone)收发各一次;场:PC(本机)↔ MIX 3 ↔ iPhone(TestFlight 构建,Mac mini 直传档凭据见项目记忆)。
- [ ] **Step 2: 全量验证** — 本机 `cargo test --workspace`;WSL 全量(命令见 Global Constraints);`contract_golden` 单列零漂移确认;ui `npm test && npx tsc --noEmit && npm run build`(零改动回归);`gradlew.bat :app:testDebugUnitTest`(零改动回归);netns 矩阵若 WSL 可跑则跑(暖路径下 chaos 断链注入重点),不可跑记冒烟场代跑。
- [x] **Step 3: Commit** `docs: smoke-v3p5 Stage B 冒烟清单 + 传输复用收口`

---

## 执行顺序与闸门

- T1→T2 并行可行(T1 信令层/T2 传输层互不依赖);T3 依赖 T1+T2;T4 依赖 T2+T3;T5 依赖全部;T6 收口。
- 全部合并 + 实机场(spec §1 Stage B 验收线,含新旧混跑)= **V3-5 全波收官**;版本 bump/打包归 V3-6(1.3.0)。
- 若实机发现暖复用在极端 NAT/弱网下劣化(如静默死连导致暖臂比打洞更慢),回spec §1 预案:暖臂失败→打洞段的结构已内置当轮重打,无需回滚面。
