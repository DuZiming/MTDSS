# MTDSS M2b(NAT 打洞)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 交付跨公网 NAT 打洞直连:自建 STUN(候选收集+对称预判)、quinn 单 socket 双角色端点、双方并发互拨(QUIC 即打洞)、候选双向交换。

**Architecture:** STUN 服务端为独立小 crate(双端口 Binding,仅 XOR-MAPPED-ADDRESS);客户端在 raw socket 上做 Binding 查询(移交 quinn 之前),收集本地+srflx 候选并做映射行为预判;`make_dual_endpoint` 把 socket 交给 quinn 使其同时 dial/accept;`punch_connect` 在总预算内并发 dial 对方全部候选并与 accept 竞速,先到先用;信令编排改为 A/B 双向各发 Candidates。

**Tech Stack:** Rust 2021;quinn 0.11.12(`Endpoint::new(EndpointConfig, Option<ServerConfig>, UdpSocket 按值, Arc<dyn Runtime>)`,`quinn::TokioRuntime` 为默认 feature);tokio;prost。

**Spec:** `docs/superpowers/specs/2026-10-16-mtdss-m2b-nat-design.md`

## Global Constraints

- 打洞机制为 **QUIC 即打洞**(spec §2 裁定):不新增 HMAC punch 协议、不实现自定义 UDP 解复用 socket
- STUN:RFC 5389 Binding 子集,magic cookie `0x2112A442`,双端口默认 `0.0.0.0:3478`/`:3479`,响应**仅** XOR-MAPPED-ADDRESS(`0x0020`),每源 IP 令牌桶限速默认 20 req/s
- punch 默认参数:总预算 **10s**、单候选握手超时 **2s**(`PunchConfig` 可注入)
- 每侧候选上限 `MAX_CANDIDATES = 8`(沿用既有常量);`signal.proto` **零改动**;`signal.proto`(package mtdss.ws)的 HelloOk 增加 `repeated string stun_addrs = 2`
- 测试门禁(每任务提交前):`cargo test --workspace && cargo clippy --workspace -- -D warnings`
- 本机(Windows):bash 里 `export PATH="$HOME/.cargo/bin:$PATH"; export PROTOC="/c/Users/Administrator/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"`
- 提交信息 conventional 前缀;代码注释跟随仓库中文风格

## Review Focus

1. **伪造/垃圾 UDP 注入打洞端口**:不得建立连接也不得挂死互拨 → Task 5 注入测试(乱灌垃圾包后 punch 仍成功,连接仅经证书固定握手建立)。
2. **无 STUN 配置(老服务器/HelloOk 空表)必须降级可用**:仅本地候选 + 警告,LAN 直连不受影响 → Task 6:既有明文/TLS e2e 在空 stun_addrs 下不改一行仍绿(回归即钉)。
3. **STUN 响应伪造不得被采信**:错事务 ID/错类型/截断必须拒绝 → Task 2 编解码负例。
4. **双方并发互拨的双连接不得破坏传输**:恰好一条连接承载 send/recv → Task 5 双向互拨测试(A、B 各持一条连接跑完整 send_file/recv_files)。
5. **预算耗尽必须限时报错**:不得无限轮次/挂死 → Task 5 失败路径测试(断言在预算+裕量内返回 Err 且文案含候选数)。

---

### Task 1: STUN 服务端(`server/stun` 新 crate)

**Files:**
- Create: `server/stun/Cargo.toml`、`server/stun/src/lib.rs`、`server/stun/src/main.rs`
- Modify: 根 `Cargo.toml`(members 加 `"server/stun"`)
- Test: `server/stun/tests/stun.rs`(新建)

**Interfaces:**
- Consumes: 无(独立 crate,依赖 tokio/anyhow/tracing/rand)
- Produces: `stun::parse_request(&[u8]) -> Option<[u8; 12]>`、`stun::build_response(&[u8; 12], SocketAddr) -> Vec<u8>`、`stun::run_listener(UdpSocket, rate_per_sec: u32, capacity: u32) -> Result<()>`。Task 2/6 的集成测试以 dev-dep 驱动此 crate。

- [ ] **Step 1: 建 crate 骨架并入 workspace**

根 `Cargo.toml` members 改为 `["crates/core", "crates/cli", "proto", "server/signaling", "server/stun"]`。

`server/stun/Cargo.toml`:

```toml
[package]
name = "stun-server"
edition.workspace = true
version.workspace = true

[lib]
name = "stun"
path = "src/lib.rs"

[[bin]]
name = "stun-server"
path = "src/main.rs"

[dependencies]
tokio.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
rand.workspace = true
```

- [ ] **Step 2: 写失败测试**

`server/stun/tests/stun.rs`:

```rust
//! STUN 服务端:编解码向量 + 双端口真实查询 + 限速。

use std::net::SocketAddr;
use std::time::Duration;

const MAGIC: u32 = 0x2112_A442;

/// 测试内构造 Binding Request(与实现解耦):type=0x0001, len=0, magic, txn。
fn request(txn: [u8; 12]) -> Vec<u8> {
    let mut p = Vec::with_capacity(20);
    p.extend_from_slice(&0x0001u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&MAGIC.to_be_bytes());
    p.extend_from_slice(&txn);
    p
}

#[test]
fn parse_request_accepts_valid_rejects_garbage() {
    let txn: [u8; 12] = core::array::from_fn(|i| i as u8);
    assert_eq!(stun::parse_request(&request(txn)), Some(txn));
    // 带无关属性的请求(属性区被忽略)仍接受
    let mut with_attr = request(txn);
    with_attr.extend_from_slice(&0x8028u16.to_be_bytes()); // FINGERPRINT 类型占位
    with_attr.extend_from_slice(&0x0004u16.to_be_bytes());
    with_attr.extend_from_slice(b"abcd");
    let n = with_attr.len();
    with_attr[2..4].copy_from_slice(&((n - 20) as u16).to_be_bytes());
    assert_eq!(stun::parse_request(&with_attr), Some(txn));
    // 负例:截断 / 错类型 / 错 magic / 声明长度超包
    assert_eq!(stun::parse_request(&request(txn)[..19]), None);
    let mut wrong_type = request(txn); wrong_type[0..2].copy_from_slice(&0x0011u16.to_be_bytes());
    assert_eq!(stun::parse_request(&wrong_type), None);
    let mut wrong_magic = request(txn); wrong_magic[4] ^= 0xFF;
    assert_eq!(stun::parse_request(&wrong_magic), None);
    let mut over = request(txn); over[2..4].copy_from_slice(&0xFFFFu16.to_be_bytes());
    assert_eq!(stun::parse_request(&over), None);
}

#[test]
fn response_vector_ipv4() {
    // 手工向量:映射地址 192.0.2.1:32853
    // xport = 0x8055 ^ 0x2112 = 0xA147;xaddr = C0000201 ^ 2112A442 = E112A643
    let txn: [u8; 12] = core::array::from_fn(|i| 0xA0 + i as u8);
    let got = stun::build_response(&txn, "192.0.2.1:32853".parse().unwrap());
    let mut want = Vec::new();
    want.extend_from_slice(&[0x01, 0x01, 0x00, 0x0B]); // type, msg len = 2+2+7
    want.extend_from_slice(&MAGIC.to_be_bytes());
    want.extend_from_slice(&txn);
    want.extend_from_slice(&[0x00, 0x20, 0x00, 0x07]); // XOR-MAPPED-ADDRESS, len 7
    want.extend_from_slice(&[0x00, 0x01, 0xA1, 0x47, 0xE1, 0x12, 0xA6, 0x43][..7 + 1]);
    assert_eq!(got, want);
}

async fn spawn_two() -> (SocketAddr, SocketAddr) {
    let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr_a = a.local_addr().unwrap();
    let addr_b = b.local_addr().unwrap();
    tokio::spawn(stun::run_listener(a, 20, 20));
    tokio::spawn(stun::run_listener(b, 20, 20));
    (addr_a, addr_b)
}

#[tokio::test]
async fn two_loopback_listeners_answer_binding() {
    let (pa, pb) = spawn_two().await;
    for server in [pa, pb] {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let txn: [u8; 12] = rand::random();
        sock.send_to(&request(txn), server).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = [0u8; 128];
        let (n, from) = sock.recv_from(&mut buf).unwrap();
        assert_eq!(from, server);
        assert_eq!(&buf[8..20], &txn); // 事务 ID 回显
        let port = sock.local_addr().unwrap().port();
        // XOR-MAPPED = 127.0.0.1:<本机端口>
        assert_eq!(&buf[26..28], &(port as u16 ^ 0x2112).to_be_bytes());
        assert_eq!(&buf[28..32], &(0x7F000001u32 ^ MAGIC).to_be_bytes());
        let _ = n;
    }
}

#[tokio::test]
async fn rate_limit_drops_beyond_capacity() {
    let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = a.local_addr().unwrap();
    tokio::spawn(stun::run_listener(a, 100, 2)); // 容量 2
    tokio::time::sleep(Duration::from_millis(50)).await;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let mut got = 0;
    for i in 0..4 {
        let txn: [u8; 12] = rand::random();
        let _ = i;
        sock.send_to(&request(txn), addr).unwrap();
        let mut buf = [0u8; 128];
        if sock.recv_from(&mut buf).is_ok() {
            got += 1;
        }
    }
    assert_eq!(got, 2, "capacity=2 must serve exactly 2 of 4 rapid requests");
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p stun-server`
Expected: 编译失败(lib 不存在)。

