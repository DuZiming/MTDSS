//! RelaySocket/make_relay_endpoint 端点级测试:出站 16B 标签 + 目标改写、
//! 入站直通、恒单段、MTU 上界。伪中继沿服务端数据面
//! (server/signaling relay/mod.rs)的转发语义:超短(<16B)或未登记 tag
//! 静默丢弃;按 tag 重绑源地址(不按源过滤,裁定 D);strip 16B 后转
//! 对侧槽(对侧未上线即丢);记录最大观测入包(mtu_capped 断言用)。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::endpoint::TransportTuning;
use mtdss_core::transport::punch::{self, PunchConfig};
use mtdss_core::transport::relay_sock::make_relay_endpoint;

/// 中继线路单包观测上界:RELAY_MAX_MTU 1400 + 16B 标签头。
const RELAY_WIRE_MAX: usize = 1400 + 16;

/// 起伪中继(随机回环端口):固定 tag_a/tag_b 双槽互转,返回
/// (中继地址, 最大观测入包字节数)。
async fn spawn_fake_relay(tag_a: [u8; 16], tag_b: [u8; 16]) -> (SocketAddr, Arc<AtomicUsize>) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let max_seen = Arc::new(AtomicUsize::new(0));
    let seen = max_seen.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65_535];
        // 槽位当前绑定地址:tag_a → 槽 0,tag_b → 槽 1,互为对侧
        let mut slots: [Option<SocketAddr>; 2] = [None, None];
        loop {
            // Windows 回环:向已关端口转发后 recv 可能见 WSAECONNRESET
            // (ICMP 端口不可达),沿服务端数据面容错语义吞掉续跑
            let Ok((n, src)) = socket.recv_from(&mut buf).await else { continue };
            seen.fetch_max(n, Ordering::Relaxed);
            if n < 16 {
                continue; // 超短:无完整 16B 头
            }
            let tag: [u8; 16] = buf[..16].try_into().unwrap();
            let slot = if tag == tag_a { 0 } else if tag == tag_b { 1 } else { continue };
            slots[slot] = Some(src); // 按 tag 重绑(裁定 D),不按源地址过滤
            if let Some(peer) = slots[1 - slot] {
                // strip 16B 头转对侧槽;对侧未上线即丢
                let _ = socket.send_to(&buf[16..n], peer).await;
            }
        }
    });
    (addr, max_seen)
}

/// 中继路径调优:max_mtu=1400(+16B 标签 ≤ 1416,不跨典型路径 MTU)。
fn relay_tuning() -> TransportTuning {
    TransportTuning {
        max_mtu: 1400,
        ..Default::default()
    }
}

/// 一对身份各建中继端点(白名单互指,房风沿 tests/punch.rs 的身份对)。
fn relay_endpoints(
    a: &DeviceIdentity,
    b: &DeviceIdentity,
    tag_a: [u8; 16],
    tag_b: [u8; 16],
    relay_addr: SocketAddr,
) -> (quinn::Endpoint, quinn::Endpoint) {
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a.public_key())].into_iter().collect();
    let ep_a = make_relay_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        a,
        &allowed_a,
        relay_tuning(),
        tag_a,
        relay_addr,
    )
    .unwrap();
    let ep_b = make_relay_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        b,
        &allowed_b,
        relay_tuning(),
        tag_b,
        relay_addr,
    )
    .unwrap();
    (ep_a, ep_b)
}

