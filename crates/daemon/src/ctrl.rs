//! 控制通道(发起端连接池 + 响应端常驻循环,Task 4)。
//!
//! 远程浏览 list/stat 走与传输面同一套信令枢纽与打洞原语,只是消息语义
//! 换成 CtrlConnect/CtrlAccept(与 ConnectRequest/ConnectAccept 平行的
//! 变体,常驻接收按变体过滤,互不串扰)与会话内 QUIC 请求-响应
//! (`transport::ctrl::send_request/serve_conn`,每请求一条独立双向流)。
//!
//! 发起端([`CtrlPool`]):本机请求短路 core fs 直调;目录快照离线预检
//! 先于任何打洞(快速失败 code 3);per-device 单飞锁让并发首查不双打洞
//! (不同设备互不阻塞);连接缓存按 `conn.closed()` 惰性驱逐(无 keep-alive,
//! [`CTRL_TUNING`] 60s 空闲上界治理静默死链),请求级失败一律驱逐后整程
//! 重试恰一次。端点随连接一并入池——quinn 端点 drop 会终止其驱动并连带
//! 关闭其上的连接。
//!
//! 响应端([`responder_loop`]):常驻消费枢纽的 CtrlConnect 定向通道
//! (随代际安装即存在,取件晚于上线的窗口里来件在缓冲排队),每条
//! CtrlConnect(按 sid 去重,重发噪声不重复起会话)spawn 一个会话任务
//! (多会话并发互不拖累);会话按对端来的方向天然定为响应侧,编舞按
//! brief 钉的顺序:CtrlAccept → 双方各自建端点 → 互发 Candidates →
//! punch → serve_conn。请求分发把 List/Stat 映射 core fs,version 不符回
//! UNSUPPORTED_VERSION,FsError 原码透传(截断标志原样携带)。

use crate::flow;
use crate::hub::wait_signal;
use crate::state::DaemonState;
use ed25519_dalek::VerifyingKey;
use mtdss_core::signaling::envelope::{seal, SignalBody};
use mtdss_core::transport::ctrl::{send_request, serve_conn};
use mtdss_core::transport::punch;
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 控制通道传输调优:无 keep-alive + 60s 空闲上界——池内空闲连接懒驱逐
/// (下次取用时按 `closed()` 惰性判死),静默死链最迟 60s 显形关闭。
pub(crate) const CTRL_TUNING: mtdss_core::transport::endpoint::TransportTuning =
    mtdss_core::transport::endpoint::TransportTuning {
        keep_alive_interval: None,
        max_idle_timeout: Duration::from_secs(60),
    };

/// 发起端单次 ctrl 请求-响应往返上界(打洞已另算;覆盖慢盘大目录枚举的
/// 合理余量,对端死链靠 QUIC 连接关闭/超时显形)。
const CTRL_SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// CtrlAccept 单次等待窗口:CtrlConnect 本身已由枢纽定向通道保帧(随代际
/// 安装即存在,不存在「订阅前丢弃」窗),但响应端会话任务仍可能有启动
/// 开销(身份加载/目录缓存未命中),Accept 晚到时发起端短窗重发同一
/// CtrlConnect 兜底——任一次等到即过,多余会话随 CTRL_TUNING 空闲上界
/// 自灭(见 [`respond_session`] 注)。
const CTRL_ACCEPT_WINDOW: Duration = Duration::from_millis(2500);
/// CtrlAccept 重发上限(同一会话 id;总预算 3×2.5s ≈ 8s,与请求级 15s
/// 上界相加仍满足被杀对端场景 ≤30s 的有界失败约束)。
const CTRL_ACCEPT_ATTEMPTS: u32 = 3;
/// CtrlAccept 重发间隔(窗口之间的小歇,给响应端留建端点的时间)。
const CTRL_ACCEPT_RETRY_PACE: Duration = Duration::from_millis(400);

