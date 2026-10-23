# MTDSS v2-P1c 实施计划:客户端中继兜底(第一刀·传输面)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 打洞穷尽且资格在手时,传输面(发送回合 + 常驻接收)自动经服务端中继完成端到端 QUIC 传输:四点位统一的 connect-with-fallback 抽象本刀落地并接入传输两点位,发送端按分配速率配速,传输卡片呈「中继 · 限速」角标与「中继配额已耗尽」终态,nat-matrix 增对称×对称中继场景。

**Architecture:** 新 `core::transport::relay_sock`(自定义 `quinn::AsyncUdpSocket`:出站数据报前缀 16B 会话标签、目标改写 relay_addr;入站为服务器剥头后的纯 QUIC 报文)+ `make_relay_endpoint`(双角色端点,复用 `punch_connect` 以 `[relay_addr]` 为候选互拨——角色对称性与接受令牌舞步全部照旧,QUIC 引擎零改动)。daemon 侧新 `fallback.rs` 收口 connect-with-fallback:发起方(发送回合)持 per-peer 串行锁走 `RelayAllocate`;响应方(常驻接收)在轮首订阅的中继事件流上按 `peer_device` 关联服务器推送。proto 给 `RelayAllocated`/`RelaySession` 补对端标识与死因字段(P1b 落地时缺的关联面)。

**Tech Stack:** 既有栈;无新增依赖(quinn 0.11 `AsyncUdpSocket`、tokio broadcast/oneshot 均已在树)。

**Spec:** docs/superpowers/specs/2026-10-22-mtdss-v2p1-relay-design.md(§2 目标 1/3、§6 决策 2/4、§6.1 裁定 C/D/G/H/I、§7.3、§7.4、§8)

**范围裁定(spec §8 授权两刀,本计划为第一刀):** 传输面两点位 + 配速 + UI 角标 + 传输 nat-matrix 场景;ctrl 两点位(`CtrlPool::punch_via_hub`/`respond_session`)与 ctrl nat-matrix 场景留第二刀——抽象按 `FallbackRole` 参数化,ctrl 接入是机械改调。响应方配速(远端拉取场景数据发送方在响应侧)也留第二刀:两端同账号,届时响应方按自身 HelloOk rate 配速即可,无需 proto 变更。

## Global Constraints

