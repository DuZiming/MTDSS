# MTDSS M2a(债务清偿)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 M1 搁置的四项硬性债务——manifest 签名帧化 v2、信令面 TLS、WS keepalive、接收方流式落盘。

**Architecture:** 四项相互独立:签名帧化改 `signable_bytes` 的输入格式(manifest 版本硬切 1→2);TLS 在信令服务器进程内以 rustls 终止(单端口、仅 TLS),CLI 以 `--ca` 信任自签 CA;keepalive 为对称 WS Ping/pong(服务端判死清 presence,客户端判死发 `Closed`);流式化把接收方从"整文件驻内存 BTreeMap"改为"定位写 .part + 顺序读回校验"。

**Tech Stack:** Rust 2021 workspace;rustls 0.23(ring)、axum-server 0.7(tls-rustls)、rcgen 0.13、rustls-pemfile 2、reqwest 0.12(rustls-tls)、tokio-tungstenite 0.24。

**Spec:** `docs/superpowers/specs/2026-10-16-mtdss-m2a-debt-design.md`

## Global Constraints

- 测试门禁(每个任务提交前):`cargo test --workspace && cargo clippy --workspace -- -D warnings`
- 本开发机(Windows):cargo 走 rsproxy.cn 镜像(已配好勿动);**bash 子进程不继承 setx 设置的 PROTOC**——在 bash 里跑 cargo 前先 `export PROTOC="/path/to/protoc"`(PowerShell 里不需要);git 走 SSH 远端
- 服务器默认端口 8787 不变;**仅 TLS,不再有明文端口**
- keepalive 生产常量:ping 间隔 20s、静默判死上限 60s(两侧一致;必须可注入以便测试)
- `MANIFEST_VERSION` 硬切为 2;签名域分隔标签字节串:`b"MTDSS-MAN-SIG-V2"`
- 新依赖精确版本(workspace `[workspace.dependencies]` 统一管理):`axum-server = { version = "0.7", features = ["tls-rustls"] }`、`rustls-pemfile = "2"`
- 提交信息用 conventional 前缀(feat/fix/test/docs),与 M1 历史一致

## Review Focus

实现中最容易咬人的五类输入/失效模式,每条已钉进对应任务的测试:

1. **旧 v1 manifest 不能被误验**(升级后旧端/旧清单混入):Task 1 `verify_rejects_v1_manifest` 断言干净报错 "unsupported manifest version"。
2. **不受信 CA 必须握手失败**(不得静默降级或忽略证书错误):Task 6 `wrong_ca_is_rejected` 断言连接错误。
3. **半开 WS 连接必须被判死**(TCP 活着但对端沉默):Task 2 `silent_ws_client_reaped_by_keepalive`(服务端清 presence)+ Task 3 `client_detects_silent_server`(客户端发 Closed)。
4. **流式化不得降低安全性**:坏块必须先拒哈希再落盘(坏字节永不出现在 .part):Task 7 `corrupt_chunk_never_written` 断言 .part 中该偏移仍为零。
5. **零字节文件与乱序块在定位写下仍正确**:Task 7 把 `("empty.txt", 0)` 加入参数化用例 + `reverse_order_chunks_assemble_correctly`。

---

### Task 1: signable_bytes v2 长度帧化

**Files:**
- Modify: `proto/src/lib.rs:8`(`MANIFEST_VERSION`)
- Modify: `proto/proto/manifest.proto:14,17`(注释描述 v2 帧化)
- Modify: `crates/core/src/protocol/manifest.rs:38-50`(`signable_bytes`)
- Test: `crates/core/tests/manifest.rs`(追加)

**Interfaces:**
- Consumes: `FileEntry`/`ChunkDesc`(mtdss_proto::manifest,不变)
- Produces: `signable_bytes(&str, &[FileEntry]) -> Vec<u8>` 签名不变、语义变更为 v2 帧;`mtdss_proto::MANIFEST_VERSION == 2`。后续所有任务(及 M2c 多文件)依赖此格式。

- [ ] **Step 1: 写失败测试**

在 `crates/core/tests/manifest.rs` 末尾追加(需要 `use mtdss_proto::manifest::{ChunkDesc, FileEntry};`,若文件头已有 `use mtdss_core::protocol::manifest::*;` 则 ChunkDesc/FileEntry 从 proto 引入):

```rust
fn entry(rel: &str, size_le: [u8; 8], hash: Vec<u8>) -> FileEntry {
    FileEntry {
        rel_path: rel.to_string(),
        size: u64::from_le_bytes(size_le),
        sha256: hash,
        chunks: vec![],
    }
}

#[test]
fn signable_bytes_v2_golden() {
    let files = vec![FileEntry {
        rel_path: "f".into(),
        size: 1,
        sha256: vec![7u8; 32],
        chunks: vec![ChunkDesc { offset: 0, length: 1, sha256: vec![9u8; 32] }],
    }];
    let got = signable_bytes("ab", &files);
    let mut want = Vec::new();
    want.extend_from_slice(b"MTDSS-MAN-SIG-V2");
    want.extend_from_slice(&2u32.to_be_bytes()); // len(transfer_id)
    want.extend_from_slice(b"ab");
    want.extend_from_slice(&1u32.to_be_bytes()); // file_count
    want.extend_from_slice(&1u32.to_be_bytes()); // len(rel_path)
    want.extend_from_slice(b"f");
    want.extend_from_slice(&1u64.to_le_bytes()); // size
    want.extend_from_slice(&[7u8; 32]); // 整文件哈希
    want.extend_from_slice(&1u32.to_be_bytes()); // chunk_count
    want.extend_from_slice(&0u64.to_le_bytes()); // offset
    want.extend_from_slice(&1u64.to_le_bytes()); // length
    want.extend_from_slice(&[9u8; 32]); // 块哈希
    assert_eq!(got, want);
}

#[test]
fn signable_bytes_v2_disambiguates_rel_path_boundaries() {
    // v1 串接格式下字节完全相同的两个清单,v2 必须产生不同签名字节。
    // A 布局: "a" || S1(8) || H1(32) || "bc" || S2(8) || H2(32)
    // B 布局: "ab" || S1'(8) || H1'(32) || "c" || S2(8) || H2(32)
    // 其中 S1[0]='b'、S1' = S1[1..8]||H1[0]、H1' = H1[1..32]||'b',逐字节重排相等。
    let h1: Vec<u8> = (0u8..32).collect();
    let s2 = [7u8; 8];
    let h2: Vec<u8> = (32u8..64).collect();
    let mut s1 = [0u8; 8];
    s1[0] = b'b';
    let a_files = vec![entry("a", s1, h1.clone()), entry("bc", s2, h2.clone())];

    let b_f1_size: [u8; 8] = {
        let mut v = s1[1..8].to_vec();
        v.push(h1[0]);
        v.try_into().unwrap()
    };
    let b_f1_hash = {
        let mut v = h1[1..32].to_vec();
        v.push(b'b');
        v
    };
    let b_files = vec![entry("ab", b_f1_size, b_f1_hash), entry("c", s2, h2.clone())];

    // 测试内复刻 v1 串接(无长度前缀),证明输入对确实在 v1 下碰撞
    let v1 = |files: &[FileEntry]| -> Vec<u8> {
        let mut out = b"tid".to_vec();
        for f in files {
            out.extend_from_slice(f.rel_path.as_bytes());
            out.extend_from_slice(&f.size.to_le_bytes());
            out.extend_from_slice(&f.sha256);
        }
        out
    };
    assert_eq!(v1(&a_files), v1(&b_files), "sanity: v1 collides on this pair");
    assert_ne!(
        signable_bytes("tid", &a_files),
        signable_bytes("tid", &b_files),
        "v2 must disambiguate"
    );
}

#[test]
fn verify_rejects_v1_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_tmp(dir.path(), "v1.bin", 10);
    let id = DeviceIdentity::generate();
    let mut m = build_single(&p, "v1.bin", &id).unwrap();
    m.version = 1;
    let err = verify(&m, &id.public_key()).unwrap_err();
    assert!(err.to_string().contains("unsupported manifest version"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test manifest`
