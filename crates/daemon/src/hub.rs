//! 信令枢纽(SignalHub):全进程恰一条与信令服务器的 WS 长连接。
//!
//! 旧结构下在线循环、常驻接收与发送流各自建连;服务器对同设备 presence
//! 槽「后连者胜」,发送流连接关闭时把本机从 presence 整体摘除——在线循环
//! 的连接还在,却再也收不到来件(对端 ConnectRequest 被服务器丢弃)。
//! 本模块把三条连接收敛为一条:`run`(原 online::run 职责 + 集中信封开箱)
//! 独占连接与目录快照维护;发送流与常驻接收经 `SignalHub` 的 relay/广播
//! 订阅复用同一代连接,代际更替(断连重连)时旧订阅以 `Closed` 快速失败,
//! 调用方按轮错误收口重试,不悬等超时。
//!
//! 入站信令在本模块统一解封:`Relayed → 查 pk(未命中先全量刷新一次)→
//! TOFU 固定 → open 验签 → 广播 SignalEvt`;任何开箱失败(验签失败/TOFU
//! 变更/未知设备刷新后仍缺)只 warn 丢弃该帧,连接循环不死。密钥解析
//! 永远只信服务器目录(信封内 sender 字段是未签名数据,绝不用于解析)。
//!
//! 来件请求(ConnectRequest/CtrlConnect)在广播之外另走**定向通道**:
//! 广播无订阅者即丢,而来件请求不可丢——消费方(常驻接收/控制通道响应端)
//! 的取件晚于枢纽上线时(如「信令已上线」到消费循环跟上之间的数百毫秒
//! 窗口),早到的请求若只走广播会被静默丢弃,发送方的首轮打洞凭空失败。
//! 故两条通道随代际安装即建好(容量 [`REQUEST_CAP`]),派发时双投递:广播
//! 照旧(发起端等待/其他订阅者不受影响),请求类加投通道——消费方经
//! `take_request_rx`/`take_ctrl_rx` 每代际取恰一个接收端,取件之前的来件
//! 在通道缓冲排队,代际更替旧接收端关闭(recv → None),消费方回外层重取
//! 下一代。应答类(Accept/Candidates)由发起端先订阅后发,无丢失面,维持
//! 广播。
//!
//! 中继面(v2-P1c):HelloOk 捕获账号中继资格随代际缓存(`relay_entitlement`);
//! RelaySession 推送路由成 [`RelayEvt`] 广播(`relay_events`,tag/relay_addr
//! 畸形只丢该帧不断连);分配直通 core(`relay_allocate`,10s 内建超时,
//! 外层不再叠等待);per-peer 兜底串行锁(`fallback_lock`)表在 hub 实例,
//! 跨连接代际存活。
//!
//! 回前台快探(spec §4):移动端回前台后,任何 IPC dispatch 入口若见最近
//! 入站帧陈旧(> [`RESUME_PROBE_STALE`] = 25s,> 信令 20s ping 周期)即主动
//! 拆当前代际([`SignalHub::kick`])触发快速重连——挂起中的 WS 是半开连接,
//! 靠 keepalive 静默判定最坏 ~60s 才判死,不满足移动端「进前台后就建连」
//! 的体感。快探不发任何探测帧(零协议变更),只拆让既有重连路径接管;
//! kick 旗让收口臂把退避清回 1s 首跳(见 `kick_flag` 字段注)。**拆是真拆**
//! (final review C1):kick 经**代际取消令牌**(`kick` 槽,每代一条 watch
//! 通道)唤醒 run_connected 事件循环的 kick 臂立即收口 Outcome::
//! Disconnected——本地代际句柄随返回 drop,`SignalingClient`(廉价克隆,
//! mpsc sender 包装)最后一枚引用消亡,core WS 任务 cmd_rx 收 None 退出
//! (零 core 改动),TCP 随之关闭;不等 keepalive 计时器。外层 run 循环
//! Disconnected 臂 → take_kick 清 attempt → 1s 首跳 → 新代际 ~1-2s 装回。
//! 入站打点在事件循环帧处理处([`run_connected`]),带**代际闸**
//! ([`SignalHub::note_inbound_if_current`]:仅本代仍现役才打点——kick 拆
//! 代际后旧循环的残余帧不得再刷新时钟,「非 0 ⟺ 有代际」不变量保真);
//! 握手期与离线期不计;WS Pong 由 core 上抛为 [`ClientEvent::Pong`] 进入
//! 同一打点(零线协议变更)——健康链路每 ≤20s(ping 周期)喂一次陈旧
//! 时钟,25s 阈值只对真陈旧/半开链路生效,健康链路不误触。

use crate::session::Session;
use crate::state::DaemonState;
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::client::{AllocOutcome, ClientEvent, RelayEntitlement};
use mtdss_core::signaling::envelope::{open, SignalBody};
use mtdss_ipc::EventFrame;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};

/// 事件方法名:本机在线状态翻转。`pub(crate)`:warm.rs 预热器(同名字符串
/// 事件是它的触发源)复用同一常量,不复制字面量。
pub(crate) const EV_ONLINE: &str = "event.online";
/// 事件方法名:设备目录全量快照。`pub(crate)`:同 [`EV_ONLINE`](crate::warm)。
pub(crate) const EV_DEVICES: &str = "event.devices";
/// 事件方法名:新终端审批请求(受信终端弹窗;AuthNotice join_request 路由)。
const EV_APPROVAL: &str = "event.approval";
/// 事件方法名:本机被移除(evict/unbind/懒踢;UI 回登录页)。
const EV_DEVICE_EVICTED: &str = "event.device_evicted";
/// 事件方法名:设备清单增删通告(spec §4.2 新事件;客户端据其刷新快照,
/// daemon 侧同时自动全量刷新广播 event.devices)。
const EV_DEVICES_CHANGED: &str = "event.devices_changed";
/// 事件方法名:账号数据变更通告(spec §7;params {key} = 变更 KV 键,
/// 客户端凭 key 重拉全量,favorites 消费面)。
const EV_DATA_CHANGED: &str = "event.data_changed";
/// 服务器错误码 1 = 鉴权拒(坏/过期 JWT、设备不属账号;见 ws.rs 握手段)。
const ERR_AUTH: u32 = 1;
/// 服务器错误码 9 = 本机已从账号移除(evict 即时踢/admin unbind+懒踢的
/// 绑定复核;spec §3.7)——清本地会话(identity.seed 保留),UI 回登录。
const ERR_UNBOUND: u32 = 9;
/// 重连退避封顶。
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// 回前台快探陈旧阈值(spec §4 回前台快探):IPC dispatch 时最近入站帧距
/// 当下超过它即拆当前代际触发快速重连。取 25s:大于信令 20s ping 周期
/// (core Keepalive 默认,Pong 上抛后健康链路的入站节奏以它为界——每
/// ≤20s 喂一次陈旧时钟),远小于 keepalive 静默判定的 60s 上限——挂起中
/// 的 WS 是半开连接,靠静默判定最坏 ~60s 才判死,不满足移动端「进前台后
/// 就建连」的体感;快探以 IPC 活动为触发主动自愈,不发任何探测帧(零线
/// 协议变更),拆让既有重连路径接管。
pub(crate) const RESUME_PROBE_STALE: Duration = Duration::from_secs(25);
/// 信令广播通道容量(慢订阅者以 Lagged 告警继续,不拖垮枢纽)。
const SIGNAL_CAP: usize = 256;
/// 来件请求定向通道容量(ConnectRequest/CtrlConnect 各一条,每代际各一)。
/// 消费方取件前的来件在缓冲排队;满即丢该帧并 warn(正常调度下不可达:
/// 每请求至多入队一条,128 远超任何活跃会话数)。
const REQUEST_CAP: usize = 128;
/// 中继事件广播容量(RelayEvt;兜底轮首订阅后每会话至多数帧,慢订阅者
/// Lagged 自行收口)。
const RELAY_EVT_CAP: usize = 64;

/// 中继会话推送的路由形态(T5 兜底编排消费):分配推送与会话死亡各一臂。
/// `pub`:经 relay_e2e 的 doc(hidden) 直驱面跨 crate 消费(SignalHub 本体
/// 仍 pub(crate),此处仅类型可见性)。
#[derive(Debug, Clone)]
pub enum RelayEvt {
    /// 分配推送(alive:true;tag 非 16 字节在路由时丢弃;purpose 为分配平面
    /// 回显,空 = 老客户端,响应方对空不过滤)
    Push {
        peer: String,
        sid: u64,
        tag: [u8; 16],
        relay_addr: SocketAddr,
        expires_at: i64,
        purpose: String,
    },
    /// 会话死亡(alive:false;reason 透传)
    Dead { sid: u64, reason: String },
}

/// 已解封验签的信令:`(from, SignalBody)`——枢纽已完成密钥解析、TOFU 固定
/// 与验签,订阅方直接按对端身份消费。
pub(crate) type SignalEvt = (String, SignalBody);

