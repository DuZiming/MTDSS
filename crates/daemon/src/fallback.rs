//! connect-with-fallback 编排(v2-P1c 客户端中继兜底,传输面)。
//!
//! 直连打洞穷尽(punch_connect 预算耗尽)且账号中继资格在手时,自动改走
//! 服务端中继完成端到端 QUIC 传输:发起方(发送回合)持 per-peer 串行锁走
//! `RelayAllocate` 铸票,响应方(常驻接收)在轮首订阅的中继事件流上按
//! `peer_device` + `purpose` 双判别关联服务器推送(同一对端可并发跑 ctrl
//! 与传输两条兜底,purpose 区分两面、空 purpose = 老客户端不过滤——
//! v2-P2 票①);两侧各自以本端槽签建中继端点
//! ([`relay_sock::make_relay_endpoint`],中继调优整份覆写:MTU 压 1400
//! 防加头后跨路径 MTU、keep-alive None 绝不心跳保活——配额),
//! 再以 `[relay_addr]` 为候选互拨——角色对称性与接受令牌舞步全部照旧,
//! QUIC 引擎零改动。每轮重试重新走「直连→失败→再分配」,不复用旧会话
//! (服务端 120s 空闲回收兜底)。
//!
//! armed 记忆消费(V3-6 Task 6,spec §4.3):`connect_with_fallback` 是
//! 失败记忆的单消费缝——发起方 armed ∧ 资格在手跳直连段直分配、响应方
//! armed 跳互拨直等推(窗 5s→45s),中继编排可重试失败(`RelayFailed`)
//! 且直连段被跳的轮当轮反转补拨直连(成则清账直连返场);直连段拨号本身
//! 即记账面(Ok 清账 / Err 记账,armed 阈值 1)。记忆范围=仅传输面
//! (flow.rs 两调用点传 `Some(&state.punch_memory())`;ctrl 面传 None
//! 维持现状零改)。记忆本体与语义见 [`crate::relaymem::PunchMemory`]。
//!
//! 并行预备(V3-6 Task 8,spec §4.2):未 armed 的**传输面**发起方轮
//! (ctrl/浏览面不预备——「配额面零改」红线,浏览不因预备占中继槽),
//! 直连打洞与 3s 预备计时赛跑——计时先到 ∧ 中继资格在手 → 并行 spawn
//! 预备任务(分配+互拨,**跑到完成为止不 abort**,结果经 oneshot 回);
//! 打洞胜出 → 直连返场 + 预备会话显式释放弃单(轮亡路径经预备任务自弃
//! 单,轮在场的滞留交付经 [`SignalHub::release_relay`]);打洞穷尽 →
//! 预备结果即切(省去穷尽后的串行分配+互拨段);资格不在手不预备(行为
//! 零改);预备被拒 → 静默消费落回既有串行兜底(预备失败不放大失败面)。
//! armed 轮中继即主线、响应方等推,均不预备。
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
use mtdss_core::signaling::client::{AllocOutcome, SignalingClient};
use mtdss_core::transport::{punch, relay_sock, transfer};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};

/// 响应方等待分配推送的窗口(非 armed 轮,值钉死):先排干订阅缓冲再等新
/// 帧,超窗即本轮兜底失败(可重试;发起方下一轮重试会重新分配并重推)。
/// armed 轮走 [`ARMED_PUSH_WAIT`](等推窗选择见 [`responder_window`])。
const RELAY_PUSH_WAIT: Duration = Duration::from_secs(5);

/// armed 等推窗锚(spec §4.3:5s→45s,调优族):armed 轮响应方的分配推送
/// 等待窗——armed 意味「直连已知不通」,对端未同步 armed 时其打洞穷尽需
/// 10-30s 后才分配+推,45s 盖过之,免一轮空败;双侧同步 armed 常态 ~1-2s
/// 即达,窗口无感。常量是文档锚;进程内生效值经 [`armed_push_wait`]
/// (env 注入缝)读定。
const ARMED_PUSH_WAIT: Duration = Duration::from_secs(45);

/// 并行预备计时锚(spec §4.2,调优族):未 armed 发起方轮的预备任务点火
/// 时刻——打洞跑满预算,开局 3s(≈首轮互拨失败量级)仍未直连即并行发起
/// 中继分配,穷尽即切不再有串行分配段。常量是文档锚;进程内生效值经
/// [`hedge_delay`](env 注入缝)读定。
const HEDGE_DELAY: Duration = Duration::from_secs(3);

/// 预备计时的进程内生效值(env 注入缝,`MTDSS_TEST_*` 家族口径,
/// OnceLock 一次读定):`MTDSS_TEST_HEDGE_DELAY_MS` = 毫秒值。缺失 →
/// 钉死默认;非法(非数字/空串)→ warn 保持默认([`armed_push_wait`]
/// 同款房风)。
fn hedge_delay() -> Duration {
    static CACHE: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| hedge_delay_from(|k| std::env::var(k).ok()))
}

/// 预备计时映射纯函数(`get` 注入缝,单测同款房风):见 [`hedge_delay`]。
fn hedge_delay_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    match get("MTDSS_TEST_HEDGE_DELAY_MS") {
        None => HEDGE_DELAY,
        Some(raw) => match raw.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                tracing::warn!("ignored invalid MTDSS_TEST_HEDGE_DELAY_MS={raw:?} (keep default)");
                HEDGE_DELAY
            }
        },
    }
}

// 中继路径 MTU 1400 的真源已收口 crate::ctrl::ctrl_relay_tuning()(V3-5
// 值化:keep-alive None + 空闲 60s + MTU 1400 一并钉死,relay_dial 整份
// 覆写)。

/// armed 等推窗的进程内生效值(env 注入缝,`MTDSS_TEST_*` 家族口径,
/// OnceLock 一次读定):`MTDSS_TEST_ARMED_PUSH_WAIT_MS` = 毫秒值。缺失 →
/// 钉死默认;非法(非数字/空串)→ warn 保持默认(relaymem 同款房风)。
fn armed_push_wait() -> Duration {
    static CACHE: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| armed_push_wait_from(|k| std::env::var(k).ok()))
}

/// armed 等推窗映射纯函数(`get` 注入缝,单测同款房风):见 [`armed_push_wait`]。
fn armed_push_wait_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    match get("MTDSS_TEST_ARMED_PUSH_WAIT_MS") {
        None => ARMED_PUSH_WAIT,
        Some(raw) => match raw.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                tracing::warn!("ignored invalid MTDSS_TEST_ARMED_PUSH_WAIT_MS={raw:?} (keep default)");
                ARMED_PUSH_WAIT
            }
        },
    }
}

/// 响应方等推窗选择(spec §4.3):armed 轮(直连段被跳)→ `armed_wait`
/// (进程内生效值 [`armed_push_wait`],默认 45s——盖过对端未同步 armed 的
/// 打洞穷尽 10-30s + 分配推送全程);非 armed → [`RELAY_PUSH_WAIT`](5s,
/// 现状)。`armed_wait` 注入缝:单测注入短值免真等(生产恒 armed_push_wait())。
fn responder_window(armed_round: bool, armed_wait: Duration) -> Duration {
    if armed_round {
        armed_wait
    } else {
        RELAY_PUSH_WAIT
    }
}

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

/// 直连段单次尝试(计数缝):传输编排的直连拨号统一过此——`tests` 模块以
/// 原子计数钉「armed 轮零直连拨号 / 反转恰一拨」。中继互拨(relay_dial
/// 对中继端点的 punch_connect)不经此,不计数。futures 惰性:构造不拨,
/// 首 poll 才计数拨号(跳段路径零 poll 即零拨号)。
async fn punch_once(
    direct: &quinn::Endpoint,
    me: &DeviceIdentity,
    allowed: &HashMap<String, VerifyingKey>,
    peer_pk: &VerifyingKey,
    peer_candidates: &[SocketAddr],
    punch: &punch::PunchConfig,
) -> anyhow::Result<quinn::Connection> {
    #[cfg(test)]
    tests::PUNCH_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    punch::punch_connect(direct, me, allowed, peer_pk, peer_candidates, punch).await
}

/// armed 跳段判据(spec §4.3,角色差异):注错轮恒不跳(记忆面不读不写);
/// 发起方 armed ∧ 资格在手 → 跳(资格缺维持现状,不上报「中继优先」误
/// 语义);响应方 armed → 跳(等推窗 45s 吸收对端未同步 armed 的穷尽窗,
/// 双侧记忆独立无死锁)。纯函数,单测钉组合表。
fn armed_skip(
    force_relay: bool,
    punch_mem: Option<&crate::relaymem::PunchMemory>,
    peer_id: &str,
    entitled: bool,
    role: FallbackRole,
) -> bool {
    if force_relay {
        return false; // 注错轮:记忆面不读不写(测试注错保持现状路径)
    }
    let armed = punch_mem.is_some_and(|m| m.is_armed(peer_id));
    match role {
        // 发起方:armed ∧ 资格在手 → 跳(资格缺直连穷尽后失败,现状)
        FallbackRole::Initiator => armed && entitled,
        // 响应方:armed 即跳——「直连已知不通」,对端未同步 armed 时其穷尽
        // + 分配 + 推送全程由 armed 等推窗吸收
        FallbackRole::Responder => armed,
    }
}

