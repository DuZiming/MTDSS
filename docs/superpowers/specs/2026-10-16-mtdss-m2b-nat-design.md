# MTDSS M2b 设计文档:NAT 打洞

- 日期:2026-10-16
- 状态:已评审(brainstorm 对话定稿)
- 上游规范:[2026-10-15-mtdss-v1-design.md](2026-10-15-mtdss-v1-design.md) §5、§11
- 子项目定位:M2 = M2a(债务清偿,已合并 d97a46e)→ **M2b(NAT 打洞)** → M2c(传输 v2)→ M2d(NAT 矩阵 + 混沌验收)

## 1. 目的与范围

M1/M2a 的连接建立只有本地候选串行 connect,出了局域网不可用。M2b 交付"跨公网 NAT 打洞直连":

| 交付 | 内容 |
|---|---|
| 自建 STUN 服务端 | `server/stun` 新 crate,双端口 Binding,供候选收集与 NAT 预判 |
| STUN 客户端 + 候选收集 | raw socket 上的 binding 查询(移交 quinn 之前),srflx 候选 + 对称 NAT 预判 |
| 双角色端点 | 单 UDP socket 同时 dial/accept 的 quinn 端点重构 |
| 互拨原语 | `punch_connect`:并发互拨 + 先到先用,QUIC 握手即打洞包 |
| 信令编排 | 候选交换双向化(A 也发 Candidates),STUN 地址经 HelloOk 下发 |

**范围外**:M2c(多文件/续传/ClientEvent 语义统一)、M2d(netns NAT 矩阵 + netem 混沌)、加密中继(V2)、对称 NAT 端口预测(V2+)、IPv6 候选策略(编解码天然支持,候选列表保持 M1 的 v4 策略)。

## 2. 对 V1 规范 §5.2 的修订裁定(brainstorm 定稿)

V1 规范原文:"打洞:双方对候选地址对并发发认证 punch 包(magic + 会话 ID + HMAC,防伪造注入)"。

**M2b 修订为"QUIC 即打洞"**:双方在双角色端点上向对方候选并发发起 QUIC 握手,握手 Initial 包即打洞包。理由:

- 防伪造注入目标由 **QUIC Initial + TLS 证书 SPKI 固定**密码学等价达成(注入的假包在握手处失败),零新增认证机制;
- 自定义 HMAC punch 包需要 quinn 自定义 UDP socket 解复用(quinn 版本耦合)+ 自定义帧 + 重放保护,复杂度高一个量级而能力无增量;
- QUIC Initial 自带重传,即 NAT 映射保活。

代价(接受):打洞失败诊断只能靠超时归因,无法区分"包未达"与"握手失败";V1 失败即报错,此诊断精度损失可接受。若 V2 引入中继/端口预测,可再评估显式 punch 协议。

## 3. 自建 STUN 服务端(`server/stun`)

- **形态**:lib + 薄 bin,与信令服务器结构对称;部署于同机(独立进程)。
- **协议**:手写 RFC 5389 Binding 子集(约 200 行,不引入失修依赖):
  - 仅接受 type `0x0001`(Binding Request)、magic cookie `0x2112A442` 正确的报文;无 cookie 的 RFC 3489 老报文拒绝;
  - 响应 type `0x0101`,回显 12 字节事务 ID,**仅含 XOR-MAPPED-ADDRESS(`0x0020`)属性**(不回 SOFTWARE 等,天然限制反射放大系数 ≈1);
  - IPv4/IPv6 源地址均可编码。
- **双端口**:同机监听两个 UDP 端口,默认 `0.0.0.0:3478` 与 `0.0.0.0:3479`(旗标 `--bind`/`--bind-alt` 可配)。映射行为判定只需同 IP 双端口,不需要备选 IP。
- **限速**:每源 IP 令牌桶(默认 20 req/s),超限静默丢弃;无鉴权(公共 STUN 惯例)。
- 参数:端口与限速为代码常量/旗标,测试经库 API 直接驱动。

## 4. STUN 客户端与候选收集(`mtdss-core` 新模块 `net::stun` + 编排)

### 4.1 binding 查询

```rust
/// 在 raw UDP socket 上向 server 发 Binding Request,返回 XOR-MAPPED-ADDRESS。
/// 同步 std 实现:必须在 socket 移交 quinn 之前调用(此后读权归 quinn)。
pub fn binding_query(socket: &std::net::UdpSocket, server: SocketAddr, timeout: Duration) -> Result<SocketAddr>
```

- 随机 12 字节事务 ID;响应校验:类型 0x0101、magic、事务 ID 回显一致,解析 XOR-MAPPED-ADDRESS;超时/垃圾/不匹配均为 Err。
- 单测:请求构造字节、XOR-MAPPED 解析(含手工向量)、截断/篡改/事务 ID 不符拒绝。

### 4.2 候选收集与对称预判

```rust
pub struct GatheredCandidates { pub addrs: Vec<SocketAddr>, pub nat_symmetric: bool }
pub fn gather_candidates(socket: &std::net::UdpSocket, stun_primary: SocketAddr, stun_alt: SocketAddr) -> Result<GatheredCandidates>
```

1. 本地接口地址(沿用现有 RFC1918 优先 + 截断逻辑,端口 = 本 socket 端口);
2. srflx = 主端口映射地址,与本地列表去重后追加;
3. 同 socket 再查备端口:两次映射地址(或端口)不同 ⇒ `nat_symmetric = true`(仅诊断,打洞照常)。

### 4.3 STUN 地址下发

