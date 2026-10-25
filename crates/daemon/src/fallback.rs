//! connect-with-fallback 编排(v2-P1c 客户端中继兜底,传输面)。
//!
//! 直连打洞穷尽(punch_connect 预算耗尽)且账号中继资格在手时,自动改走
//! 服务端中继完成端到端 QUIC 传输:发起方(发送回合)持 per-peer 串行锁走
//! `RelayAllocate` 铸票,响应方(常驻接收)在轮首订阅的中继事件流上按
//! `peer_device` + `purpose` 双判别关联服务器推送(同一对端可并发跑 ctrl
//! 与传输两条兜底,purpose 区分两面、空 purpose = 老客户端不过滤——
//! v2-P2 票①);两侧各自以本端槽签建中继端点
//! ([`relay_sock::make_relay_endpoint`],MTU 压 1400 防加头后跨路径 MTU),
//! 再以 `[relay_addr]` 为候选互拨——角色对称性与接受令牌舞步全部照旧,
//! QUIC 引擎零改动。每轮重试重新走「直连→失败→再分配」,不复用旧会话
//! (服务端 120s 空闲回收兜底)。
//!
//! 错误面(裁定 G + 修复轮裁定):分配被拒 4/5 = 终局([`FallbackErr::
//! RelayDenied`],文案映射收口在本模块);分配 2「对端离线」/6「中继并发
//! 会话已达上限」(容量瞬态:服务端 120s 空闲回收自愈,终局失真——修复
//! 轮改可重试)/等待推送超窗/兜底互拨败 = [`FallbackErr::RelayFailed`]
//! (可重试);直连败且无兜底(或资格未开通)= [`FallbackErr::Direct`]
//! (调用方按今日 punch_err 口径包装)。
//!
//! 模块公开仅为编排类型与死因映射(relay_e2e/flow_unit 经 doc(hidden)
//! 直驱面消费;编排本体与 SignalHub 一样 pub(crate),不外泄)。

use crate::hub::{RelayEvt, SignalHub};
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::client::AllocOutcome;
use mtdss_core::transport::{punch, relay_sock, transfer};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::broadcast;

/// 响应方等待分配推送的窗口(值钉死):先排干订阅缓冲再等新帧,超窗即本
/// 轮兜底失败(可重试;发起方下一轮重试会重新分配并重推)。
const RELAY_PUSH_WAIT: Duration = Duration::from_secs(5);
/// 中继路径 MTU 上限(值钉死):+16B 会话标签后不跨典型路径 MTU(防分片/
/// 丢弃),调用方以克隆直连 tuning 后覆写本值的方式装配中继端点。
const RELAY_MAX_MTU: u16 = 1400;

/// 兜底角色:发起方(发送回合,走分配)或响应方(常驻接收,等推送)。
/// ctrl 两点位(第二刀)按同参数接入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackRole {
    Initiator,
    Responder,
}

/// 兜底平面判别(v2-P2 票①):同一对端可并发跑 ctrl 与传输两条中继兜底,
/// 分配请求带 purpose、服务器双帧回显,响应方凭它各取各面的推送,不错绑
/// 他面会话。空 purpose(老客户端)不过滤,行为不劣化。
/// `pub`:经 relay_e2e 的 doc(hidden) 直驱面跨 crate 消费(编排本体仍
/// pub(crate),与 [`FallbackRole`] 同一面)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPurpose {
    /// 控制通道面(ctrl.rs 两点位)。
    Ctrl,
    /// 传输数据面(flow.rs 发送/接收两轮)。
    Transport,
}

impl RelayPurpose {
    /// wire 形态(RelayAllocate.purpose 及服务器双帧回显值)。
    pub fn as_str(&self) -> &'static str {
        match self {
            RelayPurpose::Ctrl => "ctrl",
            RelayPurpose::Transport => "transport",
        }
    }
}

