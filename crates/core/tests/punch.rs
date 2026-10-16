//! punch_connect:互拨先到先用、预算限时报错、垃圾包注入无碍、仅 accept 也能赢。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::{endpoint as ep, punch, transfer};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn dual(
    id: &DeviceIdentity,
    peer_pk: ed25519_dalek::VerifyingKey,
    peer_id: &str,
) -> (quinn::Endpoint, std::net::SocketAddr) {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    let allowed: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(peer_id.to_string(), peer_pk)].into_iter().collect();
    let e = ep::make_dual_endpoint(s, id, &allowed).unwrap();
    (e, addr)
}

#[tokio::test]
async fn mutual_punch_both_sides_succeed_and_transfer_runs() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    // spawn 闭包只取 Copy 的种子/公钥;身份在任务内以 from_seed 重建
    let a_pk = a.public_key();
    let b_pk = b.public_key();
    let a_seed = a.signing.to_bytes();
    let b_seed = b.signing.to_bytes();
    let (ep_a, addr_a) = dual(&a, b_pk, &b.device_id);
    let (ep_b, addr_b) = dual(&b, a_pk, &a.device_id);
    let cfg = punch::PunchConfig::default();

    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b_pk)].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a_pk)].into_iter().collect();

    // 两侧必须并发互拨:接受令牌只来自 accept_pinned,先 await 一侧会死锁
    let ja = {
        let ep = ep_a.clone();
        let me = DeviceIdentity::from_seed(&a_seed);
        let allowed = allowed_a.clone();
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed, &b_pk, &[addr_b], &cfg).await
        })
    };
    let jb = {
        let ep = ep_b.clone();
        let me = DeviceIdentity::from_seed(&b_seed);
        let allowed = allowed_b.clone();
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed, &a_pk, &[addr_a], &cfg).await
        })
    };
    let conn_a = ja.await.unwrap().unwrap();
    let conn_b = jb.await.unwrap().unwrap();

    // 双方各持一条,且必为同一连接的两端:用它跑完整传输
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("p.bin");
    let data = vec![0xABu8; 4096];
    std::fs::write(&f, &data).unwrap();
    let dst = tempfile::tempdir().unwrap();
    let dst_path = dst.path().to_path_buf();
    let send = {
        let conn = conn_a.clone();
        let f = f.clone();
        let me = DeviceIdentity::from_seed(&a_seed);
        tokio::spawn(async move { transfer::send_file(&conn, &f, "p.bin", &me, &|_, _| {}).await })
    };
    let recv = {
        let conn = conn_b.clone();
        let me = DeviceIdentity::from_seed(&b_seed);
        tokio::spawn(async move { transfer::recv_files(&conn, &dst_path, &me).await })
    };
    send.await.unwrap().unwrap();
    let landed = recv.await.unwrap().unwrap();
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}

#[tokio::test]
async fn budget_exhaustion_errors_promptly() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let (ep_a, _) = dual(&a, b.public_key(), &b.device_id);
    // 对端端点存在但无人拨/无人收:绑定即丢弃 = 黑洞地址
    let dead = UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);
    let cfg = punch::PunchConfig {
        budget: Duration::from_millis(600),
        handshake_timeout: Duration::from_millis(200),
    };
    let t0 = Instant::now();
    let err = punch::punch_connect(&ep_a, &a, &HashMap::new(), &b.public_key(), &[dead_addr], &cfg)
        .await
        .unwrap_err();
    let dt = t0.elapsed();
    assert!(
        dt < Duration::from_secs(3),
        "must fail within budget+slack, took {dt:?}"
    );
    assert!(
        err.to_string().contains("no path established"),
        "got: {err:#}"
    );
    assert!(err.to_string().contains("1"), "mentions candidate count");
}

#[tokio::test]
async fn garbage_udp_does_not_break_or_hang_punch() {
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let a_pk = a.public_key();
    let b_pk = b.public_key();
    let b_seed = b.signing.to_bytes();
    let (ep_a, _) = dual(&a, b_pk, &b.device_id);
    let (ep_b, addr_b) = dual(&b, a_pk, &a.device_id);
    // 向 B 灌伪造 QUIC-ish 垃圾包(首字节 0xC0 长头 + 伪随机)
    let junk = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut pkt = vec![0xC0u8; 1200];
    for (i, b_) in pkt.iter_mut().enumerate().skip(1) {
        *b_ = (i as u8).wrapping_mul(31).wrapping_add(7);
    }
    for _ in 0..8 {
        junk.send_to(&pkt, addr_b).unwrap();
    }
    // B 侧 accept-only punch 与 A 的真拨号并发(接受令牌只来自 accept_pinned)
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b_pk)].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a_pk)].into_iter().collect();
    let cfg = punch::PunchConfig::default();
    let jb = {
        let ep = ep_b.clone();
        let me = DeviceIdentity::from_seed(&b_seed);
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed_b, &a_pk, &[], &cfg).await
        })
    };
    punch::punch_connect(&ep_a, &a, &allowed_a, &b_pk, &[addr_b], &cfg)
        .await
        .expect("real dial must succeed despite garbage");
    let _ = jb.await; // 只验 A 侧成功;B 侧 accept 成功已由 A 拿到令牌所蕴含
}

#[tokio::test]
async fn empty_candidates_accept_only_still_wins() {
    // A 无对端候选:仅 accept;B 拨 A → A 的 punch 应经 accept 分支成功
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let a_pk = a.public_key();
    let b_pk = b.public_key();
    let (ep_a, addr_a) = dual(&a, b_pk, &b.device_id);
    let (ep_b, _) = dual(&b, a_pk, &a.device_id);
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b_pk)].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a_pk)].into_iter().collect();
    let cfg = punch::PunchConfig::default();
    let ja = {
        let ep = ep_a.clone();
        let me = DeviceIdentity::from_seed(&a.signing.to_bytes());
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed_a, &b_pk, &[], &cfg).await
        })
    };
    let jb = {
        let ep = ep_b.clone();
        let me = DeviceIdentity::from_seed(&b.signing.to_bytes());
        tokio::spawn(async move {
            punch::punch_connect(&ep, &me, &allowed_b, &a_pk, &[addr_a], &cfg).await
        })
    };
    ja.await.unwrap().unwrap();
    jb.await.unwrap().unwrap();
}