Expected: `signable_bytes_v2_golden` 与 `signable_bytes_v2_disambiguates_rel_path_boundaries` FAIL(现格式无标签无前缀);`verify_rejects_v1_manifest` 可能已 PASS(纯确认项)。

- [ ] **Step 3: 最小实现**

`proto/src/lib.rs`:`pub const MANIFEST_VERSION: u32 = 1;` → `2`。

`crates/core/src/protocol/manifest.rs` 替换 `signable_bytes`(函数名/签名不变):

```rust
/// v2 签名域标签:防跨协议签名重用(同一设备密钥还签信令体)。
pub const SIGN_DOMAIN: &[u8] = b"MTDSS-MAN-SIG-V2";

/// 签名覆盖的确定性字节(v2 帧化):域标签 || 长度前缀(变长字段) ||
/// 计数前缀(file_count/chunk_count,消除结构歧义)|| 定宽数值。
/// 格式:域标签 || u32BE len(tid) || tid || u32BE file_count ||
/// 每文件 { u32BE len(rel) || rel || u64LE size || sha256(32) ||
///          u32BE chunk_count || 每块 { u64LE offset || u64LE length || sha256(32) } }
pub fn signable_bytes(transfer_id: &str, files: &[FileEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(SIGN_DOMAIN);
    let tid = transfer_id.as_bytes();
    out.extend_from_slice(&(tid.len() as u32).to_be_bytes());
    out.extend_from_slice(tid);
    out.extend_from_slice(&(files.len() as u32).to_be_bytes());
    for f in files {
        let rp = f.rel_path.as_bytes();
        out.extend_from_slice(&(rp.len() as u32).to_be_bytes());
        out.extend_from_slice(rp);
        out.extend_from_slice(&f.size.to_le_bytes());
        out.extend_from_slice(&f.sha256);
        out.extend_from_slice(&(f.chunks.len() as u32).to_be_bytes());
        for c in &f.chunks {
            out.extend_from_slice(&c.offset.to_le_bytes());
            out.extend_from_slice(&c.length.to_le_bytes());
            out.extend_from_slice(&c.sha256);
        }
    }
    out
}
```

`proto/proto/manifest.proto` 两行注释更新:`Manifest.version` 行注释加 `// v2 = 长度帧化签名`。⚠️ 改 .proto 文件后需要 PROTOC 重新生成——本机 bash 里先 `export PROTOC=<protoc 路径>`(PowerShell 直接跑)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test manifest`
Expected: 全部 PASS(build_single/verify 用同一常量与同一函数,自动一致)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
```

```bash
git add proto/ crates/core/
git commit -m "feat(core): manifest signature framing v2 (MANIFEST_VERSION=2, hard cut)"
```

---

### Task 2: 服务端 WS keepalive

**Files:**
- Modify: `server/signaling/src/ws.rs`(Keepalive 结构 + 主循环 ping 臂)
- Modify: `server/signaling/src/lib.rs`(AppState 字段 + spawn_test_server_with)
- Modify: `server/signaling/src/main.rs`(AppState 构造补字段)
- Test: `server/signaling/tests/ws.rs`(追加)

**Interfaces:**
- Consumes: 既有 `AppState`/`handle_socket`/`PresenceRegistry`
- Produces: `signaling::ws::Keepalive { ping_interval: Duration, silent_limit: Duration }`(Default = 20s/60s);`AppState.keepalive` 字段;`signaling::spawn_test_server_with(Keepalive) -> (SocketAddr, Arc<AppState>)`。Task 4 的 main.rs 构造依赖此字段。

- [ ] **Step 1: 写失败测试**

在 `server/signaling/tests/ws.rs` 末尾追加(沿用该文件既有的 `use` 与注册辅助;若文件内已有 `register` 辅助函数直接复用,不要重复定义——先读文件头确认,下面写自包含版本):

```rust
#[tokio::test]
async fn silent_ws_client_reaped_by_keepalive() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use std::time::{Duration, Instant};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let ka = signaling::ws::Keepalive {
        ping_interval: Duration::from_millis(100),
        silent_limit: Duration::from_millis(400),
    };
    let (addr, state) = signaling::spawn_test_server_with(ka).await;

    // REST 注册拿 JWT(与既有 ws 测试同一流程;pubkey 用随机 32 字节)
    let http = reqwest::Client::new();
    let c: serde_json::Value = http
        .post(format!("http://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": "ka@x.y" }))
        .send().await.unwrap().json().await.unwrap();
    let v: serde_json::Value = http
        .post(format!("http://{addr}/auth/verify"))
        .json(&serde_json::json!({
            "email": "ka@x.y",
            "code": c["code"].as_str().unwrap(),
            "pubkey": hex::encode(rand::random::<[u8; 32]>()),
            "name": "ka",
            "platform": "t"
        }))
        .send().await.unwrap().json().await.unwrap();
    let jwt = v["jwt"].as_str().unwrap().to_string();
    let dev = v["device_id"].as_str().unwrap().to_string();

    // 连接、发 Hello、读到 HelloOk(在线确认)
    let (ws, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
    let (mut sink, mut stream) = futures_util::StreamExt::split(ws);
    let hello = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: dev.clone(),
            jwt: jwt.clone(),
        })),
    };
    sink.send(WsMessage::Binary(hello.encode_to_vec())).await.unwrap();
    loop {
        match stream.next().await {
            Some(Ok(WsMessage::Binary(b)))
                if WsServerFrame::decode(b.as_slice())
                    .unwrap()
                    .frame
                    .map(|f| matches!(f, ws_server_frame::Frame::HelloOk(_)))
                    .unwrap_or(false) =>
            {
                break;
            }
            Some(Ok(_)) => continue,
            other => panic!("closed before HelloOk: {other:?}"),
        }
    }
    assert!(state.presence.is_online(&dev));

    // 静默:丢弃读半 → 不再 poll → 不自动回 pong(写半存活,TCP 保持打开)
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.presence.is_online(&dev) {
        assert!(Instant::now() < deadline, "silent client not reaped by keepalive");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p signaling --test ws silent_ws_client`