/// 已建立的中继路径(传输阶段配速/死亡监听/作业标记的数据源)。
/// 响应方 rate 取自身 entitlement(同账号同配);0 = 不限(沿服务端口径)。
#[derive(Debug, Clone, Copy)]
pub struct RelayPath {
    pub sid: u64,
    pub rate_kbps: u32,
}

/// connect_with_fallback 的产物:`None` = 直连(兜底未参与)。
#[derive(Debug)]
pub struct ConnectOutcome {
    pub conn: quinn::Connection,
    pub relay: Option<RelayPath>,
}

/// 兜底编排的结局。
#[derive(Debug)]
pub enum FallbackErr {
    /// 直连败且无兜底(或未开资格):携带 punch_connect 原始错误,调用方按
    /// 今日 punch_err 口径包装(NAT 标签/候选数在调用方)。
    Direct(anyhow::Error),
    /// 分配被拒 4/5(终局,裁定 G):文案映射收口在 [`denied_message`]。
    /// 拒 6 超并发不走本变体——修复轮裁定改 [`FallbackErr::RelayFailed`]
    /// 可重试(容量瞬态自愈,与配额本质不同)。
    RelayDenied { code: u32 },
    /// 分配 2「对端离线」/6「中继并发会话已达上限」(修复轮裁定:容量
    /// 瞬态,服务端 120s 空闲回收自愈,下一轮重试即可通过)/等待推送超窗/
    /// 兜底互拨败:可重试(下一轮重新走直连→失败→再分配)。
    RelayFailed(String),
}

/// RelayDenied 码 → 终局文案(4/5 钉死;6 已改走 RelayFailed 可重试,
/// 此处仅防御性保留其映射;未知码防御性同归终局,不空耗重试轮次——
/// 服务端拒绝码集是封闭的)。
pub(crate) fn denied_message(code: u32) -> String {
    match code {
        4 => "中继未开通".to_string(),
        5 => "中继配额已耗尽".to_string(),
        6 => "中继并发会话已达上限".to_string(),
        other => format!("中继不可用({other})"),
    }
}

/// connect-with-fallback:直连 punch_connect Ok 即直连返回;Err(预算耗尽即
/// 为穷尽,不做 NAT 类型启发)且资格在手走中继兜底,否则 [`FallbackErr::
/// Direct`] 交调用方按今日口径包装。`relay_rx` 由调用方轮首订阅传入(先订阅
/// 后打洞:分配推送不会早于订阅窗口,迟到的推送在接收端缓冲里等等待段排
/// 干)。`force_relay` 跳过直连段(测试注错,沿 fault_close_after_files
/// 先例);两侧端点/白名单/候选交换流程不动(仅 punch 段替换)。
#[allow(clippy::too_many_arguments)] // brief 钉死的签名:角色/平面/注错位在既有参数上追加
pub(crate) async fn connect_with_fallback(
    hub: &SignalHub,
    relay_rx: &mut broadcast::Receiver<RelayEvt>,
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    direct: &quinn::Endpoint,
    peer_candidates: &[SocketAddr],
    punch: punch::PunchConfig,
    role: FallbackRole,
    purpose: RelayPurpose,
    force_relay: bool,
) -> Result<ConnectOutcome, FallbackErr> {
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(peer_id.to_string(), *peer_pk);
    // force_relay:跳过直连段直接进兜底编排(不打不拨,中继路径用例注错)
    if !force_relay {
        match punch::punch_connect(direct, me, &allowed, peer_pk, peer_candidates, &punch).await {
            Ok(conn) => return Ok(ConnectOutcome { conn, relay: None }),
            Err(e) => {
                // 直连穷尽;资格不在手(未开通/离线)→ Direct(文案照旧)
                if !hub.relay_entitlement().is_some_and(|t| t.enabled) {
                    return Err(FallbackErr::Direct(e));
                }
            }
        }
    }
    match role {
        FallbackRole::Initiator => {
            relay_via_allocate(hub, me, peer_id, peer_pk, &allowed, punch, purpose).await
        }
        FallbackRole::Responder => {
            relay_via_push(hub, relay_rx, me, peer_id, peer_pk, &allowed, punch, purpose).await
        }
    }
}

