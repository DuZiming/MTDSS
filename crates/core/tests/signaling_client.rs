use mtdss_core::signaling::client::{ClientEvent, SignalingClient};
use signaling::spawn_test_server;

async fn register(
    addr: &std::net::SocketAddr,
    email: &str,
    seed: u8,
) -> (String, String, mtdss_core::identity::DeviceIdentity) {
    let id = mtdss_core::identity::DeviceIdentity::from_seed(&[seed; 32]);
    let client = reqwest::Client::new();
    let code: serde_json::Value = client
        .post(format!("http://{addr}/auth/request-code"))
        .json(&serde_json::json!({"email": email}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .post(format!("http://{addr}/auth/verify"))
        .json(&serde_json::json!({
            "email": email, "code": code["code"].as_str().unwrap(),
            "pubkey": hex::encode(id.public_key().as_bytes()),
            "name": format!("dev-{seed}"), "platform": "test"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        v["jwt"].as_str().unwrap().into(),
        v["device_id"].as_str().unwrap().into(),
        id,
    )
}

#[tokio::test]
async fn connect_relay_presence_roundtrip() {
    let (addr, s) = spawn_test_server().await;
    // 认证收口后仅预置账号可领码:登录前先预置(幂等,下同)
    signaling::admin::user_add(&s.db.lock().unwrap(), "a@b.c").unwrap();
    let (jwt_a, dev_a, _id_a) = register(&addr, "a@b.c", 1).await;
    let (jwt_b, dev_b, _id_b) = register(&addr, "a@b.c", 2).await;

    let (ca, mut ev_a) = SignalingClient::connect(&format!("ws://{addr}/ws"), &jwt_a, &dev_a)
        .await
        .unwrap();
    let (cb, mut ev_b) = SignalingClient::connect(&format!("ws://{addr}/ws"), &jwt_b, &dev_b)
        .await
        .unwrap();

    // A 看到 B 上线（先跳过自己的 HelloOk 握手确认）
    loop {
        match ev_a.recv().await.unwrap() {
            ClientEvent::HelloOk { .. } => continue,
            ClientEvent::Presence { device_id, online } => {
                assert_eq!(device_id, dev_b);
                assert!(online);
                break;
            }
            other => panic!("{other:?}"),
        }
    }

    ca.relay(&dev_b, b"hello-b").await.unwrap();
    loop {
        match ev_b.recv().await.unwrap() {
            ClientEvent::HelloOk { .. } => continue,
            ClientEvent::Relayed { from, payload } => {
                assert_eq!(from, dev_a);
                assert_eq!(payload, b"hello-b");
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    drop(cb);
}

#[tokio::test]
async fn bad_jwt_gets_error_event() {
    let (addr, s) = spawn_test_server().await;
    signaling::admin::user_add(&s.db.lock().unwrap(), "a@b.c").unwrap();
    let (_jwt_a, dev_a, _id) = register(&addr, "a@b.c", 3).await;
    let res = SignalingClient::connect(&format!("ws://{addr}/ws"), "bogus", &dev_a).await;
    // 握手被拒：connect 返回 Err 或首个事件为 Error 后 Closed
    match res {
        Err(_) => {}
        Ok((_c, mut ev)) => match ev.recv().await.unwrap() {
            ClientEvent::Error { code, .. } => assert_eq!(code, 1),
            other => panic!("{other:?}"),
        },
    }
}

#[tokio::test]
async fn client_detects_silent_server() {
    use futures_util::SinkExt;
    use mtdss_core::signaling::client::Keepalive;
    use mtdss_proto::ws::*;
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
                stun_addrs: Vec::new(),
                // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(ok.encode_to_vec()))
            .await
            .unwrap();
        futures_util::future::pending::<()>().await; // 永不再 poll:连接活但不响应
    });

    let (_client, mut ev) = mtdss_core::signaling::client::SignalingClient::connect_with(
        &format!("ws://{addr}/ws"),
        "jwt",
        "dev-x",
        None,
        Keepalive {
            ping_interval: Duration::from_millis(100),
            silent_limit: Duration::from_millis(400),
        },
    )
    .await
    .unwrap();

    // 事件序列统一后:本地判死退出必须先 Error(code=0, 原因含 silent)再 Closed
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_error = false;
    loop {
        match tokio::time::timeout(Duration::from_millis(200), ev.recv()).await {
            Ok(Some(mtdss_core::signaling::client::ClientEvent::Error { code, message })) => {
                assert_eq!(code, 0, "local exit diagnostics use code 0");
                assert!(
                    message.contains("silent"),
                    "reason must name the cause: {message}"
                );
                assert!(!saw_error, "exactly one Error expected before Closed");
                saw_error = true;
            }
            // 假服务器先发 HelloOk:忽略,继续等退出序列
            Ok(Some(mtdss_core::signaling::client::ClientEvent::HelloOk { .. })) => continue,
            Ok(Some(mtdss_core::signaling::client::ClientEvent::Closed)) => break,
            Ok(Some(other)) => panic!("expected Error-then-Closed, got {other:?}"),
            Ok(None) => panic!("event stream ended without Closed"),
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "no exit sequence within 5s of silence"
                );
            }
        }
    }
    assert!(
        saw_error,
        "Error(code=0) must precede Closed on keepalive death"
    );
}

/// Task 6:服务器回 HelloOk 后直接断 TCP(无 Close 帧)→ 客户端事件流必须
/// 依次出现 Error(code=0, 含原因)与 Closed,而非流尽(None)。
#[tokio::test]
async fn server_drop_after_hello_yields_error_then_closed() {
    use futures_util::SinkExt;
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(ok.encode_to_vec()))
            .await
            .unwrap();
        drop(ws); // 直接断 TCP:不发 WS Close 帧
    });

    let (_client, mut ev) = SignalingClient::connect_with(
        &format!("ws://{addr}/ws"),
        "jwt",
        "dev-x",
        None,
        mtdss_core::signaling::client::Keepalive::default(),
    )
    .await
    .unwrap();

    let mut saw_error = false;
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ev.recv())
            .await
            .expect("no event within 5s of server drop")
            .expect("event stream must not end (None) before Closed");
        match msg {
            ClientEvent::HelloOk { .. } => continue, // 序言握手确认:忽略
            ClientEvent::Error { code, message } => {
                assert_eq!(code, 0, "local exit diagnostics use code 0");
                assert!(!message.is_empty(), "diagnostic must carry a reason");
                saw_error = true;
            }
            ClientEvent::Closed => break,
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert!(
        saw_error,
        "Error(code=0) must precede Closed on server drop"
    );
}

