//! 极简 RFC 5389 STUN Binding 服务端(双端口部署,仅 XOR-MAPPED-ADDRESS)。
//! 与 mtdss-core 的客户端编解码**有意各自独立**(两侧方向不同、部署解耦),
//! 由共用测试向量(same bytes)钉死互通。

use anyhow::Result;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_SUCCESS: u16 = 0x0101;
pub const MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_XOR_MAPPED: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// 解析 Binding Request:返回 12 字节事务 ID。
/// 类型非 0x0001、magic 不符、长度不足或声明长度超包 → None。
pub fn parse_request(pkt: &[u8]) -> Option<[u8; 12]> {
    if pkt.len() < HEADER_LEN {
        return None;
    }
    if u16::from_be_bytes([pkt[0], pkt[1]]) != BINDING_REQUEST {
        return None;
    }
    if u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]) != MAGIC_COOKIE {
        return None;
    }
    let declared = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    if pkt.len() < HEADER_LEN + declared {
        return None;
    }
    let mut txn = [0u8; 12];
    txn.copy_from_slice(&pkt[8..20]);
    Some(txn)
}

/// 构造 Binding Success 响应:回显事务 ID,仅含 XOR-MAPPED-ADDRESS。
/// v4: xport = port ^ (magic>>16), xaddr = addr ^ magic;
/// v6: xaddr = addr ^ (magic || txn)。
pub fn build_response(txn: &[u8; 12], mapped: SocketAddr) -> Vec<u8> {
    let port_xor = mapped.port() ^ (MAGIC_COOKIE >> 16) as u16;
    let (family, xaddr): (u8, Vec<u8>) = match mapped.ip() {
        IpAddr::V4(v4) => (
            0x01,
            v4.octets()
                .iter()
                .zip(MAGIC_COOKIE.to_be_bytes())
                .map(|(x, m)| x ^ m)
                .collect(),
        ),
        IpAddr::V6(v6) => {
            let mut mask = [0u8; 16];
            mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            mask[4..].copy_from_slice(txn);
            (0x02, v6.octets().iter().zip(mask).map(|(x, m)| x ^ m).collect())
        }
    };
    let value_len = (3 + xaddr.len()) as u16; // family + xport + xaddr
    let mut out = Vec::with_capacity(HEADER_LEN + 4 + value_len as usize);
    out.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
    out.extend_from_slice(&(4 + value_len).to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(txn);
    out.extend_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
    out.extend_from_slice(&value_len.to_be_bytes());
    out.push(0);
    out.push(family);
    out.extend_from_slice(&port_xor.to_be_bytes());
    out.extend_from_slice(&xaddr);
    out
}

/// 每源 IP 令牌桶。
struct Bucket {
    tokens: f64,
    last: Instant,
}

fn allow(buckets: &mut HashMap<IpAddr, Bucket>, ip: IpAddr, rate_per_sec: u32, capacity: u32) -> bool {
    let now = Instant::now();
    let entry = buckets.entry(ip).or_insert(Bucket {
        tokens: capacity as f64,
        last: now,
    });
    entry.tokens = (entry.tokens + now.duration_since(entry.last).as_secs_f64() * rate_per_sec as f64)
        .min(capacity as f64);
    entry.last = now;
    if entry.tokens >= 1.0 {
        entry.tokens -= 1.0;
        true
    } else {
        false
    }
}

/// 单端口监听循环:解析请求 → 限速 → 回响应。垃圾包静默丢弃。
pub async fn run_listener(sock: std::net::UdpSocket, rate_per_sec: u32, capacity: u32) -> Result<()> {
    sock.set_nonblocking(true)?;
    let sock = tokio::net::UdpSocket::from_std(sock)?;
    let mut buckets: HashMap<IpAddr, Bucket> = HashMap::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, src) = sock.recv_from(&mut buf).await?;
        let Some(txn) = parse_request(&buf[..n]) else { continue };
        if !allow(&mut buckets, src.ip(), rate_per_sec, capacity) {
            continue;
        }
        let resp = build_response(&txn, src);
        let _ = sock.send_to(&resp, src).await;
    }
}
