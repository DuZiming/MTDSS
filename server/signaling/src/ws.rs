use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message as _;
use std::sync::Arc;

/// 在线设备的发送端
#[derive(Clone)]
pub struct PeerHandle {
    pub account_id: String,
    pub tx: tokio::sync::mpsc::Sender<Message>,
}

/// WS keepalive 参数:服务端按 ping_interval 发 Ping;
/// silent_limit 内无任何入站帧(含 Pong)即判死断开。
#[derive(Debug, Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: std::time::Duration,
    pub silent_limit: std::time::Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            ping_interval: std::time::Duration::from_secs(20),
            silent_limit: std::time::Duration::from_secs(60),
        }
    }
}

pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

async fn handle_socket(state: Arc<AppState>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    // 1) 第一帧必须是合法 Hello（10s 内）
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next()).await;
    let Ok(Some(Ok(msg))) = first else { return };
    let Message::Binary(bin) = msg else { return };
    let Ok(frame) = WsClientFrame::decode(bin.as_ref()) else { return };
    let Some(ws_client_frame::Frame::Hello(h)) = frame.frame else {
        let _ = sink.send(err(1, "first frame must be hello")).await;
        return;
    };
    // JWT 签名合法且类型为 access（verify_jwt 不检查 typ，这里必须补上）
    let claims = match crate::auth::verify_jwt(&state.jwt_secret, &h.jwt) {
        Ok(c) if c.typ == "access" => c,
        _ => {
            let _ = sink.send(err(1, "bad jwt")).await;
            return;
        }
    };
    // 账号必须仍存在(防已删账号的存量 JWT 继续上线)。置于 device 归属校验
    // 之前:user_remove 会级联清 devices,先查归属只会得到笼统的
    // "device not bound",而这里要给出准确原因 account removed(code 同为 1)
    let account_exists: bool = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM accounts WHERE id=?1",
            [&claims.sub],
            |r| r.get::<_, i64>(0),
        )
        .is_ok()
    };
    if !account_exists {
        let _ = sink.send(err(1, "account removed")).await;
        return;
    }
    // 2) device_id 必须属于该账号且与 claims.device 一致（防冒用他人 device_id）
    let owner: Option<String> = {
        let conn = state.db.lock().unwrap();
        conn.query_row(
            "SELECT account_id FROM devices WHERE device_id=?1",
            [&h.device_id],
            |r| r.get(0),
        )
        .ok()
    };
    if owner.as_deref() != Some(claims.sub.as_str()) || claims.device != h.device_id {
        let _ = sink.send(err(1, "device not bound to account")).await;
        return;
    }
    let account_id = claims.sub.clone();
    let device_id = h.device_id.clone();

    // 3) 进入在线态：注册 presence + 广播给同账号其他设备
    // 注册表里放 clone；task 自留 tx 用于断线时确认注册归属（防旧连接清掉重连的新注册）
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(64);
    state.presence.insert(
        &device_id,
        PeerHandle { account_id: account_id.clone(), tx: tx.clone() },
    );
    broadcast_presence(&state, &account_id, &device_id, true);
    let ok = WsServerFrame {
        frame: Some(ws_server_frame::Frame::HelloOk(HelloOk {
            account_id: account_id.clone(),
            stun_addrs: state.stun_addrs.clone(),
        })),
    };
    let _ = sink.send(Message::Binary(ok.encode_to_vec().into())).await;

    // 4) 主循环：出站转发 + 入站 Relay 处理 + keepalive ping/判死
    let mut sink_opt = Some(sink);
    let mut ping = tokio::time::interval_at(
        tokio::time::Instant::now() + state.keepalive.ping_interval,
        state.keepalive.ping_interval,
    );
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_inbound = std::time::Instant::now();
    loop {
        tokio::select! {
            out = rx.recv() => match (out, sink_opt.as_mut()) {
                (Some(m), Some(s)) => {
                    if s.send(m).await.is_err() {
                        break;
                    }
                }
                _ => break,
            },
            _ = ping.tick() => {
                if last_inbound.elapsed() >= state.keepalive.silent_limit {
                    tracing::info!("ws keepalive: dropping silent client {device_id}");
                    break;
                }
                if let Some(s) = sink_opt.as_mut() {
                    if s.send(Message::Ping(vec![0u8].into())).await.is_err() {
                        break;
                    }
                }
            },
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else { break };
                last_inbound = std::time::Instant::now();
                let Message::Binary(bin) = msg else { continue };
                let Ok(f) = WsClientFrame::decode(bin.as_ref()) else {
                    if let Some(s) = sink_opt.as_mut() {
                        let _ = s.send(err(3, "bad frame")).await;
                    }
                    break;
                };
                match f.frame {
                    Some(ws_client_frame::Frame::Relay(r)) => {
                        if let Err((code, msg)) =
                            state.relay(&device_id, &account_id, &r.to_device, &r.payload)
                        {
                            if let Some(s) = sink_opt.as_mut() {
                                let _ = s.send(err(code, &msg)).await;
                            }
                        }
                    }
                    _ => {
                        if let Some(s) = sink_opt.as_mut() {
                            let _ = s.send(err(3, "expected relay")).await;
                        }
                    }
                }
            }
        }
    }
    // 5) 离线：仅当注册仍是本次连接自己的（未被同设备重连覆盖）才清理 + 广播
    if state.presence.remove_if_current(&device_id, &tx) {
        broadcast_presence(&state, &account_id, &device_id, false);
    }
}

fn err(code: u32, msg: &str) -> Message {
    Message::Binary(
        WsServerFrame {
            frame: Some(ws_server_frame::Frame::Error(Error {
                code,
                message: msg.into(),
            })),
        }
        .encode_to_vec()
        .into(),
    )
}

/// 同账号上下线广播（不含 device_id 本机）
fn broadcast_presence(state: &AppState, account_id: &str, device_id: &str, online: bool) {
    let msg = Message::Binary(
        WsServerFrame {
            frame: Some(ws_server_frame::Frame::Presence(Presence {
                device_id: device_id.into(),
                online,
            })),
        }
        .encode_to_vec()
        .into(),
    );
    for (peer_id, peer) in state.presence.account_peers(account_id) {
        if peer_id != device_id {
            let _ = peer.tx.try_send(msg.clone());
        }
    }
}
