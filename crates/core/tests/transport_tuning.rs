//! 空闲超时显形静默死亡;keep-alive 维持静默连接。
//!
//! quinn 默认既不保活也不设空闲超时:对端静默死亡(进程被杀/断电/
//! NAT 重绑)会让读写永久挂起、重连机制永不触发。两个用例各验证一半:
//! - 无 keep-alive + 短空闲超时:双向静默连接必须在超时内显形关闭;
//! - keep-alive + 同样短的空闲超时:静默数倍于超时仍存活且可用。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::endpoint::{self as ep, TransportTuning};
use quinn::Connection;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::Duration;

/// 同 tuning 的已建连接对:A、B 各自 make_dual_endpoint_tuned(回环),
/// A connect_pinned_tuned 拨 B,B accept_pinned——复用 punch.rs 测试的
/// dual() 模式,两侧同 tuning。accept 必须与 connect 并发轮询(接受令牌
/// 只来自 accept_pinned,顺序 await 会互相等待而死锁)。
/// 端点句柄在本函数内丢弃:quinn 中存活的连接令端点 driver 继续运转,
/// 连接与超时行为不受句柄丢弃影响。
async fn tuned_pair(t: TransportTuning) -> (Connection, Connection) {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    // A 的服务端白名单:B;B 的:A
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a.public_key())].into_iter().collect();

    let sa = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sb = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr_b = sb.local_addr().unwrap();
    let ep_a = ep::make_dual_endpoint_tuned(sa, &a, &allowed_a, t).unwrap();
    let ep_b = ep::make_dual_endpoint_tuned(sb, &b, &allowed_b, t).unwrap();

    let acc = {
        let ep = ep_b.clone();
        let allowed = allowed_b.clone();
        tokio::spawn(async move { ep::accept_pinned(&ep, &allowed).await.unwrap() })
    };
    let ca = ep::connect_pinned_tuned(&ep_a, addr_b, &b.public_key(), &a, t)
        .await
        .unwrap();
    let (cb, _dev) = acc.await.unwrap();
    (ca, cb)
}

#[tokio::test]
async fn idle_timeout_kills_silent_connection() {
    let t = TransportTuning { keep_alive_interval: None, max_idle_timeout: Duration::from_millis(400) };
    let (ca, _cb) = tuned_pair(t).await;
    // 双向静默:无 keep-alive → 两端 ~400ms 空闲超时
    tokio::time::timeout(Duration::from_secs(2), ca.closed()).await
        .expect("idle timeout must terminate silent connection");
}

#[tokio::test]
async fn keep_alive_keeps_quiet_connection_alive() {
    let t = TransportTuning { keep_alive_interval: Some(Duration::from_millis(100)),
                              max_idle_timeout: Duration::from_millis(400) };
    let (ca, cb) = tuned_pair(t).await;
    // 静默 1.5s(> 3×idle):keep-alive 必须维持连接
    let survived = tokio::time::timeout(Duration::from_millis(1500), ca.closed()).await.is_err();
    assert!(survived, "keep-alive must prevent idle death");
    // 且连接仍可用:开一条 uni 流写读正常(对端 accept_uni 收到)
    let mut tx = ca.open_uni().await.expect("open uni stream after silence");
    tx.write_all(b"alive").await.expect("write after silence");
    tx.finish().expect("finish after silence");
    let mut rx = cb.accept_uni().await.expect("peer accept uni after silence");
    let mut buf = [0u8; 5];
    rx.read_exact(&mut buf).await.expect("read after silence");
    assert_eq!(&buf, b"alive");
}