Expected: 编译失败(`Keepalive`/`spawn_test_server_with` 不存在)。

- [ ] **Step 3: 最小实现**

`server/signaling/src/ws.rs` 顶部(结构体区)加:

```rust
/// WS keepalive 参数:服务端按 ping_interval 发 Ping;
/// silent_limit 内无任何入站帧(含 Pong)即判死断开。
#[derive(Debug, Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: std::time::Duration,
    pub silent_limit: std::time::Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            ping_interval: std::time::Duration::from_secs(20),
            silent_limit: std::time::Duration::from_secs(60),
        }
    }
}
```

`handle_socket` 主循环(现 `// 4)` 注释处)替换为:

```rust
    // 4) 主循环:出站转发 + 入站 Relay 处理 + keepalive ping/判死
    let mut sink_opt = Some(sink);
    let mut ping = tokio::time::interval_at(
        tokio::time::Instant::now() + state.keepalive.ping_interval,
        state.keepalive.ping_interval,
    );
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_inbound = std::time::Instant::now();
    loop {
        tokio::select! {
            out = rx.recv() => match (out, sink_opt.as_mut()) {
                (Some(m), Some(s)) => {
                    if s.send(m).await.is_err() {
                        break;
                    }
                }
                _ => break,
            },
            _ = ping.tick() => {
                if last_inbound.elapsed() >= state.keepalive.silent_limit {
                    tracing::info!("ws keepalive: dropping silent client {device_id}");
                    break;
                }
                if let Some(s) = sink_opt.as_mut() {
                    if s.send(Message::Ping(vec![0u8].into())).await.is_err() {
                        break;
                    }
                }
            },
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else { break };
                last_inbound = std::time::Instant::now();
                let Message::Binary(bin) = msg else { continue };
                let Ok(f) = WsClientFrame::decode(bin.as_ref()) else {
                    if let Some(s) = sink_opt.as_mut() {
                        let _ = s.send(err(3, "bad frame")).await;
                    }
                    break;
                };
                match f.frame {
                    Some(ws_client_frame::Frame::Relay(r)) => {
                        if let Err((code, msg)) =
                            state.relay(&device_id, &account_id, &r.to_device, &r.payload)
                        {
                            if let Some(s) = sink_opt.as_mut() {
                                let _ = s.send(err(code, &msg)).await;
                            }
                        }
                    }
                    _ => {
                        if let Some(s) = sink_opt.as_mut() {
                            let _ = s.send(err(3, "expected relay")).await;
                        }
                    }
                }
            }
        }
    }
```

`server/signaling/src/lib.rs`:`AppState` 加字段 `pub keepalive: crate::ws::Keepalive,`;三处构造(main.rs、`spawn_test_server`、`test_state`)补 `keepalive: crate::ws::Keepalive::default()`;新增:

```rust
/// 集成测试基建:自定义 keepalive 起服务器
pub async fn spawn_test_server_with(
    keepalive: crate::ws::Keepalive,
) -> (SocketAddr, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db");
    let db = db::open(db_path.to_str().unwrap()).unwrap();
    let state = Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
        keepalive,
    });
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _keep = dir;
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}
```

原 `spawn_test_server` 改为委托:`spawn_test_server_with(crate::ws::Keepalive::default()).await`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p signaling --test ws`
Expected: 新测试 PASS,既有 ws 测试全量 PASS(默认 20s ping 不会在测试窗口内触发)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add server/signaling/
git commit -m "feat(signaling): server WS keepalive (ping 20s, reap silent 60s)"
```

---

### Task 3: 客户端 WS keepalive

**Files:**
- Modify: `crates/core/src/signaling/client.rs`
- Test: `crates/core/tests/signaling_client.rs`(追加)

**Interfaces:**
- Consumes: 既有 `SignalingClient::connect` / `ClientEvent`
- Produces: `mtdss_core::signaling::client::Keepalive { ping_interval, silent_limit }`(Default 20s/60s);`SignalingClient::connect_with(url: &str, jwt: &str, device_id: &str, ka: Keepalive) -> Result<(Self, Receiver<ClientEvent>)>`(Task 5 会加 `ca` 参数);`connect()` 保持原签名并委托默认值——既有调用零改动。

- [ ] **Step 1: 写失败测试**

在 `crates/core/tests/signaling_client.rs` 末尾追加:

```rust
#[tokio::test]
async fn client_detects_silent_server() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_proto::ws::*;
    use mtdss_core::signaling::client::Keepalive;
    use prost::Message as _;
    use std::time::{Duration, Instant};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    // 静默服务器:完成握手、回 HelloOk,之后不再 poll(不自动回 pong)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
            })),
        };
        ws.send(WsMessage::Binary(ok.encode_to_vec())).await.unwrap();
        futures_util::pending::<()>().await; // 永不再 poll:连接活但不响应
    });

    let (_client, mut ev) = mtdss_core::signaling::client::SignalingClient::connect_with(
        &format!("ws://{addr}/ws"),
        "jwt",
        "dev-x",
        Keepalive {
            ping_interval: Duration::from_millis(100),
            silent_limit: Duration::from_millis(400),
        },
    )
    .await
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout(Duration::from_millis(200), ev.recv()).await {
            Ok(Some(mtdss_core::signaling::client::ClientEvent::Closed)) => break,
            Ok(Some(other)) => panic!("expected Closed, got {other:?}"),
            Ok(None) => panic!("event stream ended without Closed"),
            Err(_) => {
                assert!(Instant::now() < deadline, "no Closed within 5s of silence");
            }
        }
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test signaling_client client_detects`
Expected: 编译失败(`connect_with`/`Keepalive` 不存在)。

- [ ] **Step 3: 最小实现**

`crates/core/src/signaling/client.rs` 整体重写(保留既有对外语义,变化点:命令通道枚举、interval 臂、`connect_with`):