/// connect-with-fallback:直连 punch_connect Ok 即直连返回(punch_mem 在场
/// 时清账);Err(预算耗尽即为穷尽,不做 NAT 类型启发)且资格在手走中继
/// 兜底,否则 [`FallbackErr::Direct`] 交调用方按今日口径包装。
///
/// armed 记忆消费(V3-6 Task 6,spec §4.3):`punch_mem` 在场时——发起方
/// armed ∧ 资格在手跳直连段直分配、响应方 armed 跳互拨直等推(窗 5s→45s),
/// 中继编排可重试失败且直连段被跳的轮当轮反转补拨直连(成则清账直连返场,
/// 败则原 RelayFailed 原样、记忆保持 armed);直连段拨号 Ok 清账 / Err 记账
/// (任一次穷尽即记账,armed 阈值 1)。`None` = 测试直驱/无记忆面,行为与
/// 无记忆旧径逐字节一致。
///
/// 并行预备(V3-6 Task 8,spec §4.2):未 armed 的传输面发起方轮(`role ==
/// Initiator ∧ purpose == Transport`,且非 `force_relay`、非 armed 跳段;
/// ctrl/浏览面不预备——配额面零改红线)直连打洞与 [`hedge_delay`] 计时
/// 赛跑——计时先到 ∧ 资格在手 → 并行预备任务(分配+互拨跑到完成,结果
/// oneshot 回):打洞胜出 → 直连返场 + 预备会话弃单释放;打洞穷尽 →
/// 预备结果即切(无串行分配段);资格不在手不预备(零改);预备被拒 →
/// 静默落回既有串行兜底。armed 轮(中继即主线)与响应方(等推)不预备。
///
/// `relay_rx` 由调用方轮首订阅传入(先订阅后打洞:分配推送不会早于订阅
/// 窗口,迟到的推送在接收端缓冲里等等待段排干)。`force_relay` 跳过直连段
/// (测试注错,沿 fault_close_after_files 先例)且记忆面零触(不读不写);
/// 两侧端点/白名单/候选交换流程不动(仅 punch 段替换)。
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
    punch_mem: Option<&crate::relaymem::PunchMemory>,
) -> Result<ConnectOutcome, FallbackErr> {
    // 资格(HelloOk 随代际缓存)读一次下传:armed 跳段判据与穷尽后的兜底
    // 闸同源同值(注入缝:离线 hub 直驱单测免真信令代际)。
    let entitled = hub.relay_entitlement().is_some_and(|t| t.enabled);
    fallback_round(
        entitled, hub, relay_rx, me, peer_id, peer_pk, direct, peer_candidates, punch, role,
        purpose, force_relay, punch_mem, hedge_delay(),
    )
    .await
}

/// 兜底轮编排(`entitled` 注入缝供离线直驱单测;语义见
/// [`connect_with_fallback`]):直连段(armed 跳段判据 gate;发起方非
/// armed 轮为预备赛跑版,计时 `hedge` 注入缝供单测)→ 角色臂(发起方
/// 预备收口/串行分配 / 响应方等推)→ RelayFailed 反转兜底。
#[allow(clippy::too_many_arguments)] // 与 connect_with_fallback 同面 + 资格注入缝
async fn fallback_round(
    entitled: bool,
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
    punch_mem: Option<&crate::relaymem::PunchMemory>,
    hedge: Duration,
) -> Result<ConnectOutcome, FallbackErr> {
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(peer_id.to_string(), *peer_pk);
    let skip_direct = armed_skip(force_relay, punch_mem, peer_id, entitled, role);
    if skip_direct {
        tracing::info!(peer = peer_id, "armed 记忆命中:跳直连段直进中继");
    }
    // 并行预备门(spec §4.2:适用面 = 未 armed **传输轮**):发起方 ∧ 传输
    // 面 ∧ 非注错非跳段轮 ∧ 资格在手——armed 轮中继即主线(§4.3)不预备;
    // 响应方等推不预备;ctrl 面(dial_session,浏览)不预备(「配额面零改」
    // 红线:浏览不因预备占中继槽);注错轮跳段照旧不预备;资格缺不预备
    //(行为零改)。
    let hedging = role == FallbackRole::Initiator
        && purpose == RelayPurpose::Transport
        && !force_relay
        && !skip_direct
        && entitled;
    // 直连段:注错轮整段绕过(不拨不记,记忆面零触);armed 轮跳段(零
    // 拨号);赛跑门在手的发起方轮走预备赛跑版([`hedged_direct_segment`]:
    // 计时先到并行预备,punch 继续跑到完成),其余单次拨号——Ok 清账直连
    // 返场,Err 记账后判资格。
    let hedged: Option<HedgeTicket> = if !force_relay && !skip_direct {
        if hedging {
            // 预备臂建造(owned 快照下传 spawn):分配句柄(代内 client
            // 廉价克隆)、per-peer 串行锁(Arc)、身份种子(DeviceIdentity
            // 不 Clone——cert_cache OnceLock;`from_seed` 重构等价身份,
            // 签名与 device_id 字节级一致,cert 缓存惰性重建)、白名单与
            // punch(预备互拨与串行分配同面)。
            let arm = || {
                let client = hub.current().map(|h| h.client.clone());
                let serial = hub.fallback_lock(peer_id);
                let (result_tx, result_rx) = oneshot::channel();
                let (verdict_tx, verdict_rx) = oneshot::channel();
                tokio::spawn(hedge_task(
                    client,
                    serial,
                    me.signing.to_bytes(),
                    peer_id.to_string(),
                    *peer_pk,
                    allowed.clone(),
                    punch,
                    purpose,
                    result_tx,
                    verdict_rx,
                ));
                HedgeTicket { result_rx, verdict_tx }
            };
            let (res, ticket) = hedged_direct_segment(
                punch_mem,
                peer_id,
                punch_once(direct, me, &allowed, peer_pk, peer_candidates, &punch),
                hedge,
                arm,
            )
            .await;
            match res {
                Ok(conn) => return Ok(ConnectOutcome { conn, relay: None }),
                // 穷尽:资格已知在手(赛跑门),不做 Direct 短路——预备收口
                // 即切(会票随返,None = 预备失败落回串行兜底)
                Err(_) => ticket,
            }
        } else {
            match direct_segment(
                punch_mem,
                peer_id,
                punch_once(direct, me, &allowed, peer_pk, peer_candidates, &punch),
            )
            .await
            {
                Ok(conn) => return Ok(ConnectOutcome { conn, relay: None }),
                Err(e) => {
                    // 直连穷尽;资格不在手(未开通/离线)→ Direct(文案照旧)
                    if !entitled {
                        return Err(FallbackErr::Direct(e));
                    }
                    None
                }
            }
        }
    } else {
        None
    };
    // 兜底编排。发起方 per-peer 串行锁升到编排层(旧在 relay_via_allocate
    // 内):锁覆盖分配→中继互拨全程(同对端并发兜底的分配不互踩,覆盖面
    // 与旧径一致;预备任务持同一把锁,预备收口先于串行兜底取锁,无死锁);
    // 反转补拨在锁外跑(guard 随角色臂块结束即释放)——与改动前直连段同位
    // 不同步,补拨持分配锁会把同对端并发分配兜底堵在打洞预算上,不为之
    // 持锁。响应方等推窗按 armed 轮选窗(45s/5s)。
    let outcome = match role {
        FallbackRole::Initiator => {
            if let Some(ticket) = hedged {
                // 预备在场:穷尽侧收口——预备已成即切(Ok,无串行段);
                // 预备失败/失联(None,静默消费)→ 落回既有串行兜底
                match ticket.resolve_for_exhaust(hub).await {
                    Some(o) => Ok(o),
                    None => {
                        initiator_serial(hub, me, peer_id, peer_pk, &allowed, punch, purpose).await
                    }
                }
            } else {
                initiator_serial(hub, me, peer_id, peer_pk, &allowed, punch, purpose).await
            }
        }
        FallbackRole::Responder => {
            relay_via_push(
                hub,
                relay_rx,
                me,
                peer_id,
                peer_pk,
                &allowed,
                punch,
                purpose,
                responder_window(skip_direct, armed_push_wait()),
            )
            .await
        }
    };
    match outcome {
        Ok(o) => Ok(o),
        Err(e) => {
            // 中继失败反转兜底(spec §4.3):跳段轮 RelayFailed 当轮补拨直连
            reversal_punch(
                e,
                skip_direct,
                punch_mem,
                peer_id,
                punch_once(direct, me, &allowed, peer_pk, peer_candidates, &punch),
            )
            .await
        }
    }
}

/// 发起方串行兜底(既有语义,穷尽后分配):per-peer 串行锁升到编排层,
/// 锁覆盖分配→中继互拨全程(同对端并发兜底的分配不互踩;预备任务持同一
/// 把锁——预备收口先于本路径取锁,预备在场时本路径只在预备失败/未预备轮
/// 进入)。
async fn initiator_serial(
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
    relay_via_allocate(hub, me, peer_id, peer_pk, allowed, punch, purpose).await
}

/// 直连段单轮拨号 + 记忆记账(spec §4.3):Ok → [`crate::relaymem::
/// PunchMemory::note_direct_success`] 清账直连返场;Err →
/// [`crate::relaymem::PunchMemory::note_exhaust`] 记账(任一次穷尽即记账,
/// armed 阈值 1 口径;代际 1 即新 armed,落「armed 建立」日志)。`punch_mem`
/// None = 无记忆面(测试直驱),纯拨号零记账——与无记忆旧径逐字节一致。
/// 跳段决策(armed ∧ 资格,角色差异)在编排侧 [`armed_skip`]——本函数只管
/// 「拨与记」。`attempt` 注入缝:生产 = [`punch_once`](真拨号;futures 惰性,
/// 构造不拨);单测注桩(决定性结果 + poll 旗,免真拨号面)。预备赛跑版
/// ([`hedged_direct_segment`])共用记账面 [`note_punch_result`]。
async fn direct_segment(
    punch_mem: Option<&crate::relaymem::PunchMemory>,
    peer_id: &str,
    attempt: impl Future<Output = anyhow::Result<quinn::Connection>> + Send,
) -> Result<quinn::Connection, anyhow::Error> {
    note_punch_result(punch_mem, peer_id, attempt.await)
}

/// 直连拨号结果的记账面(直连段与预备赛跑版共用):Ok →
/// [`crate::relaymem::PunchMemory::note_direct_success`] 清账直连返场;
/// Err → [`crate::relaymem::PunchMemory::note_exhaust`] 记账(任一次穷尽
/// 即记账,armed 阈值 1 口径;代际 1 即新 armed,落「armed 建立」日志)。
/// `punch_mem` None = 无记忆面(测试直驱),纯结果零记账——与无记忆旧径
/// 逐字节一致。
fn note_punch_result(
    punch_mem: Option<&crate::relaymem::PunchMemory>,
    peer_id: &str,
    res: anyhow::Result<quinn::Connection>,
) -> anyhow::Result<quinn::Connection> {
    match res {
        Ok(conn) => {
            if let Some(m) = punch_mem {
                m.note_direct_success(peer_id);
            }
            Ok(conn)
        }
        Err(e) => {
            if let Some(m) = punch_mem {
                let gen = m.note_exhaust(peer_id);
                if gen == 1 {
                    // armed 建立行(spec §4.3 日志三行之一):单次穷尽即
                    // armed,下一轮起中继优先
                    tracing::info!(peer = peer_id, "打洞穷尽,armed 记忆建立(下一轮起中继优先)");
                }
            }
            Err(e)
        }
    }
}