- [ ] **Step 4: 实现**

`server/stun/src/lib.rs`:

```rust
//! 极简 RFC 5389 STUN Binding 服务端(双端口部署,仅 XOR-MAPPED-ADDRESS)。
//! 与 mtdss-core 的客户端编解码**有意各自独立**(两侧方向不同、部署解耦),
//! 由共用测试向量(same bytes)钉死互通。

use anyhow::Result;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_SUCCESS: u16 = 0x0101;
pub const MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_XOR_MAPPED: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// 解析 Binding Request:返回 12 字节事务 ID。
/// 类型非 0x0001、magic 不符、长度不足或声明长度超包 → None。
pub fn parse_request(pkt: &[u8]) -> Option<[u8; 12]> {
    if pkt.len() < HEADER_LEN {
        return None;
    }
    if u16::from_be_bytes([pkt[0], pkt[1]]) != BINDING_REQUEST {
        return None;
    }
    if u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]) != MAGIC_COOKIE {
        return None;
    }
    let declared = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    if pkt.len() < HEADER_LEN + declared {
        return None;
    }
    let mut txn = [0u8; 12];
    txn.copy_from_slice(&pkt[8..20]);
    Some(txn)
}

/// 构造 Binding Success 响应:回显事务 ID,仅含 XOR-MAPPED-ADDRESS。
/// v4: xport = port ^ (magic>>16), xaddr = addr ^ magic;
/// v6: xaddr = addr ^ (magic || txn)。
pub fn build_response(txn: &[u8; 12], mapped: SocketAddr) -> Vec<u8> {
    let port_xor = (mapped.port() as u16) ^ (MAGIC_COOKIE >> 16) as u16;
    let (family, xaddr): (u8, Vec<u8>) = match mapped.ip() {
        IpAddr::V4(v4) => (
            0x01,
            v4.octets()
                .iter()
                .zip(MAGIC_COOKIE.to_be_bytes())
                .map(|(x, m)| x ^ m)
                .collect(),
        ),
        IpAddr::V6(v6) => {
            let mask = [MAGIC_COOKIE.to_be_bytes(), *txn].concat();
            (0x02, v6.octets().iter().zip(mask).map(|(x, m)| x ^ m).collect())
        }
    };
    let value_len = (3 + xaddr.len()) as u16; // family + xport + xaddr
    let mut out = Vec::with_capacity(HEADER_LEN + 4 + value_len as usize);
    out.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    out.extend_from_slice(&(4 + value_len).to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(txn);
    out.extend_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
    out.extend_from_slice(&value_len.to_be_bytes());
    out.push(0);
    out.push(family);
    out.extend_from_slice(&port_xor.to_be_bytes());
    out.extend_from_slice(&xaddr);
    out
}

/// 每源 IP 令牌桶。
struct Bucket {
    tokens: f64,
    last: Instant,
}

fn allow(buckets: &mut HashMap<IpAddr, Bucket>, ip: IpAddr, rate_per_sec: u32, capacity: u32) -> bool {
    let now = Instant::now();
    let entry = buckets.entry(ip).or_insert(Bucket {
        tokens: capacity as f64,
        last: now,
    });
    entry.tokens = (entry.tokens + now.duration_since(entry.last).as_secs_f64() * rate_per_sec as f64)
        .min(capacity as f64);
    entry.last = now;
    if entry.tokens >= 1.0 {
        entry.tokens -= 1.0;
        true
    } else {
        false
    }
}

/// 单端口监听循环:解析请求 → 限速 → 回响应。垃圾包静默丢弃。
pub async fn run_listener(sock: std::net::UdpSocket, rate_per_sec: u32, capacity: u32) -> Result<()> {
    sock.set_nonblocking(true)?;
    let sock = tokio::net::UdpSocket::from_std(sock)?;
    let mut buckets: HashMap<IpAddr, Bucket> = HashMap::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, src) = sock.recv_from(&mut buf).await?;
        let Some(txn) = parse_request(&buf[..n]) else { continue };
        if !allow(&mut buckets, src.ip(), rate_per_sec, capacity) {
            continue;
        }
        let resp = build_response(&txn, src);
        let _ = sock.send_to(&resp, src).await;
    }
}
```

`server/stun/src/main.rs`:

```rust
//! STUN 服务端入口:--bind/--bind-alt 双端口,默认 0.0.0.0:3478/:3479。

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();
    let mut bind = "0.0.0.0:3478".to_string();
    let mut bind_alt = "0.0.0.0:3479".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bind" => bind = args.next().ok_or_else(|| anyhow::anyhow!("--bind needs a value"))?,
            "--bind-alt" => {
                bind_alt = args.next().ok_or_else(|| anyhow::anyhow!("--bind-alt needs a value"))?
            }
            other => anyhow::bail!("unknown flag {other}; usage: stun-server [--bind a:p] [--bind-alt a:p]"),
        }
    }
    let a = std::net::UdpSocket::bind(&bind)?;
    let b = std::net::UdpSocket::bind(&bind_alt)?;
    tracing::info!("stun on {bind} and {bind_alt}");
    let ha = tokio::spawn(stun::run_listener(a, 20, 20));
    let hb = tokio::spawn(stun::run_listener(b, 20, 20));
    let (ra, rb) = (ha.await?, hb.await?);
    ra?; rb?;
    Ok(())
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p stun-server`
Expected: 4 项测试 PASS。