/// v2-P1c 改写 P1b 兼容锚(原 server_relay_pushes_are_ignored_without_
/// disconnect):中继推送不再整体忽略——RelaySession 以事件面呈现(字段
/// 透传,含死因 reason:死帧透传、存活帧空串);RelayAllocated 无 pending
/// 时归分配面 warn 丢弃(不进事件流)。每帧之后跟一帧 Presence 探针,
/// 钉死「帧被新分支消化且连接不断」。
#[tokio::test]
async fn relay_session_pushes_surface_as_event() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
        let hello_ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        // 死帧:reason 透传死因(quota)
        let relay_session_dead = WsServerFrame {
            frame: Some(ws_server_frame::Frame::RelaySession(RelaySession {
                sid: 7,
                alive: false,
                tag: vec![9, 9, 9],
                relay_addr: "203.0.113.9:8788".into(),
                expires_at: 1_700_000_000,
                peer_device: "dev-peer".into(),
                reason: "quota".into(),
                purpose: String::new(),
            })),
        };
        // 存活帧:reason 恒空串;purpose 回显分配平面(v2-P2 票①)
        let relay_session_alive = WsServerFrame {
            frame: Some(ws_server_frame::Frame::RelaySession(RelaySession {
                sid: 7,
                alive: true,
                tag: vec![9, 9, 9],
                relay_addr: "203.0.113.9:8788".into(),
                expires_at: 1_700_000_000,
                peer_device: "dev-peer".into(),
                reason: String::new(),
                purpose: "transport".into(),
            })),
        };
        let relay_allocated = WsServerFrame {
            frame: Some(ws_server_frame::Frame::RelayAllocated(RelayAllocated {
                sid: 8,
                tag: Vec::new(),
                relay_addr: String::new(),
                expires_at: 0,
                rate_kbps: 0,
                peer_device: String::new(),
                reason: String::new(),
                purpose: String::new(),
            })),
        };
        let presence = |online: bool| WsServerFrame {
            frame: Some(ws_server_frame::Frame::Presence(Presence {
                device_id: "dev-peer".into(),
                online,
            })),
        };
        for f in [
            hello_ok,
            relay_session_dead,
            presence(true),
            relay_session_alive,
            presence(false),
            relay_allocated,
            presence(true),
        ] {
            ws.send(WsMessage::Binary(f.encode_to_vec())).await.unwrap();
        }
        // 持续 poll(保活 Pong 由库层应答):连接保持存活供后续探针
        while ws.next().await.is_some() {}
    });

    let (_client, mut ev) = SignalingClient::connect_with(
        &format!("ws://{addr}/ws"),
        "jwt",
        "dev-x",
        None,
        mtdss_core::signaling::client::Keepalive::default(),
    )
    .await
    .unwrap();

    // 序言握手确认
    match ev.recv().await.unwrap() {
        ClientEvent::HelloOk { .. } => {}
        other => panic!("expected HelloOk first, got {other:?}"),
    }
    // 死帧以事件面呈现,字段透传,reason 透传死因
    match ev.recv().await.unwrap() {
        ClientEvent::RelaySession { sid, alive, tag, relay_addr, expires_at, peer_device, reason, .. } => {
            assert_eq!(sid, 7);
            assert!(!alive);
            assert_eq!(tag, vec![9, 9, 9]);
            assert_eq!(relay_addr, "203.0.113.9:8788");
            assert_eq!(expires_at, 1_700_000_000);
            assert_eq!(peer_device, "dev-peer");
            assert_eq!(reason, "quota");
        }
        other => panic!("expected RelaySession event, got {other:?}"),
    }
    // 探针 1:死帧后的连接仍活
    match ev.recv().await.unwrap() {
        ClientEvent::Presence { device_id, online } => {
            assert_eq!(device_id, "dev-peer");
            assert!(online, "presence must follow stub order");
        }
        other => panic!("expected presence probe, got {other:?}"),
    }
    // 存活帧:reason 恒空串;purpose 透传分配平面
    match ev.recv().await.unwrap() {
        ClientEvent::RelaySession { alive, reason, purpose, .. } => {
            assert!(alive);
            assert!(reason.is_empty(), "alive frame must carry empty reason");
            assert_eq!(purpose, "transport", "alive frame must pass purpose through");
        }
        other => panic!("expected alive RelaySession event, got {other:?}"),
    }
    // 探针 2:存活帧后的连接仍活
    match ev.recv().await.unwrap() {
        ClientEvent::Presence { device_id, online } => {
            assert_eq!(device_id, "dev-peer");
            assert!(!online, "presence must follow stub order");
        }
        other => panic!("expected presence probe, got {other:?}"),
    }
    // 无主 RelayAllocated 不进事件流(warn 丢弃):探针 3 按序可见,
    // 序即证明分配帧被消化而非缓冲重排
    match ev.recv().await.unwrap() {
        ClientEvent::Presence { device_id, online } => {
            assert_eq!(device_id, "dev-peer");
            assert!(online, "presence must follow stub order");
        }
        other => panic!("unowned allocation must be dropped silently, got {other:?}"),
    }
    // 退出序列(Error+Closed)是断连瞬间同步发出的:短窗内再无事件即为不断连
    let late = tokio::time::timeout(std::time::Duration::from_millis(300), ev.recv()).await;
    match late {
        Err(_) => {} // 超时无事件:正确
        Ok(Some(other)) => panic!("no exit sequence after relay pushes, got {other:?}"),
        Ok(None) => panic!("event stream must not end"),
    }
}