/// 预备会票(发起方 hedged 轮的竞速收口面):预备任务的结果接收端 + 直
/// 连裁决发送端。直连胜出 → 裁决 `true` 发出即弃(预备任务自弃单,会票
/// 随直连返场丢弃);直连穷尽 → [`HedgeTicket::resolve_for_exhaust`]。
struct HedgeTicket {
    result_rx: oneshot::Receiver<HedgedOutcome>,
    verdict_tx: oneshot::Sender<bool>,
}

/// 预备任务的交付载荷:互拨结果 + 滞留会话 sid(`Some` = 分配已成而会话
/// 未交付为可用——轮在场时经 [`SignalHub::release_relay`] 先还槽再走串行
/// 兜底,免预备滞留放大拒 6;轮亡路径的释放面在预备任务内,见
/// [`hedged_disposition`])。
#[derive(Debug)]
struct HedgedOutcome {
    outcome: Result<ConnectOutcome, FallbackErr>,
    stranded_sid: Option<u64>,
}

impl HedgeTicket {
    /// 直连穷尽侧收口:先裁决 `false`(预备任务知轮未胜,互拨毕即交付),
    /// 后收结果——已毕即用(「立即切换」:预备任务与打洞并行完成,穷尽后
    /// 不再有串行分配+互拨段),未毕等毕(唯一等待面是预备任务自身收尾)。
    /// 预备失败(`outcome` Err)/失联(信道断 = 预备任务恐慌)→ None =
    /// 落回既有串行兜底(预备失败静默不放大失败面);滞留会话(分配已成、
    /// 互拨败)先经 hub 显式释放再落回。
    async fn resolve_for_exhaust(self, hub: &SignalHub) -> Option<ConnectOutcome> {
        let _ = self.verdict_tx.send(false);
        match self.result_rx.await {
            Ok(HedgedOutcome { outcome: Ok(o), stranded_sid: _ }) => Some(o),
            Ok(HedgedOutcome { outcome: Err(e), stranded_sid }) => {
                // 滞留会话先还槽(轮在场,hub 面释放走当前代——比预备任务
                // 的代内句柄快照新鲜,代际更替后仍可送达)
                if let Some(sid) = stranded_sid {
                    hub.release_relay(sid);
                }
                tracing::debug!("预备任务失败,静默消费落回既有串行兜底: {e:?}");
                None
            }
            Err(_) => {
                tracing::debug!("预备任务失联(恐慌/失约),落回既有串行兜底");
                None
            }
        }
    }
}

/// 预备收口判别(纯函数,单测钉表):预备会话是否随收口弃单释放。直连
/// 胜出(`Ok(true)`)/轮亡(裁决通道断——无人接收结果)→ 恒弃单;直连
/// 穷尽(`Ok(false)`)→ 互拨成则交付即用(不释放),互拨败则弃单后交付
/// (穷尽侧走既有串行兜底重分配,滞留会话放大拒 6——「不放大失败」是
/// spec §4.2 配额纪律)。
fn hedged_disposition(verdict: Result<bool, oneshot::error::RecvError>, dial_ok: bool) -> bool {
    match verdict {
        Ok(false) => !dial_ok,
        _ => true,
    }
}

/// 预备会话弃单释放(fire-and-forget,与 `SignalHub::release_relay` 同形:
/// 即发即忘、发送失败仅 debug、丢失的释放只损失服务器 120s 空闲回收的
/// 及时性不是正确性、重复释放无害)。轮亡/胜出收口发生在预备任务内(轮已
/// 不在场,无 hub 可达)——预备任务持的是代内 client 句柄快照,在此复刻
/// 发送面;None = 轮首即离线(分配段必已败,防御分支不达)。
fn release_hedged_session(client: Option<SignalingClient>, sid: u64) {
    tracing::debug!(sid, "预备会话弃单:显式释放(直连胜出/轮亡收口)");
    let Some(client) = client else {
        tracing::debug!(sid, "信令未在线,跳过中继释放(交由服务器空闲回收)");
        return;
    };
    tokio::spawn(async move {
        if let Err(err) = client.relay_release(sid).await {
            tracing::debug!(sid, %err, "中继释放帧发送失败(交由服务器空闲回收)");
        }
    });
}

/// 预备任务体(spec §4.2):与直连段并行的中继分配+互拨——**跑到完成
/// 为止不 abort**(中途取消可能留下服务端已铸而无人跟踪的会话;编排层
/// spawn 后永不被 cancel,收口全在裁决点)。收口判别见
/// [`hedged_disposition`]:直连胜出 → 自弃单(显式释放,互拨成则并关本
/// 端连接);直连穷尽 → 结果交付轮(Ok 即用即切 / Err 带滞留 sid 交付
/// ——轮在场经 hub 释放;交付失败 = 轮恰在交付 await 点被取消的竞态窗,
/// 滞留 sid 兜底任务内释放);轮亡(裁决通道断)→ 自弃单防泄漏。分配毕
/// 设检查点:胜出/轮亡先至即弃单返场,不烧无谓互拨。
#[allow(clippy::too_many_arguments)] // 与 relay_via_allocate 同面 + 信道两端
async fn hedge_task(
    client: Option<SignalingClient>,
    serial: Arc<tokio::sync::Mutex<()>>,
    me_seed: [u8; 32],
    peer_id: String,
    peer_pk: VerifyingKey,
    allowed: HashMap<String, VerifyingKey>,
    punch: punch::PunchConfig,
    purpose: RelayPurpose,
    result_tx: oneshot::Sender<HedgedOutcome>,
    mut verdict_rx: oneshot::Receiver<bool>,
) {
    // 等价身份(DeviceIdentity 不 Clone,见编排侧 arm 注);仅本任务内的
    // 中继端点签建/互拨使用,与会话账务无关。
    let me = DeviceIdentity::from_seed(&me_seed);
    // per-peer 串行锁:预备分配与任何分配同锁(hub 纪律;胜出轮不等锁——
    // 锁持有至预备完成,释放点见各收口:穷尽侧交付前先放,串行兜底要在
    // 同锁上重分配)。
    let _serial = serial.lock().await;
    // 分配段:拒绝/离线(无会话,无释放面)→ 裁决后交付(穷尽侧走串行
    // 兜底;胜出/轮亡侧静默消费——预备失败不放大失败面,brief §6)。
    let allocated = allocate_only(client.as_ref(), &peer_id, purpose.as_str()).await;
    let (sid, tag, relay_addr, rate_kbps) = match allocated {
        Err(e) => {
            drop(_serial);
            match verdict_rx.await {
                Ok(false) => {
                    let _ = result_tx.send(HedgedOutcome { outcome: Err(e), stranded_sid: None });
                }
                _ => {
                    tracing::debug!(peer = %peer_id, "预备分配被拒,直连侧胜出/轮亡,静默消费");
                }
            }
            return;
        }
        Ok(pack) => pack,
    };
    // 检查点(分配毕):胜出/轮亡先至 → 即刻弃单返场,不烧互拨;穷尽先至
    // (Ok(false))→ 记住裁决,互拨毕即交付;未决(Empty)→ 互拨毕再收
    // 裁决。false 分支不丢裁决——try_recv 消费后收尾须用记忆值,二次
    // await 会永悬。
    let mut verdict = None;
    match verdict_rx.try_recv() {
        Ok(true) | Err(oneshot::error::TryRecvError::Closed) => {
            drop(_serial);
            release_hedged_session(client, sid);
            return;
        }
        Ok(false) => verdict = Some(Ok(false)),
        Err(oneshot::error::TryRecvError::Empty) => {}
    }
    // 互拨段(锁覆盖分配→互拨全程,与串行分配同覆盖面)
    let result =
        relay_dial(&me, &peer_id, &peer_pk, &allowed, punch, sid, tag, relay_addr, rate_kbps)
            .await;
    drop(_serial);
    // 裁决收口(判别见 hedged_disposition):先释放面,后交付面——穷尽侧
    // Ok 即用即切(不释放);互拨败交付时带滞留 sid(轮在场经 hub 释放)
    let verdict = match verdict {
        Some(v) => v,
        None => verdict_rx.await,
    };
    let is_exhaust = matches!(verdict, Ok(false));
    let release = hedged_disposition(verdict, result.is_ok());
    let stranded_sid = match &result {
        // 互拨败且轮在场(穷尽交付):会话不随交付可用——带滞留 sid 交轮
        // 释放(轮走当前代 hub 面,比任务内代内快照新鲜);轮亡侧(交付
        // 无人收)已按判别在任务内释放
        Err(_) if is_exhaust => Some(sid),
        _ => None,
    };
    if release && stranded_sid.is_none() {
        // 任务内释放面:胜出/轮亡(轮不在场,hub 不可达)。恒非穷尽交付面
        //(穷尽∧互拨成不释放、穷尽∧互拨败带滞留)——提前返使下方交付臂
        // 的兜底释放与此处所有权互斥,语义不变
        release_hedged_session(client, sid);
        return;
    }
    if is_exhaust {
        // 穷尽交付:Ok 即用即切 / Err 带滞留 sid 交既有串行兜底。交付失败
        //(轮恰在收口 await 点被取消的竞态窗)=「结果永不 dropped unread」
        // 红线的兜底:搁浅 sid 改在任务内释放,不得悬等 120s 空闲回收;
        // Ok 交付失败无会话面,连接随值弃(客户端半关触发服务端快回收)
        match result_tx.send(HedgedOutcome { outcome: result, stranded_sid }) {
            Ok(()) => {}
            Err(HedgedOutcome { outcome: Err(_), stranded_sid: Some(_) }) => {
                release_hedged_session(client, sid);
            }
            Err(_) => {}
        }
    }
}

