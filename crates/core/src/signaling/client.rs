use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use mtdss_proto::ws::*;
use prost::Message as _;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// 客户端 keepalive 参数:按 ping_interval 发 Ping;
/// silent_limit 内无任何入站帧(含 Pong)即判死,发 Closed 并退出。
#[derive(Debug, Clone, Copy)]
pub struct Keepalive {
    pub ping_interval: Duration,
    pub silent_limit: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self { ping_interval: Duration::from_secs(20), silent_limit: Duration::from_secs(60) }
    }
}

/// 出站命令:业务二进制帧。keepalive 定时臂直接经 sink 发 Ping,不经命令通道。
enum Outbound {
    Binary(Vec<u8>),
}

/// 从信令服务器流出的高层事件。HelloOk 属于握手确认,不外露。
#[derive(Debug)]
pub enum ClientEvent {
    Relayed { from: String, payload: Vec<u8> },
    Presence { device_id: String, online: bool },
    Error { code: u32, message: String },
    Closed,
}

/// 信令 WebSocket 客户端的廉价克隆句柄。仅负责传输:
/// 端到端信封加解密由上层流程负责。
#[derive(Clone)]
pub struct SignalingClient {
    tx: mpsc::Sender<Outbound>,
}

impl SignalingClient {
    /// 连接并以默认 keepalive(20s/60s)握手。
    pub async fn connect(
        url: &str,
        jwt: &str,
        device_id: &str,
    ) -> Result<(Self, mpsc::Receiver<ClientEvent>)> {
        Self::connect_with(url, jwt, device_id, None, Keepalive::default()).await
    }

    /// 同 `connect`,keepalive 参数可注入(测试用亚秒级);`ca` 指定服务端
    /// TLS CA 证书(PEM,自签场景)用于 wss,`None` 走默认 webpki 根。
    pub async fn connect_with(
        url: &str,
        jwt: &str,
        device_id: &str,
        ca: Option<&std::path::Path>,
        ka: Keepalive,
    ) -> Result<(Self, mpsc::Receiver<ClientEvent>)> {
        let (ws, _resp) = tokio_tungstenite::connect_async_tls_with_config(
            url,
            None,
            false,
            ws_connector(ca)?,
        )
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

        tokio::spawn(async move {
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
                                break;
                            }
                        }
                        // 所有句柄都已丢弃:结束任务,socket 随之关闭
                        None => break,
                    },
                    _ = ping.tick() => {
                        if last_inbound.elapsed() >= ka.silent_limit {
                            let _ = ev_tx.send(ClientEvent::Closed).await;
                            break;
                        }
                        if sink.send(WsMessage::Ping(vec![])).await.is_err() {
                            break;
                        }
                    },
                    inbound = stream.next() => match inbound {
                        Some(Ok(msg)) => {
                            last_inbound = std::time::Instant::now();
                            let WsMessage::Binary(bin) = msg else { continue };
                            let Ok(f) = WsServerFrame::decode(bin.as_slice()) else { continue };
                            let ev = match f.frame {
                                Some(ws_server_frame::Frame::HelloOk(_)) => continue,
                                Some(ws_server_frame::Frame::Relayed(r)) => ClientEvent::Relayed {
                                    from: r.from_device,
                                    payload: r.payload,
                                },
                                Some(ws_server_frame::Frame::Presence(p)) => ClientEvent::Presence {
                                    device_id: p.device_id,
                                    online: p.online,
                                },
                                Some(ws_server_frame::Frame::Error(e)) => ClientEvent::Error {
                                    code: e.code,
                                    message: e.message,
                                },
                                None => continue,
                            };
                            if ev_tx.send(ev).await.is_err() {
                                break;
                            }
                        }
                        // socket 关闭(服务器拒绝或断线)
                        _ => {
                            let _ = ev_tx.send(ClientEvent::Closed).await;
                            break;
                        }
                    },
                }
            }
        });

        Ok((Self { tx: cmd_tx }, ev_rx))
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
}

/// 由 PEM CA 构造 wss 连接器;ca 为 None → None(默认 webpki 根)。
/// 公开以便测试。
pub fn ws_connector(ca: Option<&std::path::Path>) -> Result<Option<tokio_tungstenite::Connector>> {
    let Some(p) = ca else { return Ok(None) };
    let pem =
        std::fs::read_to_string(p).with_context(|| format!("read ca {}", p.display()))?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut pem.as_bytes()) {
        roots.add(cert?).map_err(|e| anyhow!("bad ca cert: {e:?}"))?;
    }
    anyhow::ensure!(!roots.is_empty(), "ca pem contains no certificates");
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
        cfg,
    ))))
}