/// 裸 WS 桩的错误帧:码与消息同串,便于客户端断言透传。
fn stub_error_frame(code: u32) -> mtdss_proto::ws::WsServerFrame {
    use mtdss_proto::ws::*;
    WsServerFrame {
        frame: Some(ws_server_frame::Frame::Error(Error {
            code,
            message: format!("stub deny {code}"),
        })),
    }
}

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

    assert!(mtdss_core::signaling::client::ws_connector(Some(&ca_path))
        .unwrap()
        .is_some());
    // ca=None(公共 CA)如今也返回显式 rustls 连接器(见下方专项测试)
    assert!(mtdss_core::signaling::client::ws_connector(None)
        .unwrap()
        .is_some());
    let bad = dir.path().join("bad.crt");
    std::fs::write(&bad, b"not a pem").unwrap();
    assert!(mtdss_core::signaling::client::ws_connector(Some(&bad)).is_err());
}

/// 回归钉子(2026-10-30 域名切换 www.mtdss.top 公共 CA 首踩):ca=None 曾
/// 返回 Ok(None),tokio-tungstenite 落默认 TLS 路径 → `ClientConfig::builder()`
/// 惰性自动检测 process 默认 provider;工作区特性统一后 ring 与 aws-lc-rs
/// 并存,检测歧义即 panic(`get_default_or_install_from_crate_features`),
/// daemon 信令任务经 tokio::spawn 静默死。如今 None 分支显式 ring provider
/// + webpki 根建配置:返回 Some 且为 Rustls 形态,构造(含 builder)不 panic。
#[test]
fn ws_connector_none_public_ca_is_explicit_rustls() {
    let conn = mtdss_core::signaling::client::ws_connector(None).unwrap();
    assert!(
        matches!(conn, Some(tokio_tungstenite::Connector::Rustls(_))),
        "None(公共 CA)必须返回显式 Rustls 连接器,不得回退默认 TLS 路径"
    );
}

