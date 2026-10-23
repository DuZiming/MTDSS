# MTDSS v2-P1b 实施计划:服务端中继数据面

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 signaling 同进程内落地中继数据面:WS 票据分配 + 单 UDP 端口哑转发(逐包会话标签、NAT 重绑存活)+ 每会话限速 + 月配额记账与中途杀会话 + HelloOk 资格下发。

**Architecture:** 新 `relay` 模块分两层——`ticket.rs` 纯逻辑(票据/标签/注册表/令牌桶,时钟可注入)与 `mod.rs` 数据面(UDP 任务 + 记账 + 杀会话通知);WS 层新增 `RelayAllocate` 处理与对端推送;proto 增三帧 + HelloOk 三字段,全部对 1.0.x 前向兼容(旧端忽略未知服务帧;旧服务器会拒未知客户端帧,但 v2 客户端仅在收到新 HelloOk 资格字段后才会尝试分配,不会触碰旧服务器)。

**Tech Stack:** 既有栈(tokio/axum/rusqlite/prost);无新增依赖(HMAC 用已有 `sha2`?——**需 `hmac` crate**:sha2 提供摘要,票据防伪需 HMAC。允许新增 `hmac = "0.12"` 一个依赖)。

**Spec:** docs/superpowers/specs/2026-10-22-mtdss-v2p1-relay-design.md(§6 决策 4、§6.1 裁定 C/D/E/G/H、§7.1、§8 P1b)

## Global Constraints

- 门禁:每任务提交前本机 `MTDSS_UPDATE_URL=https://mtdss-update.invalid cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 全绿(本机 shell 可能 export 真实 MTDSS_UPDATE_URL,ipc consts 测试会按设计失败——必须前缀覆盖);WSL 双平台 + nat-matrix + ui 由 PR Linux CI job 承担(沙箱拦 wsl,既定裁定)
- 值钉死:UDP 端口默认 **8788**;标签 **16 字节**;票据有效期 **86400s**(实际清理靠空闲回收);每账号并发中继会话上限 **2**;空闲回收 **120s**;限速/停用巡检周期 **60s**;记账刷库阈值 **1 MiB 或 5s**(崩溃丢失 ≤1MB,spec §9);全局带宽上限 env **MTDSS_RELAY_GLOBAL_KBPS 默认 2500**;默认限速 1024 kbps / 配额 10 GiB 沿 P1a `admin::default_*`
- 线上字节口径(裁定 H):记账按**整数据报长度**(含 16B 标签头)双向累计;配额语义 = 中继线上流量
- 杀会话语义(裁定 C):配额耗尽 = 立即杀(内存检测);`relay disable` = 经 ≤60s 巡检后杀;限速变更 = ≤60s 内热调令牌桶
- 服务器对中继内容零解密零窥探(转发地址与会话计数之外的一切);注册表与 UDP 处理不得 panic 于畸形输入
- proto 字段号钉死(见 T1);错误码:4=中继未开通、5=配额耗尽、6=超并发上限;2(对端不在线)复用既有
- 注释中文、密度沿 crate 现状;无 clap

## Review Focus

1. **票据伪造/畸形输入**:错标签、超短数据报、过期票据 → 丢弃且不 panic——T2/T3 测试钉住
2. **配额月界滚动**:跨自然月时进行中会话不重复计费、新周期从零起算——T3 测试钉住(注入 period)
3. **NAT 重绑存活**(裁定 D):对端中途换源地址(新端口)仍被转发到新地址——T3 数据面测试钉住
4. **全局上限独立生效**:多会话共享 2500 kbps 总闸,与每会话限速叠加——T3 测试钉住(global bucket 逻辑层)
5. **旧版兼容**:旧 daemon 收到 RelaySession/RelayAllocated 推送走 `_ =>` 忽略不断连;新 daemon 不向旧服务器发分配(资格缺省不发)——T1 proto roundtrip + 既有 ws 客户端 match 结构保证,T1 加服务端"推送不致断连"集成测试

---

### Task 1: proto 三帧 + HelloOk 资格下发

**Files:**
- Modify: `proto/proto/signal.proto`
- Modify: `server/signaling/src/admin.rs`(+entitlement 读取)
- Modify: `server/signaling/src/ws.rs`(HelloOk 填充)
- Test: `server/signaling/tests/ws.rs`(追加)、`server/signaling/tests/admin.rs`(追加)

**Interfaces:**
- Consumes: P1a `admin::user_add`、`relay_set_enabled/limit/quota`、`user_list`、`current_period`
- Produces(T2-T4 依赖):

```proto
// signal.proto 追加(字段号钉死)
message WsClientFrame { oneof frame { Hello hello = 1; Relay relay = 2; RelayAllocate relay_allocate = 3; } }
message WsServerFrame { oneof frame { ...既有 1-4...; RelayAllocated relay_allocated = 5; RelaySession relay_session = 6; } }
message HelloOk { string account_id = 1; repeated string stun_addrs = 2;
  bool relay_enabled = 3; uint32 relay_rate_kbps = 4; uint64 relay_quota_remaining_bytes = 5; }