- 门禁:每任务提交前本机 `MTDSS_UPDATE_URL=https://mtdss-update.invalid cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 全绿(本机 shell 可能 export 真实 MTDSS_UPDATE_URL,ipc consts 测试会按设计失败——必须前缀覆盖);WSL 双平台 + nat-matrix + ui(`npm test` = vitest run,ui 独立 workspace)由 PR Linux CI job 承担(沙箱拦 wsl,既定裁定)
- proto 字段号钉死:`RelayAllocated`/`RelaySession` 增 `string peer_device = 6`、`string reason = 7`(仅死帧填 reason:quota/disabled/idle);对 1.0.x 前向兼容(prost 未知字段跳过;旧 daemon 忽略新帧路径不变)
- 值钉死:响应方等推送窗口 **RELAY_PUSH_WAIT = 5s**;分配超时 **10s**(与 `flow::RELAY_TIMEOUT` 同格);配速分片 **PACE_SLICE = 64 KiB**,credit 初始 = **1 秒配额**(与服务端桶容量对齐,容忍起步突发);中继路径 QUIC MTU 上限 **RELAY_MAX_MTU = 1400**(防 +16B 标签跨路径 MTU,1452+16 仅 Ethernet 险过);中继事件通道容量 **64**;`TransportTuning` 增 `max_mtu: u16` 默认 **1452**(quinn 默认值,既有调用零行为差)
- 文案钉死:角标「中继 · 限速」;终局错误「中继配额已耗尽」(code 5 / reason=quota)、「中继未开通」(4)、「中继并发会话已达上限」(6)、「中继已被停用」(reason=disabled);可重试「中继会话已中断」(reason=idle/空)、「对端离线」(code 2);fallback 日志行 `中继兜底已建立: sid={} rate={}kbps peer={}`(nat-matrix grep 锚)
- 兜底编排钉死:直连 `punch_connect` 返回 Err(预算耗尽)即为穷尽,不做 NAT 类型启发;每轮重试重新走「直连→失败→再分配」(不复用旧会话,服务端 120s 空闲回收兜底);分配被拒 4/5/6 = 终局不重试(裁定 G),2 = 轮失败可重试;idle/未知死因 = 可重试;客户端不预检配额余量(资格只看 enabled,余量以服务端实时拒为准,决策 4)
- 服务器对中继内容零解密零窥探边界不动;客户端对中继线路零感知(QUIC 密文原样加标签)
- 未开通账号行为与今日一致(资格缺省 → 永不分配,打洞失败走今日错误提示);配速仅数据发送方(裁定 I),仅中继路径开
- 注释中文、密度沿 crate 现状;无 clap;ui 无新依赖

## Review Focus

1. **推送先于等待订阅**(时序竞态):对端分配推送早于响应方进入兜底等待 → 纯 broadcast 无回放会永等超窗。钉:relay_rx 在轮首(打洞前)订阅,推送落缓冲;T5 测试 `push_arrives_before_punch_end`(发起方提前完成分配,响应方打洞超时后才进等待,缓冲推送仍命中)
2. **同对端并发兜底串线**:A→B 与 B→A 同时兜底,或同对端双会话推送互踩。钉:仅发起方分配 + hub per-peer 串行锁;T5 测试 `bidirectional_concurrent_fallbacks`(双向并发兜底,两条连接各自回显不串)
3. **死因分派**(裁定 C/G):quota/disable 死 = 终局文案;idle 死 = 可重试(暂停超 120s 被回收,恢复后重分配即通)。钉:T5 单测 `relay_dead_err` 三 reason 映射 + e2e `quota_kill_terminal`
4. **旧服务器/旧端互通**:旧服务器 HelloOk 无资格字段 → enabled=false 永不分配;新字段不破旧端。钉:T2 测试(无字段 → 资格 disabled;推送不断连的兼容锚保留改写);T1 帧追加不破既有测试
5. **中继路径超大报文**:QUIC 默认 1452 + 16B 标签在非 Ethernet MTU 上分片/丢弃。钉:RELAY_MAX_MTU=1400;T3 测试断言伪中继观测最大入包 ≤ 1416

---

### Task 1: proto 对端标识 + 死因字段,服务端分配推送填充

**Files:**
- Modify: `proto/proto/signal.proto`
- Modify: `server/signaling/src/ws.rs`(`relay_allocate` 增请求方设备号参数并填两帧)
- Modify: `server/signaling/src/relay/mod.rs`(`notify_relay_dead` 三处死因)
- Test: `server/signaling/tests/relay.rs`(追加)

**Interfaces:**
- Produces(T2/T4/T5 依赖):

```proto
// RelayAllocated/RelaySession 各追加(字段号钉死)
string peer_device = 6;  // 对端设备号:Allocated 填 to_device;Session 分配帧填请求方;死帧留空(按 sid 路由)
string reason = 7;       // 死因:quota|disabled|idle;仅 alive:false 帧携带,其余留空
```

- `notify_relay_dead(state, account_id, sid, reason: &str)`:配额杀点(handle_datagram)传 `"quota"`;巡检 disable 杀传 `"disabled"`;空闲回收传 `"idle"`
- `relay_allocate` 签名增 `from_device: &str`(调用点连接上下文已有);`RelayAllocated.peer_device = to_device`,对端推送 `RelaySession.peer_device = from_device`

- [ ] **Step 1: 写失败测试**——tests/relay.rs 追加:沿既有 T4 门禁用例的 WS 假 peer 装配走完整分配 → 断言请求方收到的 `RelayAllocated.peer_device == 对端` 且对端收到的 `RelaySession{peer_device == 请求方, reason 为空}`;配额杀路径断言死帧 `reason == "quota"`(直调 `handle_datagram` 灌超限包,沿既有 quota 杀用例)
- [ ] **Step 2: `cargo test -p signaling --test relay` 确认失败**(字段不存在)
- [ ] **Step 3: 实现**——proto 按 Interfaces 追加;ws.rs/relay/mod.rs 填充三处;`cargo build` 全仓(prost 重生成)
- [ ] **Step 4: 既有 signaling 测试全绿**(帧追加为加法,构造字面量缺省空串)
- [ ] **Step 5: Commit** `feat(proto+signaling): relay push peer identity + death reason`

### Task 2: core 信令客户端——资格下发、分配请求、会话事件

**Files:**
- Modify: `crates/core/src/signaling/client.rs`
- Modify: `crates/daemon/src/hub.rs`(仅机械适配 HelloOk 新字段,`..` 绑定;捕获归 T4)
- Test: `crates/core/tests/signaling_client.rs`(追加/改写)

**Interfaces:**
- Consumes: T1 proto 帧
- Produces(T4/T5 依赖):

```rust
/// HelloOk 下发的中继资格(旧服务器/未开通 → enabled=false,永不发起分配)
#[derive(Debug, Clone, Copy)]
pub struct RelayEntitlement { pub enabled: bool, pub rate_kbps: u32, pub quota_remaining: u64 }

