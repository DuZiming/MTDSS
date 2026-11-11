//! 预热器(warmer,V3-5 Stage A,spec §3.1(2)):登录/回前台后对全部在线
//! 设备常驻预热 ctrl 直连,断连退避重打 + 冷却止损。
//!
//! 触发三类(spec §3.1(2)):**代际建立**(登录 / 断线重连 / 手机回前台后
//! WS 重连——信令新代际安装 + 目录快照就绪,经 `event.devices`/
//! `event.online` 广播感知,进程恢复→WS 重连→新代际→自动预热,移动端
//! 无需任何平台代码)、**presence 翻转**(对端上线补入目标)、**断连
//! 重打**(仅主动侧:池内连接 `closed()` → 退避重打;连续失败进冷却窗
//! ——对称 NAT 等永远打不通的对端靠冷却止损,浏览时按现状按需打洞 +
//! 中继兜底,正确性不依赖预热)。
//!
//! **单侧主动,双侧入池**(spec §3.1(3)):预热主动侧由公钥**字典序**
//! 破缺(小者主动,与互拨角色同款对称推导,两端一致不会双连);预热
//! 编舞 direct-only([`crate::ctrl::CtrlPool::ensure_warm`] →
//! `dial_session(allow_relay=false)`,spec §3.1(4):打不通即失败进冷却,
//! 绝不烧服务端中继);被动侧由响应端入池承担(respond_session,T3),
//! 本模块不做被动侧重打。
//!
//! 结构:主循环([`warmer_loop`])登录 watch 挂起(同 responder_loop 房风)
//! → 订阅进程级事件广播(去抖合并,默认 750ms,env 可注入)→ 全量算
//! [`warm_targets`] → 每目标一个常驻 worker([`warm_device`]):预热 →
//! 成功挂 death watcher(池连接 `closed()` → mpsc 通知)→ 死亡通知经
//! [`should_rewrite`] 纯函数闸(未登录 / 已非 warm 目标即弃)→ 按该设备
//! [`WarmBackoff`] 退避后重打。per-device 状态在 warmer_loop 本地 HashMap
//! (worker 句柄即「在途」标记;退避态机随 worker 本地持有),不进
//! DaemonState。
//!
//! **探洞豁免与回切轮换(V3-6 T9,spec §4.4 治瘾机制)**:重打退避按活跃
//! 中继轮钳制([`clamped_reprobe_delay`]:轮在场 → 至多
//! [`crate::relaymem::PROBE_FLOOR`] 30s 一发,豁免 300s 冷却窗——中继当
//! 主线的对端保持探洞节奏,直连一通即知);预热成功(直连已入池)且该
//! 对端有活跃中继轮 → 过一发闸([`PunchMemory::take_rotation_shot`],
//! armed ∧ 本代际未发)→ [`rotation_switchback`]:清失败记忆 + drain 全部
//! 中继半连逐条应用层 close + 显式释放会话(RelayRelease 面双端 Dead 通
//! 告,对端在途轮即刻收口)——发送端既有重试轮落暖/直连(本端池内刚回切
//! 的直连;对端经 respond_session 被动入池),位图续传零重发。防振荡:一
//! 发/armed 代际,直连回切后再死须重走穷尽再 armed,绝不乒乓。中继轮登
//! 记的登记/清账面在 flow.rs 两轮守卫([`crate::flow`] RelayRoundGuard),
//! 记忆本体见 [`crate::relaymem::PunchMemory`]。

use crate::flow;
use crate::hub::{SignalHub, EV_DEVICES, EV_ONLINE};
use crate::relaymem::PunchMemory;
use crate::state::DaemonState;
use ed25519_dalek::VerifyingKey;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// 预热触发去抖默认窗:事件(event.devices/event.online)风暴折叠为一轮
/// 重算——登录后 event.online 与 event.devices 几乎同至,目录刷新与
/// presence 增量也各自广播;750ms 足以合并一次上线的全部触发源,又不至
/// 于拖慢「回前台即暖」的观感。常量是文档锚;进程内生效值经
/// [`warm_debounce`](env 注入缝)读定。
pub(crate) const WARM_DEBOUNCE: Duration = Duration::from_millis(750);

