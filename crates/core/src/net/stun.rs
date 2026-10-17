//! STUN Binding 客户端:raw socket 查询(必须发生在 socket 移交 quinn 之前)。
//! 与 server/stun 编解码有意独立,由共用测试向量钉死互通。

use anyhow::{anyhow, Context, Result};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use super::local_ipv4_addrs;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_XOR_MAPPED: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// 构造 Binding Request(20 字节,无属性)。
pub fn build_request(txn: &[u8; 12]) -> [u8; 20] {
    let mut p = [0u8; 20];
    p[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    p[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    p[8..20].copy_from_slice(txn);
    p
}

/// 解析 Binding Success:校验类型/magic/事务回显,扫描属性取首个 XOR-MAPPED-ADDRESS。
pub fn parse_xor_mapped(pkt: &[u8], txn: &[u8; 12]) -> Result<SocketAddr> {
    if pkt.len() < HEADER_LEN {
        return Err(anyhow!("short response"));
    }
    if u16::from_be_bytes([pkt[0], pkt[1]]) != BINDING_SUCCESS {
        return Err(anyhow!("not a binding success"));
    }
    if u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]) != MAGIC_COOKIE {
        return Err(anyhow!("bad magic"));
    }
    if &pkt[8..20] != txn {
        return Err(anyhow!("txn mismatch"));
    }
    let declared = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    anyhow::ensure!(pkt.len() >= HEADER_LEN + declared, "declared length exceeds packet");
    let mut off = HEADER_LEN;
    let end = HEADER_LEN + declared;
    while off + 4 <= end {
        let t = u16::from_be_bytes([pkt[off], pkt[off + 1]]);
        let l = u16::from_be_bytes([pkt[off + 2], pkt[off + 3]]) as usize;
        let v = off + 4;
        anyhow::ensure!(v + l <= end, "attribute overruns message");
        if t == ATTR_XOR_MAPPED && l >= 8 {
            // family(1)+xport(2)+xaddr(4/16) 最少 8 字节(v4 恰 8,取前 8)
            let family = pkt[v + 1];
            let xport = u16::from_be_bytes([pkt[v + 2], pkt[v + 3]]) ^ (MAGIC_COOKIE >> 16) as u16;
            return match family {
                0x01 => {
                    let mut a = [0u8; 4];
                    a.copy_from_slice(&pkt[v + 4..v + 8]);
                    let m = MAGIC_COOKIE.to_be_bytes();
                    let ip = std::net::Ipv4Addr::new(
                        a[0] ^ m[0], a[1] ^ m[1], a[2] ^ m[2], a[3] ^ m[3],
                    );
                    Ok(SocketAddr::new(std::net::IpAddr::V4(ip), xport))
                }
                0x02 => {
                    anyhow::ensure!(l >= 20, "v6 xor-mapped too short");
                    let mut mask = [0u8; 16];
                    mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
                    mask[4..].copy_from_slice(txn);
                    let mut a = [0u8; 16];
                    a.copy_from_slice(&pkt[v + 4..v + 20]);
                    let oct: Vec<u8> = a.iter().zip(mask).map(|(x, m)| x ^ m).collect();
                    let oct: [u8; 16] = oct.as_slice().try_into().unwrap();
                    let ip = std::net::Ipv6Addr::from(oct);
                    Ok(SocketAddr::new(std::net::IpAddr::V6(ip), xport))
                }
                f => Err(anyhow!("unknown family {f}")),
            };
        }
        // 属性 4 字节对齐
        off = v + l + (4 - (l & 3)) % 4;
    }
    Err(anyhow!("no xor-mapped-address attribute"))
}

/// 在 raw socket 上向 server 发 Binding,返回映射地址(阻塞,socket 未进 quinn 前用)。
pub fn binding_query(socket: &std::net::UdpSocket, server: SocketAddr, timeout: Duration) -> Result<SocketAddr> {
    let txn: [u8; 12] = rand::random();
    let req = build_request(&txn);
    socket.send_to(&req, server).context("stun send")?;
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 576];
    loop {
        let remain = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| anyhow!("stun timeout ({server})"))?;
        socket
            .set_read_timeout(Some(remain))
            .context("stun read timeout")?;
        let (n, from) = socket.recv_from(&mut buf).context("stun recv")?;
        if from != server {
            continue; // 迟到的其他来源包,忽略
        }
        if let Ok(mapped) = parse_xor_mapped(&buf[..n], &txn) {
            return Ok(mapped);
        }
        // 不匹配的响应:继续等(可能碰撞了旧事务)
    }
}

/// 候选收集结果。
pub struct GatheredCandidates {
    pub addrs: Vec<SocketAddr>,
    pub nat_symmetric: bool,
    /// STUN 主端口查询是否成功(失败 → 仅本地候选,调用方记警告)。
    pub stun_ok: bool,
    /// 主端口映射地址:查询成功且不与本地候选重复时为 `Some(映射)`;
    /// STUN 不可达或映射==本地(None,与 addrs 的去重语义一致)。
    pub srflx: Option<SocketAddr>,
}

/// 收集:本地接口 + srflx(主端口)+ 对称预判(同 socket 查备端口,映射不同 ⇒ 对称)。
/// STUN 不可达不报错:降级为本地候选(stun_ok=false)。
pub fn gather_candidates(
    socket: &std::net::UdpSocket,
    primary: SocketAddr,
    alt: SocketAddr,
) -> Result<GatheredCandidates> {
    let port = socket.local_addr().context("socket local addr")?.port();
    let mut addrs = local_ipv4_addrs(port);
    let t = Duration::from_secs(2);
    match binding_query(socket, primary, t) {
        Ok(m1) => {
            let srflx = if addrs.contains(&m1) { None } else { Some(m1) };
            if !addrs.contains(&m1) {
                addrs.push(m1);
                if addrs.len() > super::MAX_CANDIDATES {
                    addrs.pop(); // srflx 挤不进就丢:本地候选优先
                }
            }
            let nat_symmetric = binding_query(socket, alt, t).map(|m2| m2 != m1).unwrap_or(false);
            Ok(GatheredCandidates { addrs, nat_symmetric, stun_ok: true, srflx })
        }
        Err(_) => Ok(GatheredCandidates { addrs, nat_symmetric: false, stun_ok: false, srflx: None }),
    }
}
