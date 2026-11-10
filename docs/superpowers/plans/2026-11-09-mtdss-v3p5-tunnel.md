# MTDSS V3-5 长连接保活 Stage A 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ctrl 浏览通道暖连化——keep-alive 保活 + 登录/回前台预热 + 断连自愈 + 大目录超时不拆连接,进目录不再逐次打洞。

**Architecture:** 全部落在 daemon 引擎层(`crates/daemon`):CtrlPool 持有直连调优(默认开心跳)、打洞编舞抽成 `dial_session`(带 direct-only 变体给预热)、响应端 accept 的直连入池(留活弃新)、新增 `warm.rs` 预热器常驻任务(事件驱动 + 退避/冷却)、hub 加回前台快探。**零协议变更**(信令三帧/QUIC 互拨/流框架全部不动,golden 不动),旧端互通无感;中继路径维持无心跳 60s 自灭。

**Tech Stack:** Rust(tokio/quinn),既有测试设施:daemon 进程内 Pair e2e(`crates/daemon/tests/ctrl_e2e.rs` 样板)+ `signaling::spawn_test_server`。

**Spec:** `docs/superpowers/specs/2026-11-09-mtdss-v3p5-tunnel-design.md`(spec 随计划同行;本计划只覆盖 **Stage A**,Stage B 传输复用在 Stage A 实机闸门后另出计划,spec §3.2)。

## Global Constraints

- **零协议变更**:信令消息(CtrlConnect/CtrlAccept/Candidates)、QUIC 互拨、流框架一字不动;`contract_golden` 必须零改动通过。
- **直连默认调优**:`keep_alive_interval = Some(15s)`、`max_idle_timeout = 60s`、`max_mtu = 1452`(spec §3.1(1))。
- **中继路径调优**:`keep_alive_interval = None`(mtu 1400 维持)——中继连接 60s 空闲自灭,**绝不保活**(spec §3.1(5),防服务器配额吃穿)。
- **warm 单侧主动**:仅本机公钥字节序 < 对端公钥的设备对,本机才预热/重打(spec §3.1(3));预热编舞 **direct-only**(打不通即失败,不走中继兜底,spec §3.1(4))。
- **池语义**:每设备恰一条;入池**留活弃新**(池内有活连接则新连接只服务当次,不入池);decline 后新连接限时服务 `WARM_DECLINE_GRACE = 90s` 再关(spec §3.1(3))。
- **重打退避**:1/2/4…封顶 30s;连续失败进冷却 `WARM_COOLDOWN = 5min`;冷却在 presence 翻转/代际更替时复位(spec §3.1(2))。
- **请求失败语义**:仅连接级错误(连接死)驱逐+整程重试恰一次;**请求超时不拆连接、错误照常上抛**(spec §3.1(6))。
- **WS 快探**:任何 IPC dispatch 时,若在线且最近入站帧 >25s(> 信令 20s ping 周期),立即拆当前代际触发重连(spec §4 回前台快探)。
- **测试注入面**:调优/等待数值可注入(e2e 经 `DaemonOpts` 直填;真实二进制经 `MTDSS_TEST_CTRL_KEEPALIVE_MS`/`MTDSS_TEST_CTRL_IDLE_MS` env,沿 `apply_test_env` 房风,非法值 warn 保持默认)。
- **登出/被踢**:`state.logout` 既有 `ctrl.reset` 不动;warmer 一切动作前必须 `state.logged_in()` 闸住。
- **不引入**平台代码/前台服务/后台保活;Android/iOS 零改动(Stage A)。
- 测试口径:cargo(本机 Windows 全量 + WSL `MSYS_NO_PATHCONV=1 wsl.exe env CARGO_TARGET_DIR=/root/t2-target /root/.cargo/bin/cargo t …`);分支 `v3p5`(worktree 执行)。

## Review Focus

(评审重点:spec 隐含但任务测试未全覆盖、最可能咬人的输入类;每条已钉到 owning 任务)