```rust
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message as _;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// 客户端 keepalive 参数:按 ping_interval 发 Ping;
/// silent_limit 内无任何入站帧(含 Pong)即判死,发 Closed 并退出。
#[derive(Debug, Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: Duration,
    pub silent_limit: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self { ping_interval: Duration::from_secs(20), silent_limit: Duration::from_secs(60) }
    }
}

/// 出站命令:业务二进制帧或协议层 Ping。
enum Outbound {
    Binary(Vec<u8>),
    Ping,
}

/// 从信令服务器流出的高层事件。HelloOk 属于握手确认,不外露。
#[derive(Debug)]
pub enum ClientEvent {
    Relayed { from: String, payload: Vec<u8> },
    Presence { device_id: String, online: bool },
    Error { code: u32, message: String },
    Closed,
}

/// 信令 WebSocket 客户端的廉价克隆句柄。仅负责传输:
/// 端到端信封加解密由上层流程负责。
#[derive(Clone)]
pub struct SignalingClient {
    tx: mpsc::Sender<Outbound>,
}

impl SignalingClient {
    /// 连接并以默认 keepalive(20s/60s)握手。
    pub async fn connect(
        url: &str,
        jwt: &str,
        device_id: &str,
    ) -> Result<(Self, mpsc::Receiver<ClientEvent>)> {
        Self::connect_with(url, jwt, device_id, Keepalive::default()).await
    }

    /// 同 `connect`,keepalive 参数可注入(测试用亚秒级)。
    pub async fn connect_with(
        url: &str,
        jwt: &str,
        device_id: &str,
        ka: Keepalive,
    ) -> Result<(Self, mpsc::Receiver<ClientEvent>)> {
        let (ws, _resp) = tokio_tungstenite::connect_async(url)
            .await
            .context("connect signaling ws")?;
        let (mut sink, mut stream) = ws.split();

        let hello = WsClientFrame {
            frame: Some(ws_client_frame::Frame::Hello(Hello {
                device_id: device_id.into(),
                jwt: jwt.into(),
            })),
        };
        sink.send(WsMessage::Binary(hello.encode_to_vec())).await?;

        let (ev_tx, ev_rx) = mpsc::channel(64);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Outbound>(64);

        tokio::spawn(async move {
            let mut ping = tokio::time::interval_at(
                tokio::time::Instant::now() + ka.ping_interval,
                ka.ping_interval,
            );
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_inbound = std::time::Instant::now();
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => match cmd {
                        Some(Outbound::Binary(bytes)) => {
                            if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                                break;
                            }
                        }
                        Some(Outbound::Ping) => {
                            if sink.send(WsMessage::Ping(vec![])).await.is_err() {
                                break;
                            }
                        }
                        // 所有句柄都已丢弃:结束任务,socket 随之关闭
                        None => break,
                    },
                    _ = ping.tick() => {
                        if last_inbound.elapsed() >= ka.silent_limit {
                            let _ = ev_tx.send(ClientEvent::Closed).await;
                            break;
                        }
                        if sink.send(WsMessage::Ping(vec![])).await.is_err() {
                            break;
                        }
                    },
                    inbound = stream.next() => match inbound {
                        Some(Ok(msg)) => {
                            last_inbound = std::time::Instant::now();
                            let WsMessage::Binary(bin) = msg else { continue };
                            let Ok(f) = WsServerFrame::decode(bin.as_slice()) else { continue };
                            let ev = match f.frame {
                                Some(ws_server_frame::Frame::HelloOk(_)) => continue,
                                Some(ws_server_frame::Frame::Relayed(r)) => ClientEvent::Relayed {
                                    from: r.from_device,
                                    payload: r.payload,
                                },
                                Some(ws_server_frame::Frame::Presence(p)) => ClientEvent::Presence {
                                    device_id: p.device_id,
                                    online: p.online,
                                },
                                Some(ws_server_frame::Frame::Error(e)) => ClientEvent::Error {
                                    code: e.code,
                                    message: e.message,
                                },
                                None => continue,
                            };
                            if ev_tx.send(ev).await.is_err() {
                                break;
                            }
                        }
                        // socket 关闭(服务器拒绝或断线)
                        _ => {
                            let _ = ev_tx.send(ClientEvent::Closed).await;
                            break;
                        }
                    },
                }
            }
        });

        Ok((Self { tx: cmd_tx }, ev_rx))
    }

    /// 请求服务器把 payload 转发给 `to_device`(同账号)。
    pub async fn relay(&self, to_device: &str, payload: &[u8]) -> Result<()> {
        let f = WsClientFrame {
            frame: Some(ws_client_frame::Frame::Relay(Relay {
                to_device: to_device.into(),
                payload: payload.to_vec(),
            })),
        };
        self.tx
            .send(Outbound::Binary(f.encode_to_vec()))
            .await
            .context("signaling client gone")
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test signaling_client`
Expected: 新测试 PASS;既有 `connect_relay_presence_roundtrip` 等 PASS(`connect` 签名未变)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/core/
git commit -m "feat(core): signaling client keepalive with injectable connect_with"
```

---

### Task 4: 信令服务器 TLS + dev 证书生成

**Files:**
- Create: `server/signaling/src/tls.rs`
- Modify: `server/signaling/src/lib.rs`(mod tls + spawn_test_tls_server)
- Modify: `server/signaling/src/main.rs`(参数解析 + TLS 启动)
- Modify: `Cargo.toml`(workspace 依赖)、`server/signaling/Cargo.toml`
- Test: `server/signaling/tests/tls.rs`(新建)

**Interfaces:**
- Consumes: `build_router(state) -> Router`;Task 2 的 `AppState.keepalive`
- Produces: `signaling::tls::serve_tls(app: Router, addr: SocketAddr, cert: &Path, key: &Path) -> Result<()>`;`signaling::tls::generate_dev_tls(dir: &Path) -> Result<()>`(写 ca.crt/server.crt/server.key,拒绝覆盖);`signaling::spawn_test_tls_server() -> (SocketAddr, PathBuf /*ca.crt*/, TempDir)`。Task 5/6 依赖。

- [ ] **Step 1: 加依赖**

根 `Cargo.toml` `[workspace.dependencies]` 追加:

```toml
axum-server = { version = "0.7", features = ["tls-rustls"] }
rustls-pemfile = "2"
```

`server/signaling/Cargo.toml` `[dependencies]` 追加:

```toml
axum-server.workspace = true
rustls.workspace = true
rcgen.workspace = true
time.workspace = true
```

`server/signaling/Cargo.toml` `[dev-dependencies]` 的 reqwest 行改为:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 2: 写失败测试**

新建 `server/signaling/tests/tls.rs`:

```rust
//! TLS 服务器:dev 证书生成 + https 路由可达。

#[test]
fn dev_tls_generates_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(dir.path()).unwrap();
    assert!(dir.path().join("ca.crt").is_file());
    assert!(dir.path().join("server.crt").is_file());
    assert!(dir.path().join("server.key").is_file());
    assert!(
        signaling::tls::generate_dev_tls(dir.path()).is_err(),
        "must refuse to overwrite existing tls files"
    );
}