/// 发起方兜底:持 per-peer 串行锁(同设备同对端恒同一把;表在 hub,跨连接
/// 代际存活)走分配 → 以请求方槽签建中继端点 → 与对端互拨。锁全程持有
/// (分配→建端点→互拨):同对端的并发兜底在此串行,分配不互踩。
#[allow(clippy::too_many_arguments)] // 与 connect_with_fallback 同面的角色分工体
async fn relay_via_allocate(
    hub: &SignalHub,
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    allowed: &HashMap<String, VerifyingKey>,
    punch: punch::PunchConfig,
    purpose: RelayPurpose,
) -> Result<ConnectOutcome, FallbackErr> {
    let lock = hub.fallback_lock(peer_id);
    let _serial = lock.lock().await;
    // 直接 await,不在 select 中取消:分配槽位会被占到下一响应/断连
    let alloc = hub
        .relay_allocate(peer_id, purpose.as_str())
        .await
        .map_err(|e| FallbackErr::RelayFailed(format!("中继分配失败: {e:#}")))?;
    let (sid, tag, relay_addr, rate_kbps) = match alloc {
        AllocOutcome::Allocated {
            sid,
            tag,
            relay_addr,
            expires_at: _,
            rate_kbps,
        } => (sid, tag, relay_addr, rate_kbps),
        // 2 = 对端离线:可重试(对端可能恰在重连窗口);6 = 超并发:容量
        // 瞬态可重试(修复轮裁定:服务端会话无释放帧、滞留至 120s 空闲
        // 回收自愈,与配额耗尽的终局本质不同——裁定 G 只钉「配额耗尽」
        // 终局);4/5 = 终局(裁定 G,行为不动)
        AllocOutcome::Denied { code: 2, .. } => {
            return Err(FallbackErr::RelayFailed("对端离线".into()))
        }
        AllocOutcome::Denied { code: 6, .. } => {
            return Err(FallbackErr::RelayFailed("中继并发会话已达上限".into()))
        }
        AllocOutcome::Denied { code, .. } => return Err(FallbackErr::RelayDenied { code }),
    };
    // 分配面畸形防御(tag 非 16B/地址不可解析):按兜底失败重试,不 panic
    let tag: [u8; 16] = tag.try_into().map_err(|_| {
        FallbackErr::RelayFailed(format!("中继分配 tag 非 16 字节(sid={sid})"))
    })?;
    let relay_addr: SocketAddr = relay_addr.parse().map_err(|_| {
        FallbackErr::RelayFailed(format!("中继地址不可解析: {relay_addr}(sid={sid})"))
    })?;
    relay_dial(me, peer_id, peer_pk, allowed, punch, sid, tag, relay_addr, rate_kbps).await
}

/// 响应方兜底:在轮首订阅的事件流上等本对端、本平面的分配推送(窗口
/// [`RELAY_PUSH_WAIT`];先排干缓冲再等新帧——推送早于等待进入的竞态由
/// 调用方的订阅时机闭合)。rate 取自身 entitlement(同账号同配;推送帧
/// 不带 rate)。
#[allow(clippy::too_many_arguments)] // 与 connect_with_fallback 同面的角色分工体
async fn relay_via_push(
    hub: &SignalHub,
    relay_rx: &mut broadcast::Receiver<RelayEvt>,
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    allowed: &HashMap<String, VerifyingKey>,
    punch: punch::PunchConfig,
    purpose: RelayPurpose,
) -> Result<ConnectOutcome, FallbackErr> {
    let (sid, tag, relay_addr) = wait_relay_push(relay_rx, peer_id, purpose.as_str()).await?;
    let rate_kbps = hub.relay_entitlement().map(|t| t.rate_kbps).unwrap_or(0);
    relay_dial(me, peer_id, peer_pk, allowed, punch, sid, tag, relay_addr, rate_kbps).await
}