/// ctrl 打洞参数(与传输面同规格:budget 10s / 握手 2s,调优换 CTRL_TUNING)。
fn ctrl_punch() -> punch::PunchConfig {
    punch::PunchConfig {
        budget: Duration::from_secs(10),
        handshake_timeout: Duration::from_secs(2),
        tuning: CTRL_TUNING,
    }
}

/// 控制通道错误。RPC 面的码位映射见 [`CtrlErr::rpc`](对端 FsError 的
/// 「远端: 」前缀也收口在那,本枚举只存原文)。
#[derive(Debug)]
pub(crate) enum CtrlErr {
    /// 对端离线(目录快照 online=false/无快照)→ RPC code 3。
    PeerOffline(String),
    /// 打洞失败(punch_err 诊断全文,含本侧 NAT 标签)→ code 1。
    Punch(String),
    /// 对端 FsError(CtrlError 透传;code 即 fs 稳定码)→ code 1。
    Peer { code: String, message: String },
    /// 信令等待/请求往返超时 → code 1。
    Timeout(String),
    /// 其余本地错误 → code 1。
    Internal(String),
}

impl CtrlErr {
    /// RPC 映射(占位契约,Task 5 沿用):离线 code 3;其余 code 1;对端
    /// 错误格式「远端: {code} {message}」,本地错误诊断全文透传。
    pub(crate) fn rpc(&self) -> (i64, String) {
        const RPC_PEER_OFFLINE: i64 = 3;
        match self {
            CtrlErr::PeerOffline(m) => (RPC_PEER_OFFLINE, m.clone()),
            CtrlErr::Peer { code, message } => (1, format!("远端: {code} {message}")),
            CtrlErr::Punch(m) | CtrlErr::Timeout(m) | CtrlErr::Internal(m) => (1, m.clone()),
        }
    }
}

// ---------- 发起端连接池 ----------

/// 池条目:连接 + 保活锚。quinn 端点 drop 会终止端点驱动并连带关闭其上
/// 连接,端点必须随连接一并存活(只随条目入池/出池,永不单独读取)。
#[derive(Clone)]
struct PooledConn {
    _keep: quinn::Endpoint,
    conn: quinn::Connection,
}

/// 发起端连接池:per-device 单飞锁 + 连接缓存。
pub(crate) struct CtrlPool {
    /// per-device 单飞锁(打洞全程串行:并发首查不双打洞;不同设备互不
    /// 阻塞——锁 Arc 在 std 锁内取出,await 期间绝不持 std 锁)。
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// per-device 连接缓存(端点随连接一并保活,见 [`PooledConn`])。
    conns: Mutex<HashMap<String, PooledConn>>,
}

