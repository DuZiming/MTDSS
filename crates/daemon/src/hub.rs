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

use crate::session::Session;
use crate::state::DaemonState;
use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::client::ClientEvent;
use mtdss_core::signaling::envelope::{open, SignalBody};
use mtdss_ipc::EventFrame;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};

/// 事件方法名:本机在线状态翻转。
const EV_ONLINE: &str = "event.online";
/// 事件方法名:设备目录全量快照。
const EV_DEVICES: &str = "event.devices";
/// 服务器错误码 1 = 鉴权拒(坏/过期 JWT、设备不属账号;见 ws.rs 握手段)。
const ERR_AUTH: u32 = 1;
/// 重连退避封顶。
const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// 信令广播通道容量(慢订阅者以 Lagged 告警继续,不拖垮枢纽)。
const SIGNAL_CAP: usize = 256;
/// 来件请求定向通道容量(ConnectRequest/CtrlConnect 各一条,每代际各一)。
/// 消费方取件前的来件在缓冲排队;满即丢该帧并 warn(正常调度下不可达:
/// 每请求至多入队一条,128 远超任何活跃会话数)。
const REQUEST_CAP: usize = 128;

/// 已解封验签的信令:`(from, SignalBody)`——枢纽已完成密钥解析、TOFU 固定
/// 与验签,订阅方直接按对端身份消费。
pub(crate) type SignalEvt = (String, SignalBody);

/// 一代连接:relay 句柄 + 信令广播 + 来件请求通道。Sender 随代际 drop →
/// 订阅方收 `Closed`(广播)/ `None`(mpsc)。
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
}

/// 信令枢纽:当前连接代际 + 设备公钥缓存(目录刷新维护)。
pub(crate) struct SignalHub {
    conn: std::sync::RwLock<Option<std::sync::Arc<ConnHandle>>>,
    pks: std::sync::RwLock<HashMap<String, VerifyingKey>>, // 设备→公钥(目录刷新维护)
}

impl SignalHub {
    pub(crate) fn new() -> Self {
        Self {
            conn: RwLock::new(None),
            pks: RwLock::new(HashMap::new()),
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
    fn install(&self, handle: Arc<ConnHandle>) {
        *self.conn.write().unwrap() = Some(handle);
    }

    /// 拆除当前连接代际(断连/登出收尾;订阅方随之收 Closed 快速收口)。
    fn uninstall(&self) {
        *self.conn.write().unwrap() = None;
    }

    /// 目录全量刷新时同步重建公钥缓存(REST 为权威,整体替换)。条目缺
    /// 字段/畸形公钥静默跳过:与发送侧 `peer_pk_from` 的硬错误不同,这里是
    /// 缓存维护——缺失设备走「未命中先刷新」路径在开箱时显式丢弃。
    fn load_pks(&self, raw: &[Value]) {
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

/// 一轮连接(握手 + 事件循环)的结局,交由外层主循环分派重连策略。
enum Outcome {
    /// 会话消失(登出):回到外层等待登录。
    LoggedOut,
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
        while let Some(session) = state.session() {
            let outcome =
                run_connected(&state, &events, &session, &mut login_rx, &mut attempt).await;
            match outcome {
                Outcome::LoggedOut => break,
                Outcome::AuthDenied => {
                    // 断连即落离线(翻转才发 event.online false,重复退避不重发)
                    go_offline(&state, &events);
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

/// 一轮在线:连信令 → 握手 → 建连接代际 → 全量快照上线 → 事件循环,直至
/// 断连/登出。断连/登出走 `go_offline` 统一拆除代际(订阅方收 Closed)。
async fn run_connected(
    state: &Arc<DaemonState>,
    events: &broadcast::Sender<EventFrame>,
    session: &Session,
    login_rx: &mut watch::Receiver<bool>,
    attempt: &mut u32,
) -> Outcome {
    let (client, mut ev) = match crate::flow::signaling_connect(session, &session.device_id).await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%err, "信令连接失败");
            return Outcome::Disconnected;
        }
    };
    // 握手:等 HelloOk(stun 本任务缓存后随代际下发)。握手期间收到的
    // Presence/Relayed 直接忽略——紧随其后的全量快照自带此刻真相,早到的
    // 增量不必暂存。
    let stun = loop {
        tokio::select! {
            frame = ev.recv() => match frame {
                Some(ClientEvent::HelloOk { stun_addrs }) => break stun_addrs,
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
                Some(_) => continue, // Presence / Relayed:见上注
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
    // 建立连接代际:relay 句柄 + 信令广播 + 来件请求通道(旧代随安装换下,
    // 广播订阅方收 Closed、请求通道消费方收 None)。请求通道随安装即建好:
    // 消费方取件之前的 ConnectRequest/CtrlConnect 在通道缓冲排队,不再依赖
    // 订阅时机(广播无订阅者即丢,来件请求不可丢;应答类由发起端先订阅后
    // 发,无丢失面,维持广播)。
    let (sig, _) = broadcast::channel(SIGNAL_CAP);
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
    });
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

    // 事件循环:Relayed 统一开箱广播;Presence 增量维护快照;断连/登出收尾
    loop {
        tokio::select! {
            frame = ev.recv() => match frame {
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

/// 目录投影:每台设备收敛为 {device_id,name,platform,online}(pubkey 不出
/// IPC 面);`self_id` 的条目 online 强制为 `self_online`——本机是否在线是
/// 本地连接态事实,不取目录快照值。
fn build_snapshot(raw: &[Value], self_id: Option<&str>, self_online: bool) -> Vec<Value> {
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
            json!({
                "device_id": id,
                "name": name,
                "platform": d["platform"].as_str().unwrap_or_default(),
                "online": online,
            })
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
            SignalBody::ConnectRequest { session_id: vec![0] },
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
            SignalBody::ConnectRequest { session_id: vec![1] },
        );
        let got = tokio::time::timeout(Duration::from_millis(500), req_rx.recv())
            .await
            .expect("取件晚于来件不得悬等")
            .expect("ConnectRequest 必须在无广播订阅者时入请求通道");
        assert_eq!(got.0, "peer");
        assert!(
            matches!(got.1, SignalBody::ConnectRequest { ref session_id } if session_id == &vec![1u8]),
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
}