/// 去抖窗的进程内生效值(env 注入缝,`MTDSS_TEST_*` 家族口径,OnceLock
/// 一次读定):`MTDSS_TEST_WARM_DEBOUNCE_MS` = 毫秒值(e2e 注入小值)。
/// 缺失 → 钉死默认;非法(非数字/空串)→ warn 保持默认(沿
/// [`flow::apply_test_env`] 房风,与 ctrl::warm_decline_grace 同款)。
fn warm_debounce() -> Duration {
    static CACHE: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| warm_debounce_from(|k| std::env::var(k).ok()))
}

/// 去抖窗映射纯函数(`get` 注入缝,单测同款房风):见 [`warm_debounce`]。
fn warm_debounce_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    match get("MTDSS_TEST_WARM_DEBOUNCE_MS") {
        None => WARM_DEBOUNCE,
        Some(raw) => match raw.parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(_) => {
                tracing::warn!("ignored invalid MTDSS_TEST_WARM_DEBOUNCE_MS={raw:?} (keep default)");
                WARM_DEBOUNCE
            }
        },
    }
}

/// 重打退避封顶(spec §3.1(2)「1/2/4…封顶 30s」)。
const WARM_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// 重打冷却窗(连续失败达 [`WARM_COOLDOWN_AFTER_FAILURES`] 次后进入;窗内
/// 恒本值——对称 NAT 等永远打不通的对端靠冷却止损,spec §3.1(2)「窗内
/// 不空转」)。presence 翻转 / 代际更替复位(见 [`warmer_loop`] recalc)。
const WARM_COOLDOWN: Duration = Duration::from_secs(300);

/// 进入冷却的连续失败次数(全局约束钉死:第 3 次失败后 next_delay 返回
/// 冷却窗)。
const WARM_COOLDOWN_AFTER_FAILURES: u32 = 3;

/// 预热重打退避梯子的纯函数分量(hub::backoff_for 同款房风):第 `n` 次
/// 连续失败按梯形的等待 1s×2^(n-1),封顶 [`WARM_BACKOFF_CAP`]。仅
/// [`WarmBackoff::next_delay`] 与单测消费——冷却自第
/// [`WARM_COOLDOWN_AFTER_FAILURES`] 次连续失败起覆盖梯形取值(恒 5min),
/// 梯形形状仍独立钉死(spec 契约锚,改形状必须显式过单测)。
fn warm_ladder_step(n: u32) -> Duration {
    let shift = (n - 1).min(16); // 先防移位溢出,再取 min 收口封顶
    Duration::from_secs(1u64 << shift).min(WARM_BACKOFF_CAP)
}

/// 重打退避态机(per device 一份):`failures` = 当前连续失败计数。
///
/// `next_delay(again)`:`again=true` = 同一序列里的**再败**(计数推进);
/// `again=false` = 新一轮**首败**(此前刚成功过或刚复位,计数回 1)。返回
/// 该次失败后的重打等待:1s → 2s(梯形);第 3 次连续失败起进入冷却,
/// 恒返 [`WARM_COOLDOWN`]——退避值只增不减,直到 [`WarmBackoff::reset`]
/// (预热成功 / 离开目标集重建 / 代际更替)。warmer 消费口径:死亡通知
/// 经闸后 `next_delay(true)` 退避后重打;`ensure_warm` 成功即 `reset()`。
struct WarmBackoff {
    failures: u32,
}

impl WarmBackoff {
    fn new() -> Self {
        Self { failures: 0 }
    }

    /// 见类型注。
    fn next_delay(&mut self, again: bool) -> Duration {
        if again {
            self.failures = self.failures.saturating_add(1);
        } else {
            self.failures = 1;
        }
        if self.failures >= WARM_COOLDOWN_AFTER_FAILURES {
            WARM_COOLDOWN
        } else {
            warm_ladder_step(self.failures)
        }
    }

    /// 复位回梯形起步(预热成功 / presence 翻转或代际更替的涉及设备)。
    fn reset(&mut self) {
        self.failures = 0;
    }
}

/// death watcher 的重打决策纯函数(单测 [`tests::should_rewrite_gate`] 钉
/// 死):未登录或已非 warm 目标即丢弃通知——登出闸门(一切打洞动作前必须
/// 已登录)与目标集闸(设备离线 / 翻成大 pk 侧 / 代际更替已重建)的双合取。
fn should_rewrite(logged_in: bool, still_target: bool) -> bool {
    logged_in && still_target
}