pub enum ClientEvent {
    HelloOk { stun_addrs: Vec<String>, relay: RelayEntitlement },   // 破坏性:全仓消费点机械适配
    Relayed { .. }, Presence { .. }, Error { .. }, Closed,
    RelaySession { sid: u64, alive: bool, tag: Vec<u8>, relay_addr: String,
                   expires_at: i64, peer_device: String },          // 新:替代旧「忽略」分支
}

pub enum AllocOutcome {
    Allocated { sid: u64, tag: Vec<u8>, relay_addr: String, expires_at: i64, rate_kbps: u32 },
    Denied { code: u32, message: String },
}
impl SignalingClient {
    /// 请求与 to_device 的中继分配。本连接内串行(内部锁);响应经 pending
    /// oneshot 关联:RelayAllocated → Allocated,Error 2/4/5/6(窗口内一律归因
    /// 分配,见计划裁定)→ Denied;10s 超时/连接断 → Err。
    pub async fn relay_allocate(&self, to_device: &str) -> anyhow::Result<AllocOutcome>;
}
```

- 裁定(钉死):分配 pending 窗口内 Error 2/4/5/6 一律归因分配不外发事件(误吞并发 relay 转发的 Error 2 仅致该轮兜底失败,不破坏连接);窗口外 Error 流向不变(含 hub 的断连分派);`RelayAllocated` 无 pending 时 warn 丢弃

- [ ] **Step 1: 写失败测试**(扩既有 register 假服务器:HelloOk 带资格字段、应答 RelayAllocate)——`hello_ok_carries_entitlement`(三字段透传)、`hello_without_relay_fields_means_disabled`(旧服务器形态 → enabled=false)、`relay_allocate_roundtrip`(发帧→回 RelayAllocated→ Allocated 各字段)、`relay_allocate_denied_codes`(回 Error 4/5/6/2 → Denied 对应码,且事件流不再出现该 Error)、`relay_session_pushes_surface_as_event`(改写 P1b 兼容锚:两帧均以事件面呈现且不断连)、`pending_allocate_fails_on_close`(服务器断连 → relay_allocate 返 Err)
- [ ] **Step 2: `cargo test -p mtdss-core --test signaling_client` 确认失败**
- [ ] **Step 3: 实现**(pending 槽 = `Arc<std::sync::Mutex<Option<oneshot::Sender<AllocOutcome>>>>` + 每客户端 `Arc<tokio::sync::Mutex<()>>` 串行;WS 任务三帧分支改写)
- [ ] **Step 4: 全仓编译 + 既有测试绿**(HelloOk 消费点 `..` 适配;daemon tests 若匹配同改)
- [ ] **Step 5: Commit** `feat(core): signaling relay entitlement, allocate, session events`

### Task 3: core 中继 UDP 套接字 + 中继双角色端点

**Files:**
- Create: `crates/core/src/transport/relay_sock.rs`
- Modify: `crates/core/src/transport/mod.rs`(挂 `pub mod relay_sock;`)
- Modify: `crates/core/src/transport/endpoint.rs`(`TransportTuning` 增 `max_mtu`,`transport_config` 应用)
- Test: `crates/core/tests/relay_sock.rs`(新建)

**Interfaces:**
- Consumes: 既有 `make_dual_endpoint_tuned`/`connect_pinned_tuned`/`accept_pinned`/`punch_connect`(全部不改语义,复用)
- Produces(T5 依赖):

```rust
/// 出站:数据报前缀 16B 标签、目标改写 relay_addr(原 destination 弃用);
/// 入站:服务器已剥头,直通(源地址即 relay_addr)。
pub struct RelaySocket { /* tokio UdpSocket + tag + relay_addr */ }
/// 中继双角色端点:socket 包 RelaySocket 后交 quinn(Endpoint::new);
/// 服务端白名单/调优语义与 make_dual_endpoint_tuned 一致。
pub fn make_relay_endpoint(
    bind: SocketAddr, me: &DeviceIdentity, allowed: &HashMap<String, VerifyingKey>,
    tuning: TransportTuning, tag: [u8; 16], relay_addr: SocketAddr,
) -> anyhow::Result<quinn::Endpoint>;
// TransportTuning { .., pub max_mtu: u16 }  // Default = 1452;transport_config 里 tc.max_mtu(实现者按 quinn 0.11 实际 API 名)
```

- 实现口径(签名与测试不决定,钉死):`quinn::AsyncUdpSocket` 实现里 `max_transmit_segments`/`max_receive_segments` 恒 1(禁 GSO/GRO 批量;`segment_size=Some` 防御性拆分逐段加 tag);`may_fragment` false;poll_send/poll_recv 的 tokio 就绪注册按实际 API(`poll_send_to`/`try_send_to`+就绪等价物);`local_addr` 透传
- 过期不本地处理(expires_at 仅透传):服务端 120s 空闲回收 + 每轮重新分配兜底

- [ ] **Step 1: 写失败测试**——测试内伪中继(固定 tag 对逐包路由、strip 16B、记录最大入包、未知 tag/超短丢弃,沿 server 端测试的转发语义):`punch_over_relay_paired_connection`(两身份各建中继端点,双方并发 `punch_connect(&ep, .., &[relay_addr], ..)` → 连接互通,双向 open_bi/accept_bi 回显)、`tag_routes_by_tag_not_source`(同一 tag 从两个不同源端口发裸数据报 → 对端均收到,伪中继按 tag 重绑)、`malformed_datagrams_dropped`(<16B、随机 tag → 丢弃不 panic,后续好流不受扰)、`mtu_capped`(打洞后灌大流量,伪中继最大观测入包 ≤ 1416)
- [ ] **Step 2: `cargo test -p mtdss-core --test relay_sock` 确认失败**
- [ ] **Step 3: 实现**(RelaySocket + make_relay_endpoint + TransportTuning.max_mtu)
- [ ] **Step 4: 既有全量测试绿**(TransportTuning 默认 1452 = quinn 默认,零行为差;transport_tuning.rs 等既有用例不动)
- [ ] **Step 5: Commit** `feat(core): relay UDP socket + dual endpoint over relay`

### Task 4: daemon 枢纽——资格缓存、中继事件路由、分配面、per-peer 串行

**Files:**
- Modify: `crates/daemon/src/hub.rs`
- Test: `crates/daemon/tests/relay_e2e.rs`(新建,本任务起步)

**Interfaces:**
- Consumes: T2 `RelayEntitlement`/`AllocOutcome`/`ClientEvent`;`signaling::spawn_test_server` + `signaling::relay::spawn_relay` + `signaling::admin::{user_add, relay_enable…}`(沿 server/tests/relay.rs 的 `spawn_relay_on` 随机端口先例)
- Produces(T5 依赖):

```rust
pub(crate) enum RelayEvt {
    /// 分配推送(alive:true;tag 非 16 字节在路由时丢弃)
    Push { peer: String, sid: u64, tag: [u8; 16], relay_addr: SocketAddr, expires_at: i64 },
    /// 会话死亡(alive:false;reason 透传)
    Dead { sid: u64, reason: String },
}
impl SignalHub {
    pub(crate) fn relay_entitlement(&self) -> Option<RelayEntitlement>;          // 离线 None
    pub(crate) fn relay_events(&self) -> Option<broadcast::Receiver<RelayEvt>>;  // 轮首订阅用
    pub(crate) async fn relay_allocate(&self, to: &str) -> anyhow::Result<AllocOutcome>; // 10s 有界
    pub(crate) fn fallback_lock(&self, peer: &str) -> Arc<tokio::sync::Mutex<()>>;        // per-peer 串行(表在 hub,跨代际存活,同 CtrlPool::locks 房风)
}
```

- `ConnHandle` 增 `relay: RelayEntitlement`(HelloOk 捕获)与 `relay_ch: broadcast::Sender<RelayEvt>`(容量 64);事件循环 `ClientEvent::RelaySession` → 广播 RelayEvt(alive 分派;relay_addr 解析失败丢弃告警)

- [ ] **Step 1: 写失败测试**(relay_e2e.rs:spawn_test_server + spawn_relay + user_add + relay enable;两 daemon state 登录并等枢纽上线,沿 jobs_e2e 装配房风)——`entitlement_cached_from_hello`(admin 默认 → hub.relay_entitlement()== (true, 1024, 10GiB))、`allocation_push_routed`(B 先 `relay_events()` 订阅,A 调 `hub.relay_allocate(B)` → A 得 Allocated(rate=1024),B 的订阅收到 `Push{peer=A 的 device_id, sid 同}`)
- [ ] **Step 2: `cargo test -p mtdss-daemon --test relay_e2e` 确认失败**
- [ ] **Step 3: 实现**
- [ ] **Step 4: 通过 + 既有 daemon 测试绿**
- [ ] **Step 5: Commit** `feat(daemon): hub relay entitlement cache, event routing, allocate`

### Task 5: connect-with-fallback 编排 + 传输两点位 + 类型化终局 + 作业标记

**Files:**
- Create: `crates/daemon/src/fallback.rs`
- Modify: `crates/daemon/src/lib.rs`(挂模块)
- Modify: `crates/daemon/src/flow.rs`(`send_round`/`recv_handshake` 接入;`SendOpts`/`RecvOpts` 增 `fault_force_relay`;`SendCtl` 增 `on_relay` 回调)
- Modify: `crates/core/src/transport/transfer.rs`(`SendError::Relay` 变体)
- Modify: `crates/daemon/src/jobs.rs`(JobSnap/事件面 `relay: bool` + `mark_relay`)
- Modify: `crates/daemon/src/api.rs`(遗留 send `SendCtl::default()` 机械适配)
- Test: `crates/daemon/tests/relay_e2e.rs`(追加)、`crates/daemon/tests/flow_unit.rs`(追加)

**Interfaces:**
- Consumes: T2/T3/T4 全部
- Produces:

```rust
// fallback.rs
pub(crate) enum FallbackRole { Initiator, Responder }
pub(crate) struct RelayPath { pub sid: u64, pub rate_kbps: u32 }   // 响应方 rate 取自身 entitlement(同账号同配)
pub(crate) struct ConnectOutcome { pub conn: quinn::Connection, pub relay: Option<RelayPath> } // None=直连
pub(crate) enum FallbackErr {
    Direct(anyhow::Error),            // 直连败且无兜底(或未开资格):调用方按今日口径包装
    RelayDenied { code: u32 },        // 4/5/6,终局(裁定 G);文案映射收口于此:4→「中继未开通」5→「中继配额已耗尽」6→「中继并发会话已达上限」
    RelayFailed(String),              // 分配 2「对端离线」/等待超窗/兜底互拨败:可重试
}
pub(crate) async fn connect_with_fallback(
    hub: &SignalHub, relay_rx: &mut broadcast::Receiver<RelayEvt>,   // 调用方轮首订阅(先订阅后打洞,防推送竞态)
    me: &DeviceIdentity, peer_id: &str, peer_pk: &VerifyingKey,
    direct: &quinn::Endpoint, peer_candidates: &[SocketAddr],
    punch: punch::PunchConfig, role: FallbackRole, force_relay: bool,
) -> Result<ConnectOutcome, FallbackErr>;
/// 传输阶段等本会话死亡通告(select 臂);命中后调用方按 reason 收束。
pub(crate) async fn wait_relay_dead(relay_rx: &mut broadcast::Receiver<RelayEvt>, sid: u64);
/// 死因 → 发送错误:quota/disabled → SendError::Relay(终局);idle/空 → Network(可重试)
pub(crate) fn relay_dead_err(reason: &str) -> transfer::SendError;