#[tokio::test]
async fn hello_ok_surfaced_as_event() {
    let (addr, s) = signaling::spawn_test_server_with(
        signaling::ws::Keepalive::default(),
        vec![
            "198.51.100.10:3478".to_string(),
            "198.51.100.10:3479".to_string(),
        ],
    )
    .await;
    signaling::admin::user_add(&s.db.lock().unwrap(), "ho@x.y").unwrap();
    let (jwt, dev, _id) = register(&addr, "ho@x.y", 7).await;

    let (_c, mut ev) = mtdss_core::signaling::client::SignalingClient::connect(
        &format!("ws://{addr}/ws"),
        &jwt,
        &dev,
    )
    .await
    .unwrap();
    match ev.recv().await.unwrap() {
        ClientEvent::HelloOk { stun_addrs, .. } => {
            assert_eq!(stun_addrs.len(), 2);
            assert_eq!(stun_addrs[0], "198.51.100.10:3478");
        }
        other => panic!("expected HelloOk first, got {other:?}"),
    }
}

/// HelloOk 资格三字段透传:真实服务器 + admin 预置(开通/2048kbps/3GiB)。
#[tokio::test]
async fn hello_ok_carries_entitlement() {
    let (addr, s) = signaling::spawn_test_server().await;
    signaling::admin::user_add(&s.db.lock().unwrap(), "ent@x.y").unwrap();
    signaling::admin::relay_set_enabled(&s.db.lock().unwrap(), "ent@x.y", true).unwrap();
    signaling::admin::relay_set_limit(&s.db.lock().unwrap(), "ent@x.y", 2048).unwrap();
    signaling::admin::relay_set_quota(&s.db.lock().unwrap(), "ent@x.y", 3).unwrap();
    let (jwt, dev, _id) = register(&addr, "ent@x.y", 11).await;

    let (_c, mut ev) = SignalingClient::connect(&format!("ws://{addr}/ws"), &jwt, &dev)
        .await
        .unwrap();
    match ev.recv().await.unwrap() {
        ClientEvent::HelloOk { relay, .. } => {
            assert!(relay.enabled, "enabled must pass through");
            assert_eq!(relay.rate_kbps, 2048);
            // 3 GiB 配额当期无用量:余量 = 3 × 1024³
            assert_eq!(relay.quota_remaining, 3 * 1024 * 1024 * 1024);
        }
        other => panic!("expected HelloOk first, got {other:?}"),
    }
}