message RelayAllocate { string to_device = 1; }
message RelayAllocated { uint64 sid = 1; bytes tag = 2; string relay_addr = 3; int64 expires_at = 4; uint32 rate_kbps = 5; }
message RelaySession { uint64 sid = 1; bool alive = 2; bytes tag = 3; string relay_addr = 4; int64 expires_at = 5; }
```

```rust
// admin.rs
/// (enabled, rate_kbps, remaining_bytes);quota=0(不限)→ u64::MAX;无 relay_config 行 → (false, 默认, 默认余量)
pub fn relay_entitlement(conn: &Connection, account_id: &str) -> anyhow::Result<(bool, u32, u64)>;
```

- [ ] **Step 1: 写失败测试**——admin.rs:`entitlement_disabled_by_default`(user_add 后 (false, 1024, 10GiB))、`entitlement_enabled_and_unlimited`(enable+quota 0 → (true, 1024, u64::MAX))、`entitlement_reflects_usage`(手工 INSERT 当期 relay_usage 1MiB → 余量 10GiB-1MiB);ws.rs:`hello_ok_carries_relay_entitlement`(预置+enable 后走完整 hello,断言 HelloOk 三字段)、`hello_ok_disabled_account`(enabled=false)
- [ ] **Step 2: 跑 `cargo test -p signaling --test admin --test ws` 确认失败**(entitlement 未定义/字段不存在)
- [ ] **Step 3: 实现**——proto 按 Interfaces 钉死追加;admin.rs 增 `relay_entitlement`(单条 SQL JOIN relay_config/relay_usage 当期,COALESCE 无行=disabled+默认);ws.rs HelloOk 填充三字段
- [ ] **Step 4: 跑测试确认通过**(既有 ws 测试因 proto 可选字段不受影响)
- [ ] **Step 5: Commit** `feat(proto+signaling): relay allocate frames + HelloOk entitlement`

### Task 2: 票据、标签、注册表、令牌桶(纯逻辑,时钟可注入)

**Files:**
- Create: `server/signaling/src/relay/mod.rs`(空壳 `pub mod ticket;`)
- Create: `server/signaling/src/relay/ticket.rs`
- Modify: `server/signaling/src/lib.rs`(挂 `pub mod relay;`)
- Test: `server/signaling/src/relay/ticket.rs` 内 `#[cfg(test)]`(纯逻辑单测随模块)

**Interfaces:**
- Produces(T3/T4 依赖):