/// 双方并发互拨(两侧都先 spawn 再 await:接受令牌只来自 accept_pinned)。
/// 预算 3s:中继首包竞态下,先拨通一侧 ~12ms 即交付(held/parked 路径,
/// punch 模块注释钉死的相配场景);被丢首 Initial 的一侧在途拨号收不到
/// 令牌,必跑满预算退回 parked——预算即本测试的墙钟,3s 对 12ms 级
/// 完成路径有百倍余量。tuning 沿生产 Default(1452,生产从不给中继路径
/// 预设小 MTU):中继线路的 MTU 上界不得靠测试自设(修复轮揭盖),由
/// 中继端点通告钳制——mtu_capped 据此断言线上单包 ≤ 1416。
async fn punch_over_relay(
    ep_a: &quinn::Endpoint,
    ep_b: &quinn::Endpoint,
    a: &DeviceIdentity,
    b: &DeviceIdentity,
    relay_addr: SocketAddr,
) -> (quinn::Connection, quinn::Connection) {
    let cfg = PunchConfig {
        budget: Duration::from_secs(3),
        ..Default::default()
    };
    let ja = {
        let ep = ep_a.clone();
        let me = DeviceIdentity::from_seed(&a.signing.to_bytes());
        let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
            [(b.device_id.clone(), b.public_key())].into_iter().collect();
        let peer = b.public_key();
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed, &peer, &[relay_addr], &cfg).await
        })
    };
    let jb = {
        let ep = ep_b.clone();
        let me = DeviceIdentity::from_seed(&b.signing.to_bytes());
        let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
            [(a.device_id.clone(), a.public_key())].into_iter().collect();
        let peer = a.public_key();
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed, &peer, &[relay_addr], &cfg).await
        })
    };
    (ja.await.unwrap().unwrap(), jb.await.unwrap().unwrap())
}

/// 双向流回显:发起端 open_bi 写 payload 并半关,响应端 accept_bi 读全量
/// 后原样回写,发起端读回校验(回显的是"收到的字节",断言更强)。
async fn echo_round(init: &quinn::Connection, resp: &quinn::Connection, payload: &[u8]) {
    let (mut tx, mut rx) = init.open_bi().await.unwrap();
    tx.write_all(payload).await.unwrap();
    tx.finish().unwrap();
    let (mut rtx, mut rrx) = resp.accept_bi().await.unwrap();
    let got = rrx.read_to_end(usize::MAX).await.unwrap();
    assert_eq!(got, payload);
    rtx.write_all(&got).await.unwrap();
    rtx.finish().unwrap();
    let echoed = rx.read_to_end(usize::MAX).await.unwrap();
    assert_eq!(echoed, payload);
}

/// 发一个带 16B 标签的裸数据报(RelaySocket 出站的线上形状)。
async fn send_tagged(
    s: &tokio::net::UdpSocket,
    tag: &[u8; 16],
    relay_addr: SocketAddr,
    payload: &[u8],
) {
    let mut d = Vec::with_capacity(16 + payload.len());
    d.extend_from_slice(tag);
    d.extend_from_slice(payload);
    s.send_to(&d, relay_addr).await.unwrap();
}

/// 收一个数据报(3s 超时,沿服务端 relay 测试房风)。
async fn recv_raw(s: &tokio::net::UdpSocket) -> Vec<u8> {
    let mut buf = [0u8; 65_535];
    let n = tokio::time::timeout(Duration::from_secs(3), s.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    buf[..n].to_vec()
}

#[tokio::test]
async fn punch_over_relay_paired_connection() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let tag_a = [0x11u8; 16];
    let tag_b = [0x22u8; 16];
    let (relay_addr, _max) = spawn_fake_relay(tag_a, tag_b).await;
    let (ep_a, ep_b) = relay_endpoints(&a, &b, tag_a, tag_b, relay_addr);
    // 双方并发互拨(以 [relay_addr] 为唯一候选)→ 必为同一连接的两端
    let (conn_a, conn_b) = punch_over_relay(&ep_a, &ep_b, &a, &b, relay_addr).await;

    // 连接互通:A→B 与 B→A 各一条双向流,回显字节一致
    echo_round(&conn_a, &conn_b, &[0xA5u8; 32 * 1024]).await;
    echo_round(&conn_b, &conn_a, &[0x5Au8; 32 * 1024]).await;
}