/// 前向兼容锚:旧服务器 HelloOk 不带资格字段。prost 对零值不编码,桩按 0 值
/// 补齐即与旧服务器线上形态逐字节一致;客户端必须缺省成 enabled=false
/// (永不发起分配)。
#[tokio::test]
async fn hello_without_relay_fields_means_disabled() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                // 零值字段不上线:即旧服务器形态
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(ok.encode_to_vec())).await.unwrap();
        while ws.next().await.is_some() {}
    });

    let (_c, mut ev) = SignalingClient::connect(&format!("ws://{addr}/ws"), "jwt", "dev-x")
        .await
        .unwrap();
    match ev.recv().await.unwrap() {
        ClientEvent::HelloOk { relay, .. } => {
            assert!(!relay.enabled, "old-server form must default to disabled");
            assert_eq!(relay.rate_kbps, 0);
            assert_eq!(relay.quota_remaining, 0);
        }
        other => panic!("expected HelloOk first, got {other:?}"),
    }
}

/// 分配往返:发 RelayAllocate(to_device 透传)→ 桩回 RelayAllocated →
/// 各字段经 pending oneshot 完整落到 Allocated;应答不进事件流(其后仅见
/// 探针 Presence),连接保持可用。
#[tokio::test]
async fn relay_allocate_roundtrip() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_core::signaling::client::AllocOutcome;
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello_ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(hello_ok.encode_to_vec())).await.unwrap();
        // 收分配请求 → 校验 to_device 透传 → 回 RelayAllocated(可区分值)+ 探针
        loop {
            let WsMessage::Binary(bin) = ws.next().await.unwrap().unwrap() else { continue };
            let Ok(f) = WsClientFrame::decode(bin.as_slice()) else { continue };
            if let Some(ws_client_frame::Frame::RelayAllocate(ra)) = f.frame {
                assert_eq!(ra.to_device, "dev-peer-9");
                assert_eq!(ra.purpose, "transport", "purpose 须随分配请求透传");
                let alloc = WsServerFrame {
                    frame: Some(ws_server_frame::Frame::RelayAllocated(RelayAllocated {
                        sid: 42,
                        tag: b"tag-42".to_vec(),
                        relay_addr: "203.0.113.10:8788".into(),
                        expires_at: 1_800_000_000,
                        rate_kbps: 2048,
                        peer_device: "dev-peer-9".into(),
                        reason: String::new(),
                        purpose: String::new(),
                    })),
                };
                ws.send(WsMessage::Binary(alloc.encode_to_vec())).await.unwrap();
                // 应答后的探针:证明连接仍活、事件流无分配应答残留
                let probe = WsServerFrame {
                    frame: Some(ws_server_frame::Frame::Presence(Presence {
                        device_id: "dev-peer".into(),
                        online: true,
                    })),
                };
                ws.send(WsMessage::Binary(probe.encode_to_vec())).await.unwrap();
            }
        }
    });

    let (client, mut ev) = SignalingClient::connect(&format!("ws://{addr}/ws"), "jwt", "dev-x")
        .await
        .unwrap();
    match ev.recv().await.unwrap() {
        ClientEvent::HelloOk { .. } => {}
        other => panic!("expected HelloOk first, got {other:?}"),
    }
    let outcome = client.relay_allocate("dev-peer-9", "transport").await.unwrap();
    match outcome {
        AllocOutcome::Allocated { sid, tag, relay_addr, expires_at, rate_kbps } => {
            assert_eq!(sid, 42);
            assert_eq!(tag, b"tag-42".to_vec());
            assert_eq!(relay_addr, "203.0.113.10:8788");
            assert_eq!(expires_at, 1_800_000_000);
            assert_eq!(rate_kbps, 2048);
        }
        other => panic!("expected Allocated, got {other:?}"),
    }
    match ev.recv().await.unwrap() {
        ClientEvent::Presence { device_id, online } => {
            assert_eq!(device_id, "dev-peer");
            assert!(online);
        }
        other => panic!("allocated frame must not leak into event stream, got {other:?}"),
    }
}