```rust
pub struct Ticket { pub sid: u64, pub expires_at: i64 }
pub fn mint_ticket(secret: &[u8; 32], expires_at: i64, rng: &mut impl rand::RngCore) -> (Ticket, [u8; 16], [u8; 16]);
    // 返回 (票据, tag_a, tag_b):tag = HMAC-SHA256(secret, b"mtdss-relay-tag" || sid.to_le_bytes() || expires_at.to_le_bytes() || slot_byte)[..16],slot_byte∈{0,1}
pub struct Session { pub ticket: Ticket, pub account_id: String, pub rate_kbps: u32,
    pub slot_addrs: [Option<SocketAddr>; 2], pub bucket_a: TokenBucket, pub bucket_b: TokenBucket,
    pub bytes_accounted: u64, pub last_refresh: Instant }
pub struct Registry { /* tag -> (sid, slot) 映射 + sid -> Session */ }
impl Registry {
    pub fn insert(&mut self, s: Session, tag_a: [u8;16], tag_b: [u8;16]);
    pub fn lookup(&self, tag: &[u8]) -> Option<(u64, u8)>;          // (sid, slot)
    pub fn get_mut(&mut self, sid: u64) -> Option<&mut Session>;
    pub fn remove(&mut self, sid: u64) -> Option<Session>;
    pub fn sweep_idle(&mut self, now: Instant) -> Vec<u64>;          // 移除 last_activity>120s,返回被移除 sid
    pub fn count_for_account(&self, account_id: &str) -> usize;
}
pub struct TokenBucket { /* rate_kbps, tokens, last_refill(注入时钟) */ }
impl TokenBucket { pub fn new(rate_kbps: u32) -> Self; pub fn try_take(&mut self, bytes: usize, now: Instant) -> bool;
    pub fn set_rate(&mut self, rate_kbps: u32, now: Instant); }
```

- [ ] **Step 1: 写失败单测**——tag 确定性(mint 两次同票 → 同 tag 对)、错 slot/错票据/过期票据 tag 查不到(伪造 16B 随机 → None)、桶:注入 now 推进 → 补 token、set_rate 热调即时生效、sweep_idle 只收超时项、count_for_account 计数
- [ ] **Step 2: `cargo test -p signaling relay::` 确认失败**(模块不存在)
- [ ] **Step 3: 实现**(hmac crate 新依赖;Registry 用 HashMap 双索引)
- [ ] **Step 4: 确认通过**
- [ ] **Step 5: Commit** `feat(signaling): relay tickets/registry/token buckets (pure logic)`

### Task 3: UDP 数据面 + 配额记账 + 杀会话通知

**Files:**
- Modify: `server/signaling/src/relay/mod.rs`
- Modify: `server/signaling/src/lib.rs`(AppState 增 `relay: RelayState`;`presence` 改 `Arc<PresenceRegistry>`——杀会话通知要向两台设备推 WS 帧)
- Test: `server/signaling/tests/relay.rs`(新建)

**Interfaces:**
- Consumes: T2 全部;P1a `admin::relay_entitlement`/`user_list`/`current_period`
- Produces(T4/T5 依赖):

```rust
pub struct RelayState {
    pub registry: std::sync::Mutex<Registry>,
    pub socket: Arc<tokio::net::UdpSocket>,
    pub secret: [u8; 32],
    pub global_bucket: std::sync::Mutex<TokenBucket>,   // 2500 kbps 默认
    pub quota: std::sync::Mutex<QuotaLedger>,           // 内存余量+脏字节,1MiB/5s 刷库
}
pub fn spawn_relay(state: Arc<AppState>, bind_port: u16, pub_addr: String) -> anyhow::Result<()>;  // 起 UDP 任务+巡检任务;pub_addr=对外通告的 ip:port(对齐 --stun-addr 房风,T5 供旗标)
```

数据报处理算法(签名与测试不决定,钉死):`recv → buf[..16] 为 tag → registry.lookup → get_mut(session)`;更新该 slot 源地址(重绑,裁定 D)+ last_activity;`bucket_{slot}.try_take(buf.len())` 且 `global_bucket.try_take(buf.len())` 不过则**丢弃**(QUIC 重传自会降速);对侧 slot 有地址则转发 `buf[16..]`,无则丢;两桶各扣 `buf.len()`(整报文,裁定 H);`QuotaLedger` 按 account 累计,超余量 → `remove(sid)` + 经 presence 向两端推 `RelaySession{sid, alive:false}`;刷库 = `UPDATE relay_usage ... period=current_period()` 幂等 upsert,月界时新 period 从零起算。巡检任务(60s):逐会话重读 `relay_config`(热调 rate;disabled → 杀)。