impl CtrlPool {
    pub(crate) fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
        }
    }

    /// 远端列表(本机短路直调;离线快错;池复用/打洞见 [`CtrlPool::request`])。
    pub(crate) async fn list(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        path: &str,
    ) -> Result<mtdss_core::fs::ListOut, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        // device == 本机 device_id → core fs 直调(短路,不建连接)
        if state.self_device_id().as_deref() == Some(device) {
            return mtdss_core::fs::list_dir(path).map_err(local_fs_err);
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::List(mtdss_proto::ctrl::ListRequest {
                path: path.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::List(r)) => Ok(mtdss_core::fs::ListOut {
                entries: r.entries.iter().map(proto_dir_entry).collect(),
                truncated: r.truncated,
            }),
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 远端单条查询(本机短路直调;其余同 [`CtrlPool::list`])。
    pub(crate) async fn stat(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        path: &str,
    ) -> Result<mtdss_core::fs::DirEntryData, CtrlErr> {
        use mtdss_proto::ctrl::ctrl_request::Body as Req;
        use mtdss_proto::ctrl::ctrl_response::Body as Resp;
        if state.self_device_id().as_deref() == Some(device) {
            return mtdss_core::fs::stat_path(path).map_err(local_fs_err);
        }
        let req = CtrlRequest {
            version: mtdss_proto::CTRL_VERSION,
            body: Some(Req::Stat(mtdss_proto::ctrl::StatRequest {
                path: path.to_string(),
            })),
        };
        let resp = self.request(state, device, req).await?;
        match resp.body {
            Some(Resp::Stat(r)) => {
                let e = r
                    .entry
                    .ok_or_else(|| CtrlErr::Internal("对端 stat 响应缺 entry".into()))?;
                Ok(proto_dir_entry(&e))
            }
            Some(Resp::Error(e)) => Err(CtrlErr::Peer {
                code: e.code,
                message: e.message,
            }),
            _ => Err(CtrlErr::Internal("意外的 ctrl 响应体".into())),
        }
    }

    /// 请求主路径:离线预检 → per-device 单飞 → 池命中(连接未死)复用 →
    /// 请求级失败(或死连接)驱逐后整程重试恰一次 → 成功连接入池。
    async fn request(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
        req: CtrlRequest,
    ) -> Result<CtrlResponse, CtrlErr> {
        // 离线预检:目录快照 online=false / 无快照 / 条目缺失一律离线,
        // 先于任何打洞(快错,不悬等信号/打洞超时)。
        if !device_online(state, device) {
            return Err(CtrlErr::PeerOffline(format!("设备离线: {device}")));
        }
        // 单飞:std 锁内取锁 Arc 即放(锁不跨 await),打洞全程持 tokio 锁
        let lock = self.device_lock(device);
        let _guard = lock.lock().await;

        // 池命中且连接未死 → 复用;请求级失败或死连接 → 驱逐(下方整程重试)
        let cached: Option<PooledConn> = {
            let conns = self.conns.lock().unwrap();
            conns.get(device).cloned()
        };
        if let Some(pooled) = cached {
            if conn_alive(&pooled.conn).await {
                match send_request(&pooled.conn, &req, CTRL_SEND_TIMEOUT).await {
                    Ok(resp) => return Ok(resp),
                    Err(e) => {
                        tracing::warn!("ctrl 复用连接请求失败,驱逐后整程重试: {e:#}");
                    }
                }
            }
            self.conns.lock().unwrap().remove(device);
        }

        // 整程打洞(CtrlConnect/CtrlAccept/Candidates/互拨)→ 请求 → 入池
        let (ep, conn) = self.punch_via_hub(state, device).await?;
        let resp = send_request(&conn, &req, CTRL_SEND_TIMEOUT)
            .await
            .map_err(|e| CtrlErr::Timeout(format!("{e:#}")))?;
        self.conns.lock().unwrap().insert(
            device.to_string(),
            PooledConn {
                _keep: ep,
                conn,
            },
        );
        Ok(resp)
    }

    /// per-device 单飞锁(缺失即建;Arc 生命周期随池,同设备恒同一把)。
    fn device_lock(&self, device: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks.entry(device.to_string()).or_default().clone()
    }

    /// 经枢纽打洞:先订阅(对端信令不会早于订阅窗口)→ CtrlConnect →
    /// 等 CtrlAccept → 建双角色端点 → 互发 Candidates → punch_connect。
    /// 返回 (端点, 连接),端点随连接保活(调用方入池)。
    async fn punch_via_hub(
        &self,
        state: &Arc<DaemonState>,
        device: &str,
    ) -> Result<(quinn::Endpoint, quinn::Connection), CtrlErr> {
        let hub = state.hub();
        let local = |e: anyhow::Error| CtrlErr::Internal(format!("{e:#}"));

        // 先订阅再发信令(Accept/Candidates 不会早于订阅窗口,漏帧即失败)
        let mut rx = hub
            .subscribe()
            .ok_or_else(|| CtrlErr::PeerOffline("信令未在线,无法发起控制通道".into()))?;
        let session = state
            .session()
            .ok_or_else(|| CtrlErr::PeerOffline("未登录".into()))?;
        let me = flow::load_identity(&state.home()).map_err(local)?;

        // 对端公钥:现拉目录(REST 为权威,刚上线的对端才在)+ TOFU 固定
        let devices = flow::fetch_devices(&session).await.map_err(local)?;
        let peer_pk = flow::peer_pk_from(&devices, device).map_err(local)?;
        flow::tofu_pin(&state.home(), device, &peer_pk).map_err(local)?;

        let sid = flow::rand_session_id();
        // CtrlConnect → 等 CtrlAccept:短窗口内重发同一 CtrlConnect(见
        // CTRL_ACCEPT_WINDOW 注),任一次等到即过。
        for attempt in 0..CTRL_ACCEPT_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(CTRL_ACCEPT_RETRY_PACE).await;
            }
            hub.relay(
                device,
                &seal(
                    &me,
                    &peer_pk,
                    &SignalBody::CtrlConnect {
                        session_id: sid.clone(),
                    },
                ),
            )
            .await
            .map_err(local)?;
            match tokio::time::timeout(
                CTRL_ACCEPT_WINDOW,
                wait_signal(&mut rx, device, |b| {
                    matches!(b, SignalBody::CtrlAccept { session_id } if *session_id == sid)
                }),
            )
            .await
            {
                Ok(Ok(_accept)) => break,
                Ok(Err(e)) if attempt + 1 == CTRL_ACCEPT_ATTEMPTS => {
                    return Err(CtrlErr::Timeout(format!("{e:#}")));
                }
                Ok(Err(e)) => {
                    tracing::debug!("ctrl: 第 {} 次等待 CtrlAccept 失败: {e:#}", attempt + 1)
                }
                Err(_) if attempt + 1 == CTRL_ACCEPT_ATTEMPTS => {
                    return Err(CtrlErr::Timeout(format!(
                        "等待 {device} 的 CtrlAccept 超时({CTRL_ACCEPT_ATTEMPTS} 次尝试)"
                    )));
                }
                Err(_) => tracing::debug!("ctrl: 第 {} 次等待 CtrlAccept 超窗,重发", attempt + 1),
            }
        }

        // Accept 后建双角色端点,回发本端 Candidates,再等对端 Candidates,互拨
        let (ep, mine, nat_symmetric, _stun_ok) =
            flow::setup_punch_endpoint(&me, device, &peer_pk, &hub.stun(), CTRL_TUNING)
                .map_err(local)?;
        hub.relay(
            device,
            &seal(
                &me,
                &peer_pk,
                &SignalBody::Candidates {
                    session_id: sid.clone(),
                    addrs: mine.iter().map(|a| a.to_string()).collect(),
                },
            ),
        )
        .await
        .map_err(local)?;
        let theirs = wait_signal(&mut rx, device, |b| {
            matches!(b, SignalBody::Candidates { session_id, .. } if *session_id == sid)
        })
        .await
        .map_err(|e| CtrlErr::Timeout(format!("{e:#}")))?;
        let theirs = match theirs {
            SignalBody::Candidates { addrs, .. } => flow::parse_candidates(&addrs),
            other => return Err(CtrlErr::Internal(format!("意外的信令体 {other:?}"))),
        };

        let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
        allowed.insert(device.to_string(), peer_pk);
        let conn = punch::punch_connect(&ep, &me, &allowed, &peer_pk, &theirs, &ctrl_punch())
            .await
            .map_err(|e| {
                CtrlErr::Punch(flow::punch_err(e, nat_symmetric, mine.len()).to_string())
            })?;
        Ok((ep, conn))
    }
}