#[tokio::test]
async fn tls_server_serves_https_with_trusted_ca() {
    let (addr, ca, _keep) = signaling::spawn_test_tls_server().await;
    let pem = std::fs::read(&ca).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap())
        .build()
        .unwrap();
    // 公开端点 200 即证明 TLS 握手 + 路由全通
    let resp = client
        .post(format!("https://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": "tls@x.y" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p signaling --test tls`
Expected: 编译失败(`tls` 模块不存在)。

- [ ] **Step 4: 最小实现**

新建 `server/signaling/src/tls.rs`:

```rust
//! TLS 终止 + 开发用自签 CA/证书生成。

use anyhow::{Context, Result};
use std::path::Path;

/// 进程级安装 ring provider(rustls 0.23 需要;重复安装忽略)。
fn ensure_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// 以 rustls 终止 TLS 服务 axum 路由(单端口,仅 TLS)。
pub async fn serve_tls(
    app: axum::Router,
    addr: std::net::SocketAddr,
    cert: &Path,
    key: &Path,
) -> Result<()> {
    ensure_provider();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .context("load tls cert/key")?;
    axum_server::bind_rustls(addr, config)
        .serve(app.into_make_service())
        .await
        .context("tls serve")?;
    Ok(())
}

/// 生成自签 CA + 服务端证书(开发/CI 用):ca.crt / server.crt / server.key。
/// dir 可不存在(自动创建);任一目标文件已存在即拒绝覆盖。
/// SAN 含 localhost 与回环 v4/v6 IP,客户端可用 https://127.0.0.1:port 直连。
pub fn generate_dev_tls(dir: &Path) -> Result<()> {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, SanType};
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let (ca_p, crt_p, key_p) =
        (dir.join("ca.crt"), dir.join("server.crt"), dir.join("server.key"));
    anyhow::ensure!(
        !ca_p.exists() && !crt_p.exists() && !key_p.exists(),
        "refusing to overwrite existing tls files in {}",
        dir.display()
    );

    let now = time::OffsetDateTime::now_utc();
    let ca_key = KeyPair::generate()?;
    let mut ca = CertificateParams::new(vec![])?;
    ca.distinguished_name.push(DnType::CommonName, "MTDSS Dev CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.not_before = now - time::Duration::days(1);
    ca.not_after = now + time::Duration::days(3650);
    let ca_cert = ca.self_signed(&ca_key)?;

    let server_key = KeyPair::generate()?;
    let mut leaf = CertificateParams::new(vec!["localhost".to_string()])?;
    leaf.distinguished_name.push(DnType::CommonName, "localhost");
    leaf.subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));
    leaf.subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
    leaf.not_before = now - time::Duration::days(1);
    leaf.not_after = now + time::Duration::days(3650);
    let leaf_cert = leaf.signed_by(&server_key, &ca_cert, &ca_key)?;

    std::fs::write(&ca_p, ca_cert.pem())?;
    std::fs::write(&crt_p, leaf_cert.pem())?;
    write_private(&key_p, &server_key.serialize_pem())?;
    Ok(())
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
```

`server/signaling/src/lib.rs`:模块声明加 `pub mod tls;`;`tls.rs` 中 `ensure_provider` 直接设为 `pub fn ensure_provider()`(测试基建需显式安装 provider);文件末尾追加(其中 `tls::ensure_provider()` 即上一步的公开函数):

```rust
/// 集成测试基建:随机端口起 TLS 服务器,返回 (地址, ca.crt 路径, 存活目录)。
/// TempDir 持有证书与 DB,存活期须覆盖整个测试。
pub async fn spawn_test_tls_server() -> (SocketAddr, std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    tls::generate_dev_tls(dir.path()).unwrap();
    let db = db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
    let state = Arc::new(AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: PresenceRegistry::new(),
        keepalive: crate::ws::Keepalive::default(),
    });
    let app = build_router(state);
    tls::ensure_provider();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        dir.path().join("server.crt"),
        dir.path().join("server.key"),
    )
    .await
    .unwrap();
    let handle = axum_server::Handle::new();
    let h = handle.clone();
    let d = dir.clone(); // TempDir clone 共享存活
    tokio::spawn(async move {
        let _keep = d;
        axum_server::bind_rustls(SocketAddr::from(([127, 0, 0, 1], 0)), config)
            .handle(h)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    loop {
        if let Some(a) = handle.listening() {
            return (a, dir.path().join("ca.crt"), dir);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
```

`tls.rs` 中 `ensure_provider` 即上面的函数(改 pub 即可,无需额外封装)。

`server/signaling/src/main.rs` 整体替换:

```rust
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let mut db_path = "signaling.db".to_string();
    let mut tls_cert: Option<String> = None;
    let mut tls_key: Option<String> = None;
    let mut dev_gen_tls: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--tls-cert" => {
                tls_cert = Some(args.next().ok_or_else(|| anyhow::anyhow!("--tls-cert needs a value"))?)
            }
            "--tls-key" => {
                tls_key = Some(args.next().ok_or_else(|| anyhow::anyhow!("--tls-key needs a value"))?)
            }
            "--dev-gen-tls" => dev_gen_tls = Some(
                args.next()
                    .ok_or_else(|| anyhow::anyhow!("--dev-gen-tls needs a value"))?,
            ),
            other if !other.starts_with("--") => db_path = other.to_string(),
            other => anyhow::bail!(
                "unknown flag {other}; usage: signaling-server [db] --tls-cert <pem> \
                 --tls-key <pem> | --dev-gen-tls <dir>"
            ),
        }
    }

    if let Some(dir) = dev_gen_tls {
        signaling::tls::generate_dev_tls(std::path::Path::new(&dir))?;
        println!("wrote ca.crt / server.crt / server.key in {dir}");
        println!("server: --tls-cert {dir}/server.crt --tls-key {dir}/server.key");
        println!("client: --ca {dir}/ca.crt");
        return Ok(());
    }

    let (cert, key) = match (tls_cert, tls_key) {
        (Some(c), Some(k)) => (c, k),
        _ => anyhow::bail!(
            "TLS required: pass --tls-cert <pem> --tls-key <pem> (dev: --dev-gen-tls <dir> first)"
        ),
    };

    let db = signaling::db::open(&db_path)?;
    let state = Arc::new(signaling::AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: signaling::PresenceRegistry::new(),
        keepalive: signaling::ws::Keepalive::default(),
    });
    let app = signaling::build_router(state);
    let addr: std::net::SocketAddr = "0.0.0.0:8787".parse().expect("valid socket addr");
    tracing::info!("signaling server (TLS) on {addr}");
    signaling::tls::serve_tls(app, addr, std::path::Path::new(&cert), std::path::Path::new(&key))
        .await
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p signaling`
Expected: tls 测试 2 项 PASS;auth/devices/ws 既有测试全量 PASS(仍走 router/spawn_test_server,不经 TLS)。

- [ ] **Step 6: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add Cargo.toml Cargo.lock server/signaling/
git commit -m "feat(signaling): TLS-only server with --dev-gen-tls and rustls termination"
```

---

### Task 5: CLI --ca 信任根(https/wss)

**Files:**
- Modify: `crates/cli/Cargo.toml`(reqwest 加 rustls-tls)
- Modify: `crates/cli/src/session.rs`(Session.ca 字段)
- Modify: `crates/cli/src/flow.rs`(http_client/register/signaling_connect)
- Modify: `crates/cli/src/main.rs`(全局 --ca)
- Modify: `crates/core/src/signaling/client.rs`(connect_with 加 ca 参数 + ws_connector)
- Modify: `crates/core/Cargo.toml`(rustls-pemfile)
- Test: `crates/core/tests/signaling_client.rs`、`crates/cli/tests/flow.rs`(追加)

**Interfaces:**
- Consumes: Task 3 的 `connect_with(url, jwt, device_id, ka)`;Task 4 未直接消费
- Produces: `Session.ca: Option<PathBuf>`(serde 默认 None,旧 session.json 兼容);`connect_with(url, jwt, device_id, ca: Option<&Path>, ka: Keepalive)`(最终签名);`pub fn ws_connector(ca: Option<&Path>) -> Result<Option<Connector>>`;CLI 全局 `--ca <path>`(register 持久化,其余命令覆盖)。Task 6 的 e2e 依赖。

