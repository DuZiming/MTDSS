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
