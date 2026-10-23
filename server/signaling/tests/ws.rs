use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message;
use signaling::spawn_test_server;

async fn register(
    state: &std::sync::Arc<signaling::AppState>,
    addr: &std::net::SocketAddr,
    email: &str,
    seed: &[u8; 32],
    name: &str,
) -> (String, String) {
    // 认证收口后仅预置账号可领码:helper 统一预置(幂等,一处覆盖全部登录点)
    signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
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
        f.encode_to_vec(),
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
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[1u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "a@b.c", &[2u8; 32], "B").await;

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
        relay.encode_to_vec(),
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
    let (addr, state) = spawn_test_server().await;
    let (_jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[3u8; 32], "A").await;
    let mut ws = connect_ws(&addr).await;
    let f = hello(&mut ws, "bogus", &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == 1));
    // 连接应被关闭
    assert!(ws.next().await.is_none_or(|r| r.is_err()));
}

#[tokio::test]
async fn relay_to_offline_and_cross_account_rejected() {
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[4u8; 32], "A").await;
    let (jwt_x, dev_x) = register(&state, &addr, "x@y.z", &[5u8; 32], "X").await;
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
        relay.encode_to_vec(),
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
        relay.encode_to_vec(),
    ))
    .await
    .unwrap();
    let f = recv(&mut ws_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == 2));
}

#[tokio::test]
async fn disconnect_broadcasts_offline() {
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[6u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "a@b.c", &[7u8; 32], "B").await;
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
    let (jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[1u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "a@b.c", &[2u8; 32], "B").await;

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
        relay.encode_to_vec(),
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

#[tokio::test]
async fn silent_ws_client_reaped_by_keepalive() {
    use std::time::{Duration, Instant};

    let ka = signaling::ws::Keepalive {
        ping_interval: Duration::from_millis(100),
        silent_limit: Duration::from_millis(400),
    };
    let (addr, state) = signaling::spawn_test_server_with(ka, Vec::new()).await;

    // REST 注册拿 JWT（复用文件头既有 register 辅助：随机 32 字节多数不是合法
    // Ed25519 曲线点，服务端 VerifyingKey::from_bytes 会拒绝）
    let (jwt, dev) = register(&state, &addr, "ka@x.y", &[8u8; 32], "ka").await;

    // 连接、发 Hello、读到 HelloOk(在线确认)
    let ws = connect_ws(&addr).await;
    let (mut sink, mut stream) = futures_util::StreamExt::split(ws);
    let hello = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: dev.clone(),
            jwt: jwt.clone(),
        })),
    };
    sink.send(tokio_tungstenite::tungstenite::Message::Binary(
        hello.encode_to_vec(),
    ))
    .await
    .unwrap();
    loop {
        match stream.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b)))
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
        assert!(
            Instant::now() < deadline,
            "silent client not reaped by keepalive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn stun_port_validation() {
    assert!(signaling::parse_stun_ports("3478,3479").is_ok());
    assert!(signaling::parse_stun_ports("3478").is_ok());
    assert!(signaling::parse_stun_ports("0").is_err()); // 0 端口
    assert!(signaling::parse_stun_ports("3478,3478").is_err()); // 重复
    assert!(signaling::parse_stun_ports("").is_err());
    assert!(signaling::parse_stun_ports("1,2,3").is_err()); // 超过 2 个
}

#[tokio::test]
async fn ws_hello_after_removal_rejected() {
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[1u8; 32], "A").await;
    // 删号前 hello 正常(登录态有效)
    let mut ws = connect_ws(&addr).await;
    let f = hello(&mut ws, &jwt_a, &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));
    drop(ws);

    // 账号被运营删除:存量 JWT 重连必须被拒(Error 帧 code 1,account removed)
    assert!(signaling::admin::user_remove(&state.db.lock().unwrap(), "a@b.c").unwrap());
    let mut ws2 = connect_ws(&addr).await;
    let f = hello(&mut ws2, &jwt_a, &dev_a).await;
    // 先落盘实际帧(matches! 会按值吃掉 f.frame,消息里不能再借 f)
    let got = format!("{f:?}");
    assert!(
        matches!(f.frame, Some(ws_server_frame::Frame::Error(e)) if e.code == 1 && e.message == "account removed"),
        "expected Error(1, account removed), got {got}"
    );
}

