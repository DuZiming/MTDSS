//! 互拨原语:QUIC 即打洞(spec M2b §2/§6)。
//! 角色固定(对称破缺):双方互拨会各建一条出向连接,若两侧都返回
//! "最先就绪"的分支,可能各持两条不同连接的一端(实测必然如此——
//! accept 比拨号少一跳令牌回程,总是先就绪)。因此按公钥字典序固定
//! 角色偏好:小者优选拨号(拨通即返回,已接受的连接仅暂存作兜底),
//! 大者优选接受(accept 即返回,拨通的连接仅暂存作兜底)。两侧对
//! 同一对公钥算出同一角色;又因"拨通"必然以"对端 accept 已写令牌"
//! 为前提,两端必然相配于同一条连接。
//! 已证明入站可达后(收到对方拨来的连接),本轮在途拨号不再被单轮
//! 限时剪断,续跑到总预算——对端 accept 可能刚完成,令牌只差一个
//! RTT;剪断它会把已配对的连接拆散。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Result, anyhow};
use ed25519_dalek::VerifyingKey;
use futures_util::stream::{FuturesUnordered, StreamExt};
use quinn::Connection;

use crate::identity::DeviceIdentity;
use crate::transport::endpoint::{self as ep, TransportTuning};

/// punch 参数(测试注入秒级;生产 10s/2s,spec §7)。
/// `tuning` 为死链检测参数(keep-alive/空闲超时),经每轮 dial 的
/// connect_pinned_tuned 挂到拨出的连接上(Task 6 编排层注入)。
#[derive(Debug, Clone, Copy)]
pub struct PunchConfig {
    pub budget: Duration,
    pub handshake_timeout: Duration,
    pub tuning: TransportTuning,
}

impl Default for PunchConfig {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(2),
            tuning: TransportTuning::default(),
        }
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
        // 单轮限时:拨号不逐个包 timeout,统一由 round_timer 剪断;
        // 已 parked 时(见 timer 分支)在途拨号续跑到总预算。
        let mut round_timer =
            Box::pin(tokio::time::sleep_until(tokio::time::Instant::now() + per));
        let mut dials = FuturesUnordered::new();
        for &c in peer_candidates {
            dials.push(ep::connect_pinned_tuned(ep, c, peer_pk, me, cfg.tuning));
        }
        round += 1;
        // 本轮:dial 全部结算;按角色偏好决定是否立即返回。
        // biased + accept 在首位:select 默认随机轮询顺序下,空/速败的
        // dials(next() 立即 Ready(None))可能抢先返回而不 poll accept,
        // 令牌写出被拖到数百 ms 后,对端拨号已被单轮限时剪断。固定先
        // poll accept 保证每次唤醒都推进令牌路径。
        loop {
            tokio::select! {
                biased;
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
                _ = round_timer.as_mut() => {
                    if parked.is_some() && deadline > tokio::time::Instant::now() {
                        // 已收到对方拨来的连接:入站路径可达已证明。在途拨号
                        // 不剪,续到总预算——对端 accept 可能刚完成,令牌只差
                        // 一个 RTT;此刻剪断会把已配对连接拆成两条残端。
                        round_timer = Box::pin(tokio::time::sleep_until(deadline));
                    } else {
                        break; // 常规单轮限时;已 parked 且到预算则交预算路径收尾
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
                        // 无 per 超时包装:此 Err 为 QUIC 内部失败(被拒/连接亡)
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
        } else if let Some(conn) = held.take() {
            // 本轮拨通且 accept 始终未结算(无人拨我):拨通的这条即交付。
            // 本端 punch 返回后不再有应用层 accept 后续入站连接,被丢弃的
            // accept 不会写令牌,故对端在途拨号收不到令牌,只会在其单轮
            // 限时处超时,退回 parked(即本条连接的服务端一端),两端仍相配。
            return Ok(conn);
        }
        // 残竞态:从此处 checked_duration_since 到 sleep 醒来之间,预算可能
        // 恰好耗尽(单节拍窗口),本轮会多睡 ≤250ms 才回到 while 条件退出。
        // 无害:预算收尾路径(parked/held 兜底与报错)本就兜住这点超支,
        // 不值得为它引入原子时钟。
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