/// 一代连接:relay 句柄 + 信令广播 + 来件请求通道。代际收口(断连/
/// 登出/kick)时 run_connected 返回,本地句柄与枢纽槽相继 drop → Sender
/// 随代际消亡 → 订阅方收 `Closed`(广播)/ `None`(mpsc)。kick 路径经
/// 代际取消令牌令 run_connected **即时**返回(见 SignalHub 的 `kick`
/// 字段注),Closed 扇出不再悬等 keepalive 判定。
pub(crate) struct ConnHandle {
    /// Clone 廉价(mpsc sender);relay 经 `SignalHub::relay` 有界包装。
    pub(crate) client: mtdss_core::signaling::client::SignalingClient,
    /// 信令广播发送端(本代全部已验签信令经它扇出)。
    pub(crate) signals: tokio::sync::broadcast::Sender<SignalEvt>,
    /// 来件 ConnectRequest 定向通道(随代际安装即存在;广播无订阅者即丢,
    /// 来件请求不可丢——常驻接收经 `SignalHub::take_request_rx` 消费)。
    pub(crate) requests: mpsc::Sender<SignalEvt>,
    /// `requests` 的唯一接收端(每代际恰一个;install 时随通道创建,
    /// `take_request_rx` 取走后为 None)。
    requests_rx: std::sync::Mutex<Option<mpsc::Receiver<SignalEvt>>>,
    /// 来件 CtrlConnect 定向通道(同 `requests`,控制通道响应端消费)。
    pub(crate) ctrl: mpsc::Sender<SignalEvt>,
    /// `ctrl` 的唯一接收端(每代际恰一个,`take_ctrl_rx` 取走后为 None)。
    ctrl_rx: std::sync::Mutex<Option<mpsc::Receiver<SignalEvt>>>,
    /// 本代握手(HelloOk)时服务器下发的 STUN 通告(`SignalHub::stun` 取用)。
    stun: Vec<String>,
    /// 本代握手捕获的账号中继资格(`SignalHub::relay_entitlement` 取用;
    /// 旧服务器缺省 enabled=false,T5 据此永不发起兜底)。
    relay: RelayEntitlement,
    /// 中继会话推送广播(容量 [`RELAY_EVT_CAP`];事件循环路由写入,
    /// `SignalHub::relay_events` 订阅)。
    relay_ch: broadcast::Sender<RelayEvt>,
}

/// 信令枢纽:当前连接代际 + 设备公钥缓存(目录刷新维护)+ 兜底串行锁表。
pub(crate) struct SignalHub {
    conn: std::sync::RwLock<Option<std::sync::Arc<ConnHandle>>>,
    pks: std::sync::RwLock<HashMap<String, VerifyingKey>>, // 设备→公钥(目录刷新维护)
    /// per-peer 兜底串行锁表(缺失即建;表在 hub 实例上,跨连接代际存活,
    /// 同 CtrlPool::locks 房风——在途者持有的锁是独立 Arc,表清不清都不
    /// 影响其解锁语义,故此处不做登出清理)。fallback.rs(T5 兜底编排)
    /// 消费,单测钉锁表语义。
    fallback_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 最近入站信令帧时刻(Unix 毫秒;0 = 初始/离线语义):事件循环每帧
    /// 经 [`SignalHub::note_inbound_if_current`] 打点(**代际闸**:仅本代
    /// 取消令牌仍现役才落值,kick 拆代际后旧循环的残余帧不得打点——
    /// final review C1 的不变量修复),含 core 上抛的 Pong 控制帧(健康
    /// 链路每 ≤20s 喂钟一次);install 打当下(unconditional 基线,代际
    /// 正要装入必新鲜),uninstall 落 0。回前台快探(spec §4,api
    /// dispatch 入口)据此判陈旧。**不变量**:时间戳非 0 ⟺ 有代际
    /// ——uninstall 是唯一拆代际路径(go_offline/kick 都走它)且先落 0
    /// 后动 conn,事件循环打点再经代际闸双保险,并发读者所见恒是
    /// 「有代际 ⟹ 已打点」或「0 ⟺ 离线」;stale/kick 据此判离线而不读
    /// conn——dispatch 热路径零锁,健康链路不误触的判定面(25s 阈值 >
    /// 20s ping 周期,见 [`RESUME_PROBE_STALE`])。
    last_inbound: AtomicU64,
    /// 快探 kick 旗(kick 置位、外层 run 循环 Disconnected 收口臂取走即
    /// 清):dispatch 任务与 run 循环是两个任务,跨任务交旗只能经共享状态。
    /// 旗语义 =「本次断连是主动拆代际自愈,非真实网络故障」——收口臂见旗
    /// 清 attempt,退避回 1s 首跳(已爬高的历史退避不得从封顶值起跳,否则
    /// 「进前台后就建连」落空);旗先置后拆(kick 内序),保证收口臂见旗
    /// 的时序必早于拆代际令牌唤醒的收口。
    kick_flag: AtomicBool,
    /// 本代取消令牌(final review C1 的真拆 seam):**每代一条** watch
    /// 通道——run_connected 于 install 前建通道,发送端经
    /// [`SignalHub::arm_kick`] 入槽、接收端留事件循环 select;kick 经
    /// [`SignalHub::fire_kick`] 对槽内令牌 send(true),事件循环的 kick 臂
    /// 即醒、立即收口 Outcome::Disconnected(不等 core keepalive 的 60s
    /// 静默判定)。时序无错过窗口:接收端在 run_connected 本地、恒先于
    /// 任何 fire 在场(kick 只在 install 后可发——last_inbound 闸,install
    /// 才打基线);令牌随 uninstall 撤下,旧代 fire 与新代接收端分居两条
    /// 通道,代际隔离由构造保证(单测钉锚)。槽内已有旧令牌时覆盖
    /// (生产不可达:拆代际必先 uninstall 撤令牌;防御性覆盖使旧接收端
    /// changed → Err 也走拆代际收口,漏拆自愈)。
    kick: std::sync::RwLock<Option<watch::Sender<bool>>>,
    /// 代际序号(install 取号单调递增;0 = 从未装过):doc(hidden) 直驱面
    /// [`crate::Daemon::hub_generation`] 的代际身份探针——e2e 断言「kick
    /// 后新代际装回」以两次读值不同为证(取号单调,不同 ⟺ 换代)。生产
    /// 面无消费。
    gen_seq: AtomicU64,
}

