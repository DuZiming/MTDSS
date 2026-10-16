//! STUN 客户端:编解码负例 + 真实回环服务 + 对称预判(假应答器)。

use mtdss_core::net::stun::{binding_query, gather_candidates, parse_xor_mapped, build_request};
use std::net::SocketAddr;
use std::time::Duration;

const MAGIC: u32 = 0x2112_A442;
const TIMEOUT: Duration = Duration::from_secs(2);

fn valid_response(txn: [u8; 12], mapped: SocketAddr) -> Vec<u8> {
    let port_xor = (mapped.port() as u16) ^ 0x2112;
    let addr_xor: Vec<u8> = match mapped.ip() {
        std::net::IpAddr::V4(v4) => v4
            .octets()
            .iter()
            .zip(MAGIC.to_be_bytes())
            .map(|(x, m)| x ^ m)
            .collect(),
        std::net::IpAddr::V6(_) => unreachable!("测试仅 v4"),
    };
    let mut out = vec![0x01, 0x01, 0x00, 0x0C];
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(&txn);
    out.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
    out.extend_from_slice(&port_xor.to_be_bytes());
    out.extend_from_slice(&addr_xor);
    out
}

#[test]
fn parse_accepts_and_rejects() {
    let txn: [u8; 12] = core::array::from_fn(|i| 0x10 + i as u8);
    let mapped: SocketAddr = "203.0.113.7:50000".parse().unwrap();
    assert_eq!(parse_xor_mapped(&valid_response(txn, mapped), &txn).unwrap(), mapped);
    // 错事务 ID
    let other = core::array::from_fn(|i| 0x20 + i as u8);
    assert!(parse_xor_mapped(&valid_response(txn, mapped), &other).is_err());
    // 错类型 / 截断 / 声明超包
    let mut bad_type = valid_response(txn, mapped);
    bad_type[0..2].copy_from_slice(&0x0111u16.to_be_bytes());
    assert!(parse_xor_mapped(&bad_type, &txn).is_err());
    assert!(parse_xor_mapped(&valid_response(txn, mapped)[..15], &txn).is_err());
    let mut over = valid_response(txn, mapped);
    over[2..4].copy_from_slice(&0xFFFFu16.to_be_bytes());
    assert!(parse_xor_mapped(&over, &txn).is_err());
    // 无 XOR 属性 → Err
    let mut no_attr = vec![0x01, 0x01, 0x00, 0x00];
    no_attr.extend_from_slice(&MAGIC.to_be_bytes());
    no_attr.extend_from_slice(&txn);
    assert!(parse_xor_mapped(&no_attr, &txn).is_err());
}

#[test]
fn request_is_20_bytes_binding() {
    let txn: [u8; 12] = core::array::from_fn(|i| i as u8);
    let p = build_request(&txn);
    assert_eq!(p.len(), 20);
    assert_eq!(&p[0..2], &0x0001u16.to_be_bytes());
    assert_eq!(&p[2..4], &0u16.to_be_bytes());
    assert_eq!(&p[4..8], &MAGIC.to_be_bytes());
    assert_eq!(&p[8..20], &txn);
}

async fn spawn_real() -> (SocketAddr, SocketAddr) {
    let a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (pa, pb) = (a.local_addr().unwrap(), b.local_addr().unwrap());
    tokio::spawn(stun_server_lib_wrap(a));
    tokio::spawn(stun_server_lib_wrap(b));
    (pa, pb)
}

// 薄别名:dev-dep crate 名为 stun(lib 名)
async fn stun_server_lib_wrap(sock: std::net::UdpSocket) {
    let _ = stun::run_listener(sock, 100, 100).await;
}

// multi_thread:binding_query 用 std 阻塞 recv,会占死当前线程;
// current_thread 运行时下 spawn 的监听任务将永远得不到轮询。
#[tokio::test(flavor = "multi_thread")]
async fn binding_query_against_real_server() {
    let (pa, _pb) = spawn_real().await;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let got = binding_query(&sock, pa, TIMEOUT).unwrap();
    assert_eq!(got, sock.local_addr().unwrap());
}

/// 假应答器:对任何请求按端口回**不同**映射地址 → 模拟对称 NAT。
/// 接收**已绑定** socket 的所有权(线程内不再 rebind):bind-then-drop-then-rebind
/// 在 Linux 上是 TOCTOU——内核按顺序分配临时端口,并发测试可能立刻占走被
/// drop 的端口,导致 EADDRINUSE、应答器线程静默 panic。返回两个应答器地址。
fn spawn_fake(sa: std::net::UdpSocket, sb: std::net::UdpSocket) -> (SocketAddr, SocketAddr) {
    let handle = |sock: std::net::UdpSocket, fake_port: u16| {
        let srv = sock.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 128];
            loop {
                let (n, src) = match sock.recv_from(&mut buf) {
                    Ok(v) => v,
                    Err(_) => return,
                };
                if n < 20 {
                    continue;
                }
                let mut txn = [0u8; 12];
                txn.copy_from_slice(&buf[8..20]);
                let resp = valid_response(txn, format!("198.51.100.9:{fake_port}").parse().unwrap());
                let _ = sock.send_to(&resp, src);
            }
        });
        srv
    };
    (handle(sa, 40000), handle(sb, 40001))
}

#[tokio::test(flavor = "multi_thread")]
async fn gather_detects_symmetric_and_nonsymmetric() {
    // 真实服务:同源同映射 → 非对称
    let (pa, pb) = spawn_real().await;
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let g = gather_candidates(&sock, pa, pb).unwrap();
    assert!(g.stun_ok);
    assert!(!g.nat_symmetric);
    // srflx = 127.0.0.1:port,应与本地候选去重(本地已含该回环)
    assert!(g.addrs.contains(&sock.local_addr().unwrap()));

    // 假服务:两端口映射不同 → 对称(已绑定 socket 所有权移交应答器,无 rebind 竞争)
    let sa = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let sb = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let sock2 = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (fa, fb) = spawn_fake(sa, sb);
    let g2 = gather_candidates(&sock2, fa, fb).unwrap();
    assert!(g2.stun_ok);
    assert!(g2.nat_symmetric);
    // 对称时 srflx(198.51.100.9:40000)仍进候选列表(多一条路)
    assert!(g2.addrs.iter().any(|a| a.port() == 40000));
}

#[test]
fn local_addrs_ordering_and_cap() {
    use mtdss_core::net::local_ipv4_addrs;
    let c = local_ipv4_addrs(9999);
    assert_eq!(c[0], "127.0.0.1:9999".parse::<SocketAddr>().unwrap());
    assert!(c.len() <= 8);
    let uniq: std::collections::HashSet<_> = c.iter().collect();
    assert_eq!(uniq.len(), c.len());
}

#[tokio::test]
async fn unreachable_stun_degrades_to_local_only() {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead = "127.0.0.1:1".parse().unwrap(); // 无监听
    let g = gather_candidates(&sock, dead, dead).unwrap();
    assert!(!g.stun_ok);
    assert!(!g.nat_symmetric);
    assert!(g.addrs.len() >= 1);
}