- [ ] **Step 1: 加依赖**

`crates/cli/Cargo.toml` reqwest 行改为:

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

`crates/core/Cargo.toml` `[dependencies]` 追加 `rustls-pemfile.workspace = true`。

- [ ] **Step 2: 写失败测试**

`crates/core/tests/signaling_client.rs` 追加:

```rust
#[test]
fn ws_connector_from_generated_ca() {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    let dir = tempfile::tempdir().unwrap();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let cert = params.self_signed(&key).unwrap();
    let ca_path = dir.path().join("ca.crt");
    std::fs::write(&ca_path, cert.pem()).unwrap();

    assert!(mtdss_core::signaling::client::ws_connector(Some(&ca_path)).unwrap().is_some());
    assert!(mtdss_core::signaling::client::ws_connector(None).unwrap().is_none());
    let bad = dir.path().join("bad.crt");
    std::fs::write(&bad, b"not a pem").unwrap();
    assert!(mtdss_core::signaling::client::ws_connector(Some(&bad)).is_err());
}
```

`crates/cli/tests/flow.rs` 追加:

```rust
#[test]
fn session_ca_roundtrip_and_legacy_load() {
    let dir = tempfile::tempdir().unwrap();
    let s = mtdss_cli::session::Session {
        server: "https://x".into(),
        jwt: "j".into(),
        refresh: "r".into(),
        device_id: "d".into(),
        ca: Some(dir.path().join("ca.crt")),
    };
    s.save(dir.path()).unwrap();
    let l = mtdss_cli::session::Session::load(dir.path()).unwrap();
    assert_eq!(l.ca, s.ca);
    // 旧 session.json(无 ca 字段)仍可加载
    std::fs::write(
        dir.path().join("session.json"),
        r#"{"server":"http://x","jwt":"j","refresh":"r","device_id":"d"}"#,
    )
    .unwrap();
    assert!(mtdss_cli::session::Session::load(dir.path()).unwrap().ca.is_none());
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p mtdss-core --test signaling_client ws_connector && cargo test -p mtdss-cli --test flow session_ca`
Expected: 编译失败(`ws_connector`、`Session.ca` 不存在)。

- [ ] **Step 4: 最小实现**

`crates/cli/src/session.rs` `Session` 结构:

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub server: String, // http(s)://host:port
    pub jwt: String,
    pub refresh: String,
    pub device_id: String,
    /// 服务端 TLS CA 证书(自签场景);None = 系统/webpki 根(域名证书)。
    #[serde(default)]
    pub ca: Option<PathBuf>,
}
```

`crates/cli/src/flow.rs`:

```rust
fn http_client(ca: Option<&Path>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(HTTP_TIMEOUT);
    if let Some(ca) = ca {
        let pem = std::fs::read(ca).with_context(|| format!("read ca {}", ca.display()))?;
        let cert = reqwest::Certificate::from_pem(&pem)?;
        b = b.add_root_certificate(cert);
    }
    b.build().context("build http client")
}
```

`fetch_devices` 内 `http_client()?` 改 `http_client(s.ca.as_deref())?`。

`register` 签名改为 `pub async fn register(home: &Path, server: &str, email: &str, name: String, ca: Option<&Path>) -> Result<()>`(name 由值传入避免借用冲突,调用点同步改),内部两处 `http_client()` 改 `http_client(ca)?`,构造 `Session` 时加 `ca: ca.map(|p| p.to_path_buf())`。

`signaling_connect` 内连接调用改为:

```rust
let (client, ev) = tokio::time::timeout(
    SIGNAL_TIMEOUT,
    SignalingClient::connect_with(
        &url,
        &session.jwt,
        device_id,
        session.ca.as_deref(),
        mtdss_core::signaling::client::Keepalive::default(),
    ),
)
.await
.map_err(|_| anyhow!("timed out connecting signaling at {url}"))?
.context("connect signaling ws")?;
```

`crates/core/src/signaling/client.rs`:`connect_with` 加 `ca: Option<&std::path::Path>` 参数(置于 `ka` 之前),内部:

```rust
let (ws, _resp) = tokio_tungstenite::connect_async_with_config(
    url,
    None,
    false,
    ws_connector(ca)?,
)
.await
.context("connect signaling ws")?;
```

`connect()` 委托处补 `None`。新增公开函数:

```rust
/// 由 PEM CA 构造 wss 连接器;ca 为 None → None(默认 webpki 根)。
/// 公开以便测试。
pub fn ws_connector(ca: Option<&std::path::Path>) -> Result<Option<tokio_tungstenite::Connector>> {
    let Some(p) = ca else { return Ok(None) };
    let pem = std::fs::read_to_string(p)
        .with_context(|| format!("read ca {}", p.display()))?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut pem.as_bytes()) {
        roots
            .add(&cert?)
            .map_err(|e| anyhow!("bad ca cert: {e:?}"))?;
    }
    anyhow::ensure!(!roots.is_empty(), "ca pem contains no certificates");
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
        cfg,
    ))))
}
```

`crates/cli/src/main.rs`:`Cli` 结构加全局旗标,命令分发应用覆盖:

```rust
struct Cli {
    /// Trust this CA cert (PEM) for https/wss signaling; register 持久化到 session
    #[arg(long, global = true)]
    ca: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}
```

`Register` 分支传 `cli.ca.as_deref()`;其余三个分支(`Devices`/`Send`/`Recv`)统一模式——加载后覆盖,再传引用:

```rust
// Devices 分支(替换原 devices_cmd(&home))
let mut s = mtdss_cli::session::Session::load(&home)?;
if let Some(c) = cli.ca.clone() {
    s.ca = Some(c);
}
mtdss_cli::flow::devices_cmd(&s).await
// Send/Recv 分支同样:load → 覆盖 ca → 以 &s 调 run_send/run_recv
```

`devices_cmd` 签名改为 `pub async fn devices_cmd(s: &Session) -> Result<()>`(内部自行 load 的两行删除,内部用 `fetch_devices(s)`)。

既有 `crates/cli/tests/cli_e2e.rs` 的两处 `Session { ... }` 字面量补 `ca: None`;既有 Task 3 测试 `client_detects_silent_server` 中 `connect_with(&format!(...), "jwt", "dev-x", Keepalive {...})` 的调用在 `Keepalive` 实参前补 `None`(新 ca 参数占位)。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test signaling_client && cargo test -p mtdss-cli`
Expected: 新测试 PASS;既有测试(含 e2e)PASS。

