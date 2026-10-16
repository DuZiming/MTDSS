//! 互拨原语:QUIC 即打洞(spec M2b §2/§6)。
//! 在总预算内并发 dial 对方全部候选,与本地 accept 竞速;任一成功即返回。
//!
//! 角色固定(对称破缺):双方互拨会各建一条出向连接,若两侧都返回
//! "最先就绪"的分支,可能各持两条不同连接的一端(实测必然如此——
//! accept 比拨号少一跳令牌回程,总是先就绪)。因此按公钥字典序固定
//! 角色偏好:小者优选拨号(拨通即返回,已接受的连接仅暂存作兜底),
//! 大者优选接受(accept 即返回,拨通的连接仅暂存作兜底)。两侧对
//! 同一对公钥算出同一角色;又因"拨通"必然以"对端 accept 已写令牌"
//! 为前提,两端必然相配于同一条连接。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Result, anyhow};
use ed25519_dalek::VerifyingKey;
use futures_util::stream::{FuturesUnordered, StreamExt};
use quinn::Connection;

use crate::identity::DeviceIdentity;
use crate::transport::endpoint as ep;

/// punch 参数(测试注入秒级;生产 10s/2s,spec §7)。
#[derive(Debug, Clone, Copy)]
pub struct PunchConfig {
    pub budget: Duration,
    pub handshake_timeout: Duration,
}

impl Default for PunchConfig {
    fn default() -> Self {
        Self { budget: Duration::from_secs(10), handshake_timeout: Duration::from_secs(2) }
    }
}

/// 互拨:ep 必须是双角色端点(make_dual_endpoint);allowed 为本端服务端白名单
/// (单条:对端);peer_candidates 为对方候选。返回与对端相配的连接:
/// 双方并发互拨时必为同一连接的两端(公钥字典序定角色,见模块注释)。
pub async fn punch_connect(
    ep: &quinn::Endpoint,
    me: &DeviceIdentity,
    allowed: &HashMap<String, VerifyingKey>,
    peer_pk: &VerifyingKey,
    peer_candidates: &[SocketAddr],
    cfg: &PunchConfig,
) -> Result<Connection> {
    // 公钥小者优选拨号端,大者优选接受端;同一对公钥两侧算出互补角色。
    let dial_pref = me.public_key().as_bytes() < peer_pk.as_bytes();
    let deadline = tokio::time::Instant::now() + cfg.budget;
    let mut accept = Box::pin(ep::accept_pinned(ep, allowed));
    let mut accept_done = false; // accept future 已结算,select 不再 poll
    let mut parked: Option<Connection> = None; // 拨号优选侧:已接受、拨号全败才用
    let mut held: Option<Connection> = None; // 接受优选侧:已拨通、accept 败后兜底
    let tried = peer_candidates.len();
    let mut round: u32 = 0;
    while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
        let per = cfg.handshake_timeout.min(remaining);
        let mut dials = FuturesUnordered::new();
        for &c in peer_candidates {
            dials.push(async move {
                match tokio::time::timeout(per, ep::connect_pinned(ep, c, peer_pk, me)).await {
                    Ok(Ok(conn)) => Ok(conn),
                    Ok(Err(e)) => Err(anyhow!("dial {c}: {e:#}")),
                    Err(_) => Err(anyhow!("dial {c}: handshake timeout")),
                }
            });
        }
        round += 1;
        // 本轮:dial 全部结算;按角色偏好决定是否立即返回
        loop {
            tokio::select! {
                r = accept.as_mut(), if !accept_done => {
                    match r {
                        Ok((conn, _dev)) => {
                            accept_done = true;
                            if dial_pref {
                                parked = Some(conn); // 暂存,继续等拨号
                            } else {
                                return Ok(conn); // 首选:accept 即返回
                            }
                        }
                        Err(_) => {
                            accept_done = true;
                            if !dial_pref {
                                if let Some(conn) = held.take() {
                                    return Ok(conn); // 兜底:accept 已败,用拨通的那条
                                }
                            }
                        }
                    }
                }
                r = dials.next() => {
                    match r {
                        Some(Ok(conn)) => {
                            if dial_pref || accept_done {
                                return Ok(conn); // 首选拨通即返回;或 accept 已败的兜底
                            }
                            held = held.take().or(Some(conn)); // 暂存,继续等 accept
                        }
                        Some(Err(_)) => {}
                        None => break,
                    }
                }
            }
        }
        if dial_pref {
            if let Some(conn) = parked.take() {
                return Ok(conn); // 本轮拨号全败,退回已接受的那条
            }
        }
        if let Some(remain) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            tokio::time::sleep(remain.min(Duration::from_millis(250))).await;
        }
        tracing::debug!("punch round {round} exhausted, retrying");
    }
    // 预算耗尽:手头若有任何一条已建立的连接仍可交付,否则报错
    if let Some(conn) = parked.or(held) {
        return Ok(conn);
    }
    Err(anyhow!(
        "no path established within budget {:?} ({tried} peer candidates tried)",
        cfg.budget
    ))
}
