//! 网络原语:本地候选枚举 + STUN 客户端。

pub mod stun;

use std::net::{IpAddr, SocketAddr};

/// 每侧候选上限(沿用 M1 语义:无界列表拖垮对端互拨窗口)。
pub const MAX_CANDIDATES: usize = 8;

fn is_rfc1918(a: &SocketAddr) -> bool {
    match a.ip() {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(_) => false,
    }
}

/// 本机 IPv4 候选:环回第一,随后 RFC1918 私网(最可能真实直连),再其余;截断 8。
pub fn local_ipv4_addrs(port: u16) -> Vec<SocketAddr> {
    let mut out = vec![SocketAddr::from(([127, 0, 0, 1], port))];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        let (mut private, mut other): (Vec<SocketAddr>, Vec<SocketAddr>) = ifaces
            .into_iter()
            .filter(|ia| ia.ip().is_ipv4() && !ia.ip().is_loopback())
            .map(|ia| SocketAddr::new(ia.ip(), port))
            .partition(is_rfc1918);
        out.append(&mut private);
        out.append(&mut other);
    }
    out.truncate(MAX_CANDIDATES);
    out
}
