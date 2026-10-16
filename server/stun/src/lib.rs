//! 极简 RFC 5389 STUN Binding 服务端(双端口部署,仅 XOR-MAPPED-ADDRESS)。
//! 与 mtdss-core 的客户端编解码**有意各自独立**(两侧方向不同、部署解耦),
//! 由共用测试向量(same bytes)钉死互通。

use anyhow::Result;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

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
    let value_len = (4 + xaddr.len()) as u16; // reserved + family + xport + xaddr (RFC 5389 §15.2)
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

/// 每源 IP 令牌桶。跟踪上限与空闲过期:公网部署下防攻击者用轮换源 IP 把
/// 20 字节包刷成永久堆条目(远程内存耗尽)。
const MAX_TRACKED_IPS: usize = 10_000;
const IDLE_EXPIRY: Duration = Duration::from_secs(60);

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// 纯-ish 决策核心:`now` 由调用方注入以便单测。
/// 满员时先清扫 60s 空闲项;仍满则拒绝**新** IP(不入表)。
/// 已跟踪 IP 跳过上限检查,走正常令牌路径。
fn allow_at(
    buckets: &mut HashMap<IpAddr, Bucket>,
    ip: IpAddr,
    rate_per_sec: u32,
    capacity: u32,
    now: Instant,
) -> bool {
    if buckets.len() >= MAX_TRACKED_IPS && !buckets.contains_key(&ip) {
        buckets.retain(|_, b| now.duration_since(b.last) < IDLE_EXPIRY);
        if buckets.len() >= MAX_TRACKED_IPS {
            return false;
        }
    }
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

fn allow(buckets: &mut HashMap<IpAddr, Bucket>, ip: IpAddr, rate_per_sec: u32, capacity: u32) -> bool {
    allow_at(buckets, ip, rate_per_sec, capacity, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u32) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::from(n))
    }

    #[test]
    fn bucket_map_bounded_and_sweeps_idle() {
        let t0 = Instant::now();
        let mut buckets: HashMap<IpAddr, Bucket> = HashMap::new();
        // 填满到跟踪上限(每 IP 独立桶,首包均消耗 1 令牌)
        for i in 0..MAX_TRACKED_IPS as u32 {
            assert!(allow_at(&mut buckets, ip(i), 1, 1, t0));
        }
        assert_eq!(buckets.len(), MAX_TRACKED_IPS);
        // 满员且无空闲项:新 IP 被拒绝且不入表
        assert!(!allow_at(&mut buckets, ip(MAX_TRACKED_IPS as u32), 1, 1, t0));
        assert_eq!(buckets.len(), MAX_TRACKED_IPS);
        // 已跟踪 IP 不受上限影响(跳过满员检查;61s 后令牌已回满)
        let t2 = t0 + IDLE_EXPIRY + Duration::from_secs(1);
        assert!(allow_at(&mut buckets, ip(0), 1, 1, t2));
        // 清扫 60s 空闲项后:仅最近活跃的 ip(0) 幸存,新 IP 被接受
        assert!(allow_at(&mut buckets, ip(MAX_TRACKED_IPS as u32 + 1), 1, 1, t2));
        assert_eq!(buckets.len(), 2);
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
