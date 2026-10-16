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
    let (addr, _s) = spawn_test_server().await;
    let (jwt_a, dev_a, _id_a) = register(&addr, "a@b.c", 1).await;
    let (jwt_b, dev_b, _id_b) = register(&addr, "a@b.c", 2).await;

    let (ca, mut ev_a) = SignalingClient::connect(
        &format!("ws://{addr}/ws"), &jwt_a, &dev_a).await.unwrap();
    let (cb, mut ev_b) = SignalingClient::connect(
        &format!("ws://{addr}/ws"), &jwt_b, &dev_b).await.unwrap();

    // A 看到 B 上线
    match ev_a.recv().await.unwrap() {
        ClientEvent::Presence { device_id, online } => {
            assert_eq!(device_id, dev_b);
            assert!(online);
        }
        other => panic!("{other:?}"),
    }

    ca.relay(&dev_b, b"hello-b").await.unwrap();
    match ev_b.recv().await.unwrap() {
        ClientEvent::Relayed { from, payload } => {
            assert_eq!(from, dev_a);
            assert_eq!(payload, b"hello-b");
        }
        other => panic!("{other:?}"),
    }
    drop(cb);
}

#[tokio::test]
async fn bad_jwt_gets_error_event() {
    let (addr, _s) = spawn_test_server().await;
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