- [ ] **Step 6: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add Cargo.lock crates/
git commit -m "feat(cli): --ca trust root for https/wss signaling"
```

---

### Task 6: TLS 全链路 e2e + 错 CA 负例 + 演示文档

**Files:**
- Test: `crates/cli/tests/cli_e2e.rs`(追加两个测试)
- Modify: `docs/demo-m1.md`(启动命令改 TLS)

**Interfaces:**
- Consumes: Task 4 `spawn_test_tls_server()`;Task 5 `Session.ca`
- Produces: 无代码接口;M2a 验收项"CLI --ca 全链路 https/wss e2e 通过,错 CA 负例通过"。

- [ ] **Step 1: 写测试**

`crates/cli/tests/cli_e2e.rs` 追加(`register_rest_tls` 与既有 `register_rest` 并列;既有 http 版保留不动——router 明文路径仍是 keepalive 等测试的载体):

```rust
/// TLS 版注册:信任 spawn_test_tls_server 的 CA。
async fn register_rest_tls(
    base: &str,
    ca_path: &std::path::Path,
    email: &str,
    pubkey_hex: &str,
    name: &str,
) -> (String, String, String) {
    let pem = std::fs::read(ca_path).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap())
        .build()
        .unwrap();
    let c: serde_json::Value = client
        .post(format!("{base}/auth/request-code"))
        .json(&serde_json::json!({ "email": email }))
        .send().await.unwrap().json().await.unwrap();
    let v: serde_json::Value = client
        .post(format!("{base}/auth/verify"))
        .json(&serde_json::json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": pubkey_hex,
            "name": name,
            "platform": "test"
        }))
        .send().await.unwrap().json().await.unwrap();
    (
        v["jwt"].as_str().unwrap().into(),
        v["refresh"].as_str().unwrap().into(),
        v["device_id"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn cli_send_recv_end_to_end_over_tls() {
    let run = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let (addr, ca, _dir) = signaling::spawn_test_tls_server().await;
        let server = format!("https://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();

        let (jwt_a, ref_a, dev_a) = register_rest_tls(
            &server, &ca, "tls@x.y",
            &hex::encode(id_a.public_key().as_bytes()), "side-A",
        ).await;
        let (jwt_b, ref_b, dev_b) = register_rest_tls(
            &server, &ca, "tls@x.y",
            &hex::encode(id_b.public_key().as_bytes()), "side-B",
        ).await;

        let mk = |jwt: String, refresh: String, dev: String| Session {
            server: server.clone(),
            jwt,
            refresh,
            device_id: dev,
            ca: Some(ca.clone()),
        };
        let sess_a = mk(jwt_a, ref_a, dev_a.clone());
        let sess_b = mk(jwt_b, ref_b, dev_b.clone());
        sess_a.save(home_a.path()).unwrap();
        sess_b.save(home_b.path()).unwrap();

        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv(&sess, &home, &out)
                    .await
                    .expect("run_recv failed")
            })
        };

        // 等 B 上线(经 https 的 devices 查询)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "receiver never online");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        let src = tempfile::tempdir().unwrap();
        let name = "tls 传输 📦.bin";
        let fpath = src.path().join(name);
        let mut data = vec![0u8; 8192];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        mtdss_cli::flow::run_send(&sess_a, home_a.path(), &fpath, &dev_b, &|_| {})
            .await
            .expect("run_send failed");

        let landed = tokio::time::timeout(std::time::Duration::from_secs(15), b_task)
            .await
            .expect("receiver task did not finish in time")
            .unwrap();
        assert_eq!(
            landed.file_name().and_then(|n| n.to_str()),
            Some(name)
        );
        assert_eq!(std::fs::read(&landed).unwrap(), data);
    });
    run.await.expect("tls e2e exceeded 90s");
}

#[tokio::test]
async fn wrong_ca_is_rejected() {
    let (addr, _ca, _dir) = signaling::spawn_test_tls_server().await;
    // 另一套独立 CA:不受信
    let other = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(other.path()).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_pem(
                &std::fs::read(other.path().join("ca.crt")).unwrap(),
            )
            .unwrap(),
        )
        .build()
        .unwrap();
    let res = client
        .post(format!("https://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": "x@y.z" }))
        .send()
        .await;
    assert!(res.is_err(), "untrusted CA must fail the TLS handshake");
}
```

- [ ] **Step 2: 跑测试确认行为**

Run: `cargo test -p mtdss-cli --test cli_e2e`
Expected: 两测试 PASS(此为验收测试,若 FAIL 按错误修 Task 4/5 的实现——测试本身是规范)。

- [ ] **Step 3: 更新演示文档**

`docs/demo-m1.md` 的"起服务器"与注册两节替换为:

```markdown
## 起服务器(终端 1)
```bash
# 一次性生成自签 CA 与证书(目录可任意,勿提交入库)
cargo run -p signaling -- --dev-gen-tls .dev-tls
cargo run -p signaling -- signaling.db --tls-cert .dev-tls/server.crt --tls-key .dev-tls/server.key
```

## 设备 A 注册(终端 2)
```bash
cargo run -p mtdss-cli -- register --server https://127.0.0.1:8787 --ca .dev-tls/ca.crt --email a@x.y --name home-pc
# 按提示输入 dev-mode 验证码
```

## 设备 B 注册(终端 3,同一邮箱)
```bash
$env:MTDSS_HOME = "$PWD\.dev-b"   # Windows;Linux/macOS: export MTDSS_HOME=$PWD/.dev-b
cargo run -p mtdss-cli -- register --server https://127.0.0.1:8787 --ca .dev-tls/ca.crt --email a@x.y --name office-pc
```
```

(B 接收/A 发送两节命令不变;--ca 已持久化在各自 session.json。)

- [ ] **Step 4: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/cli/ docs/demo-m1.md
git commit -m "test(cli): full-chain e2e over TLS + wrong-CA negative; demo doc TLS flow"
```

---

### Task 7: 接收方流式落盘

**Files:**
- Modify: `crates/core/src/transport/transfer.rs:160-200`(recv_files 接收段)
- Test: `crates/core/tests/transfer.rs`(追加/修改)

**Interfaces:**
- Consumes: 既有 `send_file`/`recv_files`/`read_chunk`/`connected_pair`(测试辅助)
- Produces: `recv_files` 对外签名不变;行为变化:接收方内存 O(1 块),`.part` 预分配 + 定位写 + 顺序读回校验。M2c(多文件/续传)在此之上构建。

- [ ] **Step 1: 写失败测试**

`crates/core/tests/transfer.rs`:把既有参数化循环 `for (name, size) in [("小.md", 100), ("big.bin", CHUNK_SIZE*2+555)]` 扩为含零字节:

```rust
for (name, size) in [
    ("小.md", 100usize),
    ("empty.txt", 0usize),
    ("big.bin", transfer::CHUNK_SIZE * 2 + 555),
] {
```

文件末尾追加三个测试:

```rust
#[tokio::test]
async fn large_file_streams_correctly() {
    // 16 MiB(16 块)覆盖定位写 + 读回校验
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "large.bin", transfer::CHUNK_SIZE * 16);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let f_path = f.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_file(&sc, &f_path, "large.bin", &a, &|_, _| {}).await.unwrap()
    });
    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, dst.path(), &b).await.unwrap() });
    send_task.await.unwrap();
    let landed = recv_task.await.unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}

#[tokio::test]
async fn corrupt_chunk_never_written() {
    // 手工驱动协议:manifest 合法,但块数据与清单哈希不符 →
    // 接收方必须拒块,坏字节不得落盘(.part 该偏移保持预分配零)。
    use prost::Message as _;
    use tokio::io::AsyncWriteExt;

    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "victim.bin", transfer::CHUNK_SIZE);
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = mtdss_core::protocol::manifest::build_single(&f, "victim.bin", &a).unwrap();
    let tid = man.transfer_id.clone();
    let part_path = out.path().join(format!("{tid}.mtdss-part"));

    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, out.path(), &b).await });

    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let mb = man.encode_to_vec();
    ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
    ctl_tx.write_all(&mb).await.unwrap();
    let mut verdict = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut ctl_rx, &mut verdict).await.unwrap();
    assert_eq!(verdict[0], 1, "receiver must accept (space is fine)");

    // 唯一一块,发坏数据
    let mut uni = sc.open_uni().await.unwrap();
    uni.write_all(&0u64.to_be_bytes()).await.unwrap();
    uni.write_all(&(transfer::CHUNK_SIZE as u64).to_be_bytes()).await.unwrap();
    uni.write_all(&vec![0xDEu8; transfer::CHUNK_SIZE]).await.unwrap();
    uni.finish().unwrap();

    let err = recv_task.await.unwrap().unwrap_err();
    assert!(
        err.to_string().contains("chunk hash mismatch"),
        "unexpected error: {err:#}"
    );
    let part = std::fs::read(&part_path).unwrap();
    assert_eq!(part.len(), transfer::CHUNK_SIZE, "preallocated size");
    assert!(
        part.iter().all(|&b| b == 0),
        "corrupt chunk bytes must never hit the .part file"
    );
}

#[tokio::test]
async fn reverse_order_chunks_assemble_correctly() {
    // 两个块,后块先完整发送:接收方按任意顺序定位写仍能重组正确文件。
    use prost::Message as _;
    use tokio::io::AsyncWriteExt;

    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "ooo.bin", transfer::CHUNK_SIZE * 2);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = mtdss_core::protocol::manifest::build_single(&f, "ooo.bin", &a).unwrap();
    let chunks = man.files[0].chunks.clone();

    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, out.path(), &b).await });

    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let mb = man.encode_to_vec();
    ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
    ctl_tx.write_all(&mb).await.unwrap();
    let mut verdict = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut ctl_rx, &mut verdict).await.unwrap();
    assert_eq!(verdict[0], 1);

    async fn send_chunk(
        conn: &quinn::Connection,
        id: u64,
        data: &[u8],
    ) {
        let mut uni = conn.open_uni().await.unwrap();
        uni.write_all(&id.to_be_bytes()).await.unwrap();
        uni.write_all(&(data.len() as u64).to_be_bytes()).await.unwrap();
        uni.write_all(data).await.unwrap();
        uni.finish().unwrap();
        // 不等待:先开的流流 ID 更低,QUIC 按流交付,accept_uni 顺序确定
    }

    // 后块先发
    let second = &data[(chunks[1].offset as usize)..(chunks[1].offset + chunks[1].length) as usize];
    send_chunk(&sc, chunks[1].offset, second).await;
    let first = &data[..(chunks[0].length as usize)];
    send_chunk(&sc, chunks[0].offset, first).await;

    // 完成标记 + 最终确认
    ctl_tx.write_all(&[0xFF]).await.unwrap();
    let mut ack = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut ctl_rx, &mut ack).await.unwrap();
    assert_eq!(ack[0], 0x01);

    let landed = recv_task.await.unwrap().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}
```

- [ ] **Step 2: 跑测试确认行为**

Run: `cargo test -p mtdss-core --test transfer`
Expected: 现实现下 `corrupt_chunk_never_written` FAIL——旧代码把坏块插进 `got` BTreeMap 后在整文件哈希处才失败,.part 一次性写入含坏字节(或旧实现在哈希处直接拒绝→.part 为空文件,长度断言也 FAIL:旧代码无 set_len);`reverse_order` 大概率 PASS(旧 BTreeMap 排序天然乱序安全);`empty.txt` 与 `large_file` PASS。若 corrupt 测试意外 PASS,检查断言粒度(必须含 .part 全零断言)。

- [ ] **Step 3: 最小实现**

`crates/core/src/transport/transfer.rs` 顶部 use 增加 `AsyncSeekExt`(现有 `AsyncReadExt, AsyncWriteExt` 处)与 `std::io::SeekFrom`。`recv_files` 第 3)/4) 段(从 `let entry = &files[0];` 到整文件哈希比对)替换为:

```rust
    // 3) 接收块(流式落盘):验块哈希 → seek+offset 定位写;内存 O(1 块)
    let entry = &files[0];
    let part_path = out_dir.join(format!("{}.mtdss-part", man.transfer_id));
    let mut part = tokio::fs::File::create(&part_path)
        .await
        .with_context(|| part_path.display().to_string())?;
    part.set_len(entry.size)
        .await
        .with_context(|| format!("preallocate {}", part_path.display()))?;
    let mut pending: HashMap<u64, &ChunkDesc> =
        entry.chunks.iter().map(|c| (c.offset, c)).collect();
    let mut remaining = pending.len();

    while remaining > 0 {
        let uni = conn.accept_uni().await?;
        let (id, data) = read_chunk(uni).await?;
        let desc = pending
            .remove(&id)
            .context("chunk id not in signed manifest")?;
        anyhow::ensure!(
            data.len() == desc.length as usize,
            "chunk length mismatch at offset {id}"
        );
        let mut h = Sha256::new();
        h.update(&data);
        anyhow::ensure!(
            h.finalize().to_vec() == desc.sha256,
            "chunk hash mismatch at offset {id}"
        );
        // 先验后写:坏块永远不落盘
        part.seek(std::io::SeekFrom::Start(desc.offset)).await?;
        part.write_all(&data).await?;
        remaining -= 1;
    }

    // 4) 整文件校验:顺序读回 .part 流式计算(第二遍顺序读,磁盘缓存友好)
    part.seek(std::io::SeekFrom::Start(0)).await?;
    let mut whole = Sha256::new();
    let mut rbuf = vec![0u8; 256 * 1024];
    loop {
        let n = part.read(&mut rbuf).await?;
        if n == 0 {
            break;
        }
        whole.update(&rbuf[..n]);
    }
    anyhow::ensure!(
        whole.finalize().to_vec() == entry.sha256,
        "whole-file hash mismatch"
    );
```

(原 `got: BTreeMap` 及其写出循环删除;模块文档注释同步更新接收路径描述。)

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-core --test transfer && cargo test -p mtdss-core --test e2e`
Expected: 全部 PASS(含既有坏块/空间预检/同名冲突用例)。

- [ ] **Step 5: 全量门禁 + 提交**

```bash
cargo test --workspace && cargo clippy --workspace -- -D warnings
git add crates/core/
git commit -m "feat(core): streaming receiver (preallocate, positioned writes, read-back verify)"
```

---

## 完成后

1. 核对 spec §6 验收清单逐项打勾
2. M2a 分支经 PR 合入 main(执行技能的收尾流程),随后启动 M2b(NAT 打洞)brainstorm