/// warm 目标集(纯函数,单测 [`tests::warm_targets_excludes_self_offline_unknown_pk_and_large_side`]
/// 钉死排除表):目录快照里 **在线 ∧ 非本机 ∧ 公钥已知 ∧ 本机公钥字节序
/// 更小** 的设备 id 列表(快照序,保序)。任一前提缺席即排除;`me_pk=None`
/// (身份不可用)或快照缺席 → 空集——单侧主动无从推导时宁可不预热
/// (正确性不依赖预热,浏览按需打洞兜底)。
fn warm_targets(
    devices: Option<&[Value]>,
    self_id: Option<&str>,
    me_pk: Option<&VerifyingKey>,
    peer_pks: &HashMap<String, VerifyingKey>,
) -> Vec<String> {
    let (Some(list), Some(self_id), Some(me_pk)) = (devices, self_id, me_pk) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|d| {
            let id = d["device_id"].as_str()?;
            if id == self_id {
                return None; // 非本机
            }
            if !d["online"].as_bool().unwrap_or(false) {
                return None; // 在线
            }
            let pk = peer_pks.get(id)?;
            if me_pk.as_bytes() >= pk.as_bytes() {
                return None; // 公钥字节序:小者主动(大 pk 侧等对端暖)
            }
            Some(id.to_string())
        })
        .collect()
}

/// 当前 warm 目标集(warmer 主循环与 worker 的 still_target 判定共用的
/// 现读面):快照 → 枢纽公钥缓存逐条解析 → [`warm_targets`]。
fn current_targets(state: &DaemonState) -> Vec<String> {
    let devices = state.devices();
    let self_id = state.self_device_id();
    // 身份不可用(种子缺失/损坏)→ me_pk=None → warm_targets 恒空集
    let me_pk = flow::load_identity(&state.home()).ok().map(|id| id.public_key());
    let hub = state.hub();
    let peer_pks: HashMap<String, VerifyingKey> = devices
        .iter()
        .flatten()
        .filter_map(|d| d["device_id"].as_str())
        .filter_map(|id| hub.peer_pk(id).map(|pk| (id.to_string(), pk)))
        .collect();
    warm_targets(devices.as_deref(), self_id.as_deref(), me_pk.as_ref(), &peer_pks)
}

/// per-device 预热状态(warmer_loop 本地表项,不进 DaemonState):worker
/// 句柄即「在途」标记——表内有 worker = 该设备已有常驻预热循环,重算不
/// 重复 spawn(并发纪律:同设备不并发两个 ensure_warm,单飞锁兜底)。
struct DevWarm {
    worker: tokio::task::JoinHandle<()>,
}

