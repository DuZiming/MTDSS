//! 常驻在线循环:已登录时维持与信令服务器的长连接,维护设备目录快照与
//! 在线状态广播。
//!
//! 状态机(单任务,通知驱动,无忙等):
//! 未登录 → watch 通知挂起等待;登录翻转即醒,连信令 → HelloOk → 全量目录
//! 快照(`event.devices`)→ 置在线(`event.online true`);此后 Presence
//! 增量就地更新缓存并重发全量快照(未知设备走全量刷新补齐)。断连
//! (Error/Closed)→ 置离线(`event.online false`)→ 指数退避重连
//! (1s×2^(n-1) 封顶 30s,连接成功即清零);鉴权拒(服务器 code=1,见
//! server/signaling/src/ws.rs 握手段)先 `account::refresh` 轮换令牌并写回
//! session.json 再退避重连。登出翻转 → 弃连回到等待登录态。

use crate::session::Session;
use crate::state::DaemonState;
use mtdss_core::signaling::client::ClientEvent;
use mtdss_ipc::EventFrame;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

/// 事件方法名:本机在线状态翻转。
const EV_ONLINE: &str = "event.online";
/// 事件方法名:设备目录全量快照。
const EV_DEVICES: &str = "event.devices";
/// 服务器错误码 1 = 鉴权拒(坏/过期 JWT、设备不属账号;见 ws.rs 握手段)。
const ERR_AUTH: u32 = 1;
/// 重连退避封顶。
const BACKOFF_CAP: Duration = Duration::from_secs(30);

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

/// 在线循环主体:由 `Daemon::start` 拉起,随 Daemon shutdown 中止。
/// 未登录自挂起(watch 通知),登录翻转即上线;登出翻转自动回等。
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

/// 一轮在线:连信令 → 握手 → 全量快照上线 → 事件循环,直至断连/登出。
/// `_client`(句柄)与 `ev` 随返回 drop,WS 随之关闭(登出路径同此断开)。
async fn run_connected(
    state: &Arc<DaemonState>,
    events: &broadcast::Sender<EventFrame>,
    session: &Session,
    login_rx: &mut watch::Receiver<bool>,
    attempt: &mut u32,
) -> Outcome {
    let (_client, mut ev) = match crate::flow::signaling_connect(session, &session.device_id).await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%err, "信令连接失败");
            return Outcome::Disconnected;
        }
    };
    // 握手:等 HelloOk(stun 本任务不用)。握手期间收到的 Presence/Relayed
    // 直接忽略——紧随其后的全量快照自带此刻真相,早到的增量不必暂存。
    loop {
        tokio::select! {
            frame = ev.recv() => match frame {
                Some(ClientEvent::HelloOk { .. }) => break,
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
    }
    *attempt = 0; // 连接成功:退避清零
    // 全量目录快照:REST 为权威,本机条目 online=自身连接态(此刻已连接)
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

    // 事件循环:Presence 增量更新缓存重发快照;断连/登出收尾
    loop {
        tokio::select! {
            frame = ev.recv() => match frame {
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
                Some(ClientEvent::HelloOk { .. }) | Some(ClientEvent::Relayed { .. }) => {}
            },
            _ = login_rx.changed() => {
                if !state.logged_in() {
                    return Outcome::LoggedOut; // _client/ev drop → WS 关闭
                }
            }
        }
    }
}

/// 全量刷新目录(REST 为权威)并广播 `event.devices` 快照。
/// `pub(crate)`:常驻接收循环轮首同样调用——接收连接持有 presence 槽期间
/// 在线循环收不到事件,接收侧必须承担同一份快照维护(同一投影,不重复发射)。
pub(crate) async fn refresh_snapshot(
    state: &DaemonState,
    events: &broadcast::Sender<EventFrame>,
    session: &Session,
) -> anyhow::Result<()> {
    let raw = crate::account::fetch_devices(session).await?;
    let snap = build_snapshot(&raw, state.self_device_id().as_deref(), true);
    state.set_devices(snap);
    emit_current(state, events);
    Ok(())
}

/// 把当前缓存目录作为 `event.devices` 全量快照广播。
pub(crate) fn emit_current(state: &DaemonState, events: &broadcast::Sender<EventFrame>) {
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

/// 上线 → 离线:仅真翻转时发 `event.online false`;缓存里的本机条目保留
/// 并置 online=false(快照不缺本机,断连期间 devices RPC 亦自洽)。
fn go_offline(state: &DaemonState, events: &broadcast::Sender<EventFrame>) {
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
}