/// 分配拒绝面:pending 窗口内服务器回 Error 4/5/6/2 → 一律归因分配成
/// Denied(码与消息透传),且不外发事件(事件流短窗探测无 Error);末轮
/// 回非归因码 1 钉窗口外流向不变——仍走事件流,桩随即断连,本轮分配
/// 立即 Err 并按既有退出序列(Error 0 + Closed)收口。
#[tokio::test]
async fn relay_allocate_denied_codes() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_core::signaling::client::AllocOutcome;
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello_ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(hello_ok.encode_to_vec())).await.unwrap();
        // 五轮:前四轮逐码拒绝(4/5/6/2),末轮回非归因码 1 后断连。
        // 每轮读至分配请求为止(跳过连接序言的 Hello 等非分配帧)。
        for code in [4u32, 5, 6, 2, 1] {
            loop {
                let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
                    .await
                    .expect("client frame did not arrive")
                    .unwrap()
                    .unwrap();
                let WsMessage::Binary(bin) = msg else { continue };
                let f = WsClientFrame::decode(bin.as_slice()).unwrap();
                if matches!(f.frame, Some(ws_client_frame::Frame::RelayAllocate(_))) {
                    break;
                }
            }
            ws.send(WsMessage::Binary(stub_error_frame(code).encode_to_vec()))
                .await
                .unwrap();
        }
        drop(ws);
    });

    let (client, mut ev) = SignalingClient::connect(&format!("ws://{addr}/ws"), "jwt", "dev-x")
        .await
        .unwrap();
    match ev.recv().await.unwrap() {
        ClientEvent::HelloOk { .. } => {}
        other => panic!("expected HelloOk first, got {other:?}"),
    }
    for code in [4u32, 5, 6, 2] {
        match client.relay_allocate("dev-peer", "transport").await.unwrap() {
            AllocOutcome::Denied { code: c, message } => {
                assert_eq!(c, code);
                assert_eq!(message, format!("stub deny {code}"));
            }
            other => panic!("expected Denied({code}), got {other:?}"),
        }
    }
    // 四轮拒绝均未外发事件:短窗探测必须无事件
    let probe = tokio::time::timeout(Duration::from_millis(200), ev.recv()).await;
    assert!(
        probe.is_err(),
        "denied errors must not leak into the event stream"
    );
    // 窗口边界锚:Error 1(非归因码)在 pending 窗口内仍走事件流;
    // 断连使本轮分配立即 Err 而非 10s 超时
    let res = client.relay_allocate("dev-peer", "transport").await;
    assert!(res.is_err(), "closed connection must fail the allocation");
    match ev.recv().await.unwrap() {
        ClientEvent::Error { code, message } => {
            assert_eq!(code, 1);
            assert_eq!(message, "stub deny 1");
        }
        other => panic!("error 1 must flow to event stream, got {other:?}"),
    }
    // 断连退出序列照旧:Error(0, 诊断) + Closed
    match ev.recv().await.unwrap() {
        ClientEvent::Error { code, message } => {
            assert_eq!(code, 0, "local exit diagnostics use code 0");
            assert!(!message.is_empty(), "diagnostic must carry a reason");
        }
        other => panic!("expected local diagnostic error, got {other:?}"),
    }
    assert!(matches!(ev.recv().await.unwrap(), ClientEvent::Closed));
}

