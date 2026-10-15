use std::collections::HashMap;
use std::net::SocketAddr;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::endpoint::*;

fn allowed_map(id: &DeviceIdentity) -> HashMap<String, ed25519_dalek::VerifyingKey> {
    let mut m = HashMap::new();
    m.insert(id.device_id.clone(), id.public_key());
    m
}

fn localhost0() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

#[tokio::test]
async fn pinned_connect_and_identify_peer() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();

    let ep_b = make_server_endpoint(localhost0(), &b, &allowed_map(&a)).unwrap();
    let addr_b = ep_b.local_addr().unwrap();

    let ep_a = make_endpoint(localhost0()).unwrap();

    let allowed = allowed_map(&a);
    let server_task = tokio::spawn(async move {
        let (conn, device_id) = accept_pinned(&ep_b, &allowed).await.unwrap();
        (conn, device_id)
    });

    let conn = connect_pinned(&ep_a, addr_b, &b.public_key(), &a)
        .await
        .unwrap();
    let (sconn, device_id) = server_task.await.unwrap();
    // 服务端从客户端证书反查出 A 的 device_id
    assert_eq!(device_id, a.device_id);

    // 通道双向可用：服务端 open_bi 写，客户端 accept_bi 读
    let (mut s_tx, mut s_rx) = sconn.open_bi().await.unwrap();
    s_tx.write_all(b"ping").await.unwrap();
    s_tx.finish().unwrap();
    let (_c_tx, mut c_rx) = conn.accept_bi().await.unwrap();
    let mut buf = [0u8; 4];
    c_rx.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    let _ = s_rx.stop(quinn::VarInt::from_u32(0));
}

#[tokio::test]
async fn wrong_expected_peer_key_fails_handshake() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mallory = DeviceIdentity::generate();

    let allowed = allowed_map(&a);
    let ep_b = make_server_endpoint(localhost0(), &b, &allowed).unwrap();
    let addr_b = ep_b.local_addr().unwrap();
    let ep_a = make_endpoint(localhost0()).unwrap();

    // quinn 服务端只有在应用层 accept 时才会驱动握手；spawn 一个 acceptor
    // 让 B 真正发出证书，使客户端的 pin 校验路径真正走到。
    let server_task = tokio::spawn(async move { accept_pinned(&ep_b, &allowed).await });

    // A 期待的是 mallory 的公钥，实际对端是 B → 握手必须失败
    let r = connect_pinned(&ep_a, addr_b, &mallory.public_key(), &a).await;
    assert!(r.is_err());
    // 客户端中断握手后，服务端侧同样失败
    assert!(server_task.await.unwrap().is_err());
}

#[tokio::test]
async fn server_rejects_unlisted_client() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();

    // 空 allowed：A 不在列表中
    let allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    let ep_b = make_server_endpoint(localhost0(), &b, &allowed).unwrap();
    let addr_b = ep_b.local_addr().unwrap();
    let ep_a = make_endpoint(localhost0()).unwrap();

    let server_task = tokio::spawn(async move { accept_pinned(&ep_b, &allowed).await });
    let r = connect_pinned(&ep_a, addr_b, &b.public_key(), &a).await;
    assert!(r.is_err());
    assert!(server_task.await.unwrap().is_err());
}