/// 等 peer 的分配推送:按 (peer, purpose) 双判别过滤——同对端另一面的推送
/// (purpose 不符)与空 purpose 外的其他对端/其他会话/死帧一律跳过;
/// purpose 为空的推送视为老客户端,不过滤(混合版本不劣化);Lagged warn
/// 后继续排干(沿 hub wait_signal 房风);超窗 Err(可重试);代际更替
/// (Closed)同按超窗失败——发起方重试会重新分配并重推。
async fn wait_relay_push(
    relay_rx: &mut broadcast::Receiver<RelayEvt>,
    peer_id: &str,
    purpose: &str,
) -> Result<(u64, [u8; 16], SocketAddr), FallbackErr> {
    let wait = async {
        loop {
            match relay_rx.recv().await {
                Ok(RelayEvt::Push {
                    peer,
                    sid,
                    tag,
                    relay_addr,
                    expires_at: _,
                    purpose: evt_purpose,
                }) if peer == peer_id
                    && (evt_purpose == purpose || evt_purpose.is_empty()) =>
                {
                    return Ok((sid, tag, relay_addr))
                }
                Ok(_) => {} // 其他对端/其他平面/其他会话/死帧:不属本会话,忽略
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("中继事件滞后丢 {n} 帧,继续排干(丢失的推送随窗口超时兜底)");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("信令连接代际更替(中继事件源已关闭)".to_string())
                }
            }
        }
    };
    match tokio::time::timeout(RELAY_PUSH_WAIT, wait).await {
        Ok(r) => r.map_err(FallbackErr::RelayFailed),
        Err(_) => Err(FallbackErr::RelayFailed(format!(
            "等待中继分配推送超时(>{RELAY_PUSH_WAIT:?})"
        ))),
    }
}

/// 拿到本端槽签后的共用互拨:建中继端点 + 整份克隆 PunchConfig 覆写
/// MTU 1400(端点压 accept 侧,cfg 压 dial 侧)→ 以 `[relay_addr]` 为候选
/// punch_connect(真中继同样丢首轮拨号——对侧槽未绑定;竞态下经
/// parked/held 配对,确定性返回)→ 就绪日志行(nat-matrix grep 锚,文案
/// 钉死)。
#[allow(clippy::too_many_arguments)] // 角色分工体的共用尾部(槽签/地址/速率逐位下传)
async fn relay_dial(
    me: &DeviceIdentity,
    peer_id: &str,
    peer_pk: &VerifyingKey,
    allowed: &HashMap<String, VerifyingKey>,
    punch: punch::PunchConfig,
    sid: u64,
    tag: [u8; 16],
    relay_addr: SocketAddr,
    rate_kbps: u32,
) -> Result<ConnectOutcome, FallbackErr> {
    // 中继路径调优:克隆整份 PunchConfig 后覆写 max_mtu(keep-alive/空闲
    // 超时语义与直连一致)。MTU 覆写必须连 PunchConfig 一起——端点只管
    // accept 侧,dial 侧挂在 cfg.tuning(punch_connect 对每条拨出连接按
    // cfg.tuning 构造 ClientConfig);Initiator/Responder 两条互拨路径都
    // 汇入本函数,一处覆写两侧生效。
    let mut punch = punch;
    punch.tuning.max_mtu = RELAY_MAX_MTU;
    let relay_ep = relay_sock::make_relay_endpoint(
        SocketAddr::from(([0, 0, 0, 0], 0)),
        me,
        allowed,
        punch.tuning,
        tag,
        relay_addr,
    )
    .map_err(|e| FallbackErr::RelayFailed(format!("中继端点建立失败: {e:#}")))?;
    let conn = punch::punch_connect(&relay_ep, me, allowed, peer_pk, &[relay_addr], &punch)
        .await
        .map_err(|e| FallbackErr::RelayFailed(format!("中继互拨失败: {e:#}")))?;
    tracing::info!("中继兜底已建立: sid={} rate={}kbps peer={}", sid, rate_kbps, peer_id);
    Ok(ConnectOutcome {
        conn,
        relay: Some(RelayPath { sid, rate_kbps }),
    })
}