/// 当下 Unix 毫秒(入站打点与陈旧判定共用的时间源;钟面异常由
/// [`SignalHub::stale`] 的 saturating_sub 兜底——差值取 0 转新鲜,不误 kick)。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl SignalHub {
    pub(crate) fn new() -> Self {
        Self {
            conn: RwLock::new(None),
            pks: RwLock::new(HashMap::new()),
            fallback_locks: std::sync::Mutex::new(HashMap::new()),
            last_inbound: AtomicU64::new(0),
            kick_flag: AtomicBool::new(false),
            kick: RwLock::new(None),
            gen_seq: AtomicU64::new(0),
        }
    }

    /// 当前连接代际;离线 None。
    pub(crate) fn current(&self) -> Option<std::sync::Arc<ConnHandle>> {
        self.conn.read().unwrap().clone()
    }

    /// 订阅当前代信令广播;离线 None。
    pub(crate) fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<SignalEvt>> {
        let g = self.conn.read().unwrap();
        g.as_ref().map(|h| h.signals.subscribe())
    }

    /// 取当前代际的来件请求通道(ConnectRequest)接收端;每代际恰一个,已取
    /// 过/离线返回 None。通道随代际安装即存在——取件之前的来件已在缓冲排队;
    /// 代际更替旧接收端关闭(recv → None),消费方回外层对下一代重取。
    pub(crate) fn take_request_rx(&self) -> Option<mpsc::Receiver<SignalEvt>> {
        let g = self.conn.read().unwrap().clone()?;
        let mut slot = g.requests_rx.lock().unwrap();
        slot.take()
    }

    /// 同 [`SignalHub::take_request_rx`]:控制通道来件(CtrlConnect)。
    pub(crate) fn take_ctrl_rx(&self) -> Option<mpsc::Receiver<SignalEvt>> {
        let g = self.conn.read().unwrap().clone()?;
        let mut slot = g.ctrl_rx.lock().unwrap();
        slot.take()
    }

    /// 当前连接的 STUN 通告;离线空(打洞端点自动降级本地候选)。
    pub(crate) fn stun(&self) -> Vec<String> {
        self.current().map(|h| h.stun.clone()).unwrap_or_default()
    }

    /// 本代握手缓存的中继资格;离线 None(T5 据此决定是否尝试兜底)。
    pub(crate) fn relay_entitlement(&self) -> Option<RelayEntitlement> {
        self.current().map(|h| h.relay)
    }

    /// 订阅当前代的中继事件广播;离线 None。兜底轮首订阅:先订后打洞,
    /// 分配推送不会早于订阅窗口。
    pub(crate) fn relay_events(&self) -> Option<broadcast::Receiver<RelayEvt>> {
        self.current().map(|h| h.relay_ch.subscribe())
    }

    /// 中继分配:10s 有界(core 内建超时,外层不再叠长等待;也不得在
    /// select 中取消本等待——分配槽位会被占到下一响应/断连)。Denied 是
    /// Ok 分支(服务器明确拒绝,码与消息透传),由调用方分派;离线 Err。
    /// `purpose` 为兜底平面判别(v2-P2 票①,经服务器双帧回显,响应方凭
    /// 它区分同对端的 ctrl/传输两面;空串 = 老客户端形态)。
    pub(crate) async fn relay_allocate(&self, to: &str, purpose: &str) -> anyhow::Result<AllocOutcome> {
        let client = {
            let g = self.conn.read().unwrap();
            g.as_ref()
                .map(|h| h.client.clone())
                .ok_or_else(|| anyhow::anyhow!("信令未在线,无法中继分配给 {to}"))?
        };
        client.relay_allocate(to, purpose).await
    }

    /// per-peer 兜底串行锁(缺失即建;同设备恒同一把,Arc 生命周期独立于
    /// 表)。取 Arc 即放(锁表短临界区);锁本体由 fallback.rs(T5)以
    /// tokio Mutex 跨 await 持有——Initiator 从分配到互拨全程串行,同对端
    /// 的并发兜底不互踩。单测钉锁表语义。
    pub(crate) fn fallback_lock(&self, peer: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.fallback_locks.lock().unwrap();
        locks.entry(peer.to_string()).or_default().clone()
    }

    /// 设备公钥缓存查询(目录刷新维护;未知设备 None)。
    pub(crate) fn peer_pk(&self, device_id: &str) -> Option<VerifyingKey> {
        self.pks.read().unwrap().get(device_id).copied()
    }

    /// 经当前连接把 payload 转发给 `to`;RELAY_TIMEOUT 有界,离线即 Err。
    /// 锁不跨 await(句柄块内取出即放),relay 本身无内建超时由外层截断。
    pub(crate) async fn relay(&self, to: &str, payload: &[u8]) -> anyhow::Result<()> {
        let client = {
            let g = self.conn.read().unwrap();
            g.as_ref()
                .map(|h| h.client.clone())
                .ok_or_else(|| anyhow::anyhow!("信令未在线,无法 relay 给 {to}"))?
        };
        crate::flow::bounded(
            "relay to peer",
            crate::flow::RELAY_TIMEOUT,
            client.relay(to, payload),
        )
        .await
    }

    /// 安装一代连接(换代:旧代 Sender 随写入换下而 drop,订阅方收 Closed)。
    /// 先打当下入站基线并清残留 kick 旗、后装代际(见 `last_inbound` 字段
    /// 注:并发快探读者不会看到「conn 已 Some、时间戳还是 0」的半程态;旗
    /// 清理兜住「旗置位后未走 Disconnected 臂的边角」跨代残留,如 kick 与
    /// 登出翻转撞车)。快探判定自此以握手完成时刻为新鲜基线,握手期不计
    /// (spec §4:握手期不参与陈旧判定)。代际序号在此取号(gen_seq 单调
    /// 递增,doc(hidden) 探针面)。**取消令牌必须已 armed**(run_connected
    /// 的装配序:arm_kick 先于 install——kick 只在 install 后可发,接收端
    /// 恒先于任何 fire 在场,见 `kick` 字段注)。
    fn install(&self, handle: Arc<ConnHandle>) {
        self.note_inbound();
        self.kick_flag.store(false, Ordering::Relaxed);
        self.gen_seq.fetch_add(1, Ordering::Relaxed);
        *self.conn.write().unwrap() = Some(handle);
    }

    /// 拆除当前连接代际(断连/登出/快探 kick 的统一收尾)。先落 0(离线
    /// 语义)、撤本代取消令牌(发送端 drop;其接收端——run_connected 尚
    /// 在场的边角——changed 得 Err 同样走拆代际收口,防卸漏拆的自愈)、
    /// 后拆代际槽(见 `last_inbound` 字段注:离线判定与代际状态在并发读者
    /// 眼里同步翻转)。订阅方随代际 Sender 消亡收 Closed(kick 路径下
    /// run_connected 已被令牌即时唤醒返回,Closed 扇出不悬等 keepalive)。
    fn uninstall(&self) {
        self.last_inbound.store(0, Ordering::Relaxed);
        *self.kick.write().unwrap() = None;
        *self.conn.write().unwrap() = None;
    }

    /// 最近入站信令帧打点(事件循环每帧调,Unix 毫秒当下);时间注入缝见
    /// [`SignalHub::note_inbound_with`]。
    fn note_inbound(&self) {
        self.note_inbound_with(now_ms());
    }

    /// 内部变体(单测注入缝):手写毫秒时刻,mock 时钟对拍陈旧语义不 sleep。
    fn note_inbound_with(&self, now_ms: u64) {
        self.last_inbound.store(now_ms, Ordering::Relaxed);
    }

    /// 事件循环的入站打点(**代际闸**,final review C1 的不变量修复):仅当
    /// 本代取消令牌仍是现役代际(kick 槽内同通道)才落时间戳。kick 拆代际
    /// (uninstall 撤令牌)后,旧事件循环在 kick 臂唤醒前可能再处理最后一帧
    /// ——无闸会把离线态的时钟误翻新鲜,「非 0 ⟺ 有代际」不变量(stale/
    /// kick 的判定面)被破坏;闸后旧代帧一律不打点。安装侧基线
    /// ([`SignalHub::note_inbound`],install 调)不受闸约束(hub 内部,
    /// 代际正要装入必新鲜)。以发送端克隆为闸凭据与槽内比较同通道
    /// (`watch::Sender::same_channel`,零额外分配;Sender 克隆廉价,由
    /// run_connected 本地持有兼作代际身份)。
    fn note_inbound_if_current(&self, mine: &watch::Sender<bool>) {
        let g = self.kick.read().unwrap();
        if g.as_ref().map(|tx| tx.same_channel(mine)).unwrap_or(false) {
            self.last_inbound.store(now_ms(), Ordering::Relaxed);
        }
    }

    /// 建本代取消令牌(run_connected 的装配序:**先于 install 调**):发送
    /// 端入槽(kick 经 [`SignalHub::fire_kick`] 对槽发射),接收端由调用方
    /// 持有进事件循环 select。时序论证见 `kick` 字段注——kick 只在 install
    /// 后可发(last_inbound 闸,install 才打基线),接收端恒先于任何 fire
    /// 在场,send 无错过窗口;槽内已有旧令牌时覆盖(防御性,生产不可达,
    /// 见字段注的自愈语义)。
    fn arm_kick(&self, tx: watch::Sender<bool>) {
        *self.kick.write().unwrap() = Some(tx); // 覆盖式写入:残留(生产不可达)即替换,自愈语义见字段注
    }

    /// 发射当前代际的取消令牌(kick 内序:置旗后、拆槽前):槽内令牌
    /// send(true),事件循环的 kick 臂即醒、立即收口 Disconnected。槽空
    /// (离线/竞态已拆)即无操作;send 失败(接收端已亡 = run_connected
    /// 已返回)忽略——无人可唤醒,拆槽照常收口。
    fn fire_kick(&self) {
        if let Some(tx) = self.kick.read().unwrap().as_ref() {
            let _ = tx.send(true);
        }
    }

    /// 回前台快探的主动拆代际(spec §4):**真拆**(final review C1)——
    /// 置 kick 旗 + 发射本代取消令牌 + 拆枢纽槽,三步内序:旗先置(收口臂
    /// 见旗的时序必早于令牌唤醒,否则已按真实断连递增退避,1s 首跳快跳
    /// 落空);令牌 send(true) 唤醒 run_connected 事件循环的 kick 臂,本代
    /// **立即**收口 Outcome::Disconnected——本地代际句柄随返回 drop,枢纽
    /// 槽已空,`SignalingClient`(廉价克隆,mpsc sender 包装)最后一枚引用
    /// 消亡 → core WS 任务 cmd_rx 收 None 退出(零 core 改动)、TCP 随之
    /// 关闭,**不等 keepalive 计时器**(挂起半开链路靠静默判定最坏 ~60s);
    /// uninstall 拆槽(落 0 + 撤令牌),订阅方 Closed 扇出随 run_connected
    /// 即时返回即时发生。外层 run 循环 Disconnected 收口臂见旗清 attempt,
    /// 退避回 1s 首跳,新代际 ~1-2s 装回——「进前台后就建连」。离线(无
    /// 代际)为无操作:stale 已恒 false,此处再闸一道(last_inbound ⟺ 有
    /// 代际,见字段注),旗不残留、令牌不发。
    pub(crate) fn kick(&self) {
        if self.last_inbound.load(Ordering::Relaxed) != 0 {
            self.kick_flag.store(true, Ordering::Relaxed);
            self.fire_kick();
            self.uninstall();
        }
    }

    /// 当前代际序号(doc(hidden) 测试直驱面后端,[`crate::Daemon::
    /// hub_generation`] 透传):已装代际的取号(install 单调递增);离线
    /// None。kick 前后两次读值不同 ⟺ 新代际已装回(e2e 断言拆代际→重连
    /// 时序的代际身份探针;生产面无消费)。
    pub(crate) fn current_generation(&self) -> Option<u64> {
        self.conn
            .read()
            .unwrap()
            .is_some()
            .then(|| self.gen_seq.load(Ordering::Relaxed))
    }

    /// 回前台快探的陈旧判定(spec §4):最近入站帧距当下超过 `threshold`
    /// 即 true。**离线恒 false**(时间戳 0 = 离线/无代际语义,见
    /// `last_inbound` 字段注)——不许离线误 kick;纯读无副作用零锁,分派
    /// 入口一行调用,不阻塞分派。
    pub(crate) fn stale(&self, threshold: Duration) -> bool {
        let last = self.last_inbound.load(Ordering::Relaxed);
        if last == 0 {
            return false; // 离线/无代际:恒 false
        }
        now_ms().saturating_sub(last) > threshold.as_millis() as u64
    }

    /// 取走 kick 旗(外层 run 循环 Disconnected 收口臂消费):置位 = 本次
    /// 断连是快探主动拆代际,退避回 1s 首跳;取走即清零(一次性旗)。
    fn take_kick(&self) -> bool {
        self.kick_flag.swap(false, Ordering::Relaxed)
    }

    /// 目录全量刷新时同步重建公钥缓存(REST 为权威,整体替换)。条目缺
    /// 字段/畸形公钥静默跳过:与发送侧 `peer_pk_from` 的硬错误不同,这里是
    /// 缓存维护——缺失设备走「未命中先刷新」路径在开箱时显式丢弃。
    /// `pub(crate)`:devices RPC 现拉(Task 7)经同一入口维护缓存。
    pub(crate) fn load_pks(&self, raw: &[Value]) {
        let mut map = HashMap::new();
        for d in raw {
            let Some(id) = d["device_id"].as_str() else {
                continue;
            };
            let Some(hexk) = d["pubkey"].as_str() else {
                continue;
            };
            let Ok(bytes) = hex::decode(hexk) else {
                continue;
            };
            let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) else {
                continue;
            };
            if let Ok(pk) = VerifyingKey::from_bytes(&arr) {
                map.insert(id.to_string(), pk);
            }
        }
        *self.pks.write().unwrap() = map;
    }
}