/// 直连段与预备计时赛跑(spec §4.2,发起方非 armed 非注错轮):punch 与
/// `hedge` 计时 tokio::select 双臂赛跑——两臂 future 均 pin 引用轮询,计时
/// 先到**不取消**直连段(punch 继续跑到完成,「赛跑不弃赛」的根基);
/// punch 先毕 → [`direct_segment`] 同款记账,无预备(快胜/快败 < 计时 =
/// 现状零改)。计时先到 → `arm` 建造预备任务(oneshot 收结果)→ punch
/// 继续:Ok 清账 + 胜出裁决 `true`(预备任务自弃单)+ 直连返场;Err 记账
/// + 会票随返(穷尽侧收结果即切)。`hedge` 注入缝:单测注入短计时免真等。
async fn hedged_direct_segment(
    punch_mem: Option<&crate::relaymem::PunchMemory>,
    peer_id: &str,
    attempt: impl Future<Output = anyhow::Result<quinn::Connection>> + Send,
    hedge: Duration,
    arm: impl FnOnce() -> HedgeTicket,
) -> (Result<quinn::Connection, anyhow::Error>, Option<HedgeTicket>) {
    tokio::pin!(attempt);
    let hedge_sleep = tokio::time::sleep(hedge);
    tokio::pin!(hedge_sleep);
    let ticket = tokio::select! {
        // punch 先毕(成或穷尽):记账面与 direct_segment 逐字节同款
        res = &mut attempt => return (note_punch_result(punch_mem, peer_id, res), None),
        // 计时先到:预备任务在场,punch 继续跑(不弃赛)
        _ = &mut hedge_sleep => Some(arm()),
    };
    match attempt.await {
        Ok(conn) => {
            if let Some(t) = ticket {
                // 胜出裁决(预备任务恐慌失联即发送失败,忽略——任务自身
                // 收口面兜底,见 hedged_disposition 的轮亡臂)
                let _ = t.verdict_tx.send(true);
            }
            (note_punch_result(punch_mem, peer_id, Ok(conn)), None)
        }
        Err(e) => (note_punch_result(punch_mem, peer_id, Err(e)), ticket),
    }
}

/// 中继失败反转兜底(spec §4.3):armed 轮(`skipped` = 直连段被跳)中继
/// 编排可重试失败(`RelayFailed`)→ 当轮补拨直连保活命:成 → 清账直连
/// 返场;败 → 原 RelayFailed 原样返回(记忆保持 armed——直连仍不可达,
/// 下一轮继续中继优先)。仅 RelayFailed 反转(RelayDenied 终局不遮蔽,
/// 裁定 G);已拨轮(直连段已穷尽过,非跳段)不二拨。`attempt` 注入缝同
/// [`direct_segment`]。
async fn reversal_punch(
    err: FallbackErr,
    skipped: bool,
    punch_mem: Option<&crate::relaymem::PunchMemory>,
    peer_id: &str,
    attempt: impl Future<Output = anyhow::Result<quinn::Connection>> + Send,
) -> Result<ConnectOutcome, FallbackErr> {
    // 仅跳段轮 + 可重试类反转:RelayDenied 终局(裁定 G)不遮蔽;已拨轮
    // (直连段已穷尽过)不二拨。
    if !skipped || !matches!(err, FallbackErr::RelayFailed(_)) {
        return Err(err);
    }
    match attempt.await {
        Ok(conn) => {
            if let Some(m) = punch_mem {
                m.note_direct_success(peer_id);
            }
            // 反转返场行(spec §4.3「中继失败反转兜底」)
            tracing::info!(peer = peer_id, "中继编排失败,反转直连成功(armed 清账)");
            Ok(ConnectOutcome { conn, relay: None })
        }
        // 补拨败:原兜底错误原样返回(记忆保持 armed——直连仍不可达,下一
        // 轮继续中继优先)
        Err(_) => Err(err),
    }
}

/// 发起方兜底:走分配 → 以请求方槽签建中继端点 → 与对端互拨。per-peer
/// 串行锁在编排层([`fallback_round`] 的 [`initiator_serial`]),锁覆盖本
/// 函数全程(分配→中继互拨串行);预备任务([`hedge_task`])持同一把锁。
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
    // 分配句柄随取(廉价克隆;离线 None 由 [`allocate_only`] 按今日离线
    // 文案失败——与 hub.relay_allocate 逐字节同形)
    let client = hub.current().map(|h| h.client.clone());
    let (sid, tag, relay_addr, rate_kbps) =
        allocate_only(client.as_ref(), peer_id, purpose.as_str()).await?;
    relay_dial(me, peer_id, peer_pk, allowed, punch, sid, tag, relay_addr, rate_kbps).await
}

/// 中继分配段(发起方串行兜底与预备任务的共用脊):分配 + 畸形防御 →
/// (sid, tag, relay_addr, rate)。拒绝面与今日逐字节——2「对端离线」/6
/// 「中继并发会话已达上限」可重试 [`FallbackErr::RelayFailed`](修复轮
/// 裁定:容量瞬态,服务端 120s 空闲回收自愈;裁定 G 只钉「配额耗尽」
/// 终局),4/5 = 终局 [`FallbackErr::RelayDenied`];句柄 None(离线)→
/// 今日「信令未在线」文案的可重试 RelayFailed。分配槽位占有纪律
/// (hub.relay_allocate 文档同款):调用方直接 await 到完成为止,不得在
/// select 中取消本等待——分配槽位会被占到下一响应/断连。
async fn allocate_only(
    client: Option<&SignalingClient>,
    peer_id: &str,
    purpose: &str,
) -> Result<(u64, [u8; 16], SocketAddr, u32), FallbackErr> {
    #[cfg(test)]
    tests::ALLOC_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let alloc = match client {
        Some(c) => c
            .relay_allocate(peer_id, purpose)
            .await
            .map_err(|e| FallbackErr::RelayFailed(format!("中继分配失败: {e:#}")))?,
        None => {
            return Err(FallbackErr::RelayFailed(format!(
                "中继分配失败: 信令未在线,无法中继分配给 {peer_id}"
            )))
        }
    };
    match alloc {
        AllocOutcome::Allocated {
            sid,
            tag,
            relay_addr,
            expires_at: _,
            rate_kbps,
        } => {
            // 分配面畸形防御(tag 非 16B/地址不可解析):按兜底失败重试,
            // 不 panic
            let tag: [u8; 16] = tag.try_into().map_err(|_| {
                FallbackErr::RelayFailed(format!("中继分配 tag 非 16 字节(sid={sid})"))
            })?;
            let relay_addr: SocketAddr = relay_addr.parse().map_err(|_| {
                FallbackErr::RelayFailed(format!("中继地址不可解析: {relay_addr}(sid={sid})"))
            })?;
            Ok((sid, tag, relay_addr, rate_kbps))
        }
        AllocOutcome::Denied { code: 2, .. } => Err(FallbackErr::RelayFailed("对端离线".into())),
        AllocOutcome::Denied { code: 6, .. } => {
            Err(FallbackErr::RelayFailed("中继并发会话已达上限".into()))
        }
        AllocOutcome::Denied { code, .. } => Err(FallbackErr::RelayDenied { code }),
    }
}

/// 响应方兜底:在轮首订阅的事件流上等本对端、本平面的分配推送(窗口由
/// 编排层按 armed 轮选定,[`responder_window`]:armed 45s / 非 armed 5s;
/// 先排干缓冲再等新帧——推送早于等待进入的竞态由调用方的订阅时机闭合)。
/// rate 取自身 entitlement(同账号同配;推送帧不带 rate)。
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
    wait_window: Duration,
) -> Result<ConnectOutcome, FallbackErr> {
    let (sid, tag, relay_addr) =
        wait_relay_push(relay_rx, peer_id, purpose.as_str(), wait_window).await?;
    let rate_kbps = hub.relay_entitlement().map(|t| t.rate_kbps).unwrap_or(0);
    relay_dial(me, peer_id, peer_pk, allowed, punch, sid, tag, relay_addr, rate_kbps).await
}