/// 目录快照离线预检:无快照/条目缺失/online=false 一律视为离线。
/// 快照与公钥缓存同源刷新,缺失条目打洞也无从解析公钥,报离线最诚实。
fn device_online(state: &DaemonState, device: &str) -> bool {
    state
        .devices()
        .and_then(|list| {
            list.iter()
                .find(|d| d["device_id"].as_str() == Some(device))
                .cloned()
        })
        .map(|d| d["online"].as_bool().unwrap_or(false))
        .unwrap_or(false)
}

/// 连接活性:`closed()` future 就绪 = 对端已关闭/连接已死。一次性零等待
/// 轮询(poll_fn,不阻塞、不跨 await 点),池内条目使用前必查。
async fn conn_alive(conn: &quinn::Connection) -> bool {
    use std::future::Future as _;
    let mut closed = Box::pin(conn.closed());
    std::future::poll_fn(move |cx| match closed.as_mut().poll(cx) {
        std::task::Poll::Ready(_) => std::task::Poll::Ready(false),
        std::task::Poll::Pending => std::task::Poll::Ready(true),
    })
    .await
}

/// 本机短路的 core fs 错误映射:与对端 FsError 同语义(CtrlErr::Peer,
/// code 即 fs 稳定码,RPC 层同样加「远端: 」前缀);正式契约 Task 5 收口。
fn local_fs_err(e: mtdss_core::fs::FsError) -> CtrlErr {
    CtrlErr::Peer {
        code: e.code().into(),
        message: e.to_string(),
    }
}