impl Default for SignalHub {
    fn default() -> Self {
        Self::new()
    }
}

/// 在订阅流上等来自 `from` 且满足 `pred` 的信令体;SIGNAL_TIMEOUT 有界;
/// 广播 Lagged(本订阅者落后丢帧)→ warn 后继续等(丢失即随轮超时失败);
/// 广播 Closed(连接代际更替)→ 立即 Err(调用方按轮错误收口,不等超时)。
pub(crate) async fn wait_signal(
    rx: &mut tokio::sync::broadcast::Receiver<SignalEvt>,
    from: &str,
    pred: impl Fn(&SignalBody) -> bool,
) -> anyhow::Result<SignalBody> {
    let wait = async {
        loop {
            match rx.recv().await {
                Ok((f, body)) => {
                    if f == from && pred(&body) {
                        return Ok(body);
                    }
                    // 其他发送者/其他信令体:忽略,继续等
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("信令广播滞后丢 {n} 帧,继续等待(丢失的帧随轮超时兜底)");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("信令连接代际更替(订阅源已关闭)");
                }
            }
        }
    };
    tokio::time::timeout(crate::flow::SIGNAL_TIMEOUT, wait)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out after {:?} waiting for signal from {from}",
                crate::flow::SIGNAL_TIMEOUT
            )
        })?
}

/// 第 `attempt` 次重连前的退避时长:1s×2^(n-1),封顶 30s。
/// 纯函数(attempt ≥ 1):1,2,4,8,16,30,30,…;单测锚点见文件尾部。
pub(crate) fn backoff_for(attempt: u32) -> Duration {
    let shift = (attempt - 1).min(16); // 先防移位溢出,再取 min 收口封顶
    Duration::from_secs(1u64 << shift).min(BACKOFF_CAP)
}

/// 一轮连接的结局,交由外层主循环分派重连策略。
enum Outcome {
    /// 会话消失(登出):回到外层等待登录。
    LoggedOut,
    /// 本机已被移除(code=9 / 临别帧):外层直接收尾回等登录。
    Unbound,
    /// 服务器以 code=1 拒绝(鉴权):外层先 refresh 一次再退避重连。
    AuthDenied,
    /// 普通断连/连接失败:外层退避重连。
    Disconnected,
}

/// 常驻主循环(原 online::run 职责 + 集中信封开箱):由 `Daemon::start`
/// 拉起,随 Daemon shutdown 中止。未登录自挂起(watch 通知),登录翻转即
/// 上线;登出翻转自动回等。
pub(crate) async fn run(state: Arc<DaemonState>, events: broadcast::Sender<EventFrame>) {
    let mut login_rx = state.login_watch();
    loop {
        // 未登录:通知驱动挂起(不轮询)。发送端随 state 常驻,Err 不可达,
        // 防御性返回。
        while !state.logged_in() {
            if login_rx.changed().await.is_err() {
                return;
            }
        }
        let mut attempt: u32 = 0;
        // 连续鉴权拒计数(spec §3.7「重连 code 1 device not bound → 清会话」
        // 的落地面):连接成功清零;第 2 次连续拒(刷新后仍拒 = 设备已不在
        // 账号目录,如离线期间被移除/服务端换签)按本机已移除收口——清会话
        // 回登录态,不再无限退避空转。第 1 次拒仍先走 refresh 自愈(访问
        // 令牌过期的常规路径,不得误杀会话)。
        let mut denies: u32 = 0;
        while let Some(session) = state.session() {
            let outcome = run_connected(
                &state,
                &events,
                &session,
                &mut login_rx,
                &mut attempt,
                &mut denies,
            )
            .await;
            match outcome {
                Outcome::LoggedOut => break,
                Outcome::Unbound => {
                    go_offline(&state, &events);
                    break;
                }
                Outcome::AuthDenied => {
                    go_offline(&state, &events);
                    denies += 1;
                    if denies >= 2 {
                        tracing::warn!("刷新后仍被拒,按本机已移除收口(清会话回登录态)");
                        handle_unbound(&state, &events).await;
                        break;
                    }
                    attempt += 1;
                    // 鉴权拒:refresh 一次轮换令牌并写回 session.json;刷新
                    // 失败沿用旧会话继续退避重试(不自弃,等服务器侧恢复)。
                    match crate::account::refresh(
                        &session.server,
                        &session.refresh,
                        session.ca.as_deref(),
                    )
                    .await
                    {
                        Ok((jwt, refresh)) => {
                            let mut rotated = session.clone();
                            rotated.jwt = jwt;
                            rotated.refresh = refresh;
                            match state.update_session(rotated) {
                                Ok(()) => tracing::info!("访问令牌已轮换并写回 session.json"),
                                Err(err) => {
                                    tracing::warn!(%err, "轮换令牌落盘失败,沿用旧会话重试")
                                }
                            }
                        }
                        Err(err) => tracing::warn!(%err, "令牌刷新失败,沿用旧会话退避重试"),
                    }
                }
                Outcome::Disconnected => {
                    go_offline(&state, &events);
                    // 快探 kick 收口(spec §4 回前台快探):旗置位 = 本次断连
                    // 是 dispatch 入口的主动拆代际(挂起半开 WS 的自愈),非
                    // 真实网络故障——attempt 清零,退避回到 1s 首跳(kick 的
                    // 重连必须快:已爬到封顶的历史退避不得从封顶值起跳,否则
                    // 「进前台后就建连」落空);真实断连照常递增退避。
                    if state.hub().take_kick() {
                        attempt = 0;
                    }
                    attempt += 1;
                }
            }
            // 退避睡眠,登出翻转即醒;睡满后再核登录态(睡眠期间登出的收口)
            if backoff_wait(&state, &mut login_rx, backoff_for(attempt)).await {
                break;
            }
        }
        go_offline(&state, &events);
    }
}

/// 本机已被移除(evict 临别帧 / code=9 / 连续鉴权拒):清会话(identity.seed
/// 保留,设备身份不变)+ 广播 `event.device_evicted`(UI 回登录页)。登录
/// 互斥锁内收口(与 verify/logout 串行);以登录态为闸保证幂等——Notice
/// 与 Error code=9 先后到达只清理一次,不重复广播。
async fn handle_unbound(state: &Arc<DaemonState>, events: &broadcast::Sender<EventFrame>) {
    let _guard = state.lock_login().await;
    if !state.logged_in() {
        return; // 已被先到路径收口
    }
    if let Err(err) = state.logout() {
        tracing::warn!(%err, "被移除后的会话清理失败");
    }
    let _ = events.send(EventFrame {
        method: EV_DEVICE_EVICTED.into(),
        params: json!({}),
    });
}

