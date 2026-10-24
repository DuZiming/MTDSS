use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// 中继分配应答超时:与 daemon flow::RELAY_TIMEOUT 同格(10s)。
const RELAY_ALLOC_TIMEOUT: Duration = Duration::from_secs(10);

/// HelloOk 下发的中继资格(旧服务器/未开通 → enabled=false,永不发起分配)
#[derive(Debug, Clone, Copy)]
pub struct RelayEntitlement {
    pub enabled: bool,
    pub rate_kbps: u32,
    pub quota_remaining: u64,
}

/// 中继分配请求的结局:成功 → Allocated(凭 tag 到 relay_addr 握手);
/// 服务器拒(分配窗口内 Error 2/4/5/6)→ Denied(码与消息透传)。
#[derive(Debug, Clone)]
pub enum AllocOutcome {
    Allocated {
        sid: u64,
        tag: Vec<u8>,
        relay_addr: String,
        expires_at: i64,
        rate_kbps: u32,
    },
    Denied {
        code: u32,
        message: String,
    },
}

/// 客户端 keepalive 参数:按 ping_interval 发 Ping;
/// silent_limit 内无任何入站帧(含 Pong)即判死,发 Closed 并退出。
#[derive(Debug, Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: Duration,
    pub silent_limit: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(20),
            silent_limit: Duration::from_secs(60),
        }
    }
}

/// 出站命令:业务二进制帧。keepalive 定时臂直接经 sink 发 Ping,不经命令通道。
enum Outbound {
    Binary(Vec<u8>),
}

/// 从信令服务器流出的高层事件。HelloOk 是握手确认,携带服务器通告的 STUN
/// 地址与本账号的中继资格(旧服务器无资格字段 → prost 缺省 enabled=false)。
#[derive(Debug)]
pub enum ClientEvent {
    HelloOk {
        stun_addrs: Vec<String>,
        relay: RelayEntitlement,
    },
    Relayed {
        from: String,
        payload: Vec<u8>,
    },
    Presence {
        device_id: String,
        online: bool,
    },
    Error {
        code: u32,
        message: String,
    },
    Closed,
    /// 中继会话状态推送(v2):保活/对端存活通告;分配应答(RelayAllocated)
    /// 不在此面——它经 [`SignalingClient::relay_allocate`] 的 oneshot 回填,
    /// 无主时 warn 丢弃。reason 为死因("quota"/"disabled"/"idle"):仅
    /// alive:false 死帧携带,存活帧与旧服务器形态均为空串;T4/T5 据以
    /// 分派终局/可重试语义。
    RelaySession {
        sid: u64,
        alive: bool,
        tag: Vec<u8>,
        relay_addr: String,
        expires_at: i64,
        peer_device: String,
        reason: String,
    },
}

/// WS 任务本地退出路径的统一序列:先发诊断 `Error(code=0, 原因)` 再发
/// `Closed`——调用方永远能区分"为什么断"而不是只看到流终止。语义不变:
/// 调用方仍一律以 Closed(或接收方已丢弃时的流尽 None)终止循环。
/// code=0 保留给本侧传输诊断,与服务器侧错误码(1=鉴权、2=目标离线、
/// 3=坏帧)互不冲突。接收方已丢弃时不阻塞(send 失败即返回)。
async fn emit_exit(ev_tx: &mpsc::Sender<ClientEvent>, reason: &str) {
    let _ = ev_tx
        .send(ClientEvent::Error {
            code: 0,
            message: reason.to_string(),
        })
        .await;
    let _ = ev_tx.send(ClientEvent::Closed).await;
}

/// 信令 WebSocket 客户端的廉价克隆句柄。仅负责传输:
/// 端到端信封加解密由上层流程负责。
#[derive(Clone)]
pub struct SignalingClient {
    tx: mpsc::Sender<Outbound>,
    /// 中继分配应答槽:relay_allocate 存入 oneshot 发送端,WS 任务收到
    /// RelayAllocated(或归因分配的 Error 2/4/5/6)时取槽回填。std Mutex
    /// 不跨 await,锁内只有 take/put。任务退出统一摘槽,等待方立即 Err。
    pending_alloc: Arc<std::sync::Mutex<Option<oneshot::Sender<AllocOutcome>>>>,
    /// 分配串行锁:同一连接同时至多一个分配在途(每客户端一把)。
    alloc_lock: Arc<tokio::sync::Mutex<()>>,
}

impl SignalingClient {
    /// 连接并以默认 keepalive(20s/60s)握手。
    /// 服务器拒绝(坏 JWT)不会在连接时失败:随后事件流会依次出现 Error 与 Closed。
    pub async fn connect(
        url: &str,
        jwt: &str,
        device_id: &str,
    ) -> Result<(Self, mpsc::Receiver<ClientEvent>)> {
        Self::connect_with(url, jwt, device_id, None, Keepalive::default()).await
    }