/// proto DirEntry → 数据侧 DirEntryData(逐字段拷贝)。
fn proto_dir_entry(e: &mtdss_proto::ctrl::DirEntry) -> mtdss_core::fs::DirEntryData {
    mtdss_core::fs::DirEntryData {
        name: e.name.clone(),
        path: e.path.clone(),
        size: e.size,
        mtime_ms: e.mtime_ms,
        is_dir: e.is_dir,
    }
}

// ---------- 响应端常驻循环 ----------

/// 响应端等订阅的小步步距(防快败热旋;与常驻接收的轮间小歇同规格)。
const RESPONDER_PACE: Duration = Duration::from_millis(500);

/// 活跃 CtrlConnect 会话的 session id 集合(每代际一份):发起端对
/// CtrlAccept 短窗重发**同一** CtrlConnect,无丢失面的定向通道会逐份送达
/// ——同一 sid 必须恰起一个会话任务,重发噪声不得放大成并发打洞(负载下
/// 多会话同时建端点/收集候选会互相挤兑请求预算)。guard 随会话任务结束
/// (drop)释放该 sid。
type LiveSids = Arc<Mutex<HashSet<Vec<u8>>>>;

/// sid 占位守卫:被 spawn 的会话任务持有,任务收尾即从活跃集合释放。
struct SidGuard {
    live: LiveSids,
    sid: Vec<u8>,
}

impl Drop for SidGuard {
    fn drop(&mut self) {
        self.live.lock().unwrap().remove(&self.sid);
    }
}

/// 响应端常驻任务:登录后取枢纽的 CtrlConnect 定向通道(随代际安装即
/// 存在,取件晚于上线的窗口里来件在通道缓冲排队——广播无订阅者即丢,
/// 来件请求不可丢),每条 CtrlConnect(按 sid 去重,见 [`LiveSids`])spawn
/// 一个会话任务(CtrlAccept → punch → serve_conn,见 [`respond_session`]),
/// 多会话并发。通道关闭(代际更替)回外层重取下一代;登出回外层按登录态
/// 重进;未登录自挂起(通知驱动);收尾由 Daemon shutdown abort。
pub(crate) async fn responder_loop(state: Arc<DaemonState>) {
    let mut login_rx = state.login_watch();
    loop {
        // 未登录:通知驱动挂起(不轮询);发送端随 state 常驻,Err 不可达,
        // 防御性返回。
        while !state.logged_in() {
            if login_rx.changed().await.is_err() {
                return;
            }
        }
        // 等连接代际并取 CtrlConnect 定向通道:枢纽连接建立即可取;离线
        // 期间小步轮询等待,登出翻转即分流。
        let mut ctrl_rx = loop {
            if let Some(rx) = state.hub().take_ctrl_rx() {
                break rx;
            }
            tokio::select! {
                _ = tokio::time::sleep(RESPONDER_PACE) => {}
                r = login_rx.changed() => {
                    if r.is_err() {
                        return;
                    }
                }
            }
        };
        // 消费循环:CtrlConnect → spawn 会话(通道只投递 CtrlConnect,防御
        // 分支忽略其余体);同一 sid 去重恰一个会话;通道关闭(代际更替:
        // 断连/登出)回外层重取新代。
        let live: LiveSids = Arc::default();
        loop {
            match ctrl_rx.recv().await {
                Some((from, SignalBody::CtrlConnect { session_id })) => {
                    // 防御:本机请求走 CtrlPool 短路,不存在自拨
                    if Some(from.as_str()) == state.self_device_id().as_deref() {
                        continue;
                    }
                    // 去重:重发的同一 CtrlConnect 不再重复起会话
                    let claimed = live.lock().unwrap().insert(session_id.clone());
                    if !claimed {
                        continue;
                    }
                    let guard = SidGuard {
                        live: live.clone(),
                        sid: session_id.clone(),
                    };
                    let st = state.clone();
                    tokio::spawn(async move {
                        let _guard = guard;
                        respond_session(st, from, session_id).await;
                    });
                }
                Some(_) => {}
                None => break, // 代际更替(断连/登出):回外层重取新代
            }
        }
    }
}

