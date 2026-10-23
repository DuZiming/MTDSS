//! 中继 UDP 套接字与中继双角色端点(v2-P1c 客户端中继兜底,传输面)。
//!
//! 服务端中继在单 UDP 端口上按 16 字节会话标签逐包哑转发:本模块把普通
//! tokio UdpSocket 包成 [`quinn::AsyncUdpSocket`],令其上的 QUIC 端点在
//! 引擎零改动的情况下走中继——
//! - 出站:每个数据报前缀 16B 标签、目标一律改写为中继地址(原
//!   `Transmit::destination` 弃用,QUIC 眼中对端地址即中继);
//! - 入站:服务器已剥头,载荷直通上交(源地址自然就是中继地址)。
//!
//! 端点装配(白名单 server config + 调优)与
//! [`crate::transport::endpoint::make_dual_endpoint_tuned`] 一致,可继续
//! 用 punch_connect 以 `[relay_addr]` 为候选互拨。
//!
//! 就绪注册用 tokio 原生 API:发送 `try_send_to` + `poll_send_ready`,
//! 接收 `try_recv_from` + `poll_recv_ready`(WouldBlock 清就绪位后由
//! 轮询方重注册),与 quinn 自带 TokioRuntime socket 的语义同构。
//! 标签过期不在本地处理(expires_at 由上层透传):服务端 120s 空闲
//! 回收 + 每轮重新分配兜底。

use std::collections::HashMap;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use anyhow::{Context as _, Result, anyhow};
use ed25519_dalek::VerifyingKey;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, TokioRuntime, UdpPoller};

use crate::identity::DeviceIdentity;
use crate::transport::endpoint::{TransportTuning, server_rustls_config, transport_config};

/// 会话标签长度(服务端中继按此前缀逐包路由)。
const TAG_LEN: usize = 16;

/// 出站:数据报前缀 16B 标签、目标改写 relay_addr(原 destination 弃用);
/// 入站:服务器已剥头,直通(源地址即 relay_addr)。
#[derive(Debug)]
pub struct RelaySocket {
    io: tokio::net::UdpSocket,
    /// 会话标签:服务端据以路由到对端(本端槽位)。
    tag: [u8; TAG_LEN],
    /// 服务端中继单 UDP 端口地址(出站唯一目标)。
    relay_addr: SocketAddr,
}

impl RelaySocket {
    /// 单个数据报加 16B 标签后发往中继(tokio 无向量 send_to,
    /// 标签与载荷须先拼成连续缓冲)。
    fn send_tagged(&self, payload: &[u8]) -> io::Result<()> {
        let mut dgram = Vec::with_capacity(TAG_LEN + payload.len());
        dgram.extend_from_slice(&self.tag);
        dgram.extend_from_slice(payload);
        match self.io.try_send_to(&dgram, self.relay_addr) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(e),
            Err(e) => {
                // 与 quinn 自带 socket 的错误吞噬语义一致:UDP 发送失败
                // 按丢包处理(QUIC 重传兜底),勿让单包错误杀死连接/驱动
                tracing::debug!("relay send to {}: {e}", self.relay_addr);
                Ok(())
            }
        }
    }
}

