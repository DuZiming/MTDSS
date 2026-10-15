use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message;
use signaling::spawn_test_server;

async fn register(
    addr: &std::net::SocketAddr,
    email: &str,
    seed: &[u8; 32],
    name: &str,
) -> (String, String) {
    // 常量字节多数不是合法 Ed25519 曲线点（VerifyingKey::from_bytes 会拒绝）；
    // 用 seed 派生真实密钥对，保证 pubkey 可解压且各设备不同。
    let vk = ed25519_dalek::SigningKey::from_bytes(seed)
        .verifying_key()
        .to_bytes();
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
            "pubkey": hex::encode(vk), "name": name, "platform": "test"
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
    )
}

async fn connect_ws(
    addr: &std::net::SocketAddr,
) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    ws
}

async fn hello(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    jwt: &str,
    device_id: &str,
) -> WsServerFrame {
    let f = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: device_id.into(),
            jwt: jwt.into(),
        })),
    };
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        f.encode_to_vec().into(),
    ))
    .await
    .unwrap();
    recv(ws).await
}

async fn recv(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> WsServerFrame {
    loop {
        match ws.next().await.unwrap().unwrap() {
            tokio_tungstenite::tungstenite::Message::Binary(b) => {
                return WsServerFrame::decode(b.as_ref()).unwrap()
            }
            tokio_tungstenite::tungstenite::Message::Ping(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn hello_ok_presence_and_relay_roundtrip() {
    let (addr, _state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&addr, "a@b.c", &[1u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&addr, "a@b.c", &[2u8; 32], "B").await;

    let mut ws_a = connect_ws(&addr).await;
    let f = hello(&mut ws_a, &jwt_a, &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));

    let mut ws_b = connect_ws(&addr).await;
    let f = hello(&mut ws_b, &jwt_b, &dev_b).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));

    // A 应看到 B 上线广播
    let f = recv(&mut ws_a).await;
    match f.frame {
        Some(ws_server_frame::Frame::Presence(p)) => {
            assert_eq!(p.device_id, dev_b);
            assert!(p.online);
        }
        other => panic!("expected presence, got {other:?}"),
    }

    // A -> B relay
    let relay = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Relay(Relay {
            to_device: dev_b.clone(),
            payload: vec![9, 9, 9],
        })),
    };
    ws_a.send(tokio_tungstenite::tungstenite::Message::Binary(
        relay.encode_to_vec().into(),
    ))
    .await
    .unwrap();
    let f = recv(&mut ws_b).await;
    match f.frame {
        Some(ws_server_frame::Frame::Relayed(r)) => {
            assert_eq!(r.from_device, dev_a);
            assert_eq!(r.payload, vec![9, 9, 9]);
        }
        other => panic!("expected relayed, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_jwt_rejected() {
    let (addr, _state) = spawn_test_server().await;
    let (_jwt_a, dev_a) = register(&addr, "a@b.c", &[3u8; 32], "A").await;
    let mut ws = connect_ws(&addr).await;
    let f = hello(&mut ws, "bogus", &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == 1));
    // 连接应被关闭
    assert!(ws.next().await.is_none_or(|r| r.is_err()));
}

#[tokio::test]
async fn relay_to_offline_and_cross_account_rejected() {
    let (addr, _state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&addr, "a@b.c", &[4u8; 32], "A").await;
    let (jwt_x, dev_x) = register(&addr, "x@y.z", &[5u8; 32], "X").await;
    let mut ws_a = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;

    // 目标不在线
    let relay = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Relay(Relay {
            to_device: "ffffffffffffffffffffffffffffffff".into(),
            payload: vec![1],
        })),
    };
    ws_a.send(tokio_tungstenite::tungstenite::Message::Binary(
        relay.encode_to_vec().into(),
    ))
    .await
    .unwrap();
    let f = recv(&mut ws_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == 2));

    // 跨账号设备在线也不允许 relay
    let mut ws_x = connect_ws(&addr).await;
    hello(&mut ws_x, &jwt_x, &dev_x).await;
    let relay = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Relay(Relay {
            to_device: dev_x.clone(),
            payload: vec![2],
        })),
    };
    ws_a.send(tokio_tungstenite::tungstenite::Message::Binary(
        relay.encode_to_vec().into(),
    ))
    .await
    .unwrap();
    let f = recv(&mut ws_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == 2));
}

#[tokio::test]
async fn disconnect_broadcasts_offline() {
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&addr, "a@b.c", &[6u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&addr, "a@b.c", &[7u8; 32], "B").await;
    let mut ws_a = connect_ws(&addr).await;
    hello(&mut ws_a, &jwt_a, &dev_a).await;
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    let _ = recv(&mut ws_a).await; // B 上线
    drop(ws_b);
    // 等待服务器感知断线
    for _ in 0..50 {
        if !state.presence.is_online(&dev_b) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!state.presence.is_online(&dev_b));
    let f = recv(&mut ws_a).await;
    match f.frame {
        Some(ws_server_frame::Frame::Presence(p)) => {
            assert_eq!(p.device_id, dev_b);
            assert!(!p.online);
        }
        other => panic!("expected offline presence, got {other:?}"),
    }
}

#[tokio::test]
async fn reconnect_same_device_keeps_new_registration() {
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&addr, "a@b.c", &[1u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&addr, "a@b.c", &[2u8; 32], "B").await;

    // 同一设备两次连接：第二次 hello 覆盖注册表
    let mut ws1 = connect_ws(&addr).await;
    let f = hello(&mut ws1, &jwt_a, &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));
    let mut ws2 = connect_ws(&addr).await;
    let f = hello(&mut ws2, &jwt_a, &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));

    // 旧连接断开：旧 task 的清理不得删掉新注册（2s 轮询期间必须始终在线）
    drop(ws1);
    for _ in 0..100 {
        assert!(
            state.presence.is_online(&dev_a),
            "old connection's cleanup must not remove the new registration"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(state.presence.is_online(&dev_a));

    // 新连接仍可达：同账号 B 中继成功并被 ws2 收到
    let mut ws_b = connect_ws(&addr).await;
    hello(&mut ws_b, &jwt_b, &dev_b).await;
    let f = recv(&mut ws2).await; // B 上线广播
    match f.frame {
        Some(ws_server_frame::Frame::Presence(p)) => {
            assert_eq!(p.device_id, dev_b);
            assert!(p.online);
        }
        other => panic!("expected presence, got {other:?}"),
    }
    let relay = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Relay(Relay {
            to_device: dev_a.clone(),
            payload: vec![7, 7, 7],
        })),
    };
    ws_b.send(tokio_tungstenite::tungstenite::Message::Binary(
        relay.encode_to_vec().into(),
    ))
    .await
    .unwrap();
    let f = recv(&mut ws2).await;
    match f.frame {
        Some(ws_server_frame::Frame::Relayed(r)) => {
            assert_eq!(r.from_device, dev_b);
            assert_eq!(r.payload, vec![7, 7, 7]);
        }
        other => panic!("expected relayed, got {other:?}"),
    }

    // 新连接断开：正常清理 + offline 广播到 B
    drop(ws2);
    for _ in 0..50 {
        if !state.presence.is_online(&dev_a) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!state.presence.is_online(&dev_a));
    let f = recv(&mut ws_b).await;
    match f.frame {
        Some(ws_server_frame::Frame::Presence(p)) => {
            assert_eq!(p.device_id, dev_a);
            assert!(!p.online);
        }
        other => panic!("expected offline presence, got {other:?}"),
    }
}
