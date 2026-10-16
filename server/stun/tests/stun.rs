//! STUN 服务端:编解码向量 + 双端口真实查询 + 限速。

use std::net::SocketAddr;
use std::time::Duration;

const MAGIC: u32 = 0x2112_A442;

/// 测试内构造 Binding Request(与实现解耦):type=0x0001, len=0, magic, txn。
fn request(txn: [u8; 12]) -> Vec<u8> {
    let mut p = Vec::with_capacity(20);
    p.extend_from_slice(&0x0001u16.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p.extend_from_slice(&MAGIC.to_be_bytes());
    p.extend_from_slice(&txn);
    p
}

#[test]
fn parse_request_accepts_valid_rejects_garbage() {
    let txn: [u8; 12] = core::array::from_fn(|i| i as u8);
    assert_eq!(stun::parse_request(&request(txn)), Some(txn));
    // 带无关属性的请求(属性区被忽略)仍接受
    let mut with_attr = request(txn);
    with_attr.extend_from_slice(&0x8028u16.to_be_bytes()); // FINGERPRINT 类型占位
    with_attr.extend_from_slice(&0x0004u16.to_be_bytes());
    with_attr.extend_from_slice(b"abcd");
    let n = with_attr.len();
    with_attr[2..4].copy_from_slice(&((n - 20) as u16).to_be_bytes());
    assert_eq!(stun::parse_request(&with_attr), Some(txn));
    // 负例:截断 / 错类型 / 错 magic / 声明长度超包
    assert_eq!(stun::parse_request(&request(txn)[..19]), None);
    let mut wrong_type = request(txn); wrong_type[0..2].copy_from_slice(&0x0011u16.to_be_bytes());
    assert_eq!(stun::parse_request(&wrong_type), None);
    let mut wrong_magic = request(txn); wrong_magic[4] ^= 0xFF;
    assert_eq!(stun::parse_request(&wrong_magic), None);
    let mut over = request(txn); over[2..4].copy_from_slice(&0xFFFFu16.to_be_bytes());
    assert_eq!(stun::parse_request(&over), None);
}

#[test]
fn response_vector_ipv4() {
    // 手工向量:映射地址 192.0.2.1:32853
    // xport = 0x8055 ^ 0x2112 = 0xA147;xaddr = C0000201 ^ 2112A442 = E112A643
    let txn: [u8; 12] = core::array::from_fn(|i| 0xA0 + i as u8);
    let got = stun::build_response(&txn, "192.0.2.1:32853".parse().unwrap());
    let mut want = Vec::new();
    want.extend_from_slice(&[0x01, 0x01, 0x00, 0x0C]); // type, msg len = attr头4 + 值8 (RFC 5389 §15.2)
    want.extend_from_slice(&MAGIC.to_be_bytes());
    want.extend_from_slice(&txn);
    want.extend_from_slice(&[0x00, 0x20, 0x00, 0x08]); // XOR-MAPPED-ADDRESS, 值长 8
    want.extend_from_slice(&[0x00, 0x01, 0xA1, 0x47, 0xE1, 0x12, 0xA6, 0x43]);
    assert_eq!(got, want);
}

#[test]
fn response_vector_ipv6() {
    // 手工向量:映射地址 [2001:db8::1]:32853
    // xport = 0x8055 ^ 0x2112 = 0xA147
    // mask  = magic_be || txn = 21 12 A4 42 | A0 A1 A2 A3 A4 A5 A6 A7 A8 A9 AA AB
    // xaddr = 2001:0db8::1 ^ mask
    //       = 01 13 A9 FA A0 A1 A2 A3 A4 A5 A6 A7 A8 A9 AA AA
    let txn: [u8; 12] = core::array::from_fn(|i| 0xA0 + i as u8);
    let got = stun::build_response(&txn, "[2001:db8::1]:32853".parse().unwrap());
    let mut want = Vec::new();
    want.extend_from_slice(&[0x01, 0x01, 0x00, 0x18]); // type, msg len = attr头4 + 值20
    want.extend_from_slice(&MAGIC.to_be_bytes());
    want.extend_from_slice(&txn);
    want.extend_from_slice(&[0x00, 0x20, 0x00, 0x14]); // XOR-MAPPED-ADDRESS, 值长 20
    want.extend_from_slice(&[0x00, 0x02, 0xA1, 0x47]); // 保留0 + family v6 + xport
    want.extend_from_slice(&[
        0x01, 0x13, 0xA9, 0xFA, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAA,
    ]);
    assert_eq!(got, want);
}

async fn spawn_two() -> (SocketAddr, SocketAddr) {
    let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr_a = a.local_addr().unwrap();
    let addr_b = b.local_addr().unwrap();
    tokio::spawn(stun::run_listener(a, 20, 20));
    tokio::spawn(stun::run_listener(b, 20, 20));
    (addr_a, addr_b)
}

#[tokio::test(flavor = "multi_thread")]
async fn two_loopback_listeners_answer_binding() {
    let (pa, pb) = spawn_two().await;
    for server in [pa, pb] {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let txn: [u8; 12] = rand::random();
        sock.send_to(&request(txn), server).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = [0u8; 128];
        let (n, from) = sock.recv_from(&mut buf).unwrap();
        assert_eq!(from, server);
        assert_eq!(&buf[8..20], &txn); // 事务 ID 回显
        let port = sock.local_addr().unwrap().port();
        // XOR-MAPPED = 127.0.0.1:<本机端口>
        assert_eq!(&buf[26..28], &(port as u16 ^ 0x2112).to_be_bytes());
        assert_eq!(&buf[28..32], &(0x7F000001u32 ^ MAGIC).to_be_bytes());
        let _ = n;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_drops_beyond_capacity() {
    let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = a.local_addr().unwrap();
    tokio::spawn(stun::run_listener(a, 2, 2)); // 容量 2;速率 2/s,确保 200ms 探测窗口不足以回填 1 个令牌
    tokio::time::sleep(Duration::from_millis(50)).await;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let mut got = 0;
    for i in 0..4 {
        let txn: [u8; 12] = rand::random();
        let _ = i;
        sock.send_to(&request(txn), addr).unwrap();
        let mut buf = [0u8; 128];
        if sock.recv_from(&mut buf).is_ok() {
            got += 1;
        }
    }
    assert_eq!(got, 2, "capacity=2 must serve exactly 2 of 4 rapid requests");
}