// transfer.rs
pub enum SendError { .., #[error("relay: {message}")] Relay { code: u32, message: String } }  // retryable()=false
// flow.rs
pub struct SendOpts { .., #[doc(hidden)] pub fault_force_relay: bool }   // RecvOpts 同
pub struct SendCtl { .., pub on_relay: Option<Arc<dyn Fn() + Send + Sync>> }  // 兜底生效即调恰一次(jobs 印 relay 标记)
// jobs.rs
impl Jobs { pub(crate) fn mark_relay(&self, id: &str); }   // JobMut.relay=true + 事件即发
```

- 编排钉死:直连 `punch_connect` Ok → `(conn, None)`;Err 且 entitlement.enabled(否则 FallbackErr::Direct 包 punch_err 文案)→ Initiator:持 `hub.fallback_lock(peer)` → `hub.relay_allocate` → `make_relay_endpoint(本端 tag, tuning 覆写 max_mtu=1400)` → `punch_connect(&relay_ep, .., &[relay_addr], punch)` → info 日志行(全局约束文案)→ `(conn, Some(RelayPath))`;Responder:在 relay_rx 上过滤 `Push{peer == peer_id}`(窗口 RELAY_PUSH_WAIT,先排干缓冲再等新帧;超窗 → RelayFailed)→ 同上互拨。force_relay 跳过直连段(测试注错,沿 fault_close_after_files 先例);两侧端点/白名单/候选交换流程不动(仅 punch 段替换)
- `send_round` 接入:轮首 `hub.relay_events()`(None 同「信令未在线」);punch 调用换 `connect_with_fallback(.., Initiator, force)`;RelayDenied → `SendError::Relay`(终局);RelayFailed/Direct → Network;relay 路径 → `on_relay` 回调 + `send_opts.rate_kbps = Some(rate)`(T6) + `select! { send_files, wait_relay_dead(relay_rx, sid) }`(后者 → `relay_dead_err`)
- `recv_handshake` 接入:返回 `(Connection, Option<RelayPath>)`;resident 轮传输段 `select! { recv_on_conn, 取消, wait_relay_dead }`;轮内 relay 路径 → `jobs().mark_relay(eff_job_id)`;传输错误按 reason 映射(job failed 文案 = 终局文案 / idle 走既有失败路径)
- jobs `upload_driver`:装配 `on_relay`(闭包捕获 state+job_id → mark_relay);api.rs 遗留 send 传 None

- [ ] **Step 1: 写失败测试**(relay_e2e.rs 追加;直连候选用 192.0.2.1:9 黑洞 + PunchConfig 预算 ~300ms 快败)——`push_arrives_before_punch_end`(发起方 force_relay 即刻分配,响应方打洞 1s 超时后进等待,缓冲推送命中——钉 Review Focus #1)、`fallback_connect_over_relay`(Initiator/Responder 得同一会话,双向回显)、`bidirectional_concurrent_fallbacks`(A→B 与 B→A 并发兜底,两连接互不串线)、`transfer_via_relay_full`(双侧 fault_force_relay,经 jobs 注册表发起 upload → 落盘齐;两作业 snapshot.relay=true;`relay_usage` 当期字节 ≥ 文件字节且 ≤ 1.5×,裁定 H 容差)、`quota_kill_terminal`(quota 注入 1KB 沿 server 测试先例 → 传输终局失败,job error 含「中继配额已耗尽」,尝试轮数有界)、`refusal_disabled_terminal`(admin disable 后 force_relay → 分配拒 4 → 终局「中继未开通」);flow_unit.rs 追加 `relay_error_not_retryable` 与 `relay_dead_err_maps_reasons`(quota/disabled → `SendError::Relay` 终局文案,idle/空 → `Network` 可重试)
- [ ] **Step 2: `cargo test -p mtdss-daemon --test relay_e2e --test flow_unit` 确认失败**
- [ ] **Step 3: 实现**(fallback.rs + 两点位接入 + SendError::Relay + jobs 标记)
- [ ] **Step 4: 全量门禁(workspace + clippy)通过**(既有 transfer/e2e/daemon 测试不受直连路径影响)
- [ ] **Step 5: Commit** `feat(daemon): connect-with-fallback for transfer planes + typed relay errors`

### Task 6: 发送端配速

**Files:**
- Modify: `crates/core/src/transport/transfer.rs`(`SendOptions.rate_kbps` + Pacer + `push_file` 分片写)
- Modify: `crates/daemon/src/flow.rs`(`send_round` 注入 relay rate)
- Test: `crates/core/tests/transfer.rs`(追加)、transfer.rs 模块内 `#[cfg(test)]`(纯数学)

**Interfaces:**
- Consumes: T5 `RelayPath.rate_kbps`
- Produces:

```rust
// SendOptions 增
pub rate_kbps: Option<u32>,   // None=直连/不限;Some=发送端配速(合作式,决策 2;服务端桶仍权威)
// 纯数学(模块内单测):want 字节前需补的睡眠与扣后余额
fn pace_plan(credit: f64, elapsed: Duration, rate_bps: u64, want: usize) -> (f64, Duration);
```

- 实现口径:整次 `send_files` 一个共享 `Arc<Pacer>`(波/文件/块任务同源,会话级速率);`push_file` 在配速开时按 PACE_SLICE 分片写,每片 `write_all` 前 `pacer.acquire(slice_len)`(credit 初始 1 秒配额);配速关 → 现状单次 `write_all` 零改动;时序断言不做(P1b 先例:只验逻辑层)

- [ ] **Step 1: 写失败测试**——模块内 `pace_plan` 纯数学(满币即过/欠账睡眠时长/补币速率);tests/transfer.rs 追加 `paced_transfer_completes`(rate 极高如 10_000_000 kbps 跑通既有多块多轮场景,断言完成与字节计数,不做时序断言)
- [ ] **Step 2: 确认失败 → Step 3: 实现**(`send_opts.rate_kbps = Some(rate)` 于 T5 预留点接线)
- [ ] **Step 4: 全量测试绿**
- [ ] **Step 5: Commit** `feat(core+daemon): sender pacing by allocated rate`

### Task 7: 传输卡片中继角标 + IPC/事件面 relay 字段

**Files:**
- Modify: `crates/daemon/src/jobs.rs`(emit_locked JSON 增 `"relay": bool`,沿 `skipped` 先例)
- Modify: `crates/daemon/src/api.rs`(transfer.list 投影同面)
- Modify: `ui/src/lib/transfer.ts`(`JobSnapUI.relay: boolean`,reducer 缺省 false)
- Modify: `ui/src/components/TransferCards.tsx`(CardShell 头部角标)
- Modify: `ui/src/i18n/zh.json`(`transfer.relayBadge`)
- Modify: `ui/src/App.css`(`.tcard-relay` 轻样式)
- Test: `ui/src/lib/transfer.test.ts`(追加)

**Interfaces:**
- Consumes: T5 `mark_relay`/JobSnap.relay
- Produces: 事件/快照 JSON 增 `"relay"`;UI 角标 `job.relay && <span className="tcard-relay">{t("transfer.relayBadge")}</span>`(终态卡同样显示);终态文案「中继配额已耗尽」经 job.error 直显(FailedCard 现状,无 UI 改动)

- [ ] **Step 1: 写失败测试**(transfer.test.ts:事件含 relay:true → 快照透传;缺字段(旧 daemon 事件)→ false 容错)
- [ ] **Step 2: `npm test`(ui/)确认失败 → Step 3: 实现**(daemon JSON 面 + UI 字段/角标/文案/样式)
- [ ] **Step 4: ui vitest 绿 + workspace 门禁绿**(daemon 侧仅序列化追加)
- [ ] **Step 5: Commit** `feat(daemon+ui): relay badge on transfer cards`

### Task 8: nat-matrix 中继场景 + 部署文档

**Files:**
- Modify: `scripts/nat-matrix/lib.sh`(signaling 起 `--relay-port 8788 --relay-addr $NM_PUB_IP_A` 与 `mtdss-admin relay enable/limit dev@test`,均以 `NM_RELAY=1` 门控——14 个既有用例不开中继,symmetric fail 语义保持)
- Modify: `scripts/nat-matrix/run_case.sh`(NM_RELAY 透传;ok 断言追加 daemon 日志 grep「中继兜底已建立」)
- Modify: `scripts/nat-matrix/matrix.sh`(追加用例:`NM_RELAY=1 symmetric symmetric ok`,注释与用例计数更新)
- Modify: `scripts/nat-matrix/README.md`(场景与 env 说明)
- Modify: `scripts/deploy/README.md`(中继小节补一句:客户端打洞穷尽自动兜底,账号需 `mtdss-admin relay enable`)

**Interfaces:**
- Consumes: T5 日志锚「中继兜底已建立」
- Produces: 矩阵第 16 例;中继用例对账号 `relay limit dev@test 8192`(12MiB 默认载荷 ≈ 12s,矩阵时限内)

- [ ] **Step 1: 实现**(脚本门控改造 + 用例追加;本机沙箱不可复跑 netns,以 PR Linux CI job 的 nat-matrix 为准——沿 P1b 既定裁定,脚本改动靠 shell 语法自检 `bash -n`)
- [ ] **Step 2: `bash -n` 四脚本通过 + README/矩阵注释与实际用例一致**
- [ ] **Step 3: Commit** `test(nat-matrix): symmetric×symmetric relay fallback case`

## 第二刀交接(不在本计划)

- ctrl 两点位接入:`CtrlPool::punch_via_hub`(Initiator)/`respond_session`(Responder)改调 `connect_with_fallback`,`CTRL_TUNING` 覆写 max_mtu=1400;ctrl 发起方分配拒绝沿用 `CtrlErr` 类型化(code 4/5/6 → Internal 文案换终局文案)
- 响应方配速:ctrl orchestrate 的源端(数据发送方在响应侧)按自身 HelloOk rate 配速(两端同账号同配,无需 proto 变更)
- nat-matrix ctrl 场景(远端浏览走中继)