#[tokio::test]
async fn tag_routes_by_tag_not_source() {
    let tag_a = [0x55u8; 16];
    let tag_b = [0x66u8; 16];
    let (relay_addr, _max) = spawn_fake_relay(tag_a, tag_b).await;

    // 自然槽位就绪(沿服务端测试的握手舞步):s1 首包只绑槽 0 被丢,
    // 对端以 tag_b 回包才双槽就绪——收到即证两槽都已绑定
    let s1 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    send_tagged(&s1, &tag_a, relay_addr, b"hs-a").await;
    send_tagged(&peer, &tag_b, relay_addr, b"hs-b").await;
    assert_eq!(recv_raw(&s1).await, b"hs-b");

    // 同一 tag(tag_a)从两个不同源端口发裸数据报 → 对端均收到:
    // 路由只认 tag,不认源端口
    send_tagged(&s1, &tag_a, relay_addr, b"from-port-1").await;
    assert_eq!(recv_raw(&peer).await, b"from-port-1");

    // 全新源端口 s2:按 tag 重绑(裁定 D:换源地址无歧义)后照转
    let s2 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    send_tagged(&s2, &tag_a, relay_addr, b"from-port-2").await;
    assert_eq!(recv_raw(&peer).await, b"from-port-2");

    // 重绑语义的回程证明:对端回包须追到 s2(最新绑定)
    send_tagged(&peer, &tag_b, relay_addr, b"back-to-new").await;
    assert_eq!(recv_raw(&s2).await, b"back-to-new");
}

#[tokio::test]
async fn malformed_datagrams_dropped() {
    let tag_a = [0x77u8; 16];
    let tag_b = [0x88u8; 16];
    let (relay_addr, _max) = spawn_fake_relay(tag_a, tag_b).await;
    let a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    // 好流先就绪(双槽绑定,同 tag_routes 的握手舞步)
    send_tagged(&a, &tag_a, relay_addr, b"hs-a").await;
    send_tagged(&b, &tag_b, relay_addr, b"hs-b").await;
    assert_eq!(recv_raw(&a).await, b"hs-b");

    // 超短报文(<16B 无完整头)
    a.send_to(b"short", relay_addr).await.unwrap();
    // 随机 16B 伪标签(必异于两个真标签)
    let forged = [0xABu8; 24];
    assert_ne!(&forged[..16], &tag_a[..]);
    assert_ne!(&forged[..16], &tag_b[..]);
    a.send_to(&forged, relay_addr).await.unwrap();

    // 畸形输入静默丢弃:不 panic、不扰后续好流,双向照通
    send_tagged(&a, &tag_a, relay_addr, b"still-alive").await;
    assert_eq!(recv_raw(&b).await, b"still-alive");
    send_tagged(&b, &tag_b, relay_addr, b"me-too").await;
    assert_eq!(recv_raw(&a).await, b"me-too");
}

#[tokio::test]
async fn mtu_capped() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let tag_a = [0x33u8; 16];
    let tag_b = [0x44u8; 16];
    let (relay_addr, max_seen) = spawn_fake_relay(tag_a, tag_b).await;
    let (ep_a, ep_b) = relay_endpoints(&a, &b, tag_a, tag_b, relay_addr);
    let (conn_a, conn_b) = punch_over_relay(&ep_a, &ep_b, &a, &b, relay_addr).await;

    // 打洞后灌大流量(双向各 256 KiB):MTU 探测展开后的稳态大包也在路上
    echo_round(&conn_a, &conn_b, &[0xC3u8; 256 * 1024]).await;
    echo_round(&conn_b, &conn_a, &[0x3Cu8; 256 * 1024]).await;

    // punch 配置为 Default 接线(生产形态:dial 侧 tuning 1452,端点侧
    // 1400 只管 accept)——伪中继最大观测入包(含 16B 标签)仍须
    // ≤ 1400 + 16:中继线路上的 QUIC 数据报永不越 RELAY_MAX_MTU,
    // 防跨路径 MTU 分片/丢弃(修复轮揭盖:此前测试自设 1400 tuning 掩盖
    // dial 半边)
    let seen = max_seen.load(Ordering::Relaxed);
    assert!(seen <= RELAY_WIRE_MAX, "max datagram seen by relay: {seen}");
}