/// 单会话响应任务:先订阅再发信令 → CtrlAccept(即刻回,不先建端点)→
/// 建双角色端点 → 本端 Candidates → 等对端 Candidates(session id 过滤)→
/// punch_connect → serve_conn(连接存活期服务请求)。任一环节失败只告警
/// 弃会话,不影响其余会话;会话任务无返回值,失败即静默收尾。
///
/// 编舞按 brief 钉的「等 CtrlAccept → 双方各自建端点 → 互发 Candidates」:
/// Accept 先行让发起端尽早解锁(建端点的候选收集耗时落在两侧 Candidates
/// 窗口,该窗口 10s,远比 Accept 窗口宽裕)。
///
/// 发起端对 CtrlAccept 短窗重发同一 CtrlConnect(`CTRL_ACCEPT_WINDOW`);
/// 响应端按 sid 去重(见 [`LiveSids`]),重发只补投递、不重复起会话——
/// CtrlConnect 的第一份到达即起会话,后续重发在响应端被丢弃。
async fn respond_session(state: Arc<DaemonState>, from: String, session_id: Vec<u8>) {
    let hub = state.hub();
    // 先订阅再发信令:对端 Candidates 只会在我方 CtrlAccept 之后到达,
    // 订阅先于 CtrlAccept 即不漏帧。
    let Some(mut rx) = hub.subscribe() else {
        tracing::warn!("ctrl 会话:信令未在线,弃会话");
        return;
    };
    let Ok(me) = flow::load_identity(&state.home()) else {
        tracing::warn!("ctrl 会话:加载本机身份失败,弃会话");
        return;
    };
    // 发送方公钥:枢纽缓存(开箱时已经服务器目录解析 + TOFU 固定,常态
    // 必命中;未命中属防御分支,弃会话等重试)
    let Some(peer_pk) = hub.peer_pk(&from) else {
        tracing::warn!("ctrl 会话:来件方 {from} 不在公钥缓存,弃会话");
        return;
    };
    // Accept 先行:发起端等的就是它,刻不容缓
    if let Err(e) = hub
        .relay(
            &from,
            &seal(
                &me,
                &peer_pk,
                &SignalBody::CtrlAccept {
                    session_id: session_id.clone(),
                },
            ),
        )
        .await
    {
        tracing::warn!("ctrl 会话:回 CtrlAccept 失败,弃会话: {e:#}");
        return;
    }
    let (ep, mine, _nat_symmetric, _stun_ok) = match flow::setup_punch_endpoint(
        &me,
        &from,
        &peer_pk,
        &hub.stun(),
        CTRL_TUNING,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("ctrl 会话:建端点失败,弃会话: {e:#}");
            return;
        }
    };
    if let Err(e) = hub
        .relay(
            &from,
            &seal(
                &me,
                &peer_pk,
                &SignalBody::Candidates {
                    session_id: session_id.clone(),
                    addrs: mine.iter().map(|a| a.to_string()).collect(),
                },
            ),
        )
        .await
    {
        tracing::warn!("ctrl 会话:回 Candidates 失败,弃会话: {e:#}");
        return;
    }
    let theirs = match wait_signal(&mut rx, &from, |b| {
        matches!(b, SignalBody::Candidates { session_id: s, .. } if *s == session_id)
    })
    .await
    {
        Ok(SignalBody::Candidates { addrs, .. }) => flow::parse_candidates(&addrs),
        Ok(other) => {
            tracing::warn!("ctrl 会话:意外的信令体 {other:?},弃会话");
            return;
        }
        Err(e) => {
            tracing::warn!("ctrl 会话:等对端 Candidates 失败,弃会话: {e:#}");
            return;
        }
    };
    let mut allowed: HashMap<String, VerifyingKey> = HashMap::new();
    allowed.insert(from.clone(), peer_pk);
    match punch::punch_connect(&ep, &me, &allowed, &peer_pk, &theirs, &ctrl_punch()).await {
        Ok(conn) => {
            tracing::info!("ctrl 会话已打通(from={from})");
            // 连接存活期服务请求;对端退出/网络断即返回(任务收尾)
            serve_conn(conn, handle_ctrl_request).await;
        }
        Err(e) => {
            tracing::warn!("ctrl 会话:打洞失败,弃会话: {e:#}");
        }
    }
}