- [ ] **Step 6: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add Cargo.toml Cargo.lock server/stun/
git commit -m "feat(stun): self-hosted RFC5389 binding server (dual-port, xor-mapped only, rate-limited)"
```

---

### Task 2: STUN 客户端 + 候选收集(`mtdss-core` 新模块 `net`)

**Files:**
- Create: `crates/core/src/net/mod.rs`、`crates/core/src/net/stun.rs`
- Modify: `crates/core/src/lib.rs`(加 `pub mod net;`)、`crates/core/Cargo.toml`(dev-deps 加 stun-server)
- Modify: `crates/cli/src/flow.rs`(删本地 `local_candidates`,改用 `core::net::local_ipv4_addrs`)
- Test: `crates/core/tests/stun_client.rs`(新建);`crates/cli/tests/flow.rs`(`candidates_include_loopback_and_private` 移走/删除)

**Interfaces:**
- Consumes: Task 1 的 `stun-server` lib(dev-dep,集成测试起真实回环服务)
- Produces:
  - `net::local_ipv4_addrs(port: u16) -> Vec<SocketAddr>`(环回第一、RFC1918 优先、截断 8)
  - `net::stun::binding_query(&std::net::UdpSocket, SocketAddr, Duration) -> anyhow::Result<SocketAddr>`
  - `net::stun::GatheredCandidates { addrs: Vec<SocketAddr>, nat_symmetric: bool, stun_ok: bool }`
  - `net::stun::gather_candidates(&std::net::UdpSocket, primary: SocketAddr, alt: SocketAddr) -> anyhow::Result<GatheredCandidates>`
- Task 6 依赖以上全部签名。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/stun_client.rs`:

```rust
//! STUN 客户端:编解码负例 + 真实回环服务 + 对称预判(假应答器)。

use mtdss_core::net::stun::{binding_query, gather_candidates, parse_xor_mapped, build_request};
use std::net::SocketAddr;
use std::time::Duration;

const MAGIC: u32 = 0x2112_A442;
const TIMEOUT: Duration = Duration::from_secs(2);

fn valid_response(txn: [u8; 12], mapped: SocketAddr) -> Vec<u8> {
    let port_xor = (mapped.port() as u16) ^ 0x2112;
    let addr_xor: Vec<u8> = match mapped.ip() {
        std::net::IpAddr::V4(v4) => v4
            .octets()
            .iter()
            .zip(MAGIC.to_be_bytes())
            .map(|(x, m)| x ^ m)
            .collect(),
        std::net::IpAddr::V6(_) => unreachable!("测试仅 v4"),
    };
    let mut out = vec![0x01, 0x01, 0x00, 0x0B];
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(&txn);
    out.extend_from_slice(&[0x00, 0x20, 0x00, 0x07, 0x00, 0x01]);
    out.extend_from_slice(&port_xor.to_be_bytes());
    out.extend_from_slice(&addr_xor);
    out
}

#[test]
fn parse_accepts_and_rejects() {
    let txn: [u8; 12] = core::array::from_fn(|i| 0x10 + i as u8);
    let mapped: SocketAddr = "203.0.113.7:50000".parse().unwrap();
    assert_eq!(parse_xor_mapped(&valid_response(txn, mapped), &txn).unwrap(), mapped);
    // 错事务 ID
    let other = core::array::from_fn(|i| 0x20 + i as u8);
    assert!(parse_xor_mapped(&valid_response(txn, mapped), &other).is_err());
    // 错类型 / 截断 / 声明超包
    let mut bad_type = valid_response(txn, mapped);
    bad_type[0..2].copy_from_slice(&0x0111u16.to_be_bytes());
    assert!(parse_xor_mapped(&bad_type, &txn).is_err());
    assert!(parse_xor_mapped(&valid_response(txn, mapped)[..15], &txn).is_err());
    let mut over = valid_response(txn, mapped);
    over[2..4].copy_from_slice(&0xFFFFu16.to_be_bytes());
    assert!(parse_xor_mapped(&over, &txn).is_err());
    // 无 XOR 属性 → Err
    let mut no_attr = vec![0x01, 0x01, 0x00, 0x00];
    no_attr.extend_from_slice(&MAGIC.to_be_bytes());
    no_attr.extend_from_slice(&txn);
    assert!(parse_xor_mapped(&no_attr, &txn).is_err());
}

#[test]
fn request_is_20_bytes_binding() {
    let txn: [u8; 12] = core::array::from_fn(|i| i as u8);
    let p = build_request(&txn);
    assert_eq!(p.len(), 20);
    assert_eq!(&p[0..2], &0x0001u16.to_be_bytes());
    assert_eq!(&p[2..4], &0u16.to_be_bytes());
    assert_eq!(&p[4..8], &MAGIC.to_be_bytes());
    assert_eq!(&p[8..20], &txn);
}

async fn spawn_real() -> (SocketAddr, SocketAddr) {
    let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (pa, pb) = (a.local_addr().unwrap(), b.local_addr().unwrap());
    tokio::spawn(stun_server_lib_wrap(a));
    tokio::spawn(stun_server_lib_wrap(b));
    (pa, pb)
}

// 薄别名:dev-dep crate 名为 stun(lib 名)
async fn stun_server_lib_wrap(sock: std::net::UdpSocket) {
    let _ = stun::run_listener(sock, 100, 100).await;
}

#[tokio::test]
async fn binding_query_against_real_server() {
    let (pa, _pb) = spawn_real().await;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let got = binding_query(&sock, pa, TIMEOUT).unwrap();
    assert_eq!(got, sock.local_addr().unwrap());
}

/// 假应答器:对任何请求按端口回**不同**映射地址 → 模拟对称 NAT。
fn spawn_fake(pa: SocketAddr, pb: SocketAddr) {
    let handle = |srv: SocketAddr, fake_port: u16| {
        std::thread::spawn(move || {
            let sock = std::net::UdpSocket::bind(srv).unwrap();
            let mut buf = [0u8; 128];
            loop {
                let (n, src) = match sock.recv_from(&mut buf) {
                    Ok(v) => v,
                    Err(_) => return,
                };
                if n < 20 {
                    continue;
                }
                let mut txn = [0u8; 12];
                txn.copy_from_slice(&buf[8..20]);
                let resp = valid_response(txn, format!("198.51.100.9:{fake_port}").parse().unwrap());
                let _ = sock.send_to(&resp, src);
            }
        })
    };
    handle(pa, 40000);
    handle(pb, 40001);
}

#[tokio::test]
async fn gather_detects_symmetric_and_nonsymmetric() {
    // 真实服务:同源同映射 → 非对称
    let (pa, pb) = spawn_real().await;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let g = gather_candidates(&sock, pa, pb).unwrap();
    assert!(g.stun_ok);
    assert!(!g.nat_symmetric);
    // srflx = 127.0.0.1:port,应与本地候选去重(本地已含该回环)
    assert!(g.addrs.contains(&sock.local_addr().unwrap()));

    // 假服务:两端口映射不同 → 对称(取两个空闲回环端口,测试环境无竞争)
    let free = || std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let (fa, fb) = (free(), free());
    let sock2 = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    spawn_fake(fa, fb);
    let g2 = gather_candidates(&sock2, fa, fb).unwrap();
    assert!(g2.stun_ok);
    assert!(g2.nat_symmetric);
    // 对称时 srflx(198.51.100.9:40000)仍进候选列表(多一条路)
    assert!(g2.addrs.iter().any(|a| a.port() == 40000));
}

#[test]
fn local_addrs_ordering_and_cap() {
    use mtdss_core::net::local_ipv4_addrs;
    let c = local_ipv4_addrs(9999);
    assert_eq!(c[0], "127.0.0.1:9999".parse::<SocketAddr>().unwrap());
    assert!(c.len() <= 8);
    let uniq: std::collections::HashSet<_> = c.iter().collect();
    assert_eq!(uniq.len(), c.len());
}

#[tokio::test]
async fn unreachable_stun_degrades_to_local_only() {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead = "127.0.0.1:1".parse().unwrap(); // 无监听
    let g = gather_candidates(&sock, dead, dead).unwrap();
    assert!(!g.stun_ok);
    assert!(!g.nat_symmetric);
    assert!(g.addrs.len() >= 1);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test stun_client`
Expected: 编译失败(`net` 模块不存在)。

- [ ] **Step 3: 实现**

`crates/core/src/net/mod.rs`:

```rust
//! 网络原语:本地候选枚举 + STUN 客户端。

pub mod stun;

use std::net::{IpAddr, SocketAddr};

/// 每侧候选上限(沿用 M1 语义:无界列表拖垮对端互拨窗口)。
pub const MAX_CANDIDATES: usize = 8;

fn is_rfc1918(a: SocketAddr) -> bool {
    match a.ip() {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(_) => false,
    }
}

/// 本机 IPv4 候选:环回第一,随后 RFC1918 私网(最可能真实直连),再其余;截断 8。
pub fn local_ipv4_addrs(port: u16) -> Vec<SocketAddr> {
    let mut out = vec![SocketAddr::from(([127, 0, 0, 1], port))];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        let (mut private, mut other): (Vec<SocketAddr>, Vec<SocketAddr>) = ifaces
            .into_iter()
            .filter(|ia| ia.ip().is_ipv4() && !ia.ip().is_loopback())
            .map(|ia| SocketAddr::new(ia.ip(), port))
            .partition(is_rfc1918);
        out.append(&mut private);
        out.append(&mut other);
    }
    out.truncate(MAX_CANDIDATES);
    out
}
```

`crates/core/src/net/stun.rs`:

```rust
//! STUN Binding 客户端:raw socket 查询(必须发生在 socket 移交 quinn 之前)。
//! 与 server/stun 编解码有意独立,由共用测试向量钉死互通。

use anyhow::{anyhow, Context, Result};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use super::local_ipv4_addrs;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_XOR_MAPPED: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// 构造 Binding Request(20 字节,无属性)。
pub fn build_request(txn: &[u8; 12]) -> [u8; 20] {
    let mut p = [0u8; 20];
    p[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    p[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    p[8..20].copy_from_slice(txn);
    p
}

/// 解析 Binding Success:校验类型/magic/事务回显,扫描属性取首个 XOR-MAPPED-ADDRESS。
pub fn parse_xor_mapped(pkt: &[u8], txn: &[u8; 12]) -> Result<SocketAddr> {
    if pkt.len() < HEADER_LEN {
        return Err(anyhow!("short response"));
    }
    if u16::from_be_bytes([pkt[0], pkt[1]]) != BINDING_SUCCESS {
        return Err(anyhow!("not a binding success"));
    }
    if u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]) != MAGIC_COOKIE {
        return Err(anyhow!("bad magic"));
    }
    if &pkt[8..20] != txn {
        return Err(anyhow!("txn mismatch"));
    }
    let declared = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    anyhow::ensure!(pkt.len() >= HEADER_LEN + declared, "declared length exceeds packet");
    let mut off = HEADER_LEN;
    let end = HEADER_LEN + declared;
    while off + 4 <= end {
        let t = u16::from_be_bytes([pkt[off], pkt[off + 1]]);
        let l = u16::from_be_bytes([pkt[off + 2], pkt[off + 3]]) as usize;
        let v = off + 4;
        anyhow::ensure!(v + l <= end, "attribute overruns message");
        if t == ATTR_XOR_MAPPED && l >= 8 {
            // family(1)+xport(2)+xaddr(4/16) 最少 8 字节(v4 恰 8,取前 8)
            let family = pkt[v + 1];
            let xport = u16::from_be_bytes([pkt[v + 2], pkt[v + 3]]) ^ (MAGIC_COOKIE >> 16) as u16;
            return match family {
                0x01 => {
                    let mut a = [0u8; 4];
                    a.copy_from_slice(&pkt[v + 4..v + 8]);
                    let m = MAGIC_COOKIE.to_be_bytes();
                    let ip = std::net::Ipv4Addr::new(
                        a[0] ^ m[0], a[1] ^ m[1], a[2] ^ m[2], a[3] ^ m[3],
                    );
                    Ok(SocketAddr::new(std::net::IpAddr::V4(ip), xport))
                }
                0x02 => {
                    anyhow::ensure!(l >= 20, "v6 xor-mapped too short");
                    let mask = [MAGIC_COOKIE.to_be_bytes(), *txn].concat();
                    let mut a = [0u8; 16];
                    a.copy_from_slice(&pkt[v + 4..v + 20]);
                    let oct: Vec<u8> = a.iter().zip(mask).map(|(x, m)| x ^ m).collect();
                    let ip = std::net::Ipv6Addr::from(oct.as_slice().try_into().unwrap());
                    Ok(SocketAddr::new(std::net::IpAddr::V6(ip), xport))
                }
                f => Err(anyhow!("unknown family {f}")),
            };
        }
        off = v + l + (l & 3 == 0 ? 0 : 4 - (l & 3)); // 属性 4 字节对齐
    }
    Err(anyhow!("no xor-mapped-address attribute"))
}

/// 在 raw socket 上向 server 发 Binding,返回映射地址(阻塞,socket 未进 quinn 前用)。
pub fn binding_query(socket: &std::net::UdpSocket, server: SocketAddr, timeout: Duration) -> Result<SocketAddr> {
    let txn: [u8; 12] = rand::random();
    let req = build_request(&txn);
    socket.send_to(&req, server).context("stun send")?;
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 576];
    loop {
        let remain = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow!("stun timeout ({server})"))?;
        socket
            .set_read_timeout(Some(remain))
            .context("stun read timeout")?;
        let (n, from) = socket.recv_from(&mut buf).context("stun recv")?;
        if from != server {
            continue; // 迟到的其他来源包,忽略
        }
        if let Ok(mapped) = parse_xor_mapped(&buf[..n], &txn) {
            return Ok(mapped);
        }
        // 不匹配的响应:继续等(可能碰撞了旧事务)
    }
}

/// 候选收集结果。
pub struct GatheredCandidates {
    pub addrs: Vec<SocketAddr>,
    pub nat_symmetric: bool,
    /// STUN 主端口查询是否成功(失败 → 仅本地候选,调用方记警告)。
    pub stun_ok: bool,
}

/// 收集:本地接口 + srflx(主端口)+ 对称预判(同 socket 查备端口,映射不同 ⇒ 对称)。
/// STUN 不可达不报错:降级为本地候选(stun_ok=false)。
pub fn gather_candidates(
    socket: &std::net::UdpSocket,
    primary: SocketAddr,
    alt: SocketAddr,
) -> Result<GatheredCandidates> {
    let port = socket.local_addr().context("socket local addr")?.port();
    let mut addrs = local_ipv4_addrs(port);
    let t = Duration::from_secs(2);
    match binding_query(socket, primary, t) {
        Ok(m1) => {
            if !addrs.contains(&m1) {
                addrs.push(m1);
                if addrs.len() > super::MAX_CANDIDATES {
                    addrs.pop(); // srflx 挤不进就丢:本地候选优先
                }
            }
            let nat_symmetric = binding_query(socket, alt, t).map(|m2| m2 != m1).unwrap_or(false);
            Ok(GatheredCandidates { addrs, nat_symmetric, stun_ok: true })
        }
        Err(_) => Ok(GatheredCandidates { addrs, nat_symmetric: false, stun_ok: false }),
    }
}

```

`crates/core/src/lib.rs` 加 `pub mod net;`。`crates/core/Cargo.toml` `[dev-dependencies]` 加:

```toml
stun-server = { path = "../../server/stun" }
```

`crates/cli/src/flow.rs`:删除 `local_candidates`、`is_rfc1918`、`MAX_CANDIDATES` 与其单元测试(`ws_url` 等其余测试保留);`crates/cli/tests/flow.rs` 删除 `candidates_include_loopback_and_private`(该逻辑已由 core 的 `local_addrs_ordering_and_cap` 覆盖)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test stun_client && cargo test -p mtdss-cli && cargo test -p stun-server`
Expected: 全 PASS(cli 既有测试在删除 local_candidates 后编译通过、全绿)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/ Cargo.lock
git commit -m "feat(core): stun client, candidate gathering with symmetric-nat probe"
```

---

### Task 3: HelloOk 携带 stun_addrs(服务端旗标 + 客户端事件)

