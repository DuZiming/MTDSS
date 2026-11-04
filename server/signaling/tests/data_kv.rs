//! 账号数据 KV + data_changed 变更通告(V3-1 Task 7)。
//!
//! 钉面:`GET /data/:key → 200 {value, updated_at} | 404`;
//! `PUT /data/:key {value} → {updated_at}`,value ≤256KiB(超限 4xx);
//! 写后同账号**其他**在线端收 `AuthNotice{kind:"data_changed", data_key}`,
//! 写者本端不收(尽力推,try_send)。

use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message;
use signaling::spawn_test_server;

/// 256KiB 上限(与 api::KV_VALUE_LIMIT 同值;此处独立钉死,防实现常量漂移)
const LIMIT: usize = 256 * 1024;

async fn register(
    state: &std::sync::Arc<signaling::AppState>,
    addr: &std::net::SocketAddr,
    email: &str,
    seed: &[u8; 32],
    name: &str,
) -> (String, String) {
    // 认证收口后仅预置账号可领码(与 ws.rs 同款 helper)
    signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
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

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn put_get_roundtrip_and_overwrite() {
    let (addr, state) = spawn_test_server().await;
    let (jwt, _dev) = register(&state, &addr, "a@b.c", &[11u8; 32], "A").await;
    let base = format!("http://{addr}");

    // 首写
    let r = client()
        .put(format!("{base}/data/theme"))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({ "value": "dark" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let t1 = body["updated_at"].as_i64().expect("updated_at i64");

    // 读回
    let r = client()
        .get(format!("{base}/data/theme"))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["value"], "dark");
    assert_eq!(body["updated_at"].as_i64(), Some(t1));

    // 覆写:同键新值,updated_at 换新(≥ 旧值)
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let r = client()
        .put(format!("{base}/data/theme"))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({ "value": "light" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    let t2 = body["updated_at"].as_i64().unwrap();
    assert!(t2 >= t1);

    let body: serde_json::Value = client()
        .get(format!("{base}/data/theme"))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["value"], "light");
    assert_eq!(body["updated_at"].as_i64(), Some(t2));
}

#[tokio::test]
async fn get_miss_is_404() {
    let (addr, state) = spawn_test_server().await;
    let (jwt, _dev) = register(&state, &addr, "a@b.c", &[12u8; 32], "A").await;

    let r = client()
        .get(format!("http://{addr}/data/nope"))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
    // 404 是合法态(键不存在),不是 5xx;给个可读文案
    let body = r.text().await.unwrap();
    assert!(!body.is_empty(), "404 body should carry a reason");
}

#[tokio::test]
async fn value_size_limit_boundary() {
    let (addr, state) = spawn_test_server().await;
    let (jwt, _dev) = register(&state, &addr, "a@b.c", &[13u8; 32], "A").await;
    let base = format!("http://{addr}");

    // 恰在上限:通过(ASCII 单字节,字节长=字符长)
    let at_limit = "x".repeat(LIMIT);
    let r = client()
        .put(format!("{base}/data/big"))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({ "value": at_limit }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "value exactly at limit must pass");

    // 超限一字节:4xx
    let over = "x".repeat(LIMIT + 1);
    let r = client()
        .put(format!("{base}/data/big2"))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({ "value": over }))
        .send()
        .await
        .unwrap();
    let st = r.status();
    assert!(
        st.is_client_error(),
        "over-limit value must be 4xx, got {st}"
    );
    // 被拒的键不得半写
    let r = client()
        .get(format!("{base}/data/big2"))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_requires_bearer_access_token() {
    let (addr, state) = spawn_test_server().await;
    let (jwt, _dev) = register(&state, &addr, "a@b.c", &[14u8; 32], "A").await;
    let base = format!("http://{addr}");

    // 无 bearer → 401
    let r = client()
        .put(format!("{base}/data/k"))
        .json(&serde_json::json!({ "value": "v" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
    // GET 同样 401
    let r = client().get(format!("{base}/data/k")).send().await.unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
    // 存量 token 在 hand-rolled 面之外仍应被 bearer_claims 统一裁(typ=access
    // 才放行);用坏票验证 401 一致
    let r = client()
        .put(format!("{base}/data/k"))
        .bearer_auth("garbage")
        .json(&serde_json::json!({ "value": "v" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
    let _ = jwt;
}

#[tokio::test]
async fn put_notifies_other_devices_not_writer() {
    let (addr, state) = spawn_test_server().await;
    let (jwt_a, dev_a) = register(&state, &addr, "a@b.c", &[15u8; 32], "A").await;
    let (jwt_b, dev_b) = register(&state, &addr, "a@b.c", &[16u8; 32], "B").await;

    let mut ws_a = connect_ws(&addr).await;
    let f = hello(&mut ws_a, &jwt_a, &dev_a).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));
    let mut ws_b = connect_ws(&addr).await;
    let f = hello(&mut ws_b, &jwt_b, &dev_b).await;
    assert!(matches!(f.frame, Some(ws_server_frame::Frame::HelloOk(_))));

    // B 上线的 presence 广播(A 收)先消化
    let f = recv(&mut ws_a).await;
    match f.frame {
        Some(ws_server_frame::Frame::Presence(p)) => {
            assert_eq!(p.device_id, dev_b);
            assert!(p.online);
        }
        other => panic!("expected presence, got {other:?}"),
    }

    // A 写 → B 收 data_changed 通告(带 data_key)
    let r = client()
        .put(format!("http://{addr}/data/sync"))
        .bearer_auth(&jwt_a)
        .json(&serde_json::json!({ "value": "42" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    let f = recv(&mut ws_b).await;
    match f.frame {
        Some(ws_server_frame::Frame::Notice(n)) => {
            assert_eq!(n.kind, "data_changed");
            assert_eq!(n.data_key, "sync");
        }
        other => panic!("expected data_changed notice on other device, got {other:?}"),
    }

    // 写者本端不得收到通告:PUT 返回即意味着 try_send 已执行完,写者队列
    // 应为空 —— 300ms 静默窗口判定(keepalive ping 默认 20s,不落入窗口)
    let quiet = tokio::time::timeout(std::time::Duration::from_millis(300), ws_a.next()).await;
    assert!(quiet.is_err(), "writer's own socket must NOT receive data_changed");
}