- `ws.proto` 的 `HelloOk` 增加 `repeated string stun_addrs`;
- 信令服务器旗标:`--stun-addr <ip-or-host>`(对外通告地址,默认 `127.0.0.1`)与 `--stun-port <primary>[,<alt>]`(默认 `3478,3479`)→ `AppState.stun_addrs = ["{addr}:{primary}", "{addr}:{alt}"]` → HelloOk 携带。生产部署监听 `0.0.0.0` 时服务器无法自推公网地址,由运维用 `--stun-addr` 显式通告;dev/测试用回环默认;
- 客户端零配置取得;`signal.proto` **零改动**。

## 5. 双角色端点(`core/transport/endpoint.rs`)

```rust
/// 单 socket 双角色:自建 UdpSocket → 调用方先做 STUN 收集 → 交给 quinn。
pub fn make_dual_endpoint(
    socket: std::sync::Arc<std::net::UdpSocket>,
    me: &DeviceIdentity,
    allowed_peer: &VerifyingKey,
) -> Result<quinn::Endpoint>
```

- `quinn::Endpoint::new(server_config, default_client_config, socket, …)` 持有外部 socket;server config = 现有白名单单条(对端 SPKI);dial 沿用 `connect_pinned` 的按连接 client config;
- 既有 `make_endpoint` / `make_server_endpoint` 保留(测试与 M2a 已有路径不受影响);
- 证书模型、接受令牌握手(`accept_pinned` 首条服务端双向流)全部不变——令牌按 **TLS 角色**工作,与应用层发送方/接收方角色解耦(见 §6)。

## 6. 互拨原语与信令编排

### 6.1 `core::transport::punch`

```rust
pub struct PunchConfig {
    pub budget: Duration,           // 总预算,默认 10s(spec §5.4,可调)
    pub handshake_timeout: Duration,// 单候选单次握手超时,默认 2s
}
pub async fn punch_connect(
    ep: &quinn::Endpoint,
    me: &DeviceIdentity,
    peer_pk: &VerifyingKey,
    peer_candidates: &[SocketAddr],
    cfg: &PunchConfig,
) -> Result<quinn::Connection>
```

- `select!`:并发 dial 对方全部候选(每候选包握手超时,轮次间无停顿直至预算耗尽)与 `accept_pinned`,**任一成功即返回**,其余 attempt 随 drop 复位;
- 双方可能各赢一条连接(不同目的连接 ID 可短暂共存),无害;先到先用;
- 角色规则:**发送方恒为发起 ConnectRequest 的一方**;`send_file` 的 `open_bi` 与 TLS 客户端/服务端角色无关,M1 控制流语义保持。

### 6.2 信令编排(`cli/flow.rs`)

```
A: ConnectRequest(session_id)
B: 建双角色端点 + gather_candidates(自己的 STUN 配置)
B: ConnectAccept → Candidates(B 的列表)
A: 收到 Accept 后建双角色端点 + gather → 向 B 回发 Candidates(A 的列表,复用同一消息体,按 session_id 过滤)
A/B: 各自 punch_connect(对方候选, PunchConfig::default())
失败: 预算耗尽 → Err(含本侧 NAT 类型与 spec §5.4 建议)
```

- 现 `run_recv` 的 `accept_window` 串行逻辑删除,由 `punch_connect` 取代;`MAX_CANDIDATES` 上限沿用(每侧 ≤8);
- `run_send` 的逐候选串行 connect 删除,同由 `punch_connect` 取代。

## 7. 错误与诊断

| 场景 | 行为 |
|---|---|
| 预算耗尽 | Err 文案含:本侧 NAT 类型(对称时附"直连成功率较低,建议换网络/同网段")、双方候选数、预算值 |
| STUN 不可达 | srflx 缺失不致命:仅本地候选 + 警告日志(局域网场景仍可直连) |
| 对称 NAT | 预判结果只进诊断文案,不改变流程 |
| 伪造 Initial/垃圾包 | 证书固定使握手失败,超时计入预算,无额外机制 |

`PunchConfig` 可注入:测试用秒级;M2d 矩阵用真实值。

## 8. 测试策略

1. **STUN 编解码单测**:请求构造、XOR-MAPPED 解析(手工向量)、截断/篡改/事务 ID 不符/无 cookie 拒绝;
2. **STUN 服务端集成**:回环双端口真实查询;假应答器按端口回不同映射 ⇒ `nat_symmetric = true`;
3. **双角色端点**:两侧 loopback 互拨,双向轮流胜出均验证(胜者为 TLS 客户端与发送方角色两种组合);
4. **punch 失败路径**:小预算 + 不可达候选 ⇒ 限时报错,错误文案含诊断;
5. **CLI e2e 升级**:信令服务器带本地 STUN(回环双端口),全链路 send/recv(候选含 srflx=回环映射);
6. **安全负例**:向双角色端点灌伪造 Initial 与垃圾 UDP,握手必须失败、预算耗尽后报错;
7. NAT 矩阵(netns)与 netem 混沌属 M2d,本阶段不建。

## 9. 交付与验收

- 门禁:`cargo test --workspace && cargo clippy --workspace -- -D warnings` 双平台 CI 绿;
- 验收清单:
  - [ ] `server/stun` 双端口 Binding 服务,限速生效,编解码单测全绿
  - [ ] 候选收集:本地 + srflx 去重合并;对称 NAT 预判可测
  - [ ] 双角色端点:单 socket 同时 dial/accept,互拨先到先用
  - [ ] 候选双向交换(A/B 各发 Candidates),HelloOk 携带 stun_addrs
  - [ ] CLI e2e 在本地 STUN 配置下全链路直连成功;punch 失败路径限时报错含 NAT 诊断
  - [ ] 安全负例:伪造包不能建立连接
- 流程:独立 worktree 分支 → PR 合入 main;随后启动 M2c(传输 v2)设计。