/// 传输阶段等本会话死亡通告(select 臂):命中(`Dead{sid}`)返回死因,
/// 调用方按 reason 经 [`relay_dead_err`] 收束。Lagged warn 后继续(丢失的
/// 死亡通告由 QUIC 空闲超时兜底);代际更替(Closed)**不误杀在途传输**
/// ——中继数据面独立于信令连接,信令断连后传输仍可自然收尾,本臂挂起让
/// 传输侧先结算。
pub(crate) async fn wait_relay_dead(
    relay_rx: &mut broadcast::Receiver<RelayEvt>,
    sid: u64,
) -> String {
    loop {
        match relay_rx.recv().await {
            Ok(RelayEvt::Dead { sid: s, reason }) if s == sid => return reason,
            Ok(_) => {} // 其他会话的死亡/无关推送:不属本会话,忽略
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("中继事件滞后丢 {n} 帧,继续等待(本会话死亡若被丢,由 QUIC 空闲超时兜底)");
            }
            Err(broadcast::error::RecvError::Closed) => {
                // 信令代际更替:不再有死亡可感知,挂起让传输臂自然结算
                std::future::pending::<()>().await
            }
        }
    }
}

/// 死因 → 发送错误:quota/disabled → [`transfer::SendError::Relay`](终局,
/// 外层 retryable()=false);idle/空及其他未知死因 → Network(可重试
/// 「中继会话已中断」——空闲回收/旧服务器形态,下一轮重新走直连→失败→
/// 再分配)。
pub fn relay_dead_err(reason: &str) -> transfer::SendError {
    match reason {
        "quota" => transfer::SendError::Relay {
            code: 5,
            message: "中继配额已耗尽".into(),
        },
        "disabled" => transfer::SendError::Relay {
            code: 4,
            message: "中继已被停用".into(),
        },
        _ => transfer::SendError::Network("中继会话已中断".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 速构一条分配推送(sid 进 tag 便于断言各归各)。
    fn push(peer: &str, sid: u64, purpose: &str) -> RelayEvt {
        RelayEvt::Push {
            peer: peer.into(),
            sid,
            tag: [sid as u8; 16],
            relay_addr: "127.0.0.1:9".parse().unwrap(),
            expires_at: 0,
            purpose: purpose.into(),
        }
    }

    #[tokio::test]
    async fn wait_relay_push_filters_by_purpose_and_peer() {
        // 契约(v2-P2 票①):推送按 (peer, purpose) 双判别——同对端另一
        // 面的推送(purpose 不符)跳过;purpose 为空 = 老客户端,不过滤
        //(混合版本不劣化);其他对端一律忽略。三帧预入缓冲后等待段即时
        // 排干,不触 RELAY_PUSH_WAIT。
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let _ = tx.send(push("peer", 1, "transport")); // 同对端、另一面:跳过
        let _ = tx.send(push("peer", 2, "")); // 老客户端(空 purpose):接受
        let _ = tx.send(push("other", 3, "ctrl")); // 其他对端:忽略
        let got = wait_relay_push(&mut rx, "peer", "ctrl")
            .await
            .expect("空 purpose 的推送必须命中(老客户端不劣化)");
        assert_eq!(got.0, 2, "须命中第二帧(空 purpose),而非第一帧(另一面推送)");
        assert_eq!(got.1, [2u8; 16], "tag 须取自命中帧(sid 收敛进 tag)");
    }

    #[tokio::test]
    async fn wait_relay_push_accepts_matching_purpose() {
        // 契约:purpose 完全匹配的新客户端推送直接命中(判别字段主路径,
        // 双面并发各归各的正例)。
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let _ = tx.send(push("peer", 7, "ctrl"));
        let got = wait_relay_push(&mut rx, "peer", "ctrl")
            .await
            .expect("匹配 purpose 的推送必须命中");
        assert_eq!(got.0, 7);
    }
}