**Files:**
- Modify: `proto/proto/signal.proto:20`(HelloOk;注意此文件 package 是 mtdss.ws)
- Modify: `server/signaling/src/lib.rs`(AppState + 4 处构造)、`server/signaling/src/main.rs`(--stun-addr/--stun-port)
- Modify: `server/signaling/src/ws.rs`(HelloOk 回带)
- Modify: `crates/core/src/signaling/client.rs`(ClientEvent::HelloOk)
- Test: `server/signaling/tests/ws.rs`、`crates/core/tests/signaling_client.rs`(追加)

**Interfaces:**
- Consumes: 既有 AppState/HelloOk 流程
- Produces: `AppState.stun_addrs: Vec<String>`;`spawn_test_server_with(keepalive: Keepalive, stun_addrs: Vec<String>)`(签名变更);`ClientEvent::HelloOk { stun_addrs: Vec<String> }`。Task 6 依赖事件类型与 spawn 签名。

- [ ] **Step 1: 写失败测试**

`server/signaling/tests/ws.rs` 追加(复用文件内既有 `register` 辅助):

```rust
#[tokio::test]
async fn hello_ok_carries_stun_addrs() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let (addr, _state) = signaling::spawn_test_server_with(
        signaling::ws::Keepalive::default(),
        vec!["198.51.100.10:3478".into(), "198.51.100.10:3479".into()],
    )
    .await;
    let (jwt, dev, _seed) = register(&addr, "stun@x.y", &[9u8; 32], "st").await;
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let hello = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: dev,
            jwt,
        })),
    };
    sink.send(WsMessage::Binary(hello.encode_to_vec())).await.unwrap();
    // 第一帧即 HelloOk,携带 stun_addrs
    match stream.next().await {
        Some(Ok(WsMessage::Binary(b))) => {
            let f = WsServerFrame::decode(b.as_slice()).unwrap();
            match f.frame {
                Some(ws_server_frame::Frame::HelloOk(ok)) => {
                    assert_eq!(ok.stun_addrs, vec!["198.51.100.10:3478".to_string(), "198.51.100.10:3479".to_string()]);
                }
                other => panic!("expected HelloOk, got {other:?}"),
            }
        }
        other => panic!("unexpected {other:?}"),
    }
}
```

`crates/core/tests/signaling_client.rs` 追加(core 已 dev-dep `signaling`;注册辅助若无则按文件内既有 `register` 复用或以下列内联版自足):

```rust
#[tokio::test]
async fn hello_ok_surfaced_as_event() {
    use ed25519_dalek::SigningKey;
    let (addr, _s) = signaling::spawn_test_server_with(
        signaling::ws::Keepalive::default(),
        vec!["198.51.100.10:3478".to_string(), "198.51.100.10:3479".to_string()],
    )
    .await;
    // 内联注册:拿一个合法 JWT + device_id(pubkey 用真实 Ed25519 公钥)
    let seed = [7u8; 32];
    let vk = SigningKey::from_bytes(&seed).verifying_key();
    let http = reqwest::Client::new();
    let c: serde_json::Value = http
        .post(format!("http://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": "ho@x.y" }))
        .send().await.unwrap().json().await.unwrap();
    let v: serde_json::Value = http
        .post(format!("http://{addr}/auth/verify"))
        .json(&serde_json::json!({
            "email": "ho@x.y",
            "code": c["code"].as_str().unwrap(),
            "pubkey": hex::encode(vk.as_bytes()),
            "name": "ho",
            "platform": "t"
        }))
        .send().await.unwrap().json().await.unwrap();
    let jwt = v["jwt"].as_str().unwrap().to_string();
    let dev = v["device_id"].as_str().unwrap().to_string();

    let (_c, mut ev) = mtdss_core::signaling::client::SignalingClient::connect(
        &format!("ws://{addr}/ws"), &jwt, &dev).await.unwrap();
    match ev.recv().await.unwrap() {
        mtdss_core::signaling::client::ClientEvent::HelloOk { stun_addrs } => {
            assert_eq!(stun_addrs.len(), 2);
            assert_eq!(stun_addrs[0], "198.51.100.10:3478");
        }
        other => panic!("expected HelloOk first, got {other:?}"),
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling --test ws hello_ok && cargo test -p mtdss-core --test signaling_client hello_ok`
Expected: 编译失败(字段/事件不存在)。

- [ ] **Step 3: 实现**

1. `proto/proto/signal.proto`:`message HelloOk { string account_id = 1; repeated string stun_addrs = 2; }`(PROTOC 提醒同前)。
2. `AppState` 加 `pub stun_addrs: Vec<String>,`;四处构造补 `stun_addrs: Vec::new()`(main.rs 用解析值);`spawn_test_server_with` 签名改为 `(keepalive: Keepalive, stun_addrs: Vec<String>)`;原 keepalive 测试调用处补 `Vec::new()`;`spawn_test_server`/`spawn_test_tls_server`/`test_state` 传空。
3. `ws.rs` HelloOk 构造处:`stun_addrs: state.stun_addrs.clone()`。
4. `main.rs` 参数:`--stun-addr <ip-or-host>`(默认 `127.0.0.1`)、`--stun-port <p>[,<alt>]`(默认 `3478,3479`),解析为 `vec!["{addr}:{p}", "{addr}:{alt}"]` 存入 AppState(非法端口组合直接 bail)。
5. `client.rs`:`ClientEvent` 加 `HelloOk { stun_addrs: Vec<String> }`;事件循环 `Frame::HelloOk(m) => ClientEvent::HelloOk { stun_addrs: m.stun_addrs }`(不再 `continue` 吞掉)。**同步修改 Task 3 之前存在的消费点**:`crates/core/tests/signaling_client.rs` 的 `client_detects_silent_server`(假服务器先发 HelloOk → 循环遇 `HelloOk` 事件 continue 而非 panic——把 `Ok(Some(other)) => panic!` 分支改为忽略 HelloOk);`crates/cli/src/flow.rs` 的 `wait_opened`/`wait_connect_request` 匹配臂补 `ClientEvent::HelloOk { .. } => {}`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p signaling --test ws && cargo test -p mtdss-core --test signaling_client && cargo test -p mtdss-cli`
Expected: 新测试 PASS;既有测试(含改动的 silent-server 与 flow 等待循环)全绿。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add proto/ server/ crates/ Cargo.lock
git commit -m "feat(signaling): advertise stun endpoints via HelloOk (--stun-addr/--stun-port)"
```

---

### Task 4: 双角色端点(`make_dual_endpoint`)

**Files:**
- Modify: `crates/core/src/transport/endpoint.rs`(追加函数)
- Test: `crates/core/tests/dual_endpoint.rs`(新建)

**Interfaces:**
- Consumes: 既有 `server_rustls_config`/`connect_pinned`/`accept_pinned`
- Produces: `endpoint::make_dual_endpoint(socket: std::net::UdpSocket, me: &DeviceIdentity, allowed: &HashMap<String, VerifyingKey>) -> anyhow::Result<quinn::Endpoint>`(socket 按值移交 quinn——spec 原文写 Arc,以 quinn 0.11 实际签名为准的等价修正)。Task 5/6 依赖。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/dual_endpoint.rs`:

```rust
//! 双角色端点:单 socket 同时 dial/accept;两端互拨各赢一条连接。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::endpoint as ep;
use std::collections::HashMap;

fn pair() -> (DeviceIdentity, DeviceIdentity, HashMap<String, ed25519_dalek::VerifyingKey>) {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(b.device_id.clone(), b.public_key());
    (a, b, allowed)
}

fn bind0() -> (std::net::UdpSocket, std::net::SocketAddr) {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    (s, a)
}