/// 一轮在线:连信令 → 握手 → 建连接代际 → 全量快照上线 → 事件循环,直至
/// 断连/登出。断连/登出走 `go_offline` 统一拆除代际(订阅方收 Closed)。
/// `denies` 为连续鉴权拒计数(连接成功即清零,外层据此判「刷新后仍拒」)。
async fn run_connected(
    state: &Arc<DaemonState>,
    events: &broadcast::Sender<EventFrame>,
    session: &Session,
    login_rx: &mut watch::Receiver<bool>,
    attempt: &mut u32,
    denies: &mut u32,
) -> Outcome {
    let (client, mut ev) = match crate::flow::signaling_connect(session, &session.device_id).await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%err, "信令连接失败");
            return Outcome::Disconnected;
        }
    };
    // 握手:等 HelloOk(stun 与账号中继资格本任务缓存后随代际下发)。握手
    // 期间收到的 Presence/Relayed/Notice 直接忽略——紧随其后的全量快照自带
    // 此刻真相,早到的增量不必暂存。
    let (stun, relay_ent) = loop {
        tokio::select! {
            frame = ev.recv() => match frame {
                Some(ClientEvent::HelloOk { stun_addrs, relay }) => break (stun_addrs, relay),
                Some(ClientEvent::Error { code: ERR_UNBOUND, message }) => {
                    tracing::warn!(%message, "信令握手被拒(本机已被移除)");
                    return Outcome::Unbound;
                }
                Some(ClientEvent::Error { code: ERR_AUTH, message }) => {
                    tracing::warn!(%message, "信令握手被拒(鉴权)");
                    return Outcome::AuthDenied;
                }
                Some(ClientEvent::Error { code, message }) => {
                    tracing::warn!(code, %message, "信令握手错误");
                    return Outcome::Disconnected;
                }
                Some(ClientEvent::Closed) | None => {
                    tracing::warn!("信令连接在握手前关闭");
                    return Outcome::Disconnected;
                }
                Some(_) => continue, // Presence / Relayed / Notice:见上注
            },
            // 登出翻转即弃连;仍为已登录的伪唤醒(如重登覆盖会话)则继续等握手
            _ = login_rx.changed() => {
                if !state.logged_in() {
                    return Outcome::LoggedOut;
                }
            }
        }
    };
    *attempt = 0; // 连接成功:退避清零
    *denies = 0; // 连接成功:连续鉴权拒清零(令牌自愈路径的常规翻转)
    // 建立连接代际:relay 句柄 + 信令广播 + 来件请求通道(旧代随安装换下,
    // 广播订阅方收 Closed、请求通道消费方收 None)。请求通道随安装即建好:
    // 消费方取件之前的 ConnectRequest/CtrlConnect 在通道缓冲排队,不再依赖
    // 订阅时机(广播无订阅者即丢,来件请求不可丢;应答类由发起端先订阅后
    // 发,无丢失面,维持广播)。
    let (sig, _) = broadcast::channel(SIGNAL_CAP);
    let (relay_evt_tx, _) = broadcast::channel(RELAY_EVT_CAP);
    let (req_tx, req_rx) = mpsc::channel(REQUEST_CAP);
    let (ctrl_tx, ctrl_rx) = mpsc::channel(REQUEST_CAP);
    let handle = Arc::new(ConnHandle {
        client,
        signals: sig,
        requests: req_tx,
        requests_rx: std::sync::Mutex::new(Some(req_rx)),
        ctrl: ctrl_tx,
        ctrl_rx: std::sync::Mutex::new(Some(ctrl_rx)),
        stun,
        relay: relay_ent,
        relay_ch: relay_evt_tx,
    });
    // 本代取消令牌(final review C1):**先于 install 建好**——kick 只在
    // install 后可发(last_inbound 闸),接收端恒先于任何 fire 在场,send
    // 无错过窗口;发送端克隆留事件循环作打点代际闸(note_inbound_if_current
    // 的同通道凭据)。
    let (kick_tx, mut kick_rx) = watch::channel(false);
    state.hub().arm_kick(kick_tx.clone());
    state.hub().install(handle.clone());
    // 本机身份:入站信封解封用。种子损坏属持久故障,按断连退避重试。
    let me = match crate::flow::load_identity(&state.home()) {
        Ok(me) => me,
        Err(err) => {
            tracing::warn!(%err, "加载本机身份失败,按断连退避重试");
            return Outcome::Disconnected;
        }
    };
    // 全量目录快照 + 公钥缓存(REST 为权威,本机条目 online=自身连接态)
    if let Err(err) = refresh_snapshot(state, events, session).await {
        tracing::warn!(%err, "拉取设备目录失败,按断连退避重试");
        return Outcome::Disconnected;
    }
    if state.mark_online() {
        let _ = events.send(EventFrame {
            method: EV_ONLINE.into(),
            params: json!({ "online": true }),
        });
    }
    tracing::info!("信令已上线");

    // 事件循环:Relayed 统一开箱广播;Presence 增量维护快照;Notice 按
    // kind 路由(审批弹窗/被移除/快照刷新);断连/登出收尾
    loop {
        tokio::select! {
            // 本代取消令牌臂(final review C1):hub.kick 真拆(fire_kick
            // send(true))即醒,立即收口 Disconnected——本地 handle 随返回
            // drop,枢纽槽已空(kick 先 uninstall),SignalingClient 最后一
            // 枚引用消亡 → core WS 任务 cmd_rx None 臂退出(零 core 改动)、
            // TCP 关闭。挂起半开链路由此**不等 keepalive 静默判定**(60s
            // 上限)即拆;真实断连照走 Closed/None 臂——两臂各自单独足够
            // 收口。Err(令牌随 uninstall 撤下,发送端 drop)同样收口:
            // 防御性按拆代际处理,与 kick 语义一致无害。
            _ = kick_rx.changed() => {
                tracing::info!("快探 kick:主动拆除本代信令连接(不等 keepalive 判定)");
                return Outcome::Disconnected;
            }
            frame = ev.recv() => {
                // 回前台快探打点(spec §4):一切入站帧(Relayed/Presence/
                // Notice/RelaySession/Error/Pong 等)刷新最近入站时刻——api
                // dispatch 入口的陈旧判定(RESUME_PROBE_STALE,> 信令 20s
                // ping 周期)据此分辨「信令活着」与「挂起半开」;Pong(core
                // 上抛)让健康链路每 ≤20s 喂钟,25s 阈值不误触。**代际闸**
                // (C1):仅本代令牌仍现役才打点——kick 拆代际后旧循环的残余
                // 帧不得再刷新时钟(「非 0 ⟺ 有代际」不变量,见
                // note_inbound_if_current 注)。Closed/None 是本地断连收口,
                // 不打点:随 Outcome::Disconnected → go_offline 的 uninstall
                // 落 0(离线语义)。
                if frame.is_some() {
                    state.hub().note_inbound_if_current(&kick_tx);
                }
                match frame {
                Some(ClientEvent::Relayed { from, payload }) => {
                    open_and_broadcast(state, events, session, &me, &handle, &from, &payload)
                        .await;
                }
                Some(ClientEvent::Presence { device_id, online }) => {
                    if state.set_device_online(&device_id, online) {
                        emit_current(state, events);
                    } else {
                        // 目录里没有该设备(连接期间新注册的账号成员):
                        // 全量刷新补齐;失败仅告警,后续事件/重连自愈
                        if let Err(err) = refresh_snapshot(state, events, session).await {
                            tracing::warn!(%err, "未知设备 {device_id} 的目录刷新失败");
                        }
                    }
                }
                // V3-1 AuthNotice 路由:join_request → 审批弹窗事件;
                // device_evicted → 清会话回登录态;devices_changed → 全量
                // 快照刷新(修「移除只翻在线位」缺口);data_changed →
                // 事件面(spec §7,V3-3 点亮:客户端凭 key 重拉)。未知
                // kind 忽略不断连。
                Some(ClientEvent::Notice { kind, session_id, code, candidate_name, candidate_platform, expires_at, data_key }) => {
                    match kind.as_str() {
                        "join_request" => {
                            let _ = events.send(EventFrame {
                                method: EV_APPROVAL.into(),
                                params: json!({
                                    "session_id": session_id,
                                    "candidate_name": candidate_name,
                                    "candidate_platform": candidate_platform,
                                    "code": code,
                                    "expires_at": expires_at,
                                }),
                            });
                        }
                        "device_evicted" => {
                            tracing::info!("收到临别通告:本机已被移除,清会话回登录态");
                            handle_unbound(state, events).await;
                            return Outcome::Unbound;
                        }
                        "devices_changed" => {
                            // 设备表增删的服务端通告(spec §4.2 新事件):先发
                            // `event.devices_changed {}` 事件面(客户端可凭它
                            // 自行刷新),再全量刷新快照并广播 event.devices
                            // (REST 为权威;失败仅告警,后续事件/重连自愈;
                            // event.devices 语义不变,仍是快照刷新信号)
                            let _ = events.send(EventFrame {
                                method: EV_DEVICES_CHANGED.into(),
                                params: json!({}),
                            });
                            if let Err(err) = refresh_snapshot(state, events, session).await {
                                tracing::warn!(%err, "devices_changed 的快照刷新失败");
                            }
                        }
                        "data_changed" => {
                            // 账号 KV 变更的服务端通告(spec §7):点亮为
                            // `event.data_changed {key}` 事件面(镜像
                            // devices_changed 先例;客户端凭 key 重拉全量)
                            let _ = events.send(EventFrame {
                                method: EV_DATA_CHANGED.into(),
                                params: json!({ "key": data_key }),
                            });
                        }
                        other => tracing::debug!(kind = %other, "未知 AuthNotice 类别,忽略"),
                    }
                }
                Some(ClientEvent::Error { code: ERR_UNBOUND, message }) => {
                    // 即时踢/懒踢的正身信号(evict 临别帧缺席时由此收口,
                    // 如 admin unbind):清会话回登录态
                    tracing::warn!(%message, "本机已被移除(code=9),清会话回登录态");
                    handle_unbound(state, events).await;
                    return Outcome::Unbound;
                }
                Some(ClientEvent::Error { code: ERR_AUTH, message }) => {
                    tracing::warn!(%message, "在线期间收到鉴权错误");
                    return Outcome::AuthDenied;
                }
                Some(ClientEvent::Error { code, message }) => {
                    tracing::warn!(code, %message, "信令错误,按断连处理");
                    return Outcome::Disconnected;
                }
                Some(ClientEvent::Closed) | None => {
                    tracing::info!("信令断开");
                    return Outcome::Disconnected;
                }
                Some(ClientEvent::HelloOk { .. }) => {}
                // WS Pong(core Keepalive 20s Ping 的服务器应答,零线协议变更
                // 上抛):臂首统一入站打点已覆盖——健康链路每 ≤20s 喂一次快探
                // 陈旧时钟,25s 阈值(> 20s ping 周期)只对真陈旧/半开链路生效
                // (spec §4;事件本体无消费面)。
                Some(ClientEvent::Pong) => {}
                // 中继会话推送:路由成 RelayEvt 广播(alive 分派;畸形帧只
                // 丢该帧,连接不断)。分配应答(RelayAllocated)经 oneshot
                // 回填,不进事件面
                Some(ev @ ClientEvent::RelaySession { .. }) => {
                    route_relay_evt(&handle.relay_ch, ev);
                }
                }
            },
            _ = login_rx.changed() => {
                if !state.logged_in() {
                    return Outcome::LoggedOut; // go_offline 统一拆除代际
                }
            }
        }
    }
}