    /// 同 `connect`,keepalive 参数可注入(测试用亚秒级);`ca` 指定服务端
    /// TLS CA 证书(PEM,自签场景)用于 wss,`None` 走默认 webpki 根。
    /// 服务器拒绝(坏 JWT)不会在连接时失败:随后事件流会依次出现 Error 与 Closed。
    pub async fn connect_with(
        url: &str,
        jwt: &str,
        device_id: &str,
        ca: Option<&std::path::Path>,
        ka: Keepalive,
    ) -> Result<(Self, mpsc::Receiver<ClientEvent>)> {
        let (ws, _resp) =
            tokio_tungstenite::connect_async_tls_with_config(url, None, false, ws_connector(ca)?)
                .await
                .context("connect signaling ws")?;
        let (mut sink, mut stream) = ws.split();

        let hello = WsClientFrame {
            frame: Some(ws_client_frame::Frame::Hello(Hello {
                device_id: device_id.into(),
                jwt: jwt.into(),
            })),
        };
        sink.send(WsMessage::Binary(hello.encode_to_vec())).await?;

        let (ev_tx, ev_rx) = mpsc::channel(64);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Outbound>(64);
        let pending_alloc: Arc<std::sync::Mutex<Option<oneshot::Sender<AllocOutcome>>>> =
            Arc::new(std::sync::Mutex::new(None));
        // 任务持有槽的克隆;原 Arc 归调用句柄,两侧共用同一把槽锁
        let task_pending = pending_alloc.clone();

        tokio::spawn(async move {
            let pending_alloc = task_pending;
            let mut ping = tokio::time::interval_at(
                tokio::time::Instant::now() + ka.ping_interval,
                ka.ping_interval,
            );
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_inbound = std::time::Instant::now();
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => match cmd {
                        Some(Outbound::Binary(bytes)) => {
                            if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                                emit_exit(&ev_tx, "signaling transport write failed").await;
                                break;
                            }
                        }
                        // 所有句柄都已丢弃:本地主动关闭,结束任务即可
                        // (调用方不会再读事件,无需退出序列)
                        None => break,
                    },
                    _ = ping.tick() => {
                        if last_inbound.elapsed() >= ka.silent_limit {
                            emit_exit(&ev_tx, "signaling silent limit").await;
                            break;
                        }
                        if sink.send(WsMessage::Ping(vec![])).await.is_err() {
                            emit_exit(&ev_tx, "signaling transport write failed").await;
                            break;
                        }
                    },
                    inbound = stream.next() => match inbound {
                        Some(Ok(msg)) => {
                            last_inbound = std::time::Instant::now();
                            let WsMessage::Binary(bin) = msg else { continue };
                            let Ok(f) = WsServerFrame::decode(bin.as_slice()) else { continue };
                            let ev = match f.frame {
                                Some(ws_server_frame::Frame::HelloOk(m)) => {
                                    ClientEvent::HelloOk {
                                        stun_addrs: m.stun_addrs,
                                        relay: RelayEntitlement {
                                            enabled: m.relay_enabled,
                                            rate_kbps: m.relay_rate_kbps,
                                            quota_remaining: m.relay_quota_remaining_bytes,
                                        },
                                    }
                                }
                                Some(ws_server_frame::Frame::Relayed(r)) => ClientEvent::Relayed {
                                    from: r.from_device,
                                    payload: r.payload,
                                },
                                Some(ws_server_frame::Frame::Presence(p)) => ClientEvent::Presence {
                                    device_id: p.device_id,
                                    online: p.online,
                                },
                                Some(ws_server_frame::Frame::Error(e)) => {
                                    // 分配 pending 窗口内的 Error 2/4/5/6 一律归因分配
                                    //(计划裁定):回填 Denied、不外发事件——误吞并发
                                    // relay 转发的 Error 2 仅致该轮兜底失败,不破坏连接。
                                    // 窗口外(1=鉴权、3=坏帧、0=本地诊断)流向不变。
                                    let attributed = if matches!(e.code, 2 | 4 | 5 | 6) {
                                        pending_alloc.lock().unwrap().take()
                                    } else {
                                        None
                                    };
                                    match attributed {
                                        Some(tx) => {
                                            let _ = tx.send(AllocOutcome::Denied {
                                                code: e.code,
                                                message: e.message,
                                            });
                                            continue;
                                        }
                                        None => ClientEvent::Error {
                                            code: e.code,
                                            message: e.message,
                                        },
                                    }
                                }
                                Some(ws_server_frame::Frame::RelayAllocated(m)) => {
                                    // 分配应答:仅在 pending 窗口内有主,取槽回填
                                    // oneshot(不经事件流);无主(对端收到的分配
                                    // 推送/迟到帧)warn 丢弃。
                                    match pending_alloc.lock().unwrap().take() {
                                        Some(tx) => {
                                            let _ = tx.send(AllocOutcome::Allocated {
                                                sid: m.sid,
                                                tag: m.tag,
                                                relay_addr: m.relay_addr,
                                                expires_at: m.expires_at,
                                                rate_kbps: m.rate_kbps,
                                            });
                                        }
                                        None => tracing::warn!(
                                            sid = m.sid,
                                            addr = %m.relay_addr,
                                            "无主的 RelayAllocated,丢弃"
                                        ),
                                    }
                                    continue;
                                }
                                Some(ws_server_frame::Frame::RelaySession(m)) => {
                                    ClientEvent::RelaySession {
                                        sid: m.sid,
                                        alive: m.alive,
                                        tag: m.tag,
                                        relay_addr: m.relay_addr,
                                        expires_at: m.expires_at,
                                        peer_device: m.peer_device,
                                        reason: m.reason,
                                    }
                                }
                                None => continue,
                            };
                            if ev_tx.send(ev).await.is_err() {
                                break;
                            }
                        }
                        // socket 关闭(服务器拒绝或断线)
                        _ => {
                            emit_exit(&ev_tx, "signaling connection closed").await;
                            break;
                        }
                    },
                }
            }
            // 任务退出统一收口:在途分配的等待方立即 Err,不悬等超时
            pending_alloc.lock().unwrap().take();
        });

        Ok((
            Self {
                tx: cmd_tx,
                pending_alloc,
                alloc_lock: Arc::new(tokio::sync::Mutex::new(())),
            },
            ev_rx,
        ))
    }

    /// 请求服务器把 payload 转发给 `to_device`(同账号)。
    pub async fn relay(&self, to_device: &str, payload: &[u8]) -> Result<()> {
        let f = WsClientFrame {
            frame: Some(ws_client_frame::Frame::Relay(Relay {
                to_device: to_device.into(),
                payload: payload.to_vec(),
            })),
        };
        self.tx
            .send(Outbound::Binary(f.encode_to_vec()))
            .await
            .context("signaling client gone")
    }

    /// 请求与 to_device 的中继分配。本连接内串行(内部锁);响应经 pending
    /// oneshot 关联:RelayAllocated → Allocated,Error 2/4/5/6(窗口内一律归因
    /// 分配,见 [`ClientEvent`] 注)→ Denied;10s 超时/连接断 → Err。
    pub async fn relay_allocate(&self, to_device: &str) -> Result<AllocOutcome> {
        let _serial = self.alloc_lock.lock().await;
        let (tx, mut rx) = oneshot::channel();
        {
            let mut slot = self.pending_alloc.lock().unwrap();
            // 串行锁保证进入时槽必空(上一轮收口路径均摘槽);防御性钉住
            anyhow::ensure!(slot.is_none(), "previous relay allocate still pending");
            *slot = Some(tx);
        }
        let f = WsClientFrame {
            frame: Some(ws_client_frame::Frame::RelayAllocate(RelayAllocate {
                to_device: to_device.into(),
            })),
        };
        if self
            .tx
            .send(Outbound::Binary(f.encode_to_vec()))
            .await
            .is_err()
        {
            // WS 任务已退出。摘槽两侧分工:任务退出序列的统一摘槽覆盖任务
            // 侧收口;但任务早于本调用退出的时序里,退出摘槽已跑过、本分支
            // 刚放入槽的 sender 无人回收——此处自摘(否则下一轮分配误报
            // 「previous relay allocate still pending」)
            self.pending_alloc.lock().unwrap().take();
            return Err(anyhow!("signaling client gone"));
        }
        // 借用等待(&mut Receiver 亦为 Future):超时分支仍持 rx,可收边缘
        // 送达的迟到应答
        match tokio::time::timeout(RELAY_ALLOC_TIMEOUT, &mut rx).await {
            Ok(Ok(outcome)) => Ok(outcome),
            // 发送端被丢弃:WS 任务退出(断连)统一摘槽,等待方立即 Err
            Ok(Err(_)) => Err(anyhow!("signaling connection closed during relay allocate")),
            Err(_) => {
                // 超时:摘槽收口。摘到 = 本轮确超时(此后迟到的应答无主,warn
                // 丢弃);摘不到 = 响应恰在窗口边缘被 WS 任务取走回填,再收一次
                // 即得,不浪费迟到应答(发送端已消耗,必然立即完成)
                if self.pending_alloc.lock().unwrap().take().is_some() {
                    return Err(anyhow!("relay allocate timed out"));
                }
                rx.await
                    .map_err(|_| anyhow!("signaling connection closed during relay allocate"))
            }
        }
    }
}

/// 由 PEM CA 构造 wss 连接器;ca 为 None → None(默认 webpki 根)。
/// 公开以便测试。
pub fn ws_connector(ca: Option<&std::path::Path>) -> Result<Option<tokio_tungstenite::Connector>> {
    let Some(p) = ca else { return Ok(None) };
    let pem = std::fs::read_to_string(p).with_context(|| format!("read ca {}", p.display()))?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut pem.as_bytes()) {
        roots
            .add(cert?)
            .map_err(|e| anyhow!("bad ca cert: {e:?}"))?;
    }
    anyhow::ensure!(!roots.is_empty(), "ca pem contains no certificates");
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(tokio_tungstenite::Connector::Rustls(
        std::sync::Arc::new(cfg),
    )))
}