#[tokio::test]
async fn dual_both_directions() {
    let (a, b, _) = pair();
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b.public_key())].into_iter().collect();

    let (sa, addr_a) = bind0();
    let (sb, addr_b) = bind0();
    let ep_a = ep::make_dual_endpoint(sa, &a, &allowed_b).unwrap(); // A 的服务端白名单: B
    let ep_b = ep::make_dual_endpoint(sb, &b, &allowed_a).unwrap();

    // 方向 1:A 拨 B,B 收
    let c1 = ep::connect_pinned(&ep_a, addr_b, &b.public_key(), &a).await.unwrap();
    let (c1s, dev) = ep::accept_pinned(&ep_b, &allowed_a).await.unwrap();
    assert_eq!(dev, a.device_id);
    drop(c1); drop(c1s);

    // 方向 2:B 拨 A,A 收(同一对端点、同一 socket,反向亦通)
    let c2 = ep::connect_pinned(&ep_b, addr_a, &a.public_key(), &b).await.unwrap();
    let (c2s, dev2) = ep::accept_pinned(&ep_a, &allowed_b).await.unwrap();
    assert_eq!(dev2, b.device_id);
    drop(c2); drop(c2s);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test dual_endpoint`
Expected: 编译失败(`make_dual_endpoint` 不存在)。

- [ ] **Step 3: 实现**

`crates/core/src/transport/endpoint.rs` 追加:

```rust
/// 单 socket 双角色端点:调用方先在 raw socket 上完成 STUN 候选收集
/// (net::stun::gather_candidates),再按值把 socket 移交 quinn。
/// 此后该端点既能 connect_pinned(拨)又能 accept_pinned(收)。
pub fn make_dual_endpoint(
    socket: std::net::UdpSocket,
    me: &DeviceIdentity,
    allowed: &HashMap<String, VerifyingKey>,
) -> Result<quinn::Endpoint> {
    let quic_server = QuicServerConfig::try_from(server_rustls_config(me, allowed)?)?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server));
    // quinn 0.11:Endpoint::new 按值接收 std socket;TokioRuntime 为默认 feature。
    // 必须在 tokio 运行时上下文内调用(driver 会 spawn)。
    quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .map_err(|e| anyhow!("dual endpoint: {e}"))
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test dual_endpoint && cargo test -p mtdss-core --test transfer`
Expected: 全 PASS(transfer 等既有端点不受影响)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/core/
git commit -m "feat(core): dual-role quinn endpoint on externally-owned socket"
```

---

### Task 5: 互拨原语 `punch_connect`

**Files:**
- Create: `crates/core/src/transport/punch.rs`
- Modify: `crates/core/src/transport/mod.rs`(加 `pub mod punch;`)
- Test: `crates/core/tests/punch.rs`(新建)

**Interfaces:**
- Consumes: Task 4 `make_dual_endpoint`;既有 `connect_pinned`/`accept_pinned`/`send_file`/`recv_files`
- Produces:
  - `transport::punch::PunchConfig { budget: Duration, handshake_timeout: Duration }`,`Default = {10s, 2s}`
  - `transport::punch::punch_connect(ep: &quinn::Endpoint, me: &DeviceIdentity, allowed: &HashMap<String, VerifyingKey>, peer_pk: &VerifyingKey, peer_candidates: &[SocketAddr], cfg: &PunchConfig) -> anyhow::Result<quinn::Connection>`
- Task 6 依赖。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/punch.rs`:

```rust
//! punch_connect:互拨先到先用、预算限时报错、垃圾包注入无碍、双连接各用一条。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::{endpoint as ep, punch, transfer};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn dual(id: &DeviceIdentity, peer_pk: ed25519_dalek::VerifyingKey, peer_id: &str) -> (quinn::Endpoint, std::net::SocketAddr) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(peer_id.to_string(), peer_pk)].into_iter().collect();
    let e = ep::make_dual_endpoint(s, id, &allowed).unwrap();
    (e, addr)
}

#[tokio::test]
async fn mutual_punch_both_sides_succeed_and_transfer_runs() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let (ep_a, addr_a) = dual(&a, b.public_key(), &b.device_id);
    let (ep_b, addr_b) = dual(&b, a.public_key(), &a.device_id);
    let cfg = punch::PunchConfig::default();

    let allowed_a: HashMap<String, _> = [(b.device_id.clone(), b.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, _> = [(a.device_id.clone(), a.public_key())].into_iter().collect();

    let ja = {
        let ep = ep_a.clone(); let me = DeviceIdentity::from_seed(&a.signing.to_bytes());
        let allowed = allowed_a.clone();
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed, &b.public_key(), &[addr_b], &cfg).await
        })
    };
    let jb = {
        let ep = ep_b.clone(); let me = b.clone_seed_identity();
        let allowed = allowed_b.clone();
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed, &a.public_key(), &[addr_a], &cfg).await
        })
    };
    let conn_a = ja.await.unwrap().unwrap();
    let conn_b = jb.await.unwrap().unwrap();

    // 双方各持一条(可能互为两端、也可能各自独立),用它们跑完整传输
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("p.bin");
    let data = vec![0xABu8; 4096];
    std::fs::write(&f, &data).unwrap();
    let dst = tempfile::tempdir().unwrap();
    let send = tokio::spawn(transfer::send_file(&conn_a, &f, "p.bin", &a, &|_, _| {}));
    let recv = tokio::spawn(transfer::recv_files(&conn_b, dst.path(), &b));
    send.await.unwrap().unwrap();
    let landed = recv.await.unwrap().unwrap();
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}
```

(`DeviceIdentity::from_seed(&a.signing.to_bytes())` 重建同身份供 `tokio::spawn` 取所有权;`SigningKey::to_bytes` 返回 `[u8; 32]`,与 `from_seed` 入参吻合。)

继续测试(同文件追加):

```rust
#[tokio::test]
async fn budget_exhaustion_errors_promptly() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let (ep_a, _) = dual(&a, b.public_key(), &b.device_id);
    // 对端端点存在但无人拨/无人收:用真实但永不 accept 的对端地址
    let dead = UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead_addr = dead.local_addr().unwrap(); // 绑定但丢弃 = 黑洞
    drop(dead);
    let cfg = punch::PunchConfig { budget: Duration::from_millis(600), handshake_timeout: Duration::from_millis(200) };
    let t0 = Instant::now();
    let err = punch::punch_connect(&ep_a, &a, &HashMap::new(), &b.public_key(), &[dead_addr], &cfg)
        .await
        .unwrap_err();
    let dt = t0.elapsed();
    assert!(dt < Duration::from_secs(3), "must fail within budget+slack, took {dt:?}");
    assert!(err.to_string().contains("no path established"), "got: {err:#}");
    assert!(err.to_string().contains("1"), "mentions candidate count");
}

#[tokio::test]
async fn garbage_udp_does_not_break_or_hang_punch() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let (ep_a, _) = dual(&a, b.public_key(), &b.device_id);
    let (ep_b, addr_b) = dual(&b, a.public_key(), &a.device_id);
    // 向 B 灌伪造 QUIC-ish 垃圾包(首字节 0xC0 长头 + 随机)
    let junk = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut pkt = vec![0xC0u8; 1200];
    for (i, b_) in pkt.iter_mut().enumerate().skip(1) {
        *b_ = (i as u8).wrapping_mul(31).wrapping_add(7);
    }
    for _ in 0..8 {
        junk.send_to(&pkt, addr_b).unwrap();
    }
    // B 侧 accept(punch 内部)仍可与 A 的真拨号完成握手
    let allowed_a: HashMap<String, _> = [(b.device_id.clone(), b.public_key())].into_iter().collect();
    let cfg = punch::PunchConfig::default();
    punch::punch_connect(&ep_a, &a, &allowed_a, &b.public_key(), &[addr_b], &cfg)
        .await
        .expect("real dial must succeed despite garbage");
    // B 侧相应 accept 成功由上一测试模式覆盖;此处只验 A 侧成功。
}

