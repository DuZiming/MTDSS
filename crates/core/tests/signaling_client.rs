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

/// Review Focus #5(服务端推送不致断连):服务器推送 RelaySession/RelayAllocated
/// (1.0.x 客户端不认识的 v2 中继帧)必须被静默忽略——不产生 Error、不断连,
/// 连接继续可用。裸 WS 桩按序推送两帧、每帧后跟一帧 Presence 作"连接未断"
/// 探针,钉死 client.rs 的显式忽略臂。
#[tokio::test]
async fn server_relay_pushes_are_ignored_without_disconnect() {
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
        let relay_session = WsServerFrame {
            frame: Some(ws_server_frame::Frame::RelaySession(RelaySession {
                sid: 7,
                alive: false,
                tag: Vec::new(),
                relay_addr: String::new(),
                expires_at: 0,
            })),
        };
        let relay_allocated = WsServerFrame {
            frame: Some(ws_server_frame::Frame::RelayAllocated(RelayAllocated {
                sid: 7,
                tag: Vec::new(),
                relay_addr: String::new(),
                expires_at: 0,
                rate_kbps: 0,
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
            relay_session,
            presence(true),
            relay_allocated,
            presence(false),
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

    // 序言握手确认后只应看到两帧 Presence;任何 Error/Closed/流尽都是失败
    let mut seen_presence = 0;
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ev.recv())
            .await
            .expect("no event within 5s of stub pushes")
            .expect("event stream must not end (disconnect) during test");
        match msg {
            ClientEvent::HelloOk { .. } => continue, // 序言握手确认:忽略
            ClientEvent::Presence { device_id, online } => {
                assert_eq!(device_id, "dev-peer");
                // 两帧按桩序到达(先 online 后 offline):序即证明中继推送
                // 各自被跳过而非缓冲重排
                let expected_online = seen_presence == 0;
                seen_presence += 1;
                assert_eq!(online, expected_online, "presence must follow stub order");
                if seen_presence == 2 {
                    break;
                }
            }
            other => panic!("relay push must be ignored, got {other:?}"),
        }
    }
    // 退出序列(Error+Closed)是断连瞬间同步发出的:短窗内再无事件即为不断连
    let late = tokio::time::timeout(std::time::Duration::from_millis(300), ev.recv()).await;
    match late {
        Err(_) => {} // 超时无事件:正确
        Ok(Some(other)) => panic!("no exit sequence after ignored pushes, got {other:?}"),
        Ok(None) => panic!("event stream must not end"),
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
    assert!(mtdss_core::signaling::client::ws_connector(None)
        .unwrap()
        .is_none());
    let bad = dir.path().join("bad.crt");
    std::fs::write(&bad, b"not a pem").unwrap();
    assert!(mtdss_core::signaling::client::ws_connector(Some(&bad)).is_err());
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
        ClientEvent::HelloOk { stun_addrs } => {
            assert_eq!(stun_addrs.len(), 2);
            assert_eq!(stun_addrs[0], "198.51.100.10:3478");
        }
        other => panic!("expected HelloOk first, got {other:?}"),
    }
}
