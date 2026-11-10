//! V3-5 Stage B 标签原语:同一环回连接上的 tagged 传输/ctrl 往返 +
//! 调优合并锚。
//!
//! 暖复用的引擎层全链路锚:接收端 accept_bi → 读首字节流类型标签 →
//! 分派移交(transfer 的 `recv_files_on_ctl` / ctrl 的
//! `read_request_frame` + `write_response_frame`)。裸路径行为由既有
//! transfer*/ctrl 测试回归,此处只锚「标签 + 注入」的新面。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::protocol::manifest::{build_manifest, SourceFile, CHUNK_SIZE};
use mtdss_core::transport::ctrl;
use mtdss_core::transport::endpoint::{self as ep, TransportTuning};
use mtdss_core::transport::intake::FsDestIo;
use mtdss_core::transport::mux;
use mtdss_core::transport::punch;
use mtdss_core::transport::transfer::{self, RecvCtl, SendOptions};
use mtdss_proto::ctrl::{CtrlRequest, CtrlResponse};
use mtdss_proto::CTRL_VERSION;

/// 固定种子身份(e2e.rs 房风):DeviceIdentity 不可 Clone,同一身份
/// 跨作用域用 from_seed 重建(测试内即真实做法)。
const SEED_A: [u8; 32] = [0xA5; 32];
const SEED_B: [u8; 32] = [0x5B; 32];

/// 微 keepalive 调优(transport_tuning.rs 房风):100ms 心跳 + 5s 空闲
/// 上界——测试连接在两侧任务排程间隙不因空闲死亡,静默故障也能秒级显形。
fn micro_keepalive() -> TransportTuning {
    TransportTuning {
        keep_alive_interval: Some(Duration::from_millis(100)),
        max_idle_timeout: Duration::from_secs(5),
        ..Default::default()
    }
}

/// 已认证自连 QUIC 对(endpoint.rs make_dual_endpoint 房风):A 拨 B、
/// B 收,返回 (A 侧连接, B 侧连接)。accept 与 connect 并发轮询(接受
/// 令牌只来自 accept_pinned,顺序 await 会互相等待而死锁)。
async fn connected_pair() -> (quinn::Connection, quinn::Connection) {
    let a = DeviceIdentity::from_seed(&SEED_A);
    let b = DeviceIdentity::from_seed(&SEED_B);
    // A 的服务端白名单:B;B 的:A
    let allowed_a: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(b.device_id.clone(), b.public_key())].into_iter().collect();
    let allowed_b: HashMap<String, ed25519_dalek::VerifyingKey> =
        [(a.device_id.clone(), a.public_key())].into_iter().collect();

    let sa = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let sb = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr_b = sb.local_addr().unwrap();
    let tuning = micro_keepalive();
    let ep_a = ep::make_dual_endpoint_tuned(sa, &a, &allowed_a, tuning).unwrap();
    let ep_b = ep::make_dual_endpoint_tuned(sb, &b, &allowed_b, tuning).unwrap();

    let acc = {
        let ep_b = ep_b.clone();
        let allowed = allowed_b.clone();
        tokio::spawn(async move { ep::accept_pinned(&ep_b, &allowed).await.unwrap() })
    };
    let ca = ep::connect_pinned_tuned(&ep_a, addr_b, &b.public_key(), &a, tuning)
        .await
        .unwrap();
    let (cb, dev) = acc.await.unwrap();
    assert_eq!(dev, a.device_id);
    (ca, cb)
}

/// 确定性内容(transfer_v2.rs make_file 房风:xorshift)。
fn deterministic_bytes(size: usize) -> Vec<u8> {
    let mut v = vec![0u8; size];
    let mut s: u64 = size as u64 + 1;
    for b in v.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *b = (s & 0xFF) as u8;
    }
    v
}