#[tokio::test]
async fn empty_candidates_accept_only_still_wins() {
    // A 无对端候选:仅 accept;B 拨 A → A 的 punch 应经 accept 分支成功
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let (ep_a, addr_a) = dual(&a, b.public_key(), &b.device_id);
    let (ep_b, _) = dual(&b, a.public_key(), &a.device_id);
    let allowed_a: HashMap<String, _> = [(b.device_id.clone(), b.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, _> = [(a.device_id.clone(), a.public_key())].into_iter().collect();
    let cfg = punch::PunchConfig::default();
    let ja = {
        let ep = ep_a.clone(); let me = DeviceIdentity::from_seed(&a.signing.to_bytes());
        let allowed = allowed_a.clone();
        tokio::spawn(async move { punch::punch_connect(&ep, &me, &allowed, &b.public_key(), &[], &cfg).await })
    };
    let jb = {
        let ep = ep_b.clone(); let me = DeviceIdentity::from_seed(&b.signing.to_bytes());
        let allowed = allowed_b.clone();
        tokio::spawn(async move { punch::punch_connect(&ep, &me, &allowed, &a.public_key(), &[addr_a], &cfg).await })
    };
    ja.await.unwrap().unwrap();
    jb.await.unwrap().unwrap();
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test punch`
Expected: 编译失败(`punch` 模块不存在)。

- [ ] **Step 3: 实现**

`crates/core/src/transport/punch.rs`:

```rust
//! 互拨原语:QUIC 即打洞(spec M2b §2/§6)。
//! 在总预算内并发 dial 对方全部候选,与本地 accept 竞速;任一成功即返回。
//! 双方并发互拨可能各赢一条连接(不同 DCID),败者随 drop 复位,无害。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ed25519_dalek::VerifyingKey;
use futures_util::stream::{FuturesUnordered, StreamExt};
use quinn::Connection;

use crate::identity::DeviceIdentity;
use crate::transport::endpoint as ep;

/// punch 参数(测试注入秒级;生产 10s/2s,spec §7)。
#[derive(Debug, Clone, Copy)]
pub struct PunchConfig {
    pub budget: Duration,
    pub handshake_timeout: Duration,
}

impl Default for PunchConfig {
    fn default() -> Self {
        Self { budget: Duration::from_secs(10), handshake_timeout: Duration::from_secs(2) }
    }
}

/// 互拨:ep 必须是双角色端点(make_dual_endpoint);allowed 为本端服务端白名单
/// (单条:对端);peer_candidates 为对方候选。返回首条成功的连接。
pub async fn punch_connect(
    ep: &quinn::Endpoint,
    me: &DeviceIdentity,
    allowed: &HashMap<String, VerifyingKey>,
    peer_pk: &VerifyingKey,
    peer_candidates: &[SocketAddr],
    cfg: &PunchConfig,
) -> Result<Connection> {
    let deadline = tokio::time::Instant::now() + cfg.budget;
    let accept = ep::accept_pinned(ep, allowed);
    tokio::pin!(accept);

    let tried = peer_candidates.len();
    let mut round: u32 = 0;
    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            break;
        };
        let per = cfg.handshake_timeout.min(remaining);
        let mut dials = FuturesUnordered::new();
        for &c in peer_candidates {
            dials.push(async move {
                match tokio::time::timeout(per, ep::connect_pinned(ep, c, peer_pk, me)).await {
                    Ok(Ok(conn)) => Ok(conn),
                    Ok(Err(e)) => Err(anyhow!("dial {c}: {e:#}")),
                    Err(_) => Err(anyhow!("dial {c}: handshake timeout")),
                }
            });
        }
        round += 1;
        // 本轮:dial 全部结算;任一成功或 accept 成功立即返回
        let mut last_err: Option<anyhow::Error> = None;
        loop {
            tokio::select! {
                r = &mut accept => {
                    let (conn, _dev) = r?;
                    return Ok(conn);
                }
                r = dials.next() => match r {
                    Some(Ok(conn)) => return Ok(conn),
                    Some(Err(e)) => { last_err = Some(e); }
                    None => break,
                },
            }
        }
        if let Some(remain) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            tokio::time::sleep(remain.min(Duration::from_millis(250))).await;
        }
        let _ = &last_err; // 下一轮重试;last_err 供调试(可选 tracing)
        tracing::debug!("punch round {round} exhausted, retrying");
    }
    Err(anyhow!(
        "no path established within budget {:?} ({tried} peer candidates tried)",
        cfg.budget
    ))
}
```

`crates/core/src/transport/mod.rs` 加 `pub mod punch;`。

(说明:deadline 内无候选也空转 accept——`dials` 为空时内层 `dials.next()` 立即 `None` break,再 sleep 250ms,accept 持续在 select 中被 poll,正确。)

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test punch`
Expected: 4 项测试 PASS(互拨+传输、预算耗尽、垃圾包、仅 accept)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/core/
git commit -m "feat(core): punch_connect mutual-dial primitive (QUIC-as-punch)"
```

---

### Task 6: 信令编排双向候选 + CLI e2e + demo 文档

**Files:**
- Modify: `crates/cli/src/flow.rs`(run_send/run_recv 重写连接段)
- Modify: `docs/demo-m1.md`(STUN 一节)
- Test: `crates/cli/tests/cli_e2e.rs`(追加带 STUN 的 e2e)

**Interfaces:**
- Consumes: Task 2 `net::stun::gather_candidates`/`GatheredCandidates`、Task 3 `ClientEvent::HelloOk`+`spawn_test_server_with`、Task 4 `make_dual_endpoint`、Task 5 `punch_connect`/`PunchConfig`
- Produces: 无新库接口;M2b 验收在此落定。

- [ ] **Step 1: 写失败测试**

`crates/cli/tests/cli_e2e.rs` 追加:

```rust
/// M2b:信令服务器通告本地回环 STUN(真实起 stun-server),全链路
/// 候选收集(srflx=回环映射)→ 双向候选交换 → punch_connect → 传输。
#[tokio::test]
async fn cli_send_recv_with_stun_punch() {
    let run = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        // 真实 STUN 双端口(回环)
        let sa = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let sb = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let (pa, pb) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
        tokio::spawn(async move { let _ = stun::run_listener(sa, 100, 100).await; });
        tokio::spawn(async move { let _ = stun::run_listener(sb, 100, 100).await; });

        let (addr, _state) = signaling::spawn_test_server_with(
            signaling::ws::Keepalive::default(),
            vec![pa.to_string(), pb.to_string()],
        )
        .await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) = register_rest(addr, "punch@x.y",
            &hex::encode(id_a.public_key().as_bytes()), "A").await;
        let (jwt_b, ref_b, dev_b) = register_rest(addr, "punch@x.y",
            &hex::encode(id_b.public_key().as_bytes()), "B").await;

        let sess_b = Session { server: server.clone(), jwt: jwt_b, refresh: ref_b, device_id: dev_b.clone(), ca: None };
        sess_b.save(home_b.path()).unwrap();
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move { mtdss_cli::flow::run_recv(&sess, &home, &out).await.expect("run_recv") })
        };

        // 等 B 上线
        let sess_a = Session { server: server.clone(), jwt: jwt_a, refresh: ref_a, device_id: dev_a.clone(), ca: None };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| d["device_id"].as_str() == Some(dev_b.as_str())
                && d["online"].as_bool().unwrap_or(false)) { break; }
            assert!(std::time::Instant::now() < deadline, "B never online");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        let src = tempfile::tempdir().unwrap();
        let name = "打洞 成功 🕳.bin";
        let fpath = src.path().join(name);
        let mut data = vec![0u8; 300_000]; // 跨块
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        mtdss_cli::flow::run_send(&sess_a, home_a.path(), &fpath, &dev_b, &|_| {})
            .await
            .expect("run_send");
        let landed = tokio::time::timeout(std::time::Duration::from_secs(15), b_task)
            .await.expect("recv timeout").unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), data);
    });
    run.await.expect("stun punch e2e exceeded 90s");
}
```

`crates/cli/Cargo.toml` dev-deps 加 `stun-server = { path = "../../server/stun" }`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-cli --test cli_e2e cli_send_recv_with_stun_punch`
Expected: FAIL——现实现 B 不回发候选、A 不互拨(行为按实际报错定位:大概率是 A 等不到"对端也拨自己"而在旧串行路径成功或超时;无论何种失败即 RED 基线,实现后同一测试必须 PASS)。