/// 等 peer 的分配推送:按 (peer, purpose) 双判别过滤——同对端另一面的推送
/// (purpose 不符)与空 purpose 外的其他对端/其他会话/死帧一律跳过;
/// purpose 为空的推送视为老客户端,不过滤(混合版本不劣化);Lagged warn
/// 后继续排干(沿 hub wait_signal 房风);超 `window` Err(可重试;窗值由
/// 编排层按 armed 轮选定);代际更替(Closed)同按超窗失败——发起方重试会
/// 重新分配并重推。
async fn wait_relay_push(
    relay_rx: &mut broadcast::Receiver<RelayEvt>,
    peer_id: &str,
    purpose: &str,
    window: Duration,
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
    match tokio::time::timeout(window, wait).await {
        Ok(r) => r.map_err(FallbackErr::RelayFailed),
        Err(_) => Err(FallbackErr::RelayFailed(format!(
            "等待中继分配推送超时(>{window:?})"
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
    // 中继路径调优(V3-5 值化):克隆整份 PunchConfig 后整份换中继调优
    // (crate::ctrl::ctrl_relay_tuning:keep-alive None——中继会话吃服务器
    // 配额,绝不心跳保活,60s 空闲自灭;MTU 1400 防加头跨路径)。覆写必须
    // 连 PunchConfig 一起——端点只管 accept 侧,dial 侧挂在 cfg.tuning
    // (punch_connect 对每条拨出连接按 cfg.tuning 构造 ClientConfig);
    // ctrl/传输两平面的 Initiator/Responder 互拨路径都汇入本函数,一处
    // 覆写两侧生效(直连面心跳默认开,中继路一律摘除)。
    let mut punch = punch;
    punch.tuning = crate::ctrl::ctrl_relay_tuning();
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
    use crate::relaymem::PunchMemory;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

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
        // 排干,不触等推窗。
        let (tx, mut rx) = tokio::sync::broadcast::channel(8);
        let _ = tx.send(push("peer", 1, "transport")); // 同对端、另一面:跳过
        let _ = tx.send(push("peer", 2, "")); // 老客户端(空 purpose):接受
        let _ = tx.send(push("other", 3, "ctrl")); // 其他对端:忽略
        let got = wait_relay_push(&mut rx, "peer", "ctrl", RELAY_PUSH_WAIT)
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
        let got = wait_relay_push(&mut rx, "peer", "ctrl", RELAY_PUSH_WAIT)
            .await
            .expect("匹配 purpose 的推送必须命中");
        assert_eq!(got.0, 7);
    }

    // ---------- V3-6 Task 6:armed 记忆消费单测台 ----------

    /// 直连拨号计数(生产 punch_once 每 poll 恰 +1):「armed 轮零直连拨号 /
    /// 反转恰一拨」的证据面。choreo 用例经 [`CHOREO_GATE`] 串行,增量断言
    /// 精确(计数进程内共享,并发拨号会互染)。
    pub(crate) static PUNCH_CALLS: AtomicUsize = AtomicUsize::new(0);
    /// choreo 用例并发闸:PUNCH_CALLS 是进程内共享计数,涉真拨号的编排
    /// 用例全部持闸串行(增量断言才精确)。
    static CHOREO_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 直连候选黑洞:TEST-NET-1 丢弃端口,拨号无响应按预算快败(relay_e2e
    /// 同款注错形态;单测不依赖外网路由——UDP 发出即忘,预算内零响应)。
    fn black_hole() -> SocketAddr {
        "192.0.2.1:9".parse().unwrap()
    }

    /// punch 配置速构(预算参数化 + 短单轮限时快转;relay_e2e::test_punch
    /// 同款意图,ms 级完成路径百倍余量)。
    fn test_punch(budget: Duration) -> punch::PunchConfig {
        punch::PunchConfig {
            budget,
            handshake_timeout: Duration::from_millis(250),
            ..Default::default()
        }
    }

    /// 双角色端点速构(relay_e2e::direct_endpoint 同款,绑 127.0.0.1:0):
    /// 返回 (端点, 本端回环地址)。白名单 = 对端。
    fn dual_ep(
        me: &DeviceIdentity,
        peer_id: &str,
        peer_pk: &VerifyingKey,
    ) -> (quinn::Endpoint, SocketAddr) {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let allowed: HashMap<String, VerifyingKey> =
            [(peer_id.to_string(), *peer_pk)].into_iter().collect();
        let epn = mtdss_core::transport::endpoint::make_dual_endpoint_tuned(
            sock,
            me,
            &allowed,
            mtdss_core::transport::endpoint::TransportTuning::default(),
        )
        .unwrap();
        let addr = epn.local_addr().unwrap();
        (epn, addr)
    }

    /// 回环真握手取一条连接句柄(relaymem::loopback_pair 同款房风:B 服务
    /// 端白名单 A,A pin B 公钥,客户端侧即注桩交付面)。
    async fn loopback_conn() -> quinn::Connection {
        use mtdss_core::transport::endpoint::{
            accept_pinned, connect_pinned, make_endpoint, make_server_endpoint,
        };
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        let allowed: HashMap<String, VerifyingKey> =
            [(a.device_id.clone(), a.public_key())].into_iter().collect();
        let ep_b =
            make_server_endpoint(SocketAddr::from(([127, 0, 0, 1], 0)), &b, &allowed).unwrap();
        let addr = ep_b.local_addr().unwrap();
        let ep_a = make_endpoint(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let server = tokio::spawn(async move { accept_pinned(&ep_b, &allowed).await.unwrap().0 });
        let client = connect_pinned(&ep_a, addr, &b.public_key(), &a).await.unwrap();
        server.await.unwrap();
        client
    }

    /// 注桩拨号(决定性结果 + poll 旗):futures 惰性——构造不拨,首 poll
    /// 才落旗/结算(跳段路径零 poll 即零拨号的断言证据面)。
    fn stub_attempt(
        outcome: anyhow::Result<quinn::Connection>,
    ) -> (
        impl Future<Output = anyhow::Result<quinn::Connection>> + Send,
        Arc<AtomicBool>,
    ) {
        let polled = Arc::new(AtomicBool::new(false));
        let flag = polled.clone();
        (
            async move {
                flag.store(true, Ordering::SeqCst);
                outcome
            },
            polled,
        )
    }

    /// armed 记忆速构:单次穷尽即 armed(阈值 1,relaymem 契约);键随
    /// 调用方(choreo 用例键 = 对端 device_id,非字面量)。
    fn armed_mem(peer: &str) -> PunchMemory {
        let mem = PunchMemory::default();
        mem.note_exhaust(peer);
        mem
    }

    // ---------- V3-6 Task 8:并行预备(spec §4.2)单测台 ----------

    /// 既有用例的惰性预备计时(> 一切拨号预算:赛跑永不点火,行为与旧径
    /// 逐字节);预备用例注入短值。
    const INERT_HEDGE: Duration = Duration::from_millis(3_600_000);

    /// 中继分配计数(生产 allocate_only 每 call 恰 +1):预备/串行的分配
    /// 次数证据面(「预备恰一次,穷尽后无第二次分配」/「预备被拒 → 串行
    /// 再分恰第二次」)。choreo 用例经 [`CHOREO_GATE`] 串行,增量断言精确。
    pub(crate) static ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);

    /// 注桩拨号(延迟版):先真等 `delay`(预备计时的确定性时序锚——比
    /// 注入计时长的延迟保证计时臂先到)再落旗/结算。futures 惰性同
    /// [`stub_attempt`]:构造不拨,首 poll 起计时。
    fn stub_attempt_after(
        outcome: anyhow::Result<quinn::Connection>,
        delay: Duration,
    ) -> (
        impl Future<Output = anyhow::Result<quinn::Connection>> + Send,
        Arc<AtomicBool>,
    ) {
        let polled = Arc::new(AtomicBool::new(false));
        let flag = polled.clone();
        (
            async move {
                tokio::time::sleep(delay).await;
                flag.store(true, Ordering::SeqCst);
                outcome
            },
            polled,
        )
    }

    /// T1 armed 跳段判据真值表(spec §4.3 角色差异):注错轮恒 false
    /// (armed 亦不跳——记忆面注错轮不读不写);发起方 armed∧资格 → 跳,
    /// 资格缺 → 不跳(维持现状);响应方 armed → 跳(资格无关——等推窗
    /// 吸收对端穷尽窗);未 armed 恒不跳;None 记忆恒不跳。
    #[test]
    fn armed_skip_truth_table() {
        let mem = armed_mem("peer");
        let fresh = PunchMemory::default();
        let ent = true;
        // 发起方:armed ∧ 资格在手 → 跳
        assert!(
            armed_skip(false, Some(&mem), "peer", ent, FallbackRole::Initiator),
            "发起方 armed ∧ 资格在手 → 跳直连段"
        );
        // 发起方:资格缺 → 维持现状(不跳,直连穷尽后失败)
        assert!(
            !armed_skip(false, Some(&mem), "peer", false, FallbackRole::Initiator),
            "发起方资格缺不得跳段(spec §4.3 维持现状)"
        );
        // 响应方:armed 即跳(资格无关)
        assert!(
            armed_skip(false, Some(&mem), "peer", false, FallbackRole::Responder),
            "响应方 armed 跳段无资格闸"
        );
        // 未 armed / None 记忆:恒不跳
        assert!(
            !armed_skip(false, Some(&fresh), "peer", ent, FallbackRole::Initiator),
            "未 armed 不跳"
        );
        assert!(
            !armed_skip(false, None, "peer", ent, FallbackRole::Initiator),
            "None 记忆面不跳(旧径逐字节)"
        );
        // 注错轮:armed 在册也不跳(记忆面零触)
        assert!(
            !armed_skip(true, Some(&mem), "peer", ent, FallbackRole::Initiator),
            "force_relay 恒不跳(不 consult 记忆)"
        );
    }

    /// T2 直连段「拨与记」(注桩零真拨号):Ok 清账(armed → 撤);Err
    /// 记账(任一次穷尽即 armed,阈值 1);None 纯拨号零记账(无 panic);
    /// 注桩未被 poll 即未拨(旗为证)。
    #[tokio::test]
    async fn direct_segment_notes_memory() {
        let mem = armed_mem("peer");
        // Ok 清账
        let conn = loopback_conn().await;
        let (attempt, polled) = stub_attempt(Ok(conn));
        let got = direct_segment(Some(&mem), "peer", attempt)
            .await
            .expect("拨号 Ok 即直连");
        assert!(polled.load(Ordering::SeqCst), "注桩被 poll(段内真拨)");
        assert!(!mem.is_armed("peer"), "直连 Ok 必清账(armed 撤销)");
        drop(got);
        // Err 记账:fresh mem 单次穷尽即 armed
        let fresh = PunchMemory::default();
        let (attempt, polled) = stub_attempt(Err(anyhow::anyhow!("stub 拨号穷尽")));
        let e = direct_segment(Some(&fresh), "peer", attempt)
            .await
            .expect_err("拨号 Err 即穷尽上抛");
        assert!(polled.load(Ordering::SeqCst));
        assert!(fresh.is_armed("peer"), "任一次穷尽即记账(armed 阈值 1): {e}");
        // None 记忆面:两臂零记账无 panic
        let (attempt, polled) = stub_attempt(Err(anyhow::anyhow!("stub 拨号穷尽")));
        assert!(direct_segment(None, "peer", attempt).await.is_err());
        assert!(polled.load(Ordering::SeqCst));
    }

    /// T3 反转兜底(spec §4.3):跳段轮 RelayFailed → 补拨(注桩):成 →
    /// 直连返场 + 清账;败 → 原 RelayFailed 原样 + 记忆保持 armed。已拨轮
    /// (非跳段)不二拨(注桩零 poll);RelayDenied 终局不反转(零 poll)。
    #[tokio::test]
    async fn reversal_punch_semantics() {
        // 原兜底错误速构(anyhow 不可克隆,逐次新造同文案)
        let orig = || FallbackErr::RelayFailed("中继分配失败: stub".into());
        let mem = armed_mem("peer");
        // 成:直连返场 + 清账(连接即注桩交付的那条)
        let conn = loopback_conn().await;
        let sid = conn.stable_id();
        let (attempt, polled) = stub_attempt(Ok(conn));
        let out = reversal_punch(orig(), true, Some(&mem), "peer", attempt)
            .await
            .expect("反转补拨成 → 直连返场");
        assert!(polled.load(Ordering::SeqCst), "跳段轮必须补拨");
        assert!(out.relay.is_none(), "返场是直连(非中继)");
        assert_eq!(out.conn.stable_id(), sid, "返场连接 = 注桩交付连接");
        assert!(!mem.is_armed("peer"), "反转直连成必清账");
        // 败:原错误原样 + 记忆保持 armed
        let mem2 = armed_mem("peer");
        let (attempt, polled) = stub_attempt(Err(anyhow::anyhow!("stub 补拨穷尽")));
        let e = reversal_punch(orig(), true, Some(&mem2), "peer", attempt)
            .await
            .expect_err("补拨败 → 原兜底错误上抛");
        assert!(polled.load(Ordering::SeqCst));
        let FallbackErr::RelayFailed(m) = &e else {
            panic!("须保持 RelayFailed: {e:?}")
        };
        assert_eq!(m, "中继分配失败: stub", "原错误原样(不得被补拨错误遮蔽)");
        assert!(mem2.is_armed("peer"), "补拨败记忆保持 armed(下一轮中继优先)");
        // 已拨轮(非跳段):不二拨
        let mem3 = armed_mem("peer");
        let (attempt, polled) = stub_attempt(Ok(loopback_conn().await));
        let e = reversal_punch(orig(), false, Some(&mem3), "peer", attempt)
            .await
            .expect_err("已拨轮不得二拨");
        assert!(!polled.load(Ordering::SeqCst), "非跳段轮零补拨");
        assert!(matches!(e, FallbackErr::RelayFailed(_)));
        // RelayDenied 终局:不反转
        let mem4 = armed_mem("peer");
        let (attempt, polled) = stub_attempt(Ok(loopback_conn().await));
        let e = reversal_punch(
            FallbackErr::RelayDenied { code: 4 },
            true,
            Some(&mem4),
            "peer",
            attempt,
        )
        .await
        .expect_err("终局拒绝不反转");
        assert!(!polled.load(Ordering::SeqCst), "终局拒绝零补拨");
        assert!(matches!(e, FallbackErr::RelayDenied { code: 4 }));
    }

    /// T4a armed 等推窗 env 映射三臂(relaymem 房风,get 注入缝):缺省 =
    /// 钉死默认 45s;合法值毫秒换算;非法值 warn 保持默认。
    #[test]
    fn armed_push_wait_env_mapping() {
        let env =
            |v: &'static str| move |k: &str| (k == "MTDSS_TEST_ARMED_PUSH_WAIT_MS").then(|| v.to_string());
        assert_eq!(armed_push_wait_from(|_| None), ARMED_PUSH_WAIT, "缺省 = 钉死默认");
        assert_eq!(
            armed_push_wait_from(env("100")),
            Duration::from_millis(100),
            "合法值毫秒换算"
        );
        assert_eq!(
            armed_push_wait_from(env("abc")),
            ARMED_PUSH_WAIT,
            "非法值 warn 保持默认"
        );
        assert_eq!(armed_push_wait_from(env("")), ARMED_PUSH_WAIT, "空串非法保持默认");
    }

    /// T4b 常量值锚(spec §4.3:5s→45s)+ 窗选择:armed 轮取注入窗,非
    /// armed 恒 5s 现状(注入短值免真等,确定性)。
    #[test]
    fn responder_window_selection() {
        assert_eq!(ARMED_PUSH_WAIT, Duration::from_secs(45), "armed 等推窗锚 45s");
        assert_eq!(RELAY_PUSH_WAIT, Duration::from_secs(5), "非 armed 等推窗现状 5s");
        let short = Duration::from_millis(80);
        assert_eq!(responder_window(true, short), short, "armed 轮取 armed 窗");
        assert_eq!(
            responder_window(false, short),
            RELAY_PUSH_WAIT,
            "非 armed 轮恒 5s 现状(注入值不得泄漏)"
        );
    }

    /// T4c 等推窗注入生效:空通道 + 100ms 窗 → 100ms 量级超时 RelayFailed
    /// (1s 外界钉死不悬等——窗参数真实下传,45s/5s 硬编码即在此显形)。
    #[tokio::test]
    async fn wait_relay_push_honors_window() {
        let (_tx, mut rx) = tokio::sync::broadcast::channel::<RelayEvt>(8);
        let start = std::time::Instant::now();
        let r = tokio::time::timeout(
            Duration::from_secs(1),
            wait_relay_push(&mut rx, "peer", "transport", Duration::from_millis(100)),
        )
        .await
        .expect("等推不得悬等超 1s 外界(窗注入生效)");
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(90),
            "不得早于窗提前返回: {elapsed:?}"
        );
        let e = r.expect_err("空通道必超窗失败");
        let FallbackErr::RelayFailed(m) = e else {
            panic!("超窗须为可重试 RelayFailed: {e:?}")
        };
        assert!(
            m.contains("等待中继分配推送超时"),
            "超窗文案钉死: {m}"
        );
    }

    /// choreo 速构:离线 hub(无代际——资格经 `entitled` 注入,分配恒
    /// RelayFailed「中继分配失败」)+ 广播对。
    fn choreo_rx() -> (
        tokio::sync::broadcast::Sender<RelayEvt>,
        tokio::sync::broadcast::Receiver<RelayEvt>,
    ) {
        tokio::sync::broadcast::channel(8)
    }

    /// T5 armed 发起方:零直连拨号直进分配——分配离线败 RelayFailed 反转
    /// 补拨(黑洞预算快败)→ 原错误原样上抛,记忆保持 armed;直连拨号恰
    /// 一次(反转;跳段意味着拨号发生在分配之后,计数 1 = 反转独占)。
    #[tokio::test]
    async fn armed_initiator_zero_punch_goes_allocate() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = armed_mem(&peer_id);
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let hole = [black_hole()];
        let cfg = test_punch(Duration::from_millis(400));
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            true, &hub, &mut rx, &me, &peer_id, &pk, &epn, &hole, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, false, Some(&mem), INERT_HEDGE,
        )
        .await;
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        let e = out.expect_err("分配离线败 + 反转补拨败 = RelayFailed 上抛");
        let FallbackErr::RelayFailed(m) = &e else {
            panic!("须为可重试 RelayFailed: {e:?}")
        };
        assert!(
            m.contains("中继分配失败"),
            "分配段必已进入(跳段直分配的铁证): {m}"
        );
        assert_eq!(delta, 1, "直连拨号恰一次(反转独占;跳段 = 分配前零拨): {delta}");
        assert!(mem.is_armed(&peer_id), "补拨败记忆保持 armed");
    }

    /// T6 armed 发起方反转成:分配离线败 → 反转补拨对真实回环对端(对端
    /// 跑真 punch 舞伴互拨)→ 直连返场 + 清账。
    #[tokio::test]
    async fn armed_initiator_reversal_punch_ok_goes_direct() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let me_id = me.device_id.clone();
        let (pk_a, pk_b) = (me.public_key(), peer.public_key());
        let (ep_a, addr_a) = dual_ep(&me, &peer_id, &pk_b);
        let (ep_b, addr_b) = dual_ep(&peer, &me_id, &pk_a);
        let mem = armed_mem(&peer_id);
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let cfg = test_punch(Duration::from_secs(3));
        // 对端舞伴:真 punch 互拨(与发起方的反转补拨相配成对;候选绑定
        // 局部变量——join! 的 future 借用不得踩语句级临时)
        let b_allowed: HashMap<String, VerifyingKey> =
            [(me_id.clone(), pk_a)].into_iter().collect();
        let cands_a = [addr_a];
        let cands_b = [addr_b];
        let b_side = punch::punch_connect(&ep_b, &peer, &b_allowed, &pk_a, &cands_a, &cfg);
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let (out, b_conn) = tokio::join!(
            fallback_round(
                true, &hub, &mut rx, &me, &peer_id, &pk_b, &ep_a, &cands_b, cfg,
                FallbackRole::Initiator, RelayPurpose::Transport, false, Some(&mem), INERT_HEDGE,
            ),
            b_side,
        );
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        b_conn.expect("对端舞伴互拨必成(与反转补拨相配)");
        let out = out.expect("反转补拨成 → 直连返场");
        assert!(out.relay.is_none(), "返场是直连(非中继)");
        assert_eq!(delta, 1, "直连拨号恰一次(反转): {delta}");
        assert!(!mem.is_armed(&peer_id), "反转直连成必清账");
    }

    /// T7 armed 响应方:跳互拨直等推——预缓冲推送立即命中(窗值无关)，
    /// 中继互拨(指向丢弃端口)败后反转补拨(空候选预算快败)→ 原错误
    /// 原样上抛;资格不在手亦跳(响应方跳段无资格闸,spec §4.3)。
    #[tokio::test]
    async fn armed_responder_skips_punch_and_consumes_push() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = armed_mem(&peer_id);
        let hub = SignalHub::new();
        let (tx, mut rx) = choreo_rx();
        // 预缓冲本对端本平面的分配推送:跳段轮等推立即命中(不经窗超时)
        let _ = tx.send(push(&peer_id, 7, "transport"));
        let empty: [SocketAddr; 0] = [];
        let cfg = test_punch(Duration::from_millis(400));
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            false, &hub, &mut rx, &me, &peer_id, &pk, &epn, &empty, cfg,
            FallbackRole::Responder, RelayPurpose::Transport, false, Some(&mem), INERT_HEDGE,
        )
        .await;
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        let e = out.expect_err("中继互拨败 + 反转补拨败 = RelayFailed 上抛");
        let FallbackErr::RelayFailed(m) = &e else {
            panic!("须为可重试 RelayFailed: {e:?}")
        };
        assert!(
            m.contains("中继互拨失败"),
            "推送必已命中、中继互拨段必已进入(跳段等推的铁证): {m}"
        );
        assert!(
            !m.contains("等待中继分配推送超时"),
            "预缓冲推送不得等窗超时: {m}"
        );
        assert_eq!(delta, 1, "直连拨号恰一次(反转;跳段互拨零拨): {delta}");
        assert!(mem.is_armed(&peer_id), "补拨败记忆保持 armed");
    }

    /// T8 未 armed 响应方:互拨穷尽(空候选预算快败)→ 资格缺 → Direct
    /// 原错(现状);记账发生(fresh mem 单次穷尽即 armed)。
    #[tokio::test]
    async fn non_armed_responder_exhaust_no_entitlement_is_direct() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = PunchMemory::default();
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let empty: [SocketAddr; 0] = [];
        let cfg = test_punch(Duration::from_millis(400));
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            false, &hub, &mut rx, &me, &peer_id, &pk, &epn, &empty, cfg,
            FallbackRole::Responder, RelayPurpose::Transport, false, Some(&mem), INERT_HEDGE,
        )
        .await;
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        let e = out.expect_err("穷尽 + 资格缺 = Direct 上抛(现状)");
        assert!(
            matches!(e, FallbackErr::Direct(_)),
            "资格缺的穷尽须为 Direct(现状口径): {e:?}"
        );
        assert_eq!(delta, 1, "恰一次互拨(非跳段;无反转二拨): {delta}");
        assert!(mem.is_armed(&peer_id), "穷尽必记账(armed 阈值 1)");
    }

    /// T9 注错轮(force_relay)记忆面零触:armed 在册仍不跳不反转——零
    /// 直连拨号、分配离线败 RelayFailed 原样、记忆保持原值。
    #[tokio::test]
    async fn force_relay_never_touches_memory() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = armed_mem(&peer_id);
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let hole = [black_hole()];
        let cfg = test_punch(Duration::from_millis(400));
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            true, &hub, &mut rx, &me, &peer_id, &pk, &epn, &hole, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, true, Some(&mem), INERT_HEDGE,
        )
        .await;
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        let e = out.expect_err("注错轮分配离线败 = RelayFailed 上抛(现状)");
        let FallbackErr::RelayFailed(m) = &e else {
            panic!("须为可重试 RelayFailed: {e:?}")
        };
        assert!(m.contains("中继分配失败"), "分配段进入(注错轮现状): {m}");
        assert_eq!(delta, 0, "注错轮零直连拨号(跳段且无反转): {delta}");
        assert!(mem.is_armed(&peer_id), "注错轮记忆面零触(值不变)");
    }

    /// T10 armed 发起方资格缺:不跳段(资格闸)→ 互拨穷尽 → Direct 原错
    /// (spec §4.3「资格不在手维持现状」);记账发生(已 armed,计数累进)。
    #[tokio::test]
    async fn armed_initiator_without_entitlement_still_punches() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = armed_mem(&peer_id);
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let empty: [SocketAddr; 0] = [];
        let cfg = test_punch(Duration::from_millis(400));
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            false, &hub, &mut rx, &me, &peer_id, &pk, &epn, &empty, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, false, Some(&mem), INERT_HEDGE,
        )
        .await;
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        let e = out.expect_err("穷尽 + 资格缺 = Direct 上抛(维持现状)");
        assert!(
            matches!(e, FallbackErr::Direct(_)),
            "资格缺不得跳段、不得中继优先: {e:?}"
        );
        assert_eq!(delta, 1, "资格缺照常互拨(现状): {delta}");
        assert!(mem.is_armed(&peer_id), "记账照常(armed 保持)");
    }

    /// T11 None 记忆面直驱(行为零改,字节口径):发起方穷尽 + 资格缺 →
    /// Direct 原错,零记账零反转(None 面不触任何记忆路径)。
    #[tokio::test]
    async fn none_mem_initiator_byte_identical_direct() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let empty: [SocketAddr; 0] = [];
        let cfg = test_punch(Duration::from_millis(400));
        let before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            false, &hub, &mut rx, &me, &peer_id, &pk, &epn, &empty, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, false, None, INERT_HEDGE,
        )
        .await;
        let delta = PUNCH_CALLS.load(Ordering::SeqCst) - before;
        let e = out.expect_err("穷尽 + 资格缺 = Direct 上抛(旧径)");
        assert!(matches!(e, FallbackErr::Direct(_)), "None 面旧径逐字节: {e:?}");
        assert_eq!(delta, 1, "恰一次互拨(None 面无反转面): {delta}");
    }

    /// T12 预备计时 env 映射三臂(relaymem 房风,get 注入缝):缺省 = 钉死
    /// 默认 3s;合法值毫秒换算;非法/空值 warn 保持默认。常量锚 3s 一并钉。
    #[test]
    fn hedge_delay_env_mapping() {
        let env =
            |v: &'static str| move |k: &str| (k == "MTDSS_TEST_HEDGE_DELAY_MS").then(|| v.to_string());
        assert_eq!(HEDGE_DELAY, Duration::from_secs(3), "预备计时锚 3s(spec §4.2)");
        assert_eq!(hedge_delay_from(|_| None), HEDGE_DELAY, "缺省 = 钉死默认");
        assert_eq!(
            hedge_delay_from(env("1500")),
            Duration::from_millis(1500),
            "合法值毫秒换算"
        );
        assert_eq!(hedge_delay_from(env("abc")), HEDGE_DELAY, "非法值 warn 保持默认");
        assert_eq!(hedge_delay_from(env("")), HEDGE_DELAY, "空串非法保持默认");
    }

    /// T13 预备收口判别真值表(纯函数):胜出/轮亡恒弃单(互拨成亦然——
    /// 轮已不在场,结果无人接收);穷尽 + 互拨成 = 交付即用不释放;穷尽 +
    /// 互拨败 = 弃单后交付(滞留会话放大拒 6,不放大失败是 §4.2 配额纪律)。
    #[test]
    fn hedged_disposition_truth_table() {
        let win = |dial_ok: bool| hedged_disposition(Ok(true), dial_ok);
        let exhaust = |dial_ok: bool| hedged_disposition(Ok(false), dial_ok);
        assert!(win(true), "胜出恒弃单(互拨成亦然)");
        assert!(win(false), "胜出恒弃单");
        assert!(!exhaust(true), "穷尽+互拨成:交付即用不释放");
        assert!(
            exhaust(false),
            "穷尽+互拨败:弃单后交付(滞留放大拒 6)"
        );
    }

    /// T13b 轮亡臂(RecvError 不可外构——真信道断取证):发送端先亡,
    /// 接收端 await 得 Err(RecvError) → 恒弃单。
    #[tokio::test]
    async fn hedged_disposition_round_gone_releases() {
        let (tx, rx) = oneshot::channel::<bool>();
        drop(tx);
        let closed = rx.await.expect_err("发送端已亡必 Err");
        assert!(hedged_disposition(Err(closed), true), "轮亡恒弃单(无人接收结果)");
        let (tx, rx) = oneshot::channel::<bool>();
        drop(tx);
        let closed = rx.await.expect_err("发送端已亡必 Err");
        assert!(hedged_disposition(Err(closed), false), "轮亡恒弃单");
    }

    /// T14 赛跑·胜出臂(注桩延迟 200ms > 注入计时 50ms):预备计时先到
    /// (预备臂在场)→ 直连段跑到毕 Ok 清账返场 + 胜出裁决 `true` 发出
    /// (预备任务据此自弃单)。会票不随返场(裁决已发,结果面归预备任务)。
    #[tokio::test]
    async fn hedged_direct_win_after_hedge_sends_verdict_true() {
        let mem = armed_mem("peer");
        let conn = loopback_conn().await;
        let sid = conn.stable_id();
        let (attempt, polled) = stub_attempt_after(Ok(conn), Duration::from_millis(200));
        let (_result_tx, result_rx) = oneshot::channel();
        let (verdict_tx, verdict_rx) = oneshot::channel();
        let ticket = HedgeTicket { result_rx, verdict_tx };
        let (res, ticket) = hedged_direct_segment(
            Some(&mem),
            "peer",
            attempt,
            Duration::from_millis(50),
            || ticket,
        )
        .await;
        let conn = res.expect("直连胜出返场");
        assert_eq!(conn.stable_id(), sid, "返场连接 = 注桩交付连接");
        assert!(ticket.is_none(), "胜出侧会票不随返场(裁决已发,收口归预备任务)");
        assert_eq!(verdict_rx.await, Ok(true), "胜出裁决 true(预备任务自弃单)");
        assert!(polled.load(Ordering::SeqCst), "注桩被 poll(赛跑版真拨)");
        assert!(!mem.is_armed("peer"), "直连 Ok 必清账(armed 撤销)");
        // (result 发送端留活至用例尾:胜出侧结果面归预备任务,接收端不入断言)
    }
    /// T15 赛跑·穷尽臂(注桩穷尽时预备在场)+ 穷尽侧收口:预备计时 50ms
    /// 先到 → 注桩 200ms 后穷尽 → 会票随返;预备结果预载(分配+互拨已毕)
    /// 即用——穷尽后零等待零再分配(instant switch),用的是预备会话;
    /// 直连穷尽记账发生。预备败(滞留 sid)/失联(信道断)→ None 落回
    /// 串行兜底(滞留经 hub 释放面,离线 hub = noop 无 panic)。
    #[tokio::test]
    async fn hedged_direct_exhaust_resolves_ticket_instant_switch() {
        let mem = PunchMemory::default();
        let (attempt, polled) =
            stub_attempt_after(Err(anyhow::anyhow!("stub 拨号穷尽")), Duration::from_millis(200));
        let (result_tx, result_rx) = oneshot::channel();
        let (verdict_tx, _verdict_rx) = oneshot::channel();
        let ticket = HedgeTicket { result_rx, verdict_tx };
        // 预载预备结果(已毕):穷尽即用,无第二次分配
        let conn = loopback_conn().await;
        result_tx
            .send(HedgedOutcome {
                outcome: Ok(ConnectOutcome {
                    conn,
                    relay: Some(RelayPath { sid: 42, rate_kbps: 7 }),
                }),
                stranded_sid: None,
            })
            .expect("预备结果预载");
        let (res, ticket) = hedged_direct_segment(
            Some(&mem),
            "peer",
            attempt,
            Duration::from_millis(50),
            || ticket,
        )
        .await;
        assert!(res.is_err(), "直连穷尽上抛");
        assert!(polled.load(Ordering::SeqCst));
        assert!(mem.is_armed("peer"), "穷尽必记账(armed 阈值 1)");
        // 穷尽侧收口:预备已成即切(hub 离线不影响——本例无滞留释放面)
        let hub = SignalHub::new();
        let out = ticket
            .expect("穷尽侧会票随返")
            .resolve_for_exhaust(&hub)
            .await
            .expect("预备已成即切(无串行段)");
        assert_eq!(
            out.relay.expect("预备会话承载体").sid,
            42,
            "即切用的是预备会话(非重分配)"
        );
    }

    /// T15b 穷尽侧收口·预备败两臂:互拨败带滞留 sid → hub 释放滞留会话
    /// (离线 hub = noop 无 panic)→ None 落回串行;预备任务失联(信道断)
    /// → 同落回。
    #[tokio::test]
    async fn hedged_resolve_failure_falls_back_serial() {
        let hub = SignalHub::new();
        // 滞留交付:outcome Err + 滞留 sid
        let (result_tx, result_rx) = oneshot::channel();
        let (verdict_tx, _verdict_rx) = oneshot::channel();
        result_tx
            .send(HedgedOutcome {
                outcome: Err(FallbackErr::RelayFailed("中继互拨失败: stub".into())),
                stranded_sid: Some(43),
            })
            .expect("滞留预备结果预载");
        let out = HedgeTicket { result_rx, verdict_tx }.resolve_for_exhaust(&hub).await;
        assert!(out.is_none(), "预备败落回串行兜底");
        // 失联:预备任务失约(结果发送端亡;裁决发送端尚在轮侧,send
        // 对亡接收端静默失败即收口容忍面)
        let (result_tx, result_rx) = oneshot::channel::<HedgedOutcome>();
        let (verdict_tx, _verdict_rx2) = oneshot::channel::<bool>();
        drop(result_tx);
        let out = HedgeTicket { result_rx, verdict_tx }.resolve_for_exhaust(&hub).await;
        assert!(out.is_none(), "预备失联落回串行兜底");
    }

    /// T16 choreo·预备被拒链(黑洞拨号预算 400ms、预备计时 50ms、离线
    /// hub):计时先到预备即启(分配恰第一次,离线即拒),穷尽后预备败静默
    /// 消费落回既有串行兜底(分配恰第二次,同样离线拒)——预备失败不放大
    /// 失败面:终态 RelayFailed 与未预备轮同文案;非跳段轮无反转二拨;记账
    /// 照常。
    #[tokio::test]
    async fn hedged_round_exhaust_hedged_rejected_then_serial() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = PunchMemory::default();
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let hole = [black_hole()];
        let cfg = test_punch(Duration::from_millis(400));
        let punch_before = PUNCH_CALLS.load(Ordering::SeqCst);
        let alloc_before = ALLOC_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            true, &hub, &mut rx, &me, &peer_id, &pk, &epn, &hole, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, false, Some(&mem),
            Duration::from_millis(50),
        )
        .await;
        let punch_delta = PUNCH_CALLS.load(Ordering::SeqCst) - punch_before;
        let alloc_delta = ALLOC_CALLS.load(Ordering::SeqCst) - alloc_before;
        let e = out.expect_err("预备败 + 串行败 = RelayFailed 上抛");
        let FallbackErr::RelayFailed(m) = &e else {
            panic!("须为可重试 RelayFailed: {e:?}")
        };
        assert!(m.contains("中继分配失败"), "终态走串行兜底(预备败不放大文案): {m}");
        assert_eq!(punch_delta, 1, "直连拨号恰一次(穷尽,无反转二拨): {punch_delta}");
        assert_eq!(alloc_delta, 2, "分配恰两次(预备 1 + 穷尽后串行 1): {alloc_delta}");
        assert!(mem.is_armed(&peer_id), "穷尽必记账(armed 阈值 1)");
    }

    /// T17 choreo·快败 < 预备计时 → 无预备任务(预备分配 0 次),穷尽即串
    /// 行兜底(现状语义零改);资格不在手 → 连串行兜底也无(直连原错
    /// Direct,零分配零预备)。
    #[tokio::test]
    async fn hedged_round_fast_fail_and_no_entitlement_no_hedge() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let hole = [black_hole()];
        // 拨号预算 80ms << 预备计时 400ms:穷尽先至,赛跑永不点火
        let cfg = test_punch(Duration::from_millis(80));
        // 资格在手:穷尽 → 串行兜底(分配恰 1 次,离线拒;预备若点火则为 2)
        let alloc_before = ALLOC_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            true, &hub, &mut rx, &me, &peer_id, &pk, &epn, &hole, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, false, None,
            Duration::from_millis(400),
        )
        .await;
        let e = out.expect_err("穷尽 + 串行离线拒 = RelayFailed");
        assert!(matches!(e, FallbackErr::RelayFailed(_)), "{e:?}");
        assert_eq!(
            ALLOC_CALLS.load(Ordering::SeqCst) - alloc_before,
            1,
            "分配恰一次(纯串行;预备任务 0 次)"
        );
        // 资格缺:穷尽 → Direct 原错(零分配零预备,现状)
        let mem = PunchMemory::default();
        let alloc_before = ALLOC_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            false, &hub, &mut rx, &me, &peer_id, &pk, &epn, &hole, cfg,
            FallbackRole::Initiator, RelayPurpose::Transport, false, Some(&mem),
            Duration::from_millis(400),
        )
        .await;
        let e = out.expect_err("资格缺的穷尽须为 Direct(现状口径)");
        assert!(matches!(e, FallbackErr::Direct(_)), "资格缺不预备不兜底: {e:?}");
        assert_eq!(ALLOC_CALLS.load(Ordering::SeqCst) - alloc_before, 0, "资格缺零分配(无预备)");
        assert!(mem.is_armed(&peer_id), "穷尽记账照常");
    }

    /// T17b ctrl 面不预备(fix round 1:预备门加 Transport 平面判别——
    /// spec §4.2 适用面 = 未 armed **传输轮**,ctrl/浏览「配额面零改」红线:
    /// 浏览不因预备占中继槽):同 T16 形态(黑洞 400ms、计时 50ms、资格在
    /// 手、记忆面在场)但 purpose=Ctrl → 赛跑永不点火,穷尽即串行兜底
    /// (分配恰一次;预备若点火则为 2)。
    #[tokio::test]
    async fn hedged_round_ctrl_purpose_never_hedges() {
        let _g = CHOREO_GATE.lock().await;
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let peer_id = peer.device_id.clone();
        let pk = peer.public_key();
        let (epn, _addr) = dual_ep(&me, &peer_id, &pk);
        let mem = PunchMemory::default();
        let hub = SignalHub::new();
        let (_tx, mut rx) = choreo_rx();
        let hole = [black_hole()];
        let cfg = test_punch(Duration::from_millis(400));
        let alloc_before = ALLOC_CALLS.load(Ordering::SeqCst);
        let punch_before = PUNCH_CALLS.load(Ordering::SeqCst);
        let out = fallback_round(
            true, &hub, &mut rx, &me, &peer_id, &pk, &epn, &hole, cfg,
            FallbackRole::Initiator, RelayPurpose::Ctrl, false, Some(&mem),
            Duration::from_millis(50),
        )
        .await;
        let alloc_delta = ALLOC_CALLS.load(Ordering::SeqCst) - alloc_before;
        let punch_delta = PUNCH_CALLS.load(Ordering::SeqCst) - punch_before;
        let e = out.expect_err("穷尽 + 串行离线拒 = RelayFailed");
        assert!(matches!(e, FallbackErr::RelayFailed(_)), "{e:?}");
        assert_eq!(alloc_delta, 1, "分配恰一次(纯串行;ctrl 面预备任务 0 次): {alloc_delta}");
        assert_eq!(punch_delta, 1, "直连拨号恰一次(无反转二拨): {punch_delta}");
        // 记账面与预备面正交:Some 面穷尽照常记账(ctrl 生产传 None 零记账,
        // 「记忆仅传输面」红线在调用面,不在记账门)
        assert!(mem.is_armed(&peer_id), "穷尽记账照常");
    }

    /// T18 预备任务体·离线分配三收口(spawn 直驱,无真信令):穷尽交付
    /// (Err 送达,离线文案与串行兜底同形「信令未在线」)、胜出静默(不得
    /// 交付)、轮亡静默——均干净收口无 panic 无悬等。经 CHOREO_GATE 串行:
    /// 任务体走 allocate_only(ALLOC_CALLS 共享计数),T16/T17 的增量断言
    /// 不得被并行任务互染。
    #[tokio::test]
    async fn hedge_task_offline_dispositions_clean() {
        let _g = CHOREO_GATE.lock().await;
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let me = DeviceIdentity::generate();
        let peer = DeviceIdentity::generate();
        let seed = me.signing.to_bytes();
        let pk = peer.public_key();
        let cfg = test_punch(Duration::from_secs(1));
        // 穷尽交付:false 裁决 → Err(离线文案)送达,无滞留会话
        let (result_tx, result_rx) = oneshot::channel();
        let (verdict_tx, verdict_rx) = oneshot::channel();
        let task = tokio::spawn(hedge_task(
            None, lock.clone(), seed, peer.device_id.clone(), pk,
            HashMap::new(), cfg, RelayPurpose::Transport, result_tx, verdict_rx,
        ));
        verdict_tx.send(false).expect("轮在场,裁决必达");
        let got = tokio::time::timeout(Duration::from_secs(2), result_rx)
            .await
            .expect("预备任务不得悬等")
            .expect("结果送达");
        let FallbackErr::RelayFailed(m) = got.outcome.expect_err("离线分配必败") else {
            panic!("须为可重试 RelayFailed");
        };
        assert!(m.contains("信令未在线"), "离线文案与串行兜底同形: {m}");
        assert_eq!(got.stranded_sid, None, "分配被拒无滞留会话");
        task.await.expect("预备任务干净收口");
        // 胜出静默:true 裁决 → 不交付(信道随任务收口关闭),干净收口
        let (result_tx, result_rx) = oneshot::channel();
        let (verdict_tx, verdict_rx) = oneshot::channel();
        let task = tokio::spawn(hedge_task(
            None, lock.clone(), seed, peer.device_id.clone(), pk,
            HashMap::new(), cfg, RelayPurpose::Transport, result_tx, verdict_rx,
        ));
        verdict_tx.send(true).expect("裁决发送(任务在场)");
        let got = tokio::time::timeout(Duration::from_secs(2), result_rx)
            .await
            .expect("预备任务须收口(不悬等)");
        assert!(got.is_err(), "胜出侧不得交付(信道随任务收口关闭)");
        task.await.expect("预备任务干净收口");
        // 轮亡:裁决发送端先亡 → 任务静默收口(无会话,无释放面)
        let (result_tx, _result_rx) = oneshot::channel();
        let (verdict_tx, verdict_rx) = oneshot::channel();
        drop(verdict_tx);
        let task = tokio::spawn(hedge_task(
            None, lock, seed, peer.device_id.clone(), pk,
            HashMap::new(), cfg, RelayPurpose::Transport, result_tx, verdict_rx,
        ));
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("轮亡收口不悬等")
            .expect("干净收口");
    }
}