/// 预热器常驻任务:登录 watch 挂起(同 responder_loop 房风,通知驱动不
/// 轮询)→ 订阅事件广播 → event.devices / event.online 去抖合并 → 全量
/// 重算目标 → 每目标常驻 worker。代际更替(event.online false→true,信令
/// 重连 / 回前台)时在标设备 worker 全部重建——失败证据作废,退避回 1s
/// 立即重打(spec §3.1(2) 代际建立即预热;冷却窗不被跨代携带)。登出回
/// 外层挂起并拆全部 worker(连接池拆卸由 state.logout → ctrl.reset 承担);
/// 收尾由 Daemon shutdown abort。
pub(crate) async fn warmer_loop(state: Arc<DaemonState>) {
    let mut login_rx = state.login_watch();
    let mut table: HashMap<String, DevWarm> = HashMap::new();
    loop {
        // 未登录:通知驱动挂起(不轮询);发送端随 state 常驻,Err 不可达,
        // 防御性返回。
        while !state.logged_in() {
            if login_rx.changed().await.is_err() {
                return;
            }
        }
        // 登录就绪:清残留表(上一代 worker 已在登出收尾拆净,防御清一次),
        // 订阅进程级事件广播,登录即跑一轮(快照已就绪则立刻预热;未就绪
        // 等 event.devices/event.online 触发)。
        table.clear();
        let mut events = state.events().subscribe();
        recalc_targets(&state, &mut table, false);
        // 事件消费循环:event.devices(快照刷新 / presence 变化)与
        // event.online(本机在线翻转)为触发源;其余帧忽略。
        loop {
            let mut reset_staying = false;
            let triggered = tokio::select! {
                ev = events.recv() => match ev {
                    Ok(frame) => {
                        if frame.method == EV_ONLINE && frame.params["online"].as_bool() == Some(true)
                        {
                            // 本机在线翻转 true = 信价新代际建立(重连 / 回前台
                            // WS 重连;首登走外层登录臂不经过此处):在标设备
                            // 退避证据作废,本轮全量重建(spec §3.1(2) 代际
                            // 建立即预热——冷却窗不跨代携带)。
                            reset_staying = true;
                        }
                        frame.method == EV_DEVICES || frame.method == EV_ONLINE
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("warmer 事件滞后丢 {n} 帧,按触发处理(重算幂等)");
                        true
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
                r = login_rx.changed() => {
                    if r.is_err() {
                        return;
                    }
                    // 登录翻转:登出 → 出内层回外层挂起;登录伪唤醒(重复
                    // true)无害,顺手重算(幂等)。
                    !state.logged_in()
                }
            };
            if !state.logged_in() {
                break; // 登出:回外层挂起(worker 拆卸见内层出口)
            }
            if !triggered {
                continue;
            }
            // 去抖:窗内后续触发合并为一轮重算(重算幂等,读当下快照,少算
            // 不漏);窗内登出即收(不睡满)。
            tokio::select! {
                _ = tokio::time::sleep(warm_debounce()) => {}
                r = login_rx.changed() => {
                    if r.is_err() {
                        return;
                    }
                    if !state.logged_in() {
                        break;
                    }
                }
            }
            recalc_targets(&state, &mut table, reset_staying);
        }
        // 登出收尾:拆全部 worker(abort;退避睡眠 / watcher 等待 / 在途
        // dial 都在 await 点取消;池条目拆卸由 state.logout → ctrl.reset)。
        for (_, entry) in table.drain() {
            entry.worker.abort();
        }
    }
}

/// 全量重算目标并同步 worker 表:离开目标集的设备拆 worker 清条目(presence
/// 翻转涉及设备的退避复位路径——重新入标即全新 1s 起步);新入目标集的
/// 设备 spawn 常驻 worker(`reset_staying`=代际更替时在标设备也全部重建)。
/// 在标且无代际翻转的设备保持现状(worker 自循环覆盖重打;无关设备的
/// presence 抖动不复位在标设备的冷却窗,止损不被合并窗冲垮)。
fn recalc_targets(state: &Arc<DaemonState>, table: &mut HashMap<String, DevWarm>, reset_staying: bool) {
    let targets = current_targets(state);
    let target_set: HashSet<&str> = targets.iter().map(|s| s.as_str()).collect();
    if reset_staying {
        // 代际更替:全部在标 worker 重建(退避回 1s;活连接设备多付一次
        // ensure_warm 池活快查,廉价)。
        for (_, entry) in table.drain() {
            entry.worker.abort();
        }
    }
    table.retain(|dev, entry| {
        if target_set.contains(dev.as_str()) {
            true
        } else {
            entry.worker.abort();
            false
        }
    });
    for dev in targets {
        table
            .entry(dev.clone())
            .or_insert_with(|| DevWarm {
                worker: tokio::spawn(warm_device(state.clone(), dev)),
            });
    }
}

/// 重打等待 + 探洞豁免钳制(V3-6 T9,spec §4.4):退避梯形/冷却值与
/// [`PunchMemory::probe_floor`] 取 min——该对端有活跃中继轮 → 至多
/// [`crate::relaymem::PROBE_FLOOR`] 一发(豁免 300s 冷却窗:中继当主线的
/// 对端保持探洞节奏,直连一通即知,回切窗不被冷却拖住);无活跃轮 →
/// probe_floor=MAX,min 恒等(既有节奏零改)。纯函数,单测
/// [`tests::reprobe_delay_clamped_by_active_relay_round`] 钉死。
fn clamped_reprobe_delay(
    mem: &PunchMemory,
    device: &str,
    backoff: &mut WarmBackoff,
    again: bool,
) -> Duration {
    backoff.next_delay(again).min(mem.probe_floor(device))
}

/// 回切轮换(V3-6 T9,spec §4.4 治瘾机制):预热成功(直连已入池——
/// [`crate::ctrl::CtrlPool::ensure_warm`] Ok 含池活幂等,直连在场即语义
/// 成立)且该对端有活跃中继轮 → 过一发闸([`PunchMemory::
/// take_rotation_shot`]:armed ∧ 本代际未发)→ 清失败记忆 +
/// [`PunchMemory::drain_relay_rounds`] 全部中继半连逐条应用层 close **并
/// 显式释放会话**——中继两端是各自对服务端的独立 QUIC 连接,只关本端半
/// 连对端不知情:其死亡通告臂等不到 `Dead{sid}`,传输段空等至空闲超时,
/// 重试轮还会被对端的同作业轮锁堵死;经 Task 7 RelayRelease 面(双端
/// `Dead{reason=released}` 通告,死因映射可重试 Network)对端当轮即收口,
/// 发送端既有重试轮落暖/直连(本端池内刚回切的直连;对端被动入池),位
/// 图续传零重发。一发/armed 代际(防振荡,spec §4.4):直连回切后再死须
/// 重走穷尽再 armed,绝不乒乓。返回关闭的连接数(0 = 未发,单测观测面)。
fn rotation_switchback(mem: &PunchMemory, hub: &SignalHub, device: &str) -> usize {
    if !mem.relay_round_active(device) || !mem.take_rotation_shot(device) {
        return 0;
    }
    mem.note_direct_success(device);
    let rounds = mem.drain_relay_rounds(device);
    let closed = rounds.len();
    for r in &rounds {
        // 应用层码沿房风:0(VarInt)+ 短 ASCII 理由串(ctrl/flow 既有
        // close 位同款);理由串区分回切关连与驱逐/登出关连
        r.conn.close(0u32.into(), b"v3p6 rotation");
        // fire-and-forget(离线无害 no-op,沿 hub 释放面口径)
        hub.release_relay(r.sid);
    }
    tracing::info!(device = %device, closed, "回切:关闭中继半连,重试轮落直连");
    closed
}

/// 单设备预热 worker(常驻,per target 恰一个):预热([`ensure_warm`])
/// 成功 → 复位退避 → 挂 death watcher(持池条目连接克隆,`conn.closed()`
/// → mpsc 通知交回本 worker)→ 死亡通知经 [`should_rewrite`] 闸(未登录
/// / 已非 warm 目标即弃,收尾)→ 退避([`WarmBackoff::next_delay`])后重
/// 打;预热失败走同一退避循环(失败路径无 watcher,直接退避)。登出在任
/// 意 await 点 select 即收尾;被 recalc abort(离开目标集 / 代际重建)同
/// 收尾。并发纪律:同设备恒恰一个 worker(recalc 的表项即「在途」标记),
/// ensure_warm 内部另有 per-device 单飞锁兜底。
async fn warm_device(state: Arc<DaemonState>, device: String) {
    let mut login_rx = state.login_watch();
    let mut backoff = WarmBackoff::new();
    // 上一程预热是否成功(next_delay 的 again 缝:!last_ok = 同一序列的
    // 再败;两分支每轮必写,初值无读取面)。
    let mut last_ok;
    loop {
        // 登出闸门:预热一切打洞动作前必须已登录(全局约束;重算/abort 是
        // 常规收尾路径,此处兜快速翻转的窗)。
        if !state.logged_in() {
            return;
        }
        let ok = match state.ctrl().ensure_warm(&state, &device).await {
            Ok(()) => true,
            Err(e) => {
                tracing::info!(device = %device, err = %e, "warm 预热失败,进退避");
                false
            }
        };
        if ok {
            // 回切轮换(T9,spec §4.4):直连已入池 + 活跃中继轮 → 一发
            // 闸过即关中继半连 + 显式释放会话清记忆(轮换语义见
            // rotation_switchback 注;未发零副作用,一发/代际防振荡)
            rotation_switchback(state.punch_memory(), state.hub(), &device);
            tracing::debug!(device = %device, "warm 预热成功,挂 death watcher");
            backoff.reset();
            last_ok = true;
            // death watcher:持池条目连接克隆(与池共享同一条连接的句柄),
            // 连接关闭(对端关 / 断网空闲超时 / 驱逐显式 close / 登出清池)
            // 即就绪,经 mpsc 通知本 worker。None = 条目已被竞态驱逐/清池,
            // 跳过 watcher 直接走闸门(退避重打,自洽)。
            let (tx, mut rx) = mpsc::channel::<()>(1);
            let watcher = state.ctrl().warm_conn(&device).map(|conn| {
                tokio::spawn(async move {
                    let _ = conn.closed().await;
                    let _ = tx.send(()).await;
                })
            });
            if watcher.is_some() {
                // 等死亡通知(通知驱动不轮询);登出即收尾,登录伪唤醒回到
                // 等待(watch 循环直到死亡或登出)。
                loop {
                    tokio::select! {
                        _ = rx.recv() => break,
                        r = login_rx.changed() => {
                            if r.is_err() {
                                return;
                            }
                            if !state.logged_in() {
                                return;
                            }
                        }
                    }
                }
            }
        } else {
            last_ok = false;
        }
        // 重打闸门(纯函数):未登录或已非 warm 目标 → 弃(收尾;离开目标
        // 集的重建由 recalc 负责,本 worker 不越俎)。
        if !should_rewrite(state.logged_in(), still_warm_target(&state, &device)) {
            return;
        }
        // 退避(梯形 + 3 败冷却)+ 探洞豁免钳制(T9,spec §4.4:活跃中继
        // 轮期间至多 30s 一发,豁免冷却窗;无轮 min 恒等;登出即收尾不睡满)。
        let delay = clamped_reprobe_delay(state.punch_memory(), &device, &mut backoff, !last_ok);
        tracing::info!(device = %device, ?delay, "warm 断连/失败重打退避");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            r = login_rx.changed() => {
                if r.is_err() {
                    return;
                }
                if !state.logged_in() {
                    return;
                }
            }
        }
    }
}

/// 该设备当前是否 warm 目标(death 通知的 `still_target` 缝):现算目标集
/// + 包含判定。频率 = 每次死亡/失败一次(事件驱动,非轮询),现算廉价。
fn still_warm_target(state: &DaemonState, device: &str) -> bool {
    current_targets(state).iter().any(|t| t == device)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试键:种子确定性派生验签公钥(顺序断言用实际字节序,不赌派生序)。
    fn pk_from_seed(seed: [u8; 32]) -> VerifyingKey {
        ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key()
    }

    /// warm_targets 排除表(计划 Step 1 逐字):快照 5 设备——self(在线)、
    /// peer_on(在线,本机更小故为目标)、peer_off(离线)、peer_big(在线,
    /// 大 pk 侧)、peer_nopk(在线,公钥缓存缺)→ 恰 ["peer_on"];
    /// me_pk=None(身份不可用)→ 空集。
    #[test]
    fn warm_targets_excludes_self_offline_unknown_pk_and_large_side() {
        // 三键按实际字节序定角色(不赌种子派生序):me 居中——
        // peer_big < me < peer_on:peer_on 大于本机 = 本机是小 pk 侧(目标);
        // peer_big 小于本机 = 本机是其大 pk 侧(排除,等它暖本机)。
        let mut keys = [pk_from_seed([1u8; 32]), pk_from_seed([2u8; 32]), pk_from_seed([3u8; 32])];
        keys.sort_by_key(|k| *k.as_bytes());
        let (peer_big, me, peer_on) = (keys[0], keys[1], keys[2]);
        let me_id = "me-device";
        let devices = vec![
            serde_json::json!({ "device_id": me_id, "online": true }),
            serde_json::json!({ "device_id": "peer-on", "online": true }),
            serde_json::json!({ "device_id": "peer-off", "online": false }),
            serde_json::json!({ "device_id": "peer-big", "online": true }),
            serde_json::json!({ "device_id": "peer-nopk", "online": true }),
        ];
        let peer_pks: HashMap<String, VerifyingKey> = [
            (me_id.to_string(), me),
            ("peer-on".to_string(), peer_on),
            ("peer-off".to_string(), peer_on),
            ("peer-big".to_string(), peer_big),
            // peer-nopk 刻意缺席:公钥缓存缺
        ]
        .into_iter()
        .collect();
        let targets = warm_targets(
            Some(&devices),
            Some(me_id),
            Some(&me),
            &peer_pks,
        );
        assert_eq!(targets, vec!["peer-on".to_string()], "恰小 pk 侧在线已知公钥对端入选");
        // me_pk=None(身份不可用)→ 空集
        assert!(
            warm_targets(Some(&devices), Some(me_id), None, &peer_pks).is_empty(),
            "身份不可用无从推导单侧主动,恒空集"
        );
    }

    /// should_rewrite 重打闸门(计划 Step 1 逐字):未登录恒 false;
    /// 已非 warm 目标 false;登录且仍是目标 true。
    #[test]
    fn should_rewrite_gate() {
        assert!(!should_rewrite(false, false), "未登录恒不重打");
        assert!(!should_rewrite(false, true), "未登录恒不重打");
        assert!(!should_rewrite(true, false), "已非 warm 目标不重打");
        assert!(should_rewrite(true, true), "登录且仍是目标才重打");
    }

    /// 退避梯形与冷却(计划 Step 1 逐字):连续失败 1s→2s→4s→…→30s 封顶
    /// (梯形分量);第 3 次失败后 next_delay 返回 5min 冷却且冷却期内恒
    /// 5min;reset 后回 1s。
    #[test]
    fn warm_backoff_ladder_and_cooldown() {
        // 梯形分量纯函数锚(hub::backoff_for 同款;第 3 次起被冷却覆盖,
        // 形状仍须钉死——spec §3.1(2)「1/2/4…封顶 30s」)
        assert_eq!(warm_ladder_step(1), Duration::from_secs(1));
        assert_eq!(warm_ladder_step(2), Duration::from_secs(2));
        assert_eq!(warm_ladder_step(3), Duration::from_secs(4));
        assert_eq!(warm_ladder_step(4), Duration::from_secs(8));
        assert_eq!(warm_ladder_step(5), Duration::from_secs(16));
        assert_eq!(warm_ladder_step(6), Duration::from_secs(30));
        assert_eq!(warm_ladder_step(100), Duration::from_secs(30), "封顶 30s");
        // 态机:首败 1s → 第 2 败 2s → 第 3 败进冷却 5min → 冷却期内恒 5min
        let mut b = WarmBackoff::new();
        assert_eq!(b.next_delay(false), Duration::from_secs(1), "新一轮首败 1s");
        assert_eq!(b.next_delay(true), Duration::from_secs(2), "连续第 2 败");
        assert_eq!(b.next_delay(true), WARM_COOLDOWN, "第 3 败进冷却");
        assert_eq!(b.next_delay(true), WARM_COOLDOWN, "冷却期内恒 5min");
        assert_eq!(b.next_delay(true), WARM_COOLDOWN, "冷却不随失败加深");
        // reset 回 1s(presence 翻转/代际更替复位语义)
        b.reset();
        assert_eq!(b.next_delay(false), Duration::from_secs(1), "reset 后回 1s");
    }

    /// 去抖 env 映射三臂(沿 ctrl warm_decline_grace_from 单测房风,get 注入
    /// 缝):缺省 = 钉死默认 750ms;合法值毫秒换算;非法值(非数字/空串)
    /// warn 保持默认。
    #[test]
    fn warm_debounce_env_mapping() {
        let env = |v: &'static str| move |k: &str| (k == "MTDSS_TEST_WARM_DEBOUNCE_MS").then(|| v.to_string());
        assert_eq!(warm_debounce_from(|_| None), WARM_DEBOUNCE, "缺省 = 钉死默认");
        assert_eq!(
            warm_debounce_from(env("100")),
            Duration::from_millis(100),
            "合法值毫秒换算"
        );
        assert_eq!(warm_debounce_from(env("abc")), WARM_DEBOUNCE, "非法值 warn 保持默认");
        assert_eq!(warm_debounce_from(env("")), WARM_DEBOUNCE, "空串非法保持默认");
    }

    /// 回环真握手取一对连接句柄(relaymem 单测同款房风:B 服务端白名单
    /// A,A pin B 公钥)→(客户端侧,服务端侧),充作登记句柄。
    async fn loopback_pair() -> (quinn::Connection, quinn::Connection) {
        use mtdss_core::transport::endpoint::{
            accept_pinned, connect_pinned, make_endpoint, make_server_endpoint,
        };
        let a = mtdss_core::identity::DeviceIdentity::generate();
        let b = mtdss_core::identity::DeviceIdentity::generate();
        let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
            [(a.device_id.clone(), a.public_key())].into_iter().collect();
        let ep_b = make_server_endpoint(
            std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            &b,
            &allowed,
        )
        .unwrap();
        let addr = ep_b.local_addr().unwrap();
        let ep_a = make_endpoint(std::net::SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let server = tokio::spawn(async move { accept_pinned(&ep_b, &allowed).await.unwrap().0 });
        let client = connect_pinned(&ep_a, addr, &b.public_key(), &a).await.unwrap();
        (client, server.await.unwrap())
    }

    /// 探洞豁免钳制(T9 计划 Step 1 ①,spec §4.4):冷却中 300s → 活跃
    /// 中继轮 30s(PROBE_FLOOR 默认值,env 未注入);无活跃轮 min 恒等
    /// (冷却原样,warmer 既有节奏零改);drain 后回冷却原样。
    #[tokio::test]
    async fn reprobe_delay_clamped_by_active_relay_round() {
        let mem = PunchMemory::default();
        let mut b = WarmBackoff::new();
        b.next_delay(false);
        b.next_delay(true);
        assert_eq!(b.next_delay(true), WARM_COOLDOWN, "前置:第 3 败进冷却");
        assert_eq!(
            clamped_reprobe_delay(&mem, "d", &mut b, true),
            WARM_COOLDOWN,
            "无活跃中继轮:冷却原样(钳制恒不命中)"
        );
        let (c1, _s) = loopback_pair().await;
        mem.register_relay_round("d", c1, 1);
        assert_eq!(
            clamped_reprobe_delay(&mem, "d", &mut b, true),
            crate::relaymem::PROBE_FLOOR,
            "活跃中继轮:300s 钳到 30s(豁免冷却窗)"
        );
        mem.drain_relay_rounds("d");
        assert_eq!(
            clamped_reprobe_delay(&mem, "d", &mut b, true),
            WARM_COOLDOWN,
            "drain 后回冷却原样"
        );
    }

    /// 回切轮换恰一发/代际(T9 计划 Step 1 ②,防振荡 spec §4.4):首发
    /// drain 全部半连逐条 close(克隆句柄 closed() 就绪)+ 清 armed;两个
    /// warm 成功只关一次(第二个成功零关);重新 armed(新代际)再发再生。
    #[tokio::test]
    async fn rotation_switchback_fires_once_per_generation() {
        let mem = PunchMemory::default();
        let hub = SignalHub::new();
        mem.note_exhaust("d"); // armed 代际 1
        let (c1, _s1) = loopback_pair().await;
        let (c2, _s2) = loopback_pair().await;
        mem.register_relay_round("d", c1.clone(), 11);
        mem.register_relay_round("d", c2.clone(), 12);
        // 首个 warm 成功:一发闸过 → drain 两条半连逐条 close + 释放
        assert_eq!(rotation_switchback(&mem, &hub, "d"), 2, "首发:两条登记全关");
        assert!(!mem.is_armed("d"), "回切清 armed(直连返场)");
        assert!(!mem.relay_round_active("d"), "drain 即清登记");
        assert!(mem.drain_relay_rounds("d").is_empty(), "取走即空");
        // 两次 warm 成功只关一次:第二个成功零关
        assert_eq!(
            rotation_switchback(&mem, &hub, "d"),
            0,
            "同代际已发 ∧ 未 armed:零关(恰一发)"
        );
        // close 可观测面:克隆句柄 closed() 就绪(应用层码 close)
        let _ = tokio::time::timeout(Duration::from_secs(2), c1.closed())
            .await
            .expect("中继半连 1 已关(回切 close)");
        let _ = tokio::time::timeout(Duration::from_secs(2), c2.closed())
            .await
            .expect("中继半连 2 已关(回切 close)");
        // 重新 armed(新代际)再登记 → 再发再生(防振荡不跨代)
        mem.note_exhaust("d");
        mem.register_relay_round("d", c1, 13);
        assert_eq!(rotation_switchback(&mem, &hub, "d"), 1, "新代际首发再生");
    }
}