- [ ] **Step 1: 写失败集成测试**(spawn relay on 127.0.0.1 随机端口 + tempdb AppState):两合成 peer 各持 tag 互发 → 双向收到(strip 后载荷相等);**重绑**:peer A 换新 UdpSocket(新端口)再发 → B 仍收到且后续回包到 A 新地址;**畸形**:超短报文(<16B)、随机 tag → 静默丢弃不崩(后续正常会话不受扰);**记账**:传 N 字节后刷库,relay_usage 当期 ≈ 总线上字节(含头);**配额杀**:quota 设 1KB、灌包 → 会话消失 + 两端(经 presence 注册的假 peer handle)收到 alive:false;**月界**:注入 current_period 到下月 → 新会话记账从零起算
- [ ] **Step 2: `cargo test -p signaling --test relay` 确认失败**
- [ ] **Step 3: 实现**(数据面任务 + 巡检 + 记账;presence 改 Arc 的调用点机械更新;限速只验逻辑层不做过拟合时序断言)
- [ ] **Step 4: 全包测试通过(`--test relay` + 既有全部)**
- [ ] **Step 5: Commit** `feat(signaling): relay UDP data plane, quota ledger, session kills`

### Task 4: WS 分配协议 + 门禁

**Files:**
- Modify: `server/signaling/src/ws.rs`(RelayAllocate 分支)
- Test: `server/signaling/tests/relay.rs`(追加)

**Interfaces:**
- Consumes: T1 proto 帧 + `admin::relay_entitlement`、T2 mint、T3 RelayState(pub_addr 进 RelayAllocated/RelaySession)
- Produces: WS 行为契约——`RelayAllocate{to_device}` 经实时查库门禁(账号存在且 relay_config.enabled 且余量>0 且 `count_for_account<2` 且对端同账号在线)→ `mint_ticket(expires_at=now+86400)` → 插入注册表(rate=账号 rate_kbps)→ 向请求方回 `RelayAllocated{sid, tag(请求方槽), relay_addr, expires_at, rate_kbps}` + 向对端推 `RelaySession{sid, alive:true, tag(对端槽), relay_addr, expires_at}`;拒绝:Error **4**(未开通,含账号不存在)/**5**(余量≤0)/**6**(并发≥2)/**2**(对端不在线,复用)

- [ ] **Step 1: 写失败测试**——happy(两帧都到、tag 互异、rate=账号配置);四类拒绝码各自断言;`relay disable` 后 ≤60s 巡检杀会话(测试注入:直接调巡检函数一次等效);对端为旧客户端(不解新帧)不致请求方路径崩溃(推送后请求方仍正常收到响应)
- [ ] **Step 2: 确认失败**(帧分支不存在 → 旧 ws 对 unknown 走 err 3)
- [ ] **Step 3: 实现**(实时查库为准,决策 4;并发计数查注册表)
- [ ] **Step 4: 通过**
- [ ] **Step 5: Commit** `feat(signaling): WS relay allocation gate (live DB, refusal codes 2/4/5/6)`

### Task 5: 启动旗标 + 部署文档

**Files:**
- Modify: `server/signaling/src/main.rs`(`--relay-port <p>` 绑定,默认 8788;`--relay-addr <host>` 对外通告地址,默认 127.0.0.1(房风对齐 `--stun-addr`);解析 env `MTDSS_RELAY_GLOBAL_KBPS` 默认 2500;`spawn_relay(state, port, format!("{host}:{port}"))`)
- Modify: `scripts/deploy/README.md`(中继小节:UDP 8788 安全组、env 常量、mtdss-admin 开通三步)
- Test: `server/signaling/tests/plain.rs`(追加旗标解析单测——parse 逻辑提为可测函数,沿 `parse_stun_ports` 先例)

**Interfaces:**
- Consumes: T3 `spawn_relay`
- Produces: `pub fn parse_relay_args(...)` 或沿 main 现状的 USAGE/校验函数(实现者沿 `parse_stun_ports` 房风命名)

- [ ] **Step 1: 写失败测试**(端口 0/非数字拒;env 缺省 2500、坏值回退默认+warn)
- [ ] **Step 2: 确认失败 → Step 3: 实现 + README 小节**
- [ ] **Step 4: 全量门禁(本机 workspace+clippy)通过**
- [ ] **Step 5: Commit** `feat(signaling): relay startup flag + deploy docs`