/// 入站信令统一开箱:查 pk(未命中先全量刷新一次)→ TOFU 固定 → open
/// 验签 → 派发([`dispatch_body`]:广播全量扇出,请求类另入定向通道)。
/// 任何失败只 warn 丢弃该帧,连接循环不死(枢纽约定);密钥解析只信服务器
/// 目录(信封内 sender 字段未签名)。`h` 是本连接代际句柄:派发只触达其
/// 广播与两条定向通道发送端,保证来件永远落在本代通道里。
async fn open_and_broadcast(
    state: &DaemonState,
    events: &broadcast::Sender<EventFrame>,
    session: &Session,
    me: &DeviceIdentity,
    h: &ConnHandle,
    from: &str,
    payload: &[u8],
) {
    let hub = state.hub();
    let pk = match hub.peer_pk(from) {
        Some(pk) => pk,
        None => {
            // 未命中:全量刷新一次(新注册账号成员的首条信令走此路径补齐)
            if let Err(err) = refresh_snapshot(state, events, session).await {
                tracing::warn!(%err, "未知设备 {from} 的目录刷新失败,丢弃信令");
                return;
            }
            match hub.peer_pk(from) {
                Some(pk) => pk,
                None => {
                    tracing::warn!("设备 {from} 刷新后仍不在目录,丢弃信令");
                    return;
                }
            }
        }
    };
    // 打开任何信封之前先做 TOFU 固定(与发送侧同一份 peers.json):
    // 首次记录,变更即丢帧不改写缓存。
    if let Err(err) = crate::flow::tofu_pin(&state.home(), from, &pk) {
        tracing::warn!(%err, "设备 {from} 的 TOFU 校验失败,丢弃信令");
        return;
    }
    match open(payload, me, &pk) {
        Ok(body) => dispatch_body(&h.signals, &h.requests, &h.ctrl, from, body),
        Err(err) => tracing::warn!("设备 {from} 的信封开箱失败({err:?}),丢弃信令"),
    }
}

/// 已验签信令体的统一派发:广播全量扇出(发送流等发起端订阅、常驻接收的
/// Accept/Candidates 等待都靠它);请求类(ConnectRequest/CtrlConnect)另入
/// 对应定向通道——广播无订阅者即丢,来件请求不可丢。try_send 有界不阻塞:
/// 通道满/接收端已弃仅 warn(广播投递不受影响;容量 [`REQUEST_CAP`],正常
/// 调度不会满;派发循环绝不可被慢消费者拖住)。以通道三元组为参(而非
/// 代际句柄)以便单测直接构造,钉死「无广播订阅者时请求仍入通道」的回归
/// 锚点。
fn dispatch_body(
    signals: &broadcast::Sender<SignalEvt>,
    requests: &mpsc::Sender<SignalEvt>,
    ctrl: &mpsc::Sender<SignalEvt>,
    from: &str,
    body: SignalBody,
) {
    match &body {
        SignalBody::ConnectRequest { .. } => {
            if let Err(err) = requests.try_send((from.to_string(), body.clone())) {
                tracing::warn!("ConnectRequest 定向通道满或已弃({err}),仅广播投递");
            }
        }
        SignalBody::CtrlConnect { .. } => {
            if let Err(err) = ctrl.try_send((from.to_string(), body.clone())) {
                tracing::warn!("CtrlConnect 定向通道满或已弃({err}),仅广播投递");
            }
        }
        _ => {}
    }
    let _ = signals.send((from.to_string(), body));
}

/// 中继会话推送 → [`RelayEvt`] 广播(alive 分派):分配推送把 tag/relay_addr
/// 收敛为强形态,畸形(tag 非 16 字节/地址解析失败)只 warn 丢弃该帧,连接
/// 循环不死(枢纽约定);死帧按 sid 路由,reason 透传(不要求 tag/addr
/// 形态)。广播无订阅者即丢(兜底轮首订阅,迟到的分配推送本就不参与兜底)。
/// 独立函数以便单测直接构造(同 [`dispatch_body`] 房风)。
fn route_relay_evt(ch: &broadcast::Sender<RelayEvt>, ev: ClientEvent) {
    let ClientEvent::RelaySession { sid, alive, tag, relay_addr, expires_at, peer_device, reason, purpose } =
        ev
    else {
        return; // 仅 RelaySession 进路由(调用臂已过滤,防御性直返)
    };
    if !alive {
        let _ = ch.send(RelayEvt::Dead { sid, reason });
        return;
    }
    let Ok(tag) = <[u8; 16]>::try_from(tag.as_slice()) else {
        tracing::warn!(sid, "中继分配推送 tag 非 16 字节,丢弃");
        return;
    };
    let Ok(relay_addr) = relay_addr.parse::<SocketAddr>() else {
        tracing::warn!(sid, addr = %relay_addr, "中继分配推送 relay_addr 解析失败,丢弃");
        return;
    };
    let _ = ch.send(RelayEvt::Push {
        peer: peer_device,
        sid,
        tag,
        relay_addr,
        expires_at,
        purpose,
    });
}

/// 全量刷新目录(REST 为权威)并广播 `event.devices` 快照;同时重建枢纽
/// 公钥缓存(设备条目的 pubkey hex → VerifyingKey)。
async fn refresh_snapshot(
    state: &DaemonState,
    events: &broadcast::Sender<EventFrame>,
    session: &Session,
) -> anyhow::Result<()> {
    let raw = crate::account::fetch_devices(session).await?;
    let snap = build_snapshot(&raw, state.self_device_id().as_deref(), true);
    state.set_devices(snap);
    state.hub().load_pks(&raw);
    emit_current(state, events);
    Ok(())
}

/// 把当前缓存目录作为 `event.devices` 全量快照广播。
fn emit_current(state: &DaemonState, events: &broadcast::Sender<EventFrame>) {
    if let Some(list) = state.devices() {
        let _ = events.send(EventFrame {
            method: EV_DEVICES.into(),
            params: json!({ "devices": list }),
        });
    }
}

/// 目录投影:每台设备收敛为 {device_id,name,platform,online[,created_at]}
/// (pubkey 不出 IPC 面);`self_id` 的条目 online 强制为 `self_online`——
/// 本机是否在线是本地连接态事实,不取目录快照值。created_at(Unix 秒)
/// 透传为可选键:旧服务器缺该列时不投影(客户端三端可选容忍,服务端
/// 响应序即 created_at 升序,投影保序)。`pub(crate)`:devices RPC 现拉
/// (Task 7)与在线循环同一投影口径,缓存与响应不分叉。
pub(crate) fn build_snapshot(raw: &[Value], self_id: Option<&str>, self_online: bool) -> Vec<Value> {
    raw.iter()
        .map(|d| {
            let id = d["device_id"].as_str().unwrap_or_default();
            let online = if Some(id) == self_id {
                self_online
            } else {
                d["online"].as_bool().unwrap_or(false)
            };
            let name = match d["name"].as_str() {
                Some(n) => json!(n),
                None => Value::Null,
            };
            let mut row = json!({
                "device_id": id,
                "name": name,
                "platform": d["platform"].as_str().unwrap_or_default(),
                "online": online,
            });
            // R1 透传:仅当目录行带整数 created_at 才投影(缺键/非整数
            // 不出键——旧服务器升级窗内快照面不漂移)。
            if let Some(created_at) = d["created_at"].as_i64() {
                row["created_at"] = json!(created_at);
            }
            row
        })
        .collect()
}

/// 上线 → 离线:拆掉枢纽当前连接代际(订阅方随之收 Closed,发送流/常驻
/// 接收按轮错误收口);真翻转时发 `event.online false`,缓存里的本机条目
/// 保留并置 online=false(快照不缺本机,断连期间 devices RPC 亦自洽)。
fn go_offline(state: &DaemonState, events: &broadcast::Sender<EventFrame>) {
    state.hub().uninstall();
    if state.mark_offline() {
        state.set_self_online(false);
        let _ = events.send(EventFrame {
            method: EV_ONLINE.into(),
            params: json!({ "online": false }),
        });
    }
}

