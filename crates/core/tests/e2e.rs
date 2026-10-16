//! M1 验收：两台"设备"（两个身份）经信令服务器完成发现、E2E 信令、
//! QUIC 直连、5MB 文件分块校验传输。全链路真实 wire path（无 mock）。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::client::{ClientEvent, SignalingClient};
use mtdss_core::signaling::envelope::{open, seal, SignalBody};
use mtdss_core::transport::endpoint as ep;
use mtdss_core::transport::transfer;
use signaling::spawn_test_server;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// 固定种子：DeviceIdentity 含 OnceLock 不可 Clone，同一身份需要出现在
/// 两个作用域时用 from_seed 重建（测试内即真实做法，不经特殊通道）。
const SEED_A: [u8; 32] = [0xA5; 32];
const SEED_B: [u8; 32] = [0x5B; 32];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// 注册一台设备：dev-mode 下验证码由服务器响应直接带回。
async fn register(
    addr: &std::net::SocketAddr,
    email: &str,
    id: &DeviceIdentity,
    name: &str,
) -> (String, String) {
    let client = reqwest::Client::new();
    let c: serde_json::Value = client
        .post(format!("http://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .post(format!("http://{addr}/auth/verify"))
        .json(&serde_json::json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": hex::encode(id.public_key().as_bytes()),
            "name": name,
            "platform": "test"
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

/// 带超时取下一个信令事件：测试永不悬挂，失败即 panic 并给出原因。
async fn next_event(rx: &mut mpsc::Receiver<ClientEvent>) -> ClientEvent {
    tokio::time::timeout(EVENT_TIMEOUT, rx.recv())
        .await
        .expect("timed out waiting for signaling event")
        .expect("signaling event stream closed")
}

#[tokio::test]
async fn m1_full_vertical() {
    let (addr, _state) = spawn_test_server().await;
    let a = DeviceIdentity::from_seed(&SEED_A);
    let b = DeviceIdentity::from_seed(&SEED_B);
    let (jwt_a, dev_a) = register(&addr, "u@x.y", &a, "A").await;
    let (jwt_b, dev_b) = register(&addr, "u@x.y", &b, "B").await;

    // 双方在线：A 侧应看到 B 的上线 presence（先跳过自己的 HelloOk 握手确认）
    let (ca, mut ev_a) =
        SignalingClient::connect(&format!("ws://{addr}/ws"), &jwt_a, &dev_a).await.unwrap();
    let (cb, mut ev_b) =
        SignalingClient::connect(&format!("ws://{addr}/ws"), &jwt_b, &dev_b).await.unwrap();
    loop {
        match next_event(&mut ev_a).await {
            ClientEvent::Presence { device_id, online } => {
                assert_eq!(device_id, dev_b);
                assert!(online);
                break;
            }
            ClientEvent::HelloOk { .. } => continue,
            o => panic!("expected presence, got {o:?}"),
        }
    }

    // A 发起：E2E ConnectRequest（服务器只见不透明字节）
    let session_id = vec![0xAA; 16];
    ca.relay(
        &dev_b,
        &seal(&a, &b.public_key(), &SignalBody::ConnectRequest { session_id: session_id.clone() }),
    )
    .await
    .unwrap();
    let body = loop {
        match next_event(&mut ev_b).await {
            ClientEvent::Relayed { from, payload } if from == dev_a => {
                break open(&payload, &b, &a.public_key()).unwrap();
            }
            _ => continue,
        }
    };
    assert!(matches!(&body, SignalBody::ConnectRequest { session_id: s } if *s == session_id));

    // B 接受并起 QUIC 端点，发回 Accept + Candidates（真实监听地址）
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(dev_a.clone(), a.public_key());
    let ep_b = ep::make_server_endpoint("127.0.0.1:0".parse().unwrap(), &b, &allowed).unwrap();
    let port = ep_b.local_addr().unwrap().port();
    cb.relay(
        &dev_a,
        &seal(&b, &a.public_key(), &SignalBody::ConnectAccept { session_id: session_id.clone() }),
    )
    .await
    .unwrap();
    cb.relay(
        &dev_a,
        &seal(
            &b,
            &a.public_key(),
            &SignalBody::Candidates {
                session_id: session_id.clone(),
                addrs: vec![format!("127.0.0.1:{port}")],
            },
        ),
    )
    .await
    .unwrap();

    // A 等 Accept + Candidates 后直连
    let accept = loop {
        match next_event(&mut ev_a).await {
            ClientEvent::Relayed { from, payload } if from == dev_b => {
                break open(&payload, &a, &b.public_key()).unwrap();
            }
            _ => continue,
        }
    };
    assert!(matches!(&accept, SignalBody::ConnectAccept { session_id: s } if *s == session_id));
    let cands = loop {
        match next_event(&mut ev_a).await {
            ClientEvent::Relayed { from, payload } if from == dev_b => {
                match open(&payload, &a, &b.public_key()).unwrap() {
                    c @ SignalBody::Candidates { .. } => break c,
                    _ => continue,
                }
            }
            _ => continue,
        }
    };
    let addrs = match cands {
        SignalBody::Candidates { addrs, .. } => addrs,
        _ => unreachable!(),
    };

    // B 侧任务：接受固定连接（校验对端 device_id == A）后收文件
    let b_recv = DeviceIdentity::from_seed(&SEED_B);
    let dst = tempfile::tempdir().unwrap();
    let dst_path = dst.path().to_path_buf();
    let b_side = tokio::spawn(async move {
        let (conn, dev) = ep::accept_pinned(&ep_b, &allowed).await.unwrap();
        assert_eq!(dev, dev_a);
        transfer::recv_files(&conn, &dst_path, &b_recv).await.unwrap()
    });

    // A 侧：逐候选直连（5s 超时防悬挂）
    let ep_a = ep::make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let mut conn = None;
    for cand in &addrs {
        if let Ok(sa) = cand.parse::<std::net::SocketAddr>() {
            if let Ok(c) = tokio::time::timeout(
                CONNECT_TIMEOUT,
                ep::connect_pinned(&ep_a, sa, &b.public_key(), &a),
            )
            .await
            .unwrap()
            {
                conn = Some(c);
                break;
            }
        }
    }
    let conn = conn.expect("no candidate reachable");

    // 5MB 随机文件，CJK + emoji 文件名
    let src = tempfile::tempdir().unwrap();
    let name = "演示 file 中文 😄.bin";
    let f = src.path().join(name);
    let mut data = vec![0u8; 5 * 1024 * 1024];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(&f, &data).unwrap();

    // 发送并校验进度回调与发送字节数
    let total = data.len() as u64;
    let items = vec![(f.clone(), name.to_string())];
    let man = mtdss_core::protocol::manifest::build_manifest(&items, &a).unwrap();
    let progress_seen = AtomicU64::new(0);
    let report = transfer::send_files(&conn, &man, &items, &|p| {
        if let transfer::SendProgress::Bytes { done, total: t, .. } = p {
            assert_eq!(*t, total);
            assert!(*done <= total);
            progress_seen.store(*done, Ordering::SeqCst);
        }
    }, &transfer::SendOptions::default())
    .await
    .unwrap();
    assert_eq!(report.bytes_sent, 5 * 1024 * 1024);
    assert_eq!(progress_seen.load(Ordering::SeqCst), 5 * 1024 * 1024);

    // 接收方落盘字节级一致
    let paths = b_side.await.unwrap();
    assert_eq!(std::fs::read(&paths[0]).unwrap(), data);
}