- [ ] **Step 3: 实现 flow 编排**

`crates/cli/src/flow.rs`:

1. 新增辅助:

```rust
use mtdss_core::net::stun::{gather_candidates, GatheredCandidates};
use mtdss_core::transport::{endpoint as ep, punch};

/// 从事件流取 HelloOk 的 stun 下发(连接后第一帧,确定性顺序)。
async fn wait_hello_stun(ev: &mut mpsc::Receiver<ClientEvent>) -> Result<Vec<String>> {
    loop {
        match ev.recv().await {
            Some(ClientEvent::HelloOk { stun_addrs }) => return Ok(stun_addrs),
            Some(ClientEvent::Presence { .. }) | Some(ClientEvent::Relayed { .. }) => continue,
            Some(ClientEvent::Error { code, message }) =>
                anyhow::bail!("signaling error {code}: {message}"),
            Some(ClientEvent::Closed) | None => anyhow::bail!("signaling closed before HelloOk"),
        }
    }
}

/// 解析对端候选字符串 → SocketAddr(跳过不可解析并告警,截断 MAX_CANDIDATES)。
fn parse_candidates(addrs: &[String]) -> Vec<SocketAddr> {
    addrs.iter()
        .filter_map(|a| match a.parse::<SocketAddr>() {
            Ok(s) => Some(s),
            Err(_) => {
                tracing::warn!("peer sent unparsable candidate {a}");
                None
            }
        })
        .take(mtdss_core::net::MAX_CANDIDATES)
        .collect()
}

/// 绑定 raw socket → STUN 候选收集 → 移交 quinn 建双角色端点(白名单=对端)。
/// 返回 (端点, 本端候选, 本侧对称 NAT 标记)。
fn setup_punch_endpoint(
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    stun: &[String],
) -> Result<(quinn::Endpoint, Vec<SocketAddr>, bool)> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let parsed: Vec<SocketAddr> = stun.iter().filter_map(|s| s.parse().ok()).collect();
    let (primary, alt) = match (parsed.first(), parsed.get(1)) {
        (Some(p), Some(a)) => (*p, *a),
        _ => {
            tracing::warn!("no usable STUN endpoints advertised; local candidates only");
            let local = sock.local_addr()?;
            (local, local) // primary 查询必失败 → gather 降级本地
        }
    };
    let g = gather_candidates(&sock, primary, alt)?;
    if !g.stun_ok {
        tracing::warn!("STUN unreachable; local candidates only");
    }
    let nat_symmetric = g.nat_symmetric;
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(peer_id.to_string(), peer_pk.clone());
    let epn = ep::make_dual_endpoint(sock, me, &allowed)?;
    Ok((epn, g.addrs, nat_symmetric))
}

/// punch 失败的统一诊断包装(spec §7:含本侧 NAT 类型与建议)。
fn punch_err(e: anyhow::Error, nat_symmetric: bool, mine: usize) -> anyhow::Error {
    let mut msg = format!("{e:#}; local candidates: {mine}");
    if nat_symmetric {
        msg.push_str("; local NAT appears symmetric (直连成功率较低,建议换网络或同网段)");
    }
    anyhow::anyhow!(msg)
}
```

2. `run_send` 连接段(自"等 ConnectAccept"之后)改为:

```rust
// 原:等 Candidates → 逐候选串行 connect。新:Accept 后建双角色端点,
// 回发本端 Candidates,再等对端 Candidates,双方互拨。
let stun = wait_hello_stun(&mut ev).await?;
let (epn, mine, nat_symmetric) = setup_punch_endpoint(&me, to_device, &peer_pk, &stun)?;
relay_with_timeout(&client, to_device,
    &seal(&me, &peer_pk, &SignalBody::Candidates {
        session_id: session_id.clone(),
        addrs: mine.iter().map(|a| a.to_string()).collect(),
    }))
    .await?;
let theirs = match wait_opened(&mut ev, to_device, &me, &peer_pk, |b| {
    matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
}).await.context("waiting for peer Candidates")? {
    SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
    other => anyhow::bail!("unexpected signal body {other:?}"),
};
let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
allowed.insert(to_device.to_string(), peer_pk.clone());
let conn = punch::punch_connect(&epn, &me, &allowed, &peer_pk, &theirs, &punch::PunchConfig::default())
    .await
    .map_err(|e| punch_err(e, nat_symmetric, mine.len()))?;
```

(既有"等 ConnectAccept"代码保持;session_id 为 `Vec<u8>` 的既有 clone。)

3. `run_recv` 连接段(原 accept_window + make_server_endpoint + local_candidates + accept_pinned)整体替换为:

```rust
let stun = wait_hello_stun(&mut ev).await?;
let (epn, mine, nat_symmetric) = setup_punch_endpoint(&me, &from_device, &peer_pk, &stun)?;
relay_with_timeout(&client, &from_device,
    &seal(&me, &peer_pk, &SignalBody::ConnectAccept { session_id: session_id.clone() }))
    .await?;
relay_with_timeout(&client, &from_device,
    &seal(&me, &peer_pk, &SignalBody::Candidates {
        session_id: session_id.clone(),
        addrs: mine.iter().map(|a| a.to_string()).collect(),
    }))
    .await?;
let theirs = match wait_opened(&mut ev, &from_device, &me, &peer_pk, |b| {
    matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
}).await.context("waiting for sender Candidates")? {
    SignalBody::Candidates { addrs, .. } => parse_candidates(&addrs),
    other => anyhow::bail!("unexpected signal body {other:?}"),
};
let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
allowed.insert(from_device.clone(), peer_pk.clone());
let conn = punch::punch_connect(&epn, &me, &allowed, &peer_pk, &theirs, &punch::PunchConfig::default())
    .await
    .map_err(|e| punch_err(e, nat_symmetric, mine.len()))?;
let paths = transfer::recv_files(&conn, out_dir, &me).await.context("transfer")?;
Ok(paths.into_iter().next().ok_or_else(|| anyhow!("transfer produced no files"))?)
```

4. 删除 `CONNECT_TIMEOUT` 的 accept_window 相关计算与 `ep::make_server_endpoint` 调用(`CONNECT_TIMEOUT` 常量若无他用一并删除,连同其测试引用)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-cli`
Expected: 新 STUN e2e PASS;既有明文/TLS e2e 不改一行仍 PASS(空 stun_addrs → 本地候选路径)。

- [ ] **Step 5: demo 文档**

`docs/demo-m1.md` "起服务器"一节后追加:

```markdown
## (可选)起 STUN(终端 1b,跨公网直连需要)
```bash
cargo run -p stun-server   # 默认 0.0.0.0:3478/:3479
```
信令服务器通告:`--stun-addr <公网IP> --stun-port 3478,3479`(默认 127.0.0.1,仅本机演示)。
```

- [ ] **Step 6: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/ docs/demo-m1.md Cargo.lock
git commit -m "feat(cli): bidirectional candidate exchange with mutual punch"
```

---

## 完成后

1. 核对 spec §9 验收清单逐项打勾
2. M2b 分支经 PR 合入 main;随后启动 M2c(传输 v2)brainstorm