/// 退避睡眠;返回 true 表示期间已登出(立即收尾,不睡满/不重连)。
async fn backoff_wait(
    state: &DaemonState,
    login_rx: &mut watch::Receiver<bool>,
    d: Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = login_rx.changed() => {}
    }
    !state.logged_in()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_carries_created_at_with_legacy_tolerance() {
        // V3-3 返工 R1:目录行带整数 created_at → 投影携带同值;旧服务器
        // 缺键(或非整数)→ 不投影该键(三端可选字段容忍);投影保序
        // (服务端响应序 = created_at 升序 = 加入顺序,64 色色板取色序)。
        let raw = vec![
            json!({
                "device_id": "old", "name": "旧服务器条目",
                "platform": "linux", "online": false,
            }),
            json!({
                "device_id": "a", "name": "先入账",
                "platform": "windows", "online": true, "created_at": 1730000000i64,
            }),
            json!({
                "device_id": "b", "name": "后加入",
                "platform": "macos", "online": false, "created_at": 1730086400i64,
            }),
            json!({
                "device_id": "c", "name": "非整数防呆",
                "platform": "android", "online": false, "created_at": "junk",
            }),
        ];
        let snap = build_snapshot(&raw, Some("a"), false);
        assert_eq!(snap.len(), 4);
        assert!(snap[0].get("created_at").is_none(), "旧服务器缺键不投影");
        assert_eq!(snap[1]["created_at"], 1730000000);
        assert_eq!(snap[2]["created_at"], 1730086400);
        assert!(snap[3].get("created_at").is_none(), "非整数不投影");
        // 投影保序 + 本机 online 取本地连接态(原有口径不回归)
        let ids: Vec<&str> = snap.iter().map(|r| r["device_id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["old", "a", "b", "c"]);
        assert_eq!(snap[1]["online"], false, "self_id 条目强制本地连接态");
        assert_eq!(snap[0]["online"], false);
    }

    #[test]
    fn backoff_caps_at_30s() {
        // 1s×2^(n-1):1,2,4,8,16,随后 32 收口到 30,之后恒 30
        assert_eq!(backoff_for(1), Duration::from_secs(1));
        assert_eq!(backoff_for(2), Duration::from_secs(2));
        assert_eq!(backoff_for(3), Duration::from_secs(4));
        assert_eq!(backoff_for(4), Duration::from_secs(8));
        assert_eq!(backoff_for(5), Duration::from_secs(16));
        assert_eq!(backoff_for(6), Duration::from_secs(30));
        assert_eq!(backoff_for(7), Duration::from_secs(30));
        assert_eq!(backoff_for(100), Duration::from_secs(30));
        // 移位溢出防御:u32::MAX 不得 panic/回绕
        assert_eq!(backoff_for(u32::MAX), Duration::from_secs(30));
    }

    #[tokio::test]
    async fn wait_signal_closed_fails_fast() {
        // 契约:发送端已弃(连接代际更替)→ 立即 Err,不悬等 SIGNAL_TIMEOUT
        // (外层 1s 超时保证:若回归为悬等,测试显式失败而非挂死套件)
        let (tx, mut rx) = tokio::sync::broadcast::channel::<SignalEvt>(8);
        drop(tx);
        let r = tokio::time::timeout(
            Duration::from_secs(1),
            wait_signal(&mut rx, "peer", |_| true),
        )
        .await
        .expect("wait_signal must not hang on a closed generation");
        assert!(r.is_err(), "closed generation must surface as Err");
        assert!(
            format!("{:#}", r.unwrap_err()).contains("代际更替"),
            "错误须指认代际更替"
        );
    }

    #[tokio::test]
    async fn wait_signal_lagged_recovers_and_filters() {
        // 契约:Lagged(落后丢帧)→ warn 后继续等,缓冲里尚存的最新帧可达;
        // 非 pred 体/其他发送者被过滤
        let (tx, mut rx) = tokio::sync::broadcast::channel::<SignalEvt>(1);
        let _ = tx.send((
            "b".into(),
            SignalBody::ConnectRequest {
                session_id: vec![0],
                target_dir: String::new(),
                job_id: String::new(),
                display_name: String::new(),
                policy: None,
            },
        ));
        let _ = tx.send((
            "b".into(),
            SignalBody::ConnectAccept {
                session_id: vec![1],
            },
        ));
        let _ = tx.send((
            "b".into(),
            SignalBody::ConnectAccept {
                session_id: vec![2],
            },
        ));
        // 容量 1、落后 2 帧:首收必 Lagged,随后缓冲仅存最新帧([2])
        let got = tokio::time::timeout(
            Duration::from_secs(1),
            wait_signal(&mut rx, "b", |b| {
                matches!(b, SignalBody::ConnectAccept { session_id } if session_id == &vec![2u8])
            }),
        )
        .await
        .expect("wait_signal must not hang on lagged recovery");
        assert!(
            matches!(got, Ok(SignalBody::ConnectAccept { ref session_id }) if session_id == &vec![2u8]),
            "应恢复到缓冲内最新匹配帧: {got:?}"
        );
    }

    #[tokio::test]
    async fn dispatch_queues_requests_without_broadcast_receivers() {
        // 回归锚点(nat-matrix「hub 丢请求窗」):广播无订阅者即丢,来件
        // 请求不可丢——派发不依赖消费方先订阅,ConnectRequest/CtrlConnect
        // 必须落入各自定向通道(取件晚于来件时在缓冲排队);应答类
        // (Accept/Candidates)不进通道,只走广播。广播侧以「随建随弃的
        // 接收端」构造真正的零订阅者场景。
        let (signals, _) = broadcast::channel::<SignalEvt>(8);
        let (req_tx, mut req_rx) = mpsc::channel(8);
        let (ctrl_tx, mut ctrl_rx) = mpsc::channel(8);

        dispatch_body(
            &signals,
            &req_tx,
            &ctrl_tx,
            "peer",
            SignalBody::ConnectRequest {
                session_id: vec![1],
                target_dir: String::new(),
                job_id: String::new(),
                display_name: String::new(),
                policy: None,
            },
        );
        let got = tokio::time::timeout(Duration::from_millis(500), req_rx.recv())
            .await
            .expect("取件晚于来件不得悬等")
            .expect("ConnectRequest 必须在无广播订阅者时入请求通道");
        assert_eq!(got.0, "peer");
        assert!(
            matches!(got.1, SignalBody::ConnectRequest { ref session_id, .. } if session_id == &vec![1u8]),
            "请求通道收到的必须是原帧: {got:?}"
        );

        dispatch_body(
            &signals,
            &req_tx,
            &ctrl_tx,
            "peer",
            SignalBody::CtrlConnect { session_id: vec![2] },
        );
        let got = tokio::time::timeout(Duration::from_millis(500), ctrl_rx.recv())
            .await
            .expect("取件晚于来件不得悬等")
            .expect("CtrlConnect 必须在无广播订阅者时入控制通道");
        assert!(
            matches!(got.1, SignalBody::CtrlConnect { ref session_id } if session_id == &vec![2u8]),
            "控制通道收到的必须是原帧: {got:?}"
        );

        // 应答类不污染请求通道:Accept 只走广播(发起端先订阅后发)
        dispatch_body(
            &signals,
            &req_tx,
            &ctrl_tx,
            "peer",
            SignalBody::ConnectAccept {
                session_id: vec![3],
            },
        );
        let polluted = tokio::time::timeout(Duration::from_millis(100), req_rx.recv()).await;
        assert!(polluted.is_err(), "Accept 不得进入请求通道");
    }

    /// 构造 RelaySession 事件(字段速构;purpose 传空 = 老服务器形态)。
    fn relay_session(
        sid: u64,
        alive: bool,
        tag: Vec<u8>,
        relay_addr: &str,
        reason: &str,
        purpose: &str,
    ) -> ClientEvent {
        ClientEvent::RelaySession {
            sid,
            alive,
            tag,
            relay_addr: relay_addr.into(),
            expires_at: 86_400,
            peer_device: "peer-a".into(),
            reason: reason.into(),
            purpose: purpose.into(),
        }
    }

    #[test]
    fn relay_evt_routes_push_and_dead() {
        // 契约:分配推送(alive)收敛为强形态 Push(字段透传,含 purpose);
        // 死帧按 sid 路由、reason 透传,不要求 tag/addr 形态
        let (tx, mut rx) = broadcast::channel::<RelayEvt>(8);
        route_relay_evt(
            &tx,
            relay_session(7, true, vec![1; 16], "127.0.0.1:9", "", "transport"),
        );
        match rx.try_recv().expect("分配推送必须入广播") {
            RelayEvt::Push { peer, sid, tag, relay_addr, expires_at, purpose } => {
                assert_eq!(peer, "peer-a");
                assert_eq!(sid, 7);
                assert_eq!(tag, [1u8; 16]);
                assert_eq!(relay_addr, "127.0.0.1:9".parse::<SocketAddr>().unwrap());
                assert_eq!(expires_at, 86_400);
                assert_eq!(purpose, "transport", "分配平面须随推送透传");
            }
            other => panic!("须路由为 Push: {other:?}"),
        }

        route_relay_evt(&tx, relay_session(8, false, vec![], "", "quota", ""));
        match rx.try_recv().expect("死帧必须入广播") {
            RelayEvt::Dead { sid, reason } => {
                assert_eq!(sid, 8);
                assert_eq!(reason, "quota", "死因须透传");
            }
            other => panic!("须路由为 Dead: {other:?}"),
        }
    }

    #[test]
    fn relay_evt_malformed_push_dropped() {
        // 契约:tag 非 16 字节 / relay_addr 解析失败 → 该帧丢弃告警(连接
        // 循环不死,后续帧不受扰);死帧不受形态约束照常路由
        let (tx, mut rx) = broadcast::channel::<RelayEvt>(8);
        route_relay_evt(
            &tx,
            relay_session(1, true, vec![0; 15], "127.0.0.1:9", "", "ctrl"), // tag 短 1 字节
        );
        route_relay_evt(
            &tx,
            relay_session(2, true, vec![0; 16], "nope", "", "ctrl"), // 地址不可解析
        );
        route_relay_evt(
            &tx,
            relay_session(3, false, vec![], "nope", "idle", ""), // 死帧:畸形字段无关
        );
        route_relay_evt(
            &tx,
            relay_session(4, true, vec![2; 16], "127.0.0.1:9", "", "ctrl"), // 后续正常帧不受扰
        );
        match rx.try_recv().expect("死帧必须照常路由") {
            RelayEvt::Dead { sid, reason } => {
                assert_eq!((sid, reason.as_str()), (3, "idle"));
            }
            other => panic!("须为死帧: {other:?}"),
        }
        match rx.try_recv().expect("畸形推送之后的正常帧必须可达") {
            RelayEvt::Push { sid, .. } => assert_eq!(sid, 4),
            other => panic!("须为 Push: {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "两条畸形推送均不得入广播");
    }

    #[test]
    fn fallback_lock_same_peer_same_lock_across_generations() {
        // 契约:同 peer 恒同一把锁(缺失即建),不同 peer 各一把;表在 hub
        // 实例上,代际拆卸(uninstall)不换表——兜底串行跨连接代际有效
        let hub = SignalHub::new();
        let a1 = hub.fallback_lock("dev-a");
        let b = hub.fallback_lock("dev-b");
        hub.uninstall(); // 代际拆卸对锁表无影响(表与 conn 无关)
        let a2 = hub.fallback_lock("dev-a");
        assert!(Arc::ptr_eq(&a1, &a2), "同 peer 跨代际恒同一把锁");
        assert!(!Arc::ptr_eq(&a1, &b), "不同 peer 必须不同锁");
    }

    // ---------- 回前台快探(spec §4):入站陈旧判定与 kick 收口链 ----------

    #[test]
    fn stale_false_when_offline_and_recent() {
        // 契约(计划 Step 1):无代际(离线)→ stale 恒 false——不许离线误
        // kick(时间戳 0 = 离线语义,install 打当下 / uninstall 落 0 维护,
        // 见 SignalHub.last_inbound 字段注);note_inbound 后立即查 25s 阈值
        // 必 false(打点即新鲜,健康链路的活跃节奏不误判)。
        let hub = SignalHub::new();
        assert!(
            !hub.stale(RESUME_PROBE_STALE),
            "初始(从未上线,时间戳 0)恒 false"
        );
        // 活跃面:刚打点(当下时刻,非注入)→ 立即查必新鲜
        hub.note_inbound();
        assert!(!hub.stale(RESUME_PROBE_STALE), "刚打点(当下)不陈旧");
        // 代际拆卸收尾(断连/登出/快探 kick 同一入口):落 0 → 恒 false,
        // 重复查证亦然(离线态下不存在「时间自己变陈旧」的翻转)
        hub.uninstall();
        assert!(
            !hub.stale(RESUME_PROBE_STALE),
            "uninstall 后恒 false(离线语义)"
        );
        assert!(!hub.stale(RESUME_PROBE_STALE), "离线态重复查证恒 false");
    }

    #[test]
    fn stale_true_after_silence() {
        // 契约(计划 Step 1):手写入站时间为 26s 前(mock 时钟注入,
        // note_inbound_with 内部变体,不 sleep 对拍)→ stale == true(> 25s
        // 阈值);新帧打点即复活;20s(信令 ping 周期节奏,core Keepalive
        // 默认)不越 25s 阈值——健康链路的保活节奏不误判。
        let hub = SignalHub::new();
        hub.note_inbound_with(now_ms().saturating_sub(26_000));
        assert!(
            hub.stale(RESUME_PROBE_STALE),
            "26s 无入站帧必陈旧(> 25s 阈值)"
        );
        // 新帧打点即复活(回前台后的第一帧/活动帧把判定拉回新鲜)
        hub.note_inbound_with(now_ms());
        assert!(!hub.stale(RESUME_PROBE_STALE), "新帧打点即复活");
        // 20s 入站节奏(信令 20s ping 周期的最坏间隔)< 25s 阈值:不陈旧
        hub.note_inbound_with(now_ms().saturating_sub(20_000));
        assert!(
            !hub.stale(RESUME_PROBE_STALE),
            "20s 入站节奏(信令 ping 周期)不越 25s 阈值"
        );
    }

    #[test]
    fn kick_gates_on_generation_and_flags_once() {
        // 契约(计划 Step 3 的 kick 收口链):离线(时间戳 0 ⟺ 无代际,见
        // last_inbound 字段注不变量)kick 为无操作且不置旗——「不许离线误
        // kick」在 kick 本体再闸一道,缺拆无事可拆;有代际时 kick = 拆代际
        // (uninstall 落 0)+ 置旗恰一次(run 循环 Disconnected 收口臂取走即
        // 清,attempt 回 1s 首跳);拆后离线,再 kick 又是 no-op(旗不残留)。
        let hub = SignalHub::new();
        hub.kick();
        assert!(!hub.take_kick(), "离线 kick 不得置旗");
        hub.note_inbound(); // 进入活跃面(生产中此刻必有代际)
        hub.kick();
        assert!(
            hub.take_kick(),
            "kick 必置旗(Disconnected 臂据此清 attempt)"
        );
        assert!(!hub.take_kick(), "旗一次性:取走即清,退避清零恰一次");
        assert!(
            !hub.stale(RESUME_PROBE_STALE),
            "kick 拆代际落 0,离线语义同步"
        );
        hub.kick();
        assert!(!hub.take_kick(), "拆后再 kick 是 no-op,旗不残留");
    }

    // ---------- kick 真拆(final review C1):代际取消令牌 + 打点代际闸 ----------

    #[test]
    fn kick_noop_without_generation_and_during_handshake() {
        // 契约(C1):无代际两形态——未登录初始态与**握手窗**(连接中、
        // 尚未 install:last_inbound 仍 0,取消令牌也未 armed——arm 在
        // install 前)——kick 都是纯无操作:不置旗、时间戳不落、无令牌
        // 可发。生产对应:握手窗 dispatch 的 stale 恒 false,快探不触发;
        // 此处直驱 kick 本体钉死再闸一道。
        let hub = SignalHub::new();
        hub.kick(); // 初始(未登录)
        assert!(!hub.take_kick(), "无代际 kick 不得置旗");
        assert_eq!(
            hub.last_inbound.load(Ordering::Relaxed),
            0,
            "无代际 kick 不得写时间戳"
        );
        hub.kick(); // 握手窗同态(尚未 install,时间戳恒 0)
        assert!(!hub.take_kick(), "握手窗 kick 不得置旗");
        assert!(hub.kick.read().unwrap().is_none(), "握手窗无令牌可发");
        assert_eq!(
            hub.last_inbound.load(Ordering::Relaxed),
            0,
            "握手窗 kick 后仍离线语义"
        );
    }

    #[tokio::test]
    async fn kick_token_fires_current_generation_only() {
        // 契约(C1 代际隔离):有代际时 kick 发射**本代**令牌(接收端立即
        // 唤醒)+ 拆槽落 0;旧代的 fire 不得触新代接收端——每代一条通道,
        // 隔离由构造保证(新一代 arm 即换通道,旧 fire 的事实对新通道
        // 不可见);新代 kick 唤醒新代接收端。
        let hub = SignalHub::new();
        let (tx1, mut rx1) = watch::channel(false);
        hub.arm_kick(tx1);
        hub.note_inbound(); // 进入活跃面(生产中此刻 gen 已 install)
        hub.kick();
        tokio::time::timeout(Duration::from_millis(500), rx1.changed())
            .await
            .expect("本代 kick 必须唤醒本代令牌接收端(真拆,不等 keepalive)")
            .expect("发送端在场,changed 为 Ok");
        assert!(!hub.stale(RESUME_PROBE_STALE), "kick 拆代际落 0");

        // 新一代令牌:旧代已 fire 的事实不得让它唤醒(隔离锚——共享单通道
        // 或值比较的实现都会在此暴露)
        let (tx2, mut rx2) = watch::channel(false);
        hub.arm_kick(tx2);
        let woke = tokio::time::timeout(Duration::from_millis(150), rx2.changed()).await;
        assert!(woke.is_err(), "新一代令牌不得因旧代 fire 唤醒(每代一条通道)");

        // 新代 kick 照常唤醒新代接收端
        hub.note_inbound();
        hub.kick();
        tokio::time::timeout(Duration::from_millis(500), rx2.changed())
            .await
            .expect("本代 kick 必须唤醒本代令牌接收端")
            .expect("发送端在场,changed 为 Ok");
    }

    #[test]
    fn note_inbound_gated_to_current_generation() {
        // 契约(C1 不变量修复):「时间戳非 0 ⟺ 有代际」——kick 拆代际
        // (uninstall 撤令牌)后,旧事件循环的残余帧(打点带代际闸)不得
        // 再刷新时钟;新代令牌在场时旧代发送端也不得打点(非同通道)。
        // 旧实现无闸:kick 后旧循环一帧即把离线态误翻新鲜,快探判定的
        // 离线语义被搅。
        let hub = SignalHub::new();
        let (tx1, _rx1) = watch::channel(false);
        hub.arm_kick(tx1.clone());
        hub.note_inbound_if_current(&tx1); // 本代现役:打点生效
        assert!(
            hub.last_inbound.load(Ordering::Relaxed) != 0,
            "现役代际的入站帧照常打点"
        );
        hub.kick(); // 拆代际:落 0 + 撤令牌
        assert_eq!(
            hub.last_inbound.load(Ordering::Relaxed),
            0,
            "kick 拆代际落 0"
        );
        hub.note_inbound_if_current(&tx1); // 旧代残余帧:槽空 → 不打点
        assert_eq!(
            hub.last_inbound.load(Ordering::Relaxed),
            0,
            "拆代际后旧代帧不得打点(不变量保真)"
        );
        // 新代 armed 后,旧代发送端(非同通道)仍不得打新代时钟
        let (tx2, _rx2) = watch::channel(false);
        hub.arm_kick(tx2.clone());
        hub.note_inbound_if_current(&tx1);
        assert_eq!(
            hub.last_inbound.load(Ordering::Relaxed),
            0,
            "旧代令牌不得打新代时钟"
        );
        hub.note_inbound_if_current(&tx2); // 新代现役:打点生效
        assert!(
            hub.last_inbound.load(Ordering::Relaxed) != 0,
            "现役新代的入站帧照常打点"
        );
    }
}
