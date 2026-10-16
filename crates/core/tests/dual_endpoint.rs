//! 双角色端点:单 socket 同时 dial/accept;两端互拨各赢一条连接。
//!
//! 与 transfer.rs 的 connected_pair 一致:accept 必须与 connect 并发轮询
//! ——connect_pinned 要等服务端 accept_pinned 在确认流上写令牌,顺序
//! await 会互相等待而死锁。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::endpoint as ep;
use std::collections::HashMap;

fn pair() -> (DeviceIdentity, DeviceIdentity, HashMap<String, ed25519_dalek::VerifyingKey>) {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(b.device_id.clone(), b.public_key());
    (a, b, allowed)
}

fn bind0() -> (std::net::UdpSocket, std::net::SocketAddr) {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = s.local_addr().unwrap();
    (s, a)
}

#[tokio::test]
async fn dual_both_directions() {
    let (a, b, _) = pair();
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b.public_key())].into_iter().collect();

    let (sa, addr_a) = bind0();
    let (sb, addr_b) = bind0();
    let ep_a = ep::make_dual_endpoint(sa, &a, &allowed_b).unwrap(); // A 的服务端白名单: B
    let ep_b = ep::make_dual_endpoint(sb, &b, &allowed_a).unwrap();

    // 方向 1:A 拨 B,B 收(accept 任务先行,与 connect 并发)
    let acc1 = {
        let ep_b = ep_b.clone();
        let allowed_a = allowed_a.clone();
        tokio::spawn(async move { ep::accept_pinned(&ep_b, &allowed_a).await.unwrap() })
    };
    let c1 = ep::connect_pinned(&ep_a, addr_b, &b.public_key(), &a).await.unwrap();
    let (c1s, dev) = acc1.await.unwrap();
    assert_eq!(dev, a.device_id);
    drop(c1); drop(c1s);

    // 方向 2:B 拨 A,A 收(同一对端点、同一 socket,反向亦通)
    let acc2 = {
        let ep_a = ep_a.clone();
        let allowed_b = allowed_b.clone();
        tokio::spawn(async move { ep::accept_pinned(&ep_a, &allowed_b).await.unwrap() })
    };
    let c2 = ep::connect_pinned(&ep_b, addr_a, &a.public_key(), &b).await.unwrap();
    let (c2s, dev2) = acc2.await.unwrap();
    assert_eq!(dev2, b.device_id);
    drop(c2); drop(c2s);
}