/// 暖复用全链路锚:同一条连接上,一侧 `send_files_tagged`,另一侧
/// accept_bi → 首字节断言 == TAG_TRANSFER → `recv_files_on_ctl`(移交
/// 该流)→ 落盘字节一致、RecvOutcome.paths 非空。
#[tokio::test]
async fn tagged_transfer_roundtrip_on_one_conn() {
    let (client, server) = connected_pair().await;

    // 单文件、固定字节(跨 2 个块:CHUNK_SIZE + 尾差)
    let src = tempfile::tempdir().unwrap();
    let payload = deterministic_bytes(CHUNK_SIZE + 1000);
    let f = src.path().join("tagged.bin");
    std::fs::write(&f, &payload).unwrap();
    let items = vec![(SourceFile::Path(f), "tagged.bin".to_string())];
    let sender = DeviceIdentity::from_seed(&SEED_A);
    let man = build_manifest(&items, &sender).unwrap();

    let send_task = tokio::spawn(async move {
        transfer::send_files_tagged(&client, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });

    // 接收端 = mux serve 的最小形态:accept_bi → 首字节标签 → 移交引擎
    let (ctl_tx, mut ctl_rx) = server.accept_bi().await.unwrap();
    let mut tag = [0u8; 1];
    ctl_rx.read_exact(&mut tag).await.unwrap();
    assert_eq!(tag[0], mux::TAG_TRANSFER);

    let dst = tempfile::tempdir().unwrap();
    let out = transfer::recv_files_on_ctl(
        &server,
        (ctl_tx, ctl_rx),
        Arc::new(FsDestIo::new(dst.path())),
        &RecvCtl::default(),
    )
    .await;

    assert!(
        out.error.is_none(),
        "recv_files_on_ctl failed: {:?}",
        out.error
    );
    assert_eq!(out.paths.len(), 1, "landed paths: {:?}", out.paths);
    assert_eq!(std::fs::read(&out.paths[0]).unwrap(), payload);

    let report = send_task.await.unwrap();
    assert_eq!(report.bytes_sent, payload.len() as u64);
}

/// ctrl tagged 往返:`send_request_tagged` → 对端 accept_bi → 首字节
/// 断言 == TAG_CTRL → `read_request_frame` → `write_response_frame` →
/// 发起端拿到响应。
#[tokio::test]
async fn send_request_tagged_roundtrip() {
    let (client, server) = connected_pair().await;

    // 响应端持有连接直至任务收尾——QUIC 连接句柄随任务 future 释放会触发
    // 隐式 CONNECTION_CLOSE,作废在途响应帧(引擎 EOF 舞步注释同一契约);
    // 因此把连接随任务结果带出,在发起端读到响应后再丢弃。
    let responder = tokio::spawn(async move {
        let (mut tx, mut rx) = server.accept_bi().await.unwrap();
        let mut tag = [0u8; 1];
        rx.read_exact(&mut tag).await.unwrap();
        assert_eq!(tag[0], mux::TAG_CTRL);
        let req = ctrl::read_request_frame(&mut rx).await.unwrap();
        assert_eq!(req.version, CTRL_VERSION);
        let resp = CtrlResponse {
            version: CTRL_VERSION,
            ok: true,
            body: None,
        };
        ctrl::write_response_frame(&mut tx, &resp).await.unwrap();
        let _ = tx.finish();
        server
    });

    let req = CtrlRequest {
        version: CTRL_VERSION,
        body: None,
    };
    let resp = ctrl::send_request_tagged(&client, &req, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(resp.version, CTRL_VERSION);
    assert!(resp.ok);
    drop(responder.await.unwrap());
}

/// 调优合并锚(spec §3.2):暖连接沿用 ctrl 调优,传输直连打洞默认调优
/// `punch::PunchConfig::default().tuning` 的 MTU 必须与之相同——无冲突
/// 才允许「不写合并代码,以断言钉死」。daemon 侧 `ctrl_default_tuning`
/// 在 daemon crate,core 侧可断言的共同事实是 1452 字面值(与 daemon
/// flow.rs 既有断言同源):任何一端漂移都会先在这里红。
#[test]
fn warm_tuning_mtu_anchor() {
    assert_eq!(punch::PunchConfig::default().tuning.max_mtu, 1452);
}