/// ctrl 请求分发(serve_conn 的 handler,同步映射 core fs):version 不符
/// → UNSUPPORTED_VERSION(ok=false);List/Stat → core fs,FsError 原码
/// 透传;截断标志原样携带。请求读不出/响应写不出由 serve_conn 逐流兜底。
fn handle_ctrl_request(req: CtrlRequest) -> CtrlResponse {
    use mtdss_proto::ctrl::ctrl_request::Body as Req;
    use mtdss_proto::ctrl::ctrl_response::Body as Resp;
    if req.version != mtdss_proto::CTRL_VERSION {
        return err_response(
            "UNSUPPORTED_VERSION",
            format!(
                "unsupported ctrl version {} (expected {})",
                req.version,
                mtdss_proto::CTRL_VERSION
            ),
        );
    }
    let result: Result<Resp, mtdss_core::fs::FsError> = match req.body {
        Some(Req::List(r)) => mtdss_core::fs::list_dir(&r.path).map(|out| {
            Resp::List(mtdss_proto::ctrl::ListResponse {
                entries: out.entries.iter().map(entry_proto).collect(),
                truncated: out.truncated,
            })
        }),
        Some(Req::Stat(r)) => mtdss_core::fs::stat_path(&r.path).map(|d| {
            Resp::Stat(mtdss_proto::ctrl::StatResponse {
                entry: Some(entry_proto(&d)),
            })
        }),
        None => Err(mtdss_core::fs::FsError::Internal("ctrl 请求缺 body".into())),
    };
    match result {
        Ok(body) => CtrlResponse {
            version: mtdss_proto::CTRL_VERSION,
            ok: true,
            body: Some(body),
        },
        Err(e) => err_response(e.code(), e.to_string()),
    }
}

/// 错误响应(ok=false,CtrlError{code,message};version 回本端版本,
/// 发起端 send_request 按版本比对即可见新端/旧端差异)。
fn err_response(code: &str, message: String) -> CtrlResponse {
    CtrlResponse {
        version: mtdss_proto::CTRL_VERSION,
        ok: false,
        body: Some(mtdss_proto::ctrl::ctrl_response::Body::Error(
            mtdss_proto::ctrl::CtrlError {
                code: code.to_string(),
                message,
            },
        )),
    }
}

/// 数据侧 DirEntryData → proto DirEntry(逐字段拷贝)。
fn entry_proto(d: &mtdss_core::fs::DirEntryData) -> mtdss_proto::ctrl::DirEntry {
    mtdss_proto::ctrl::DirEntry {
        name: d.name.clone(),
        path: d.path.clone(),
        size: d.size,
        mtime_ms: d.mtime_ms,
        is_dir: d.is_dir,
    }
}