#[tokio::test]
async fn hello_ok_carries_stun_addrs() {
    use prost::Message as _;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let (addr, state) = signaling::spawn_test_server_with(
        signaling::ws::Keepalive::default(),
        vec!["198.51.100.10:3478".into(), "198.51.100.10:3479".into()],
    )
    .await;
    let (jwt, dev) = register(&state, &addr, "stun@x.y", &[9u8; 32], "st").await;
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let (mut sink, mut stream) = ws.split();
    let hello = WsClientFrame {
        frame: Some(ws_client_frame::Frame::Hello(Hello {
            device_id: dev,
            jwt,
        })),
    };
    sink.send(WsMessage::Binary(hello.encode_to_vec()))
        .await
        .unwrap();
    // 第一帧即 HelloOk,携带 stun_addrs
    match stream.next().await {
        Some(Ok(WsMessage::Binary(b))) => {
            let f = WsServerFrame::decode(b.as_slice()).unwrap();
            match f.frame {
                Some(ws_server_frame::Frame::HelloOk(ok)) => {
                    assert_eq!(
                        ok.stun_addrs,
                        vec![
                            "198.51.100.10:3478".to_string(),
                            "198.51.100.10:3479".to_string()
                        ]
                    );
                }
                other => panic!("expected HelloOk, got {other:?}"),
            }
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn hello_ok_carries_relay_entitlement() {
    let (addr, state) = spawn_test_server().await;
    let (jwt, dev) = register(&state, &addr, "relay@x.y", &[10u8; 32], "r").await;
    // 预置后开启中继:配置保持默认限速/默认配额
    signaling::admin::relay_set_enabled(&state.db.lock().unwrap(), "relay@x.y", true).unwrap();

    let mut ws = connect_ws(&addr).await;
    let f = hello(&mut ws, &jwt, &dev).await;
    match f.frame {
        Some(ws_server_frame::Frame::HelloOk(ok)) => {
            assert!(ok.relay_enabled, "已启用账号 HelloOk.relay_enabled 须为 true");
            assert_eq!(
                ok.relay_rate_kbps,
                signaling::admin::default_rate_kbps(),
                "默认限速 1024 kbps 须随 HelloOk 下发"
            );
            assert_eq!(
                ok.relay_quota_remaining_bytes,
                signaling::admin::default_quota_bytes() as u64,
                "无用量时余量 = 默认配额全额"
            );
        }
        other => panic!("expected HelloOk, got {other:?}"),
    }
}

#[tokio::test]
async fn hello_ok_disabled_account() {
    let (addr, state) = spawn_test_server().await;
    let (jwt, dev) = register(&state, &addr, "no-relay@x.y", &[11u8; 32], "nr").await;
    // 不开启中继:user_add 预置行 enabled=0

    let mut ws = connect_ws(&addr).await;
    let f = hello(&mut ws, &jwt, &dev).await;
    match f.frame {
        Some(ws_server_frame::Frame::HelloOk(ok)) => {
            assert!(!ok.relay_enabled, "未启用账号 relay_enabled 须为 false");
            assert_eq!(
                ok.relay_rate_kbps,
                signaling::admin::default_rate_kbps(),
                "禁用时仍下发默认限速(客户端展示用途)"
            );
            assert_eq!(
                ok.relay_quota_remaining_bytes,
                signaling::admin::default_quota_bytes() as u64,
                "禁用时仍下发默认余量"
            );
        }
        other => panic!("expected HelloOk, got {other:?}"),
    }
}