1. **登出后的 warmer 重打**:池连接随 `ctrl.reset` 死亡,death watcher 若不查 `logged_in` 会对已清会话空烧打洞——T5 的 watcher 闸门测试钉住。
2. **老发起端(旧版对端)反复打洞**:新响应端池内有活连接时逐条 decline,decline 连接若不限时关闭会凭我侧 keep-alive 永生,与新池连接并存成双连——T3 的 decline grace 测试钉住。
3. **warmer 对离线/未知公钥设备**:presence 快照里 online=false 或公钥缓存未命中的设备不得发起预热(对离线设备空烧信令/打洞必败)——T5 targets 计算测试钉住。
4. **自拨防御**:warmer 目标集必须排除本机 device_id(responder_loop 已有 from==self 跳过,但 warmer 侧不设防就是发起侧自拨)——T5 targets 计算测试钉住。
5. **挂起恢复后的死连接显形**:keep-alive 开启不改变「连接死→conn_alive 判死→驱逐重打」路径;移动端挂起恢复后首次浏览命中的若是池内僵尸,必须快速显形而非挂满 15s——T4 的 classify 决策测试(死连接→驱逐重试)钉住;真机行为进冒烟清单 T7。

---

### Task 1: ctrl 调优值化(直连心跳默认开 / 中继路保持无心跳)

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(`CTRL_TUNING` const → `CtrlPool` 字段;两处引用改读池)
- Modify: `crates/daemon/src/fallback.rs:305` 附近(relay 互拨的 PunchConfig 覆写处,keep_alive 置 None)
- Modify: `crates/daemon/src/relay_sock.rs`(中继端点 transport_config 用无心跳调优)
- Modify: `crates/daemon/src/state.rs:222`(`CtrlPool::new` 带调优入参)
- Modify: `crates/daemon/src/lib.rs`(DaemonOpts 增 `ctrl_tuning: Option<mtdss_core::transport::endpoint::TransportTuning>`,None=默认;`Daemon::start` 传入;main.rs 读 `MTDSS_TEST_CTRL_KEEPALIVE_MS`/`MTDSS_TEST_CTRL_IDLE_MS` env 填充,沿 `apply_test_env` 房风)
- Modify: 全部 `DaemonOpts {` 字面量构造点(grep `DaemonOpts {`,tests/*.rs 与 ios-bridge/android 胶合处,补 `ctrl_tuning: None`)

**Interfaces:**
- Produces: `CtrlPool::new(tuning: TransportTuning)`;`pub const fn ctrl_default_tuning() -> TransportTuning`(keepalive 15s/idle 60s/mtu 1452);`pub const fn ctrl_relay_tuning() -> TransportTuning`(keepalive None/idle 60s/mtu 1400);`DaemonOpts.ctrl_tuning`(pub 字段)。
- Consumes: 无(首任务)。

- [ ] **Step 1: 写失败测试**(ctrl.rs tests 模块)

```rust
#[test]
fn direct_tuning_heartbeat_on_relay_off() {
    let d = ctrl_default_tuning();
    assert_eq!(d.keep_alive_interval, Some(Duration::from_secs(15)));
    assert_eq!(d.max_idle_timeout, Duration::from_secs(60));
    assert_eq!(d.max_mtu, 1452);
    let r = ctrl_relay_tuning();
    assert_eq!(r.keep_alive_interval, None, "中继连接绝不心跳保活(配额)");
    assert_eq!(r.max_mtu, 1400);
}
```

- [ ] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib ctrl` → 编译错(函数未定义)。
- [ ] **Step 3: 实现**——按 Interfaces 两函数 + `CtrlPool` 增 `tuning` 字段(`punch_via_hub`/`respond_session` 里 `CTRL_TUNING` 引用点全部改 `self.tuning`/`state.ctrl().tuning()`);fallback 中继互拨 PunchConfig 用 `ctrl_relay_tuning()` 覆写;relay_sock 端点同。删除旧 const。
- [ ] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --lib` 全绿 + `cargo test -p mtdss-daemon --test ctrl_e2e`(回归,行为零差)。
- [ ] **Step 5: Commit** `feat(v3p5): ctrl 调优值化——直连心跳默认开、中继路保持无心跳`

### Task 2: dial_session 抽取(打洞编舞收敛 + direct-only 变体)

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(`punch_via_hub` 重构为 `dial_session`,增 `allow_relay: bool`)

**Interfaces:**
- Produces: `impl CtrlPool { async fn dial_session(&self, state: &Arc<DaemonState>, device: &str, allow_relay: bool) -> Result<(quinn::Endpoint, quinn::Connection, Option<fallback::RelayPath>), CtrlErr> }`——`allow_relay=false` 时互拨段直调 `punch::punch_connect`(不走 `connect_with_fallback`),失败映射 `CtrlErr::Punch`;返回三元组含中继标记(直连为 None)。
- Consumes: T1 调优字段。

- [ ] **Step 1: 失败测试**(ctrl.rs tests;直调分发面不可达打洞栈,钉的是分流逻辑——`dial_session` 编舞无法进程内直测,以回归 e2e 钉)

ctrl_e2e.rs 增用例:`request` 路径(dial_session(allow_relay=true))行为与既有完全一致——直接复跑既有套件即可,不新增用例;新增单测钉 `allow_relay` 分流判定:把「互拨段选择器」抽纯函数 `fn punch_selector(allow_relay: bool) -> PunchMode { if allow_relay { PunchMode::WithFallback } else { PunchMode::DirectOnly } }` 并测两臂。实现时 selector 内联进 dial_session 亦可,但两臂语义必须有测试钉住。

- [ ] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib ctrl` → FAIL。
- [ ] **Step 3: 实现**——`punch_via_hub` 全体(CtrlConnect/Accept 重发窗/端点/候选交换)搬入 `dial_session`;唯互拨段按 `allow_relay` 分流:`true` 走 `fallback::connect_with_fallback`(现状),`false` 走 `punch::punch_connect` + `punch_err` 包装;`request()` 改调 `dial_session(state, device, true)`。
- [ ] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --lib` + `cargo test -p mtdss-daemon --test ctrl_e2e`(全量回归)。
- [ ] **Step 5: Commit** `refactor(v3p5): 打洞编舞收敛为 dial_session,增 direct-only 变体`

### Task 3: warm_insert(留活弃新)+ 响应端入池 + decline 限时服务

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(`PooledConn` 增 `relay: Option<fallback::RelayPath>`;`warm_insert`;`respond_session` 入池;`request` 插入路径改 `warm_insert`;`WARM_DECLINE_GRACE`)
- Modify: `crates/daemon/src/lib.rs`(doc(hidden) 测试直驱面:`Daemon::ctrl_warm_state(&self, device: &str) -> Option<bool>`——Some(relay_is_some)/None=池无连接;沿 lib.rs:670 relay_e2e 直驱面先例)
- Test: `crates/daemon/tests/ctrl_e2e.rs`

**Interfaces:**
- Produces: `impl CtrlPool { pub(crate) fn warm_insert(&self, device: &str, ep: quinn::Endpoint, conn: quinn::Connection, relay: Option<fallback::RelayPath>) -> bool }`(入池成功 true;池内有**活**连接→留旧弃新返回 false,新连接不入池);`pub(crate) const WARM_DECLINE_GRACE: Duration = Duration::from_secs(90)`;`Daemon::ctrl_warm_state`。
- Consumes: T1/T2。

- [ ] **Step 1: 写失败 e2e**(ctrl_e2e.rs,Pair 样板)

```rust
// responder_pools_direct_conn: A 对 B list(打洞)后,B 池内出现 A 的直连
// 条目(respond_session 入池);B 反向 list A 立即成功(池命中)。
// 断言:B.ctrl_warm_state(A) == Some(false)(直连);先 A→B list,再 B→A list,
// 第二次往返时间 < 2s(池命中,无打洞;Pair 内网打洞本身 ~1s,这里以
// ctrl_warm_state 为准断言,时序断言仅 smoke 辅证)。

// decline_serves_then_closes: 池内已有活连接时,第二条编舞连接不入池
// (ctrl_warm_state 仍 Some(false) 且条目为旧连接),decline 连接在
// WARM_DECLINE_GRACE 后关闭——注入 knob:`DaemonOpts.ctrl_tuning` 由
// Pair opts() 填小值无关,GRACE 经 env MTDSS_TEST_DECLINE_GRACE_MS=2000
// 注入(main.rs 房风;e2e 进程内 Daemon 读同一 env——Daemon::start 内读,
// 沿 apply_test_env 仅 main 的现状扩为 start 内生效,Global Constraints 注入面)。
// 断言:decline 后 3s,B 池仍恰一条(ctrl_warm_state 不变),发起端二次请求仍成功。
```

- [ ] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --test ctrl_e2e responder_pools_direct_conn` → FAIL(池无条目,`ctrl_warm_state` 不存在)。
- [ ] **Step 3: 实现**——`warm_insert` 按接口语义(池内取条目,`conn_alive` 活→返回 false;死/缺→替换插入返回 true);`respond_session` 打洞 Ok 后:直连→`warm_insert`,true 则 `serve_conn` 直跑(池条目持端点,请求级驱逐时条目 drop→连接关→serve 返回),false 则 `tokio::select! { serve_conn, grace 计时 }`;中继→不入池照旧 serve;`request()` 打洞后的插入改 `warm_insert(...)`(原 blind insert)。
- [ ] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --test ctrl_e2e` 全绿(含既有「硬杀有界失败/重启复通」回归)。
- [ ] **Step 5: Commit** `feat(v3p5): 响应端直连入池(留活弃新)+ decline 限时服务`

### Task 4: request 失败语义——超时不拆暖连接

**Files:**
- Modify: `crates/daemon/src/ctrl.rs`(`request()` 失败分派 + 纯函数 `classify_failure`)
- Test: `crates/daemon/src/ctrl.rs` tests

**Interfaces:**
- Produces: `enum FailureAction { Surface, EvictAndRetry }`;`fn classify_failure(conn_alive_after: bool) -> FailureAction`(活→Surface 不拆;死→EvictAndRetry)。
- Consumes: T1(无连接依赖,纯逻辑)。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn request_timeout_keeps_warm_conn() {
    assert_eq!(classify_failure(true), FailureAction::Surface,
        "请求超时(连接仍活)不拆暖连接——大目录首列 15s 超时的主战场");
    assert_eq!(classify_failure(false), FailureAction::EvictAndRetry,
        "连接级死亡才驱逐+整程重试恰一次(现状契约)");
}
```

- [ ] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib classify` → 编译错。
- [ ] **Step 3: 实现**——`request()` 池命中臂:`send_request` Err 后 `conn_alive` 复查,按 `classify_failure` 分派(Surface→`CtrlErr::Timeout` 上抛;EvictAndRetry→驱逐+重打恰一次);新打洞臂同型。**注意**:重试臂结构维持「恰一次」,不引入循环。
- [ ] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --lib` + `--test ctrl_e2e`(硬杀用例=EvictAndRetry 路径回归)。
- [ ] **Step 5: Commit** `feat(v3p5): 请求超时不拆暖连接,仅连接级死亡驱逐重试`

### Task 5: warm.rs 预热器 + 接线 + 退避/冷却

**Files:**
- Create: `crates/daemon/src/warm.rs`(warmer_loop + 纯逻辑:`warm_targets`/`WarmBackoff`)
- Modify: `crates/daemon/src/ctrl.rs`(`ensure_warm`)
- Modify: `crates/daemon/src/lib.rs`(mod warm;`Daemon::start` spawn `warm::warmer_loop`,responder_loop 之后)
- Modify: `crates/daemon/src/main.rs`(env:`MTDSS_TEST_WARM_DEBOUNCE_MS`)
- Test: `crates/daemon/src/warm.rs` tests + `crates/daemon/tests/ctrl_e2e.rs`

**Interfaces:**
- Produces:
  - `pub(crate) async fn warmer_loop(state: Arc<DaemonState>)`(常驻;login watch 挂起语义同 responder_loop)
  - `fn warm_targets(devices: Option<&[Value]>, self_id: Option<&str>, me_pk: Option<&VerifyingKey>, peer_pks: &HashMap<String, VerifyingKey>) -> Vec<String>`(在线 ∧ 非本机 ∧ 公钥已知 ∧ `me_pk < peer_pk` 字节序)
  - `struct WarmBackoff`(态机:`fn next_delay(&mut self, again: bool) -> Duration`,1/2/4…30s 封顶,失败 3 次后进 5min 冷却;`fn reset(&mut self)`)
  - `fn should_rewrite(logged_in: bool, still_target: bool) -> bool`(death watcher 的重打闸门:未登录或已非 warm 目标即丢弃)
  - `impl CtrlPool { pub(crate) async fn ensure_warm(&self, state: &Arc<DaemonState>, device: &str) -> Result<(), String> }`(device_lock 单飞 → 池活即 Ok → `dial_session(device, allow_relay=false)` → `warm_insert`;失败串文案)
  - warmer 每设备一个 spawn:`ensure_warm` 成功后 spawn death watcher(`conn.closed().await` → 经 mpsc 通知 warmer;warmer 收到且 `state.logged_in()` 且仍 warm 侧且目标仍在线 → 按该设备 `WarmBackoff` 退避后 `ensure_warm`)。
- Consumes: T2 `dial_session`、T3 `warm_insert`、`state.events()`(state.rs:417)、`state.logged_in()`、`hub.peer_pk`。

- [ ] **Step 1: 写失败单测**(warm.rs tests)

```rust
// warm_targets_excludes_self_offline_unknown_pk_and_large_side:
// 快照 4 设备:self(在线)、peer_on(在线,小 pk)、peer_off(离线,小 pk)、
// peer_big(在线,大 pk)、peer_nopk(在线,公钥缓存缺)→
// 断言 warm_targets == ["peer_on"];me_pk=None(身份不可用)→ 空集。
// should_rewrite_gate: 重打决策纯函数——(logged_in=false, *) 恒 false;
// (true, still_target=false) false;(true, true) true。
// warm_backoff_ladder_and_cooldown: 连续失败 1s→2s→4s→…→30s 封顶,
// 第 3 次失败后 next_delay 返回 5min 冷却;reset 后回 1s。
```

- [ ] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib warm` → 编译错(模块不存在)。
- [ ] **Step 3: 实现**——warmer_loop:登录 watch 挂起(同 responder_loop 房风)→ `state.events().subscribe()` 只听 `event.devices`/`event.online` → 去抖(默认 750ms;env 注入)→ 算 targets → 逐设备 spawn ensure_warm + watcher;death watcher 收到死亡通知后经纯函数 `should_rewrite(logged_in, still_target)` 闸住(未登录/不再是 warm 目标→丢弃通知);退避/冷却态机 per device(HashMap);presence 翻转/代际更替(event.online/devices 全量)时对涉及设备 `reset()`。
- [ ] **Step 4: 写失败 e2e**(ctrl_e2e.rs)

```rust
// warmer_establishes_on_login: Pair 双 daemon 登录后,轮询(≤10s,
// 200ms 步距)断言两侧互见:B.ctrl_warm_state(A) == Some(false) 且
// A.ctrl_warm_state(B) == Some(false)——A(小 pk 侧)warmer 主动打,
// B 侧 respond 入池;随后 A→B 与 B→A 各一次 list,均 <2s 成功。
// (Pair 两 daemon 同进程,warmer 去抖注入 MTDSS_TEST_WARM_DEBOUNCE_MS=100)
```

- [ ] **Step 5: 跑测通过** — `cargo test -p mtdss-daemon --lib` + `--test ctrl_e2e` 全绿。
- [ ] **Step 6: Commit** `feat(v3p5): 预热器——登录/回前台批量暖连,presence 补打,断连退避重打+冷却`

### Task 6: WS 回前台快探(IPC 活动触发代际自愈)

**Files:**
- Modify: `crates/daemon/src/hub.rs`(入站帧时间戳 + `stale`/`kick`)
- Modify: `crates/daemon/src/state.rs`(hub 代理访问器,若缺)
- Modify: `crates/daemon/src/api.rs:82`(`ApiDispatcher::dispatch` 入口一行快探)
- Test: `crates/daemon/src/hub.rs` tests

**Interfaces:**
- Produces: `impl SignalHub { fn note_inbound(&self)`(事件循环每帧调,AtomicU64 毫秒时间戳)/ `fn stale(&self, threshold: Duration) -> bool`(离线恒 false)/ `fn kick(&self)`(uninstall 当前代际——run 循环按 Disconnected 收口,attempt 已重置路径立即重连) };`const RESUME_PROBE_STALE: Duration = Duration::from_secs(25)`。
- Consumes: 无前置(独立于 T2-T5)。

- [ ] **Step 1: 写失败测试**(hub.rs tests;stale 纯逻辑:注入时间戳)

```rust
// stale_false_when_offline_and_recent: 无代际(uninstall)→ stale 恒 false;
// note_inbound 后立即 stale(25s) == false。
// stale_true_after_silence: 手写 note_inbound 时间戳为 26s 前(mock 时钟
// 注入——note_inbound_with(now: u64) 内部变体)→ stale == true。
```

- [ ] **Step 2: 跑测确认失败** — `cargo test -p mtdss-daemon --lib hub::tests` → FAIL。
- [ ] **Step 3: 实现**——hub 增 `last_inbound: AtomicU64`(毫秒;0=离线语义);`run_connected` 事件循环每帧 `note_inbound()`;`kick` = `uninstall()` + drop(订阅方收 Closed 快速收口,客户端 Closed → Outcome::Disconnected → attempt 从当前值走退避:为保「立即重连」,kick 前若 attempt>0 由外层清零不可达——实现取直连方案:kick 置 `attempt=0` 需跨任务——改为 kick 直接 uninstall,依赖既有 Disconnected→backoff(attempt 递增)路径,若 attempt 已封顶 30s 则快探失效;故 kick 实现为「uninstall + 置 hub.kick_flag,run 循环 Disconnected 收口时见 flag 清 attempt」,以 run 循环侧 AtomicBool 落地);api.rs dispatch 入口:`if hub.stale(RESUME_PROBE_STALE) { hub.kick() }`(每次 RPC 一行,不 await,不阻塞分派)。
- [ ] **Step 4: 跑测通过** — `cargo test -p mtdss-daemon --lib` 全绿;`--test daemon_e2e`(IPC 全量回归:健康链路 inbound pong <25s,快探不误触)。
- [ ] **Step 5: Commit** `feat(v3p5): WS 回前台快探——IPC 活动时陈旧代际立即自愈重连`

### Task 7: 冒烟清单 + 文档 + 全绿收口

**Files:**
- Create: `docs/smoke-v3p5.md`
- Modify: 本计划勾选

**Interfaces:**
- Consumes: T1-T6 全部。

- [ ] **Step 1: 写冒烟清单**——spec §1 Stage A 验收线逐条落成实机场步骤:暖态进目录无打洞(RUST_LOG 观察 punch 编舞缺席)/ 空闲>60s 不重打 / 对端上线数秒暖连 / 断网换网自愈 / 移动端回前台首览 ≤10s(快探)/ 大目录超时不拆连接 / 旧端互通(新旧混跑)+ 中继连接 60s 自灭(NAT 差网验证);场:PC(本机)↔ MIX 3 ↔ iPhone(TestFlight 构建,Mac mini 直传档凭据见项目记忆)。
- [ ] **Step 2: 全量验证** — 本机 `cargo test --workspace`;WSL `MSYS_NO_PATHCONV=1 wsl.exe env CARGO_TARGET_DIR=/root/t2-target /root/.cargo/bin/cargo test --workspace`;ui `npm test && npx tsc --noEmit && npm run build`(零改动回归);`gradlew.bat :app:testDebugUnitTest`(零改动回归);`cargo test -p mtdss-daemon --test contract_golden` 单列确认零改动。netns 矩阵若本机 WSL 可跑(scripts/ 内设施)则跑,不可跑则记冒烟场代跑。
- [ ] **Step 3: Commit** `docs: smoke-v3p5 冒烟清单 + Stage A 收口`

---

## 执行顺序与闸门

- T1→T2→T3→T5 主链;T4 在 T1 后任意插入;T6 完全独立可并行。
- 全部合并 + 实机场(spec §1 验收线,含新旧混跑回滚测试)通过 = **Stage A 闸门**;随后出 Stage B(传输复用)计划,spec §3.2。
- 版本 bump 不在本波(V3-6 收尾统一 1.3.0);打包冒烟归 V3-6。