/// 分配在途时服务器断连:relay_allocate 必须经退出序列摘槽立即 Err,
/// 而非悬等 10s 超时。
#[tokio::test]
async fn pending_allocate_fails_on_close() {
    use futures_util::{SinkExt, StreamExt};
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello_ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(hello_ok.encode_to_vec())).await.unwrap();
        // 收到分配请求即断连(不回任何应答)
        while let Some(Ok(msg)) = ws.next().await {
            let WsMessage::Binary(bin) = msg else { continue };
            let Ok(f) = WsClientFrame::decode(bin.as_slice()) else { continue };
            if matches!(f.frame, Some(ws_client_frame::Frame::RelayAllocate(_))) {
                drop(ws);
                return;
            }
        }
    });

    let (client, _ev) = SignalingClient::connect(&format!("ws://{addr}/ws"), "jwt", "dev-x")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let res = tokio::time::timeout(Duration::from_secs(5), client.relay_allocate("dev-x", "transport"))
        .await
        .expect("allocation must fail fast on close, not hang to the 10s timeout");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "close must surface immediately, not after a timeout"
    );
    let err = res.unwrap_err();
    assert!(
        format!("{err:#}").contains("closed"),
        "error must name the connection close, got {err:#}"
    );
}

/// 连接已死后的分配调用(修复轮):send 失败早退分支必须摘 pending 槽——
/// 摘槽两侧分工,任务退出序列的统一摘槽覆盖任务侧;任务早于本调用退出的
/// 时序(退出摘槽已跑过,槽内是本分支刚放入的 sender)由该分支自摘。钉:
/// 第一次调用报「signaling client gone」,第二次调用不得误报「previous
/// relay allocate still pending」。
#[tokio::test]
async fn allocate_after_task_exit_reuses_slot() {
    use futures_util::SinkExt;
    use mtdss_proto::ws::*;
    use prost::Message as _;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        // 测试桩仅关心握手:中继资格字段按 0 值补齐(prost 可缺省)
        let hello_ok = WsServerFrame {
            frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
                account_id: "a".into(),
                stun_addrs: Vec::new(),
                relay_enabled: false,
                relay_rate_kbps: 0,
                relay_quota_remaining_bytes: 0,
            })),
        };
        ws.send(WsMessage::Binary(hello_ok.encode_to_vec())).await.unwrap();
        drop(ws); // HelloOk 后即断连:客户端 WS 任务随之退出
    });

    let (client, mut ev) = SignalingClient::connect(&format!("ws://{addr}/ws"), "jwt", "dev-x")
        .await
        .unwrap();
    // 等退出序列收尾(Closed),再让出调度让 WS 任务跑完退出摘槽
    loop {
        match ev.recv().await.unwrap() {
            ClientEvent::HelloOk { .. } | ClientEvent::Error { .. } => continue,
            ClientEvent::Closed => break,
            other => panic!("unexpected event {other:?}"),
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 第一次:占槽 → send 失败(任务已死)→ 须报 send 失败并自摘槽
    let e1 = client.relay_allocate("dev-x", "transport").await.unwrap_err();
    assert!(
        format!("{e1:#}").contains("signaling client gone"),
        "first call must name the send failure, got {e1:#}"
    );
    // 第二次:send 失败分支若未摘槽,此处误报「previous relay allocate
    // still pending」(修复轮揭盖)
    let e2 = client.relay_allocate("dev-x", "transport").await.unwrap_err();
    assert!(
        format!("{e2:#}").contains("signaling client gone"),
        "send-failure branch must drop the pending slot, second call got {e2:#}"
    );
}