impl AsyncUdpSocket for RelaySocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(RelayWritablePoller { sock: self })
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        // 出站钉死:目标一律改写为中继地址,原 destination 不参与路由。
        // 批量(段大小恒定、末段可短)已被 max_transmit_segments 恒 1
        // 关死;上层万一仍给出 GSO 批量,防御性按段拆开逐包加标签,
        // 绝不让一个数据报携带多个 QUIC 包过中继。拆分中途 WouldBlock
        // 会上抛,quinn 会整包重排——已发段重复到达,QUIC 按包号去重。
        match transmit.segment_size.filter(|&s| s > 0) {
            None => self.send_tagged(transmit.contents),
            Some(size) => {
                for seg in transmit.contents.chunks(size) {
                    self.send_tagged(seg)?;
                }
                Ok(())
            }
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if bufs.is_empty() {
            return Poll::Ready(Ok(0)); // quinn 恒传 ≥1 缓冲,防御
        }
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            // 单数据报直收(max_receive_segments 恒 1):try_recv_from
            // 自带就绪检查与 WouldBlock 清除,失败即回环重注册。非
            // WouldBlock 瞬时错(Windows 转发回 WSAECONNRESET/超长截断)
            // 按同构语义吞掉重试,不冒泡杀死端点驱动。
            if let Ok((len, addr)) = self.io.try_recv_from(&mut bufs[0]) {
                meta[0] = RecvMeta {
                    len,
                    stride: len, // 单数据报:stride 即长度(无 GRO)
                    addr,        // 中继转发后的源地址,即中继地址
                    ecn: None,   // tokio 无逐包 ECN 读取,中继路径按无 ECN 运行
                    dst_ip: None,
                };
                return Poll::Ready(Ok(1));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    /// 恒 1:禁 GSO 批量——出站逐包加头,多包合发语义不成立。
    fn max_transmit_segments(&self) -> usize {
        1
    }

    /// 恒 1:禁 GRO 批量——单次 poll_recv 只交一个数据报。
    fn max_receive_segments(&self) -> usize {
        1
    }

    /// false:数据报大小由 max_mtu 压在中继线路界内(1400+16 ≤ 典型路径
    /// MTU),不指望 IP 分片;false 同时放行 quinn 的 MTU 探测(路径
    /// MTU 缩小时黑洞检测自动回退 1200)。
    fn may_fragment(&self) -> bool {
        false
    }
}

/// 写就绪 poller:透传 tokio 的 `poll_send_ready`,注册/唤醒语义与
/// quinn 自带 socket 的 `writable()` future 相同(连接各持一份 poller;
/// tokio 侧单写就绪槽位由最后注册者占据,与 quinn 默认实现一致)。
/// 持 `Arc<RelaySocket>` 借用其内 tokio socket(tokio UdpSocket 不可
/// Clone,quinn 自带实现同样是经 Arc<Self> 借用)。
#[derive(Debug)]
struct RelayWritablePoller {
    sock: Arc<RelaySocket>,
}

impl UdpPoller for RelayWritablePoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.sock.io.poll_send_ready(cx)
    }
}

/// 中继双角色端点:socket 包 [`RelaySocket`] 后交 quinn;服务端白名单/
/// 调优语义与 `make_dual_endpoint_tuned` 一致。此后既能 connect_pinned
/// (拨)又能 accept_pinned(收),punch_connect 以 `[relay_addr]` 为候选
/// 互拨即可成对。须在 tokio 运行时上下文内调用(quinn driver 会 spawn)。
pub fn make_relay_endpoint(
    bind: SocketAddr,
    me: &DeviceIdentity,
    allowed: &HashMap<String, VerifyingKey>,
    tuning: TransportTuning,
    tag: [u8; 16],
    relay_addr: SocketAddr,
) -> Result<quinn::Endpoint> {
    let std_sock = std::net::UdpSocket::bind(bind).context("relay endpoint udp bind")?;
    std_sock
        .set_nonblocking(true)
        .context("relay endpoint udp set_nonblocking")?;
    let io = tokio::net::UdpSocket::from_std(std_sock).context("relay endpoint udp register tokio")?;
    let socket = Arc::new(RelaySocket { io, tag, relay_addr });

    // 与 make_dual_endpoint_tuned 同构:白名单 server config + 调优
    let quic_server = QuicServerConfig::try_from(server_rustls_config(me, allowed)?)?;
    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_server));
    server_config.transport_config(transport_config(tuning));
    // quinn 0.11 实际 API:自定义 socket 须走 new_with_abstract_socket
    // (Endpoint::new 只收 std socket 并以 runtime 默认包装);
    // TokioRuntime 为默认 feature。
    quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(TokioRuntime),
    )
    .map_err(|e| anyhow!("relay endpoint: {e}"))
}
