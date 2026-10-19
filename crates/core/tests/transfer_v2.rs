//! v2:多文件/计划协议/调度/断点/有界性。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::protocol::manifest::{build_manifest, CHUNK_SIZE};
use mtdss_core::transport::endpoint as ep;
use mtdss_core::transport::intake;
use mtdss_core::transport::intake::TransferPolicy;
use mtdss_core::transport::transfer::{
    self, PauseGate, RecvCtl, RecvProgress, SendError, SendOptions, SendProgress,
};

fn localhost0() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

fn make_file(dir: &Path, name: &str, size: usize) -> PathBuf {
    let p = dir.join(name);
    let mut v = vec![0u8; size];
    let mut s: u64 = size as u64 + 1;
    for b in v.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *b = (s & 0xFF) as u8;
    }
    std::fs::write(&p, v).unwrap();
    p
}

async fn connected_pair(
    a: &DeviceIdentity,
    b: &DeviceIdentity,
    allowed: &HashMap<String, ed25519_dalek::VerifyingKey>,
) -> (quinn::Connection, quinn::Connection) {
    let ep_b = ep::make_server_endpoint(localhost0(), b, allowed).unwrap();
    let addr = ep_b.local_addr().unwrap();
    let ep_a = ep::make_endpoint(localhost0()).unwrap();
    let allowed_srv = allowed.clone();
    let server =
        tokio::spawn(async move { ep::accept_pinned(&ep_b, &allowed_srv).await.unwrap().0 });
    let client = ep::connect_pinned(&ep_a, addr, &b.public_key(), a).await.unwrap();
    let server_conn = server.await.unwrap();
    (client, server_conn)
}

/// 读一帧计划回包(tag 必须为 0x01)并解析为 FilePlan 列表。
async fn read_plan(rx: &mut quinn::RecvStream) -> Vec<mtdss_core::transport::FilePlan> {
    use mtdss_core::transport::FilePlan;
    let mut tag = [0u8; 1];
    rx.read_exact(&mut tag).await.unwrap();
    assert_eq!(tag[0], 0x01, "expected plan frame");
    let mut n = [0u8; 4];
    rx.read_exact(&mut n).await.unwrap();
    let n = u32::from_be_bytes(n) as usize;
    let mut plans = Vec::with_capacity(n);
    for _ in 0..n {
        let mut k = [0u8; 1];
        rx.read_exact(&mut k).await.unwrap();
        match k[0] {
            0 => plans.push(FilePlan::Full),
            1 => plans.push(FilePlan::Skip),
            2 => {
                let mut l = [0u8; 4];
                rx.read_exact(&mut l).await.unwrap();
                let mut bmp = vec![0u8; u32::from_be_bytes(l) as usize];
                rx.read_exact(&mut bmp).await.unwrap();
                plans.push(FilePlan::Resume(bmp));
            }
            t => panic!("bad plan kind {t}"),
        }
    }
    plans
}

/// 记录位图已收位数(轮询磁盘用)。
fn intake_record_bits(out_dir: &Path, rel: &str) -> usize {
    let bytes = std::fs::read(intake::record_path(out_dir, rel)).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let bmp = hex::decode(v["bitmap"].as_str().unwrap()).unwrap();
    bmp.iter().map(|b| b.count_ones() as usize).sum()
}

fn no_part_left(out_dir: &Path) {
    for e in std::fs::read_dir(out_dir.join(intake::INTAKE_DIR))
        .unwrap()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        assert!(!name.ends_with(".part"), "leftover part file: {name}");
    }
}

/// 多文件(含子目录 rel 与 0 字节)→ 结构落盘、内容一致、无 .part 残留、
/// `.mtdss/` 仅剩记录;progress 收齐 3 个 FileDone。
#[tokio::test]
async fn multi_file_and_directory_transfers() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_a = make_file(src.path(), "a.txt", 123_456);
    let f_b = make_file(src.path(), "b.bin", CHUNK_SIZE * 2 + 13);
    let f_e = src.path().join("empty.txt");
    std::fs::write(&f_e, b"").unwrap();
    let items = vec![
        (f_a.clone(), "a.txt".to_string()),
        (f_b.clone(), "docs/b.bin".to_string()),
        (f_e.clone(), "empty.txt".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let dones: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let dones_cl = dones.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &move |p| {
            if let SendProgress::FileDone { rel } = p {
                dones_cl.lock().unwrap().push(rel.to_string());
            }
        }, &SendOptions::default())
        .await
        .unwrap()
    });
    let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    send_task.await.unwrap();

    // 内容与结构逐一对位(fresh 目录,无冲突改名)
    assert_eq!(got.len(), 3);
    for (rel, content) in [
        ("a.txt", std::fs::read(&f_a).unwrap()),
        ("docs/b.bin", std::fs::read(&f_b).unwrap()),
        ("empty.txt", Vec::<u8>::new()),
    ] {
        let p = dst.path().join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        assert!(p.exists(), "missing {rel}");
        assert_eq!(std::fs::read(&p).unwrap(), content, "content mismatch: {rel}");
    }
    // 无 .part 残留;.mtdss/ 仅剩 3 条记录
    let mut jsons = 0;
    for e in std::fs::read_dir(dst.path().join(intake::INTAKE_DIR))
        .unwrap()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        assert!(name.ends_with(".json"), "unexpected intake entry: {name}");
        jsons += 1;
    }
    assert_eq!(jsons, 3);
    let mut d = dones.lock().unwrap().clone();
    d.sort();
    let mut expect = vec![
        "a.txt".to_string(),
        "docs/b.bin".to_string(),
        "empty.txt".to_string(),
    ];
    expect.sort();
    assert_eq!(d, expect, "FileDone events must cover every file");
}

/// 同一 manifest 同一连接第二次 send_files → SendReport.bytes_sent == 0,
/// recv_files 再收一组(全部 SKIP,无数据流,返回空路径集)。
#[tokio::test]
async fn second_send_of_same_manifest_skips_everything() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "once.bin", CHUNK_SIZE + 7);
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&[(f.clone(), "once.bin".to_string())], &a).unwrap();
    let items = vec![(f.clone(), "once.bin".to_string())];
    let (sc2, man2, items2) = (sc.clone(), man.clone(), items.clone());
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let got1 = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    send_task.await.unwrap();
    assert_eq!(got1.len(), 1);
    assert_eq!(std::fs::read(&got1[0]).unwrap(), std::fs::read(&f).unwrap());

    let send_task2 = tokio::spawn(async move {
        transfer::send_files(&sc2, &man2, &items2, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let got2 = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    let report2 = send_task2.await.unwrap();
    assert!(got2.is_empty(), "SKIP files must not appear in landed paths");
    assert_eq!(report2.bytes_sent, 0, "second send must stream no data");
}

/// A/B 直连;先手打发送前半块后 abort 控制流(drop 连接);
/// 新连接对上 run 一对 endpoint → send_files(同 man) → 完成;
/// 断言本次 bytes_sent == 仅缺块字节之和(按块流计数语义),文件内容完整。
#[tokio::test]
async fn dropped_connection_then_new_connection_resumes_missing_only() {
    use prost::Message as _;
    const N_CHUNKS: usize = 20;
    const SENT_FIRST: usize = 17; // 发 17/20 块后断连,至少缺 3 块

    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "fat.bin", CHUNK_SIZE * N_CHUNKS);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());

    // 第一次:手打前 17 块,不发 checkpoint 即断连
    let flushed_bits = {
        let (sc, rc) = connected_pair(&a, &b, &allowed).await;
        let dst_path = dst.path().to_path_buf();
        let me = DeviceIdentity::from_seed(&b.signing.to_bytes());
        let recv_task =
            tokio::spawn(async move { transfer::recv_files(&rc, &dst_path, &me).await });
        let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
        let man = build_manifest(&[(f.clone(), "fat.bin".to_string())], &a).unwrap();
        let mb = man.encode_to_vec();
        ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
        ctl_tx.write_all(&mb).await.unwrap();
        let plans = read_plan(&mut ctl_rx).await;
        assert_eq!(plans, vec![mtdss_core::transport::FilePlan::Full]);
        for c in &man.files[0].chunks[..SENT_FIRST] {
            let mut uni = sc.open_uni().await.unwrap();
            uni.write_all(&0u32.to_be_bytes()).await.unwrap();
            uni.write_all(&c.offset.to_be_bytes()).await.unwrap();
            uni.write_all(&c.length.to_be_bytes()).await.unwrap();
            uni.write_all(&data[c.offset as usize..(c.offset + c.length) as usize])
                .await
                .unwrap();
            uni.finish().unwrap();
        }
        // 记录批量落盘(16 块阈值 / 2s 间隔)只在下一个 write_chunk 时检查,
        // 停止发送后不会再刷。等记录出现 ≥1 已收位且静默 >2.5s(高于间隔
        // 阈值,在途数据必已落定)再断连;缺块集合从记录实算,不依赖时序。
        let deadline = Instant::now() + Duration::from_secs(20);
        let (mut last_bits, mut last_change) = (0usize, Instant::now());
        loop {
            let bits = intake_record_bits(dst.path(), "fat.bin");
            if bits != last_bits {
                last_bits = bits;
                last_change = Instant::now();
            }
            if last_bits >= 1 && last_change.elapsed() > Duration::from_millis(2600) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "record never reached a stable partial state (bits={last_bits})"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        drop(ctl_rx);
        drop(ctl_tx);
        drop(sc);
        // 断连后 recv 侧必然 Err(等它退出,释放 .part 句柄)
        let _ = tokio::time::timeout(Duration::from_secs(5), recv_task).await;
        last_bits
    };

    // 缺块 = 记录位图 0 位对应的块(批量落盘只保证已刷子集,未刷的按缺块
    // 重收——幂等);至少缺 3 块(有 3 块从未发过)
    let man = build_manifest(&[(f.clone(), "fat.bin".to_string())], &a).unwrap();
    let chunks = &man.files[0].chunks;
    let bmp = record_bitmap(dst.path(), "fat.bin");
    let expected_bytes: u64 = (0..chunks.len())
        .filter(|&i| !bit_set(&bmp, i))
        .map(|i| chunks[i].length)
        .sum();
    assert!(
        expected_bytes >= 3 * CHUNK_SIZE as u64,
        "resume must actually skip already-flushed chunks (flushed bits={flushed_bits})"
    );

    // 第二次:新 endpoint 对 + 真 send_files(同内容清单)续传
    let (sc2, rc2) = connected_pair(&a, &b, &allowed).await;
    let items = vec![(f.clone(), "fat.bin".to_string())];
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc2, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let got = transfer::recv_files(&rc2, dst.path(), &b).await.unwrap();
    let report = send_task.await.unwrap();
    assert_eq!(got.len(), 1);
    // 仅缺块被重发(块流计数语义)
    assert_eq!(report.bytes_sent, expected_bytes);
    assert_eq!(std::fs::read(&got[0]).unwrap(), data);
    no_part_left(dst.path());
}

/// 读记录位图(hex)。记录必须已在场(调用前已轮询过)。
fn record_bitmap(out_dir: &std::path::Path, rel: &str) -> Vec<u8> {
    let bytes = std::fs::read(intake::record_path(out_dir, rel)).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    hex::decode(v["bitmap"].as_str().unwrap()).unwrap()
}

fn bit_set(bitmap: &[u8], i: usize) -> bool {
    bitmap.get(i / 8).map(|b| b & (1 << (i % 8)) != 0).unwrap_or(false)
}

/// SendOptions.fault_drop_chunks={(0, chunk[2].offset)}, until_round=0
/// → 首轮缺块 → checkpoint 计划 → 次轮补发 → final;文件完整。
#[tokio::test]
async fn corrupt_chunk_re_requested_then_completes() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "faulted.bin", CHUNK_SIZE * 3);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&[(f.clone(), "faulted.bin".to_string())], &a).unwrap();
    let mut opts = SendOptions::default();
    opts.fault_drop_chunks.insert((0, man.files[0].chunks[2].offset));
    opts.fault_drop_until_round = 0; // 仅第 0 轮丢
    let items = vec![(f.clone(), "faulted.bin".to_string())];
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &opts)
            .await
            .unwrap()
    });
    let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    let report = send_task.await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(std::fs::read(&got[0]).unwrap(), data);
    // 首轮丢 1 块,次轮补发:总发送量仍为整文件
    assert_eq!(report.bytes_sent, (CHUNK_SIZE * 3) as u64);
}

/// fault_drop_until_round=u32::MAX → send_files Err(SendError::Protocol{..}),
/// 文案含 "rounds"。
#[tokio::test]
async fn checkpoint_rounds_exhausted_is_protocol_error() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "stuck.bin", CHUNK_SIZE * 2);
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&[(f.clone(), "stuck.bin".to_string())], &a).unwrap();
    let mut opts = SendOptions::default();
    opts.fault_drop_chunks.insert((0, man.files[0].chunks[1].offset));
    opts.fault_drop_until_round = u32::MAX; // 每轮都丢
    let items = vec![(f.clone(), "stuck.bin".to_string())];
    let sc_t = sc.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc_t, &man, &items, &|_| {}, &opts).await
    });
    let recv_task = tokio::spawn(async move { transfer::recv_files(&rc, dst.path(), &b).await });

    let err = send_task.await.unwrap().unwrap_err();
    match err {
        SendError::Protocol { reason } => {
            assert!(reason.contains("rounds"), "reason must mention rounds: {reason}")
        }
        e => panic!("expected Protocol, got {e:?}"),
    }
    // 收尾:断开连接让接收任务退出,避免悬挂句柄
    sc.close(quinn::VarInt::from_u32(0), b"test-done");
    let _ = tokio::time::timeout(Duration::from_secs(5), recv_task).await;
}

/// 手打 recv 侧(manifest 后回 0x00+reason)→ 真 send_files 得
/// Err(Rejected{reason})。
#[tokio::test]
async fn rejected_maps_to_send_error_rejected() {
    let src = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "x.bin", 4096);
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let recv_task = tokio::spawn(async move {
        let (mut tx, mut rx) = rc.accept_bi().await.unwrap();
        let mut len = [0u8; 4];
        rx.read_exact(&mut len).await.unwrap();
        let mut mb = vec![0u8; u32::from_be_bytes(len) as usize];
        rx.read_exact(&mut mb).await.unwrap();
        let reason = b"no space for you";
        tx.write_all(&[0x00]).await.unwrap();
        tx.write_all(&(reason.len() as u32).to_be_bytes()).await.unwrap();
        tx.write_all(reason).await.unwrap();
        // 半关 + 等发送方 EOF(其 Rejected 路径会 finish)再返回:
        // 隐式 CONNECTION_CLOSE 会作废在途的拒绝帧
        tx.finish().unwrap();
        let mut sink = [0u8; 1];
        let _ = rx.read(&mut sink).await;
    });

    let man = build_manifest(&[(f.clone(), "x.bin".to_string())], &a).unwrap();
    let items = vec![(f.clone(), "x.bin".to_string())];
    let err = transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
        .await
        .unwrap_err();
    recv_task.await.unwrap();
    match err {
        SendError::Rejected { reason } => assert_eq!(reason, "no space for you"),
        e => panic!("expected Rejected, got {e:?}"),
    }
}

/// 手打 sender,块帧 file_idx=99 / offset 不在表 / len>1MiB
/// → recv_files Err(文案分别含 "not in manifest"/"oversized")。
#[tokio::test]
async fn forged_chunks_rejected() {
    enum Forgery {
        BadFileIdx,
        BadOffset,
        Oversized,
    }

    async fn one(forgery: Forgery) -> String {
        use prost::Message as _;
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let f = make_file(src.path(), "target.bin", CHUNK_SIZE);
        let data = std::fs::read(&f).unwrap();
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        let mut allowed = HashMap::new();
        allowed.insert(a.device_id.clone(), a.public_key());
        let (sc, rc) = connected_pair(&a, &b, &allowed).await;

        let out_path = out.path().to_path_buf();
        let recv_task =
            tokio::spawn(async move { transfer::recv_files(&rc, &out_path, &b).await });

        let man = build_manifest(&[(f.clone(), "target.bin".to_string())], &a).unwrap();
        let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
        let mb = man.encode_to_vec();
        ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
        ctl_tx.write_all(&mb).await.unwrap();
        let _ = read_plan(&mut ctl_rx).await; // 过计划,进入收块阶段

        let mut uni = sc.open_uni().await.unwrap();
        match forgery {
            Forgery::BadFileIdx => {
                uni.write_all(&99u32.to_be_bytes()).await.unwrap();
                uni.write_all(&0u64.to_be_bytes()).await.unwrap();
                uni.write_all(&(data.len() as u64).to_be_bytes()).await.unwrap();
                uni.write_all(&data).await.unwrap();
            }
            Forgery::BadOffset => {
                uni.write_all(&0u32.to_be_bytes()).await.unwrap();
                uni.write_all(&0xCAFEBABEu64.to_be_bytes()).await.unwrap();
                uni.write_all(&64u64.to_be_bytes()).await.unwrap();
                uni.write_all(&[0xABu8; 64]).await.unwrap();
            }
            Forgery::Oversized => {
                uni.write_all(&0u32.to_be_bytes()).await.unwrap();
                uni.write_all(&0u64.to_be_bytes()).await.unwrap();
                uni.write_all(&(CHUNK_SIZE as u64 + 1).to_be_bytes()).await.unwrap();
            }
        }
        uni.finish().unwrap();
        let err = recv_task.await.unwrap().unwrap_err();
        format!("{err:#}")
    }

    assert!(
        one(Forgery::BadFileIdx).await.contains("not in manifest"),
        "file_idx out of range must be rejected"
    );
    assert!(
        one(Forgery::BadOffset).await.contains("not in manifest"),
        "offset outside signed chunk table must be rejected"
    );
    assert!(
        one(Forgery::Oversized).await.contains("oversized"),
        "oversized length must be rejected before allocation"
    );
}

/// SendOptions{2,1}:1 大(4 块)2 小(1 块);清单顺序大文件在前,
/// progress 的 FileDone 顺序:两小文件先于大文件(顺序断言,非计时)。
#[tokio::test]
async fn scheduling_small_files_first() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_big = make_file(src.path(), "big.bin", CHUNK_SIZE * 4);
    let f_s1 = make_file(src.path(), "small1.bin", 1000);
    let f_s2 = make_file(src.path(), "small2.bin", 2000);
    let items = vec![
        (f_big.clone(), "big.bin".to_string()),
        (f_s1.clone(), "small1.bin".to_string()),
        (f_s2.clone(), "small2.bin".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let dones: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let dones_cl = dones.clone();
    let opts = SendOptions {
        max_concurrent_files: 2,
        max_inflight_per_file: 1,
        ..Default::default()
    };
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &move |p| {
            if let SendProgress::FileDone { rel } = p {
                dones_cl.lock().unwrap().push(rel.to_string());
            }
        }, &opts)
        .await
        .unwrap()
    });
    let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    send_task.await.unwrap();
    assert_eq!(got.len(), 3);

    let order = dones.lock().unwrap().clone();
    let pos = |name: &str| order.iter().position(|r| r == name).unwrap();
    assert_eq!(order.len(), 3, "FileDone events: {order:?}");
    assert!(pos("big.bin") > pos("small1.bin"), "order: {order:?}");
    assert!(pos("big.bin") > pos("small2.bin"), "order: {order:?}");
}

/// Fix I-2(Task 6 重连编排):真实断连(非 fault 钩子)下,recv_files_fault
/// 必须把已 finalize 落盘的路径随错误一起带出——文件已在盘,调用方据此
/// 跨轮累计,而不是随 Err 丢弃。两文件:小文件先落盘,轮询到后从外部
/// 关闭连接模拟对端消失,大文件必然在途中被切断。
#[tokio::test]
async fn real_drop_yields_landed_paths_with_error() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_small = make_file(src.path(), "drop-small.bin", 64 * 1024);
    let f_big = make_file(src.path(), "drop-big.bin", 16 * 1024 * 1024);
    let items = vec![
        (f_small.clone(), "drop-small.bin".to_string()),
        (f_big.clone(), "drop-big.bin".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let send_task = tokio::spawn(async move {
        // 断连后发送方必然报错:结果忽略,收尾 join 防任务泄漏即可
        let _ = transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default()).await;
    });

    let rc_killer = rc.clone();
    let out_dir = dst.path().to_path_buf();
    let recv_out_dir = out_dir.clone();
    let recv_task =
        tokio::spawn(async move { transfer::recv_files_fault(&rc, &recv_out_dir, &b, None).await });

    // 轮询到小文件落盘,随即从外部关闭连接:真实断连(非钩子路径)
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if out_dir.join("drop-small.bin").is_file() {
            break;
        }
        assert!(Instant::now() < deadline, "small file never landed");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    rc_killer.close(quinn::VarInt::from_u32(0), b"real drop");

    let out = recv_task.await.unwrap();
    assert!(out.error.is_some(), "real drop must surface as an error");
    let names: Vec<String> = out
        .paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "drop-small.bin"),
        "landed-so-far file must be carried alongside the error: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "drop-big.bin"),
        "big file must not report as landed: {names:?}"
    );
    // 磁盘事实与带出路径一致
    let small = out
        .paths
        .iter()
        .find(|p| p.file_name().unwrap() == "drop-small.bin")
        .unwrap();
    assert_eq!(
        std::fs::read(small).unwrap(),
        std::fs::read(&f_small).unwrap()
    );
    send_task.await.unwrap();
}

/// Fix R2-F3:预检失败(清单层)必须回 0x00 拒绝帧——发送方得到
/// Rejected 终局(不重试、报告如实),而不是误判 Network 空耗重试。
/// 篡改 transfer_id 破坏签名覆盖域:接收方 manifest::verify 预检失败。
#[tokio::test]
async fn pre_plan_failure_rejects_instead_of_silent_drop() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "tampered.bin", 4096);
    let items = vec![(f.clone(), "tampered.bin".to_string())];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let mut man = build_manifest(&items, &a).unwrap();
    man.transfer_id = "not-hex".into(); // 破坏签名覆盖域 → verify 预检失败

    let dst_path = dst.path().to_path_buf();
    let recv_task =
        tokio::spawn(async move { transfer::recv_files_fault(&rc, &dst_path, &b, None).await });
    let err = transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
        .await
        .unwrap_err();
    match err {
        SendError::Rejected { reason } => {
            assert!(!reason.is_empty(), "reject must carry a reason");
        }
        other => panic!("pre-plan failure must map to Rejected, got {other:?}"),
    }
    // 接收侧:错误如实上抛,paths 为空
    let out = recv_task.await.unwrap();
    assert!(out.error.is_some(), "receiver must surface the pre-plan failure");
    assert!(out.paths.is_empty(), "nothing can land before the plan phase");
}

/// cap 分支双侧不挂：收端超限拒绝后 stop 控制流（cap 专用码 1），发送端
/// 在途 write 得到 Stopped(1)、读出在途拒绝帧；收端不等 EOF 及时返回。
/// 修复前：`reject_exit` 的 EOF 舞步在此读到的是滞留垃圾字节（非发送方
/// EOF），且无显式 stop 码——发送方只会收到 quinn 回收未读完 RecvStream
/// 时自动补发的 Stopped(0)，不可判别。各断言带 5s 有界，失败形式为断言
/// panic 而非挂死整个测试套件。
#[tokio::test]
async fn manifest_cap_rejects_without_deadlock_both_sides() {
    let dst = tempfile::tempdir().unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    // 服务端真 recv_files：读到 cap 超限长度后应拒绝并 stop。
    // 留一条连接句柄在本侧：recv_files 出错即返回，若那是最后一条句柄，
    // quinn 的排干 close 会与在途拒绝帧在客户端竞速（连接排干后未读的
    // 流数据不可再读），断言随之偶发失败；真实编排中连接由调用方持有，
    // 不在此及时序内。
    let rc_keep = rc.clone();
    let out_dir = dst.path().to_path_buf();
    let recv_task = tokio::spawn(async move { transfer::recv_files(&rc, &out_dir, &b).await });

    // 客户端 raw open_bi：声明超限长度，再持续写超限垃圾（write 会停在
    // 流控上，直到对端 stop 击落它）
    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let over_len = (transfer::MAX_MANIFEST_BYTES + 1) as u32;
    ctl_tx.write_all(&over_len.to_be_bytes()).await.unwrap();
    let garbage = vec![0xABu8; transfer::MAX_MANIFEST_BYTES + 1];
    let write_task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), ctl_tx.write_all(&garbage)).await
    });

    // 断言 1（5s 有界）：拒绝帧 [0x00][len][reason] 到达，reason 含 "too large"
    let mut tag = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), ctl_rx.read_exact(&mut tag))
        .await
        .expect("reject tag must arrive within 5s")
        .unwrap();
    assert_eq!(tag[0], 0x00, "first R→S frame must be the reject tag");
    let mut rlen = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), ctl_rx.read_exact(&mut rlen))
        .await
        .expect("reject reason length must arrive within 5s")
        .unwrap();
    let mut reason = vec![0u8; u32::from_be_bytes(rlen) as usize];
    tokio::time::timeout(Duration::from_secs(5), ctl_rx.read_exact(&mut reason))
        .await
        .expect("reject reason must arrive within 5s")
        .unwrap();
    let reason = String::from_utf8_lossy(&reason).into_owned();
    assert!(reason.contains("too large"), "reject reason: {reason}");

    // 断言 2（5s 有界）：在途垃圾 write 以 Stopped 收场，且 stop 码为
    // cap 专用码 1（quinn 对未读完 RecvStream 的 drop 会自动补发 stop 码
    // 0——用码值区分显式 cap 拒绝与句柄回收副作用）
    let werr = write_task
        .await
        .expect("write task must not panic")
        .expect("garbage write must not hang beyond its 5s bound")
        .expect_err("garbage write must fail once the peer stops the stream");
    match werr {
        quinn::WriteError::Stopped(code) => assert_eq!(
            code,
            quinn::VarInt::from_u32(1),
            "stop must carry the cap-specific code 1"
        ),
        e => panic!("garbage write must fail with Stopped, got {e:?}"),
    }

    // 断言 3（5s 有界）：收端以原原因上抛，不等 EOF
    let rerr = tokio::time::timeout(Duration::from_secs(5), recv_task)
        .await
        .expect("recv_files must return within 5s after the cap reject")
        .unwrap()
        .expect_err("cap overflow must surface as an error");
    assert!(
        format!("{rerr:#}").contains("too large"),
        "recv error must carry the cap reason: {rerr:#}"
    );
    // 断言完毕再放连接句柄（见上方 rc_keep 注释）
    drop(rc_keep);
}

/// 发送端分类：真 send_files 对"读 4 字节即回拒绝并 stop"的对端 →
/// Rejected 而非 Network。清单为手工构建的单文件超流控大清单（块表内容
/// 无关紧要，写阶段即被拒）：manifest 体远超默认流控窗口（quinn
/// stream_receive_window ≈1.19 MiB），体 write 必然滞留在流控上，对端的
/// stop 才能确定性击中在途写；build_manifest 的真实单文件清单只有几百
/// 字节，体写先于 stop 完成，测不到该路径。修复前 Stopped 被当 Network。
#[tokio::test]
async fn manifest_cap_sender_gets_rejected_not_network() {
    let src = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "cap.bin", 4096);
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    // 留一条连接句柄在本侧：手打对端任务 stop 后立即返回，若那是最后
    // 一条句柄，quinn 的排干 close 会与在途拒绝帧竞速（连接排干后未读
    // 的流数据不可再读）。
    let rc_keep = rc.clone();
    // 手打对端：读 4 字节 man_len → 回 0x00 拒绝帧 → stop（cap 专用码 1）
    let stop_task = tokio::spawn(async move {
        let (mut tx, mut rx) = rc.accept_bi().await.unwrap();
        let mut len = [0u8; 4];
        rx.read_exact(&mut len).await.unwrap();
        let reason = b"nope";
        tx.write_all(&[0x00]).await.unwrap();
        tx.write_all(&(reason.len() as u32).to_be_bytes()).await.unwrap();
        tx.write_all(reason).await.unwrap();
        rx.stop(quinn::VarInt::from_u32(1)).unwrap();
    });

    // 单文件大清单：100_000 块 ≈ 4 MB 清单体，远超 ~1.19 MiB 流控窗口
    let man = mtdss_proto::manifest::Manifest {
        version: 2,
        transfer_id: "0".repeat(64),
        files: vec![mtdss_proto::manifest::FileEntry {
            rel_path: "cap.bin".into(),
            size: 4096,
            sha256: vec![0u8; 32],
            chunks: (0..100_000u64)
                .map(|i| mtdss_proto::manifest::ChunkDesc {
                    offset: i * CHUNK_SIZE as u64,
                    length: CHUNK_SIZE as u64,
                    sha256: vec![0u8; 32],
                })
                .collect(),
        }],
        sender_signature: Vec::new(),
    };
    let items = vec![(f.clone(), "cap.bin".to_string())];
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default()),
    )
    .await
    .expect("send_files must return within 10s")
    .unwrap_err();
    stop_task.await.unwrap();
    // 断言完毕再放连接句柄（见上方 rc_keep 注释）
    drop(rc_keep);
    match err {
        SendError::Rejected { reason } => {
            assert!(
                reason.contains("nope"),
                "reason must carry the peer reject: {reason}"
            );
        }
        e => panic!("expected Rejected, got {e:?}"),
    }
}

/// 恶意计划帧总量:65 × 1MiB 位图(65 MiB > 64 MiB cap)→ Protocol,且错误
/// 点名 total。单帧 1 MiB 在既有 MAX_PLAN_BITMAP_BYTES 内合法,总量超限
/// 必须在解析层失败(跨帧累计,含后续重请求计划帧)。修复前:解析完 65
/// 条后由 send_rounds 报 "plan covers 65 files"——不含 "total",测试红。
#[tokio::test]
async fn plan_total_bitmap_cap_is_protocol_error() {
    let src = tempfile::tempdir().unwrap();
    let f1 = make_file(src.path(), "m1.bin", 4096);
    let f2 = make_file(src.path(), "m2.bin", 4096);
    let items = vec![
        (f1.clone(), "m1.bin".to_string()),
        (f2.clone(), "m2.bin".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    // 留一条服务端连接句柄在本侧:mock 任务返回即放手里的 rc,若那是最后
    // 一条句柄,quinn 会立即整连关闭(空 reason ApplicationClose),击落
    // 发送方仍在解析的计划帧数据。
    let rc_keep = rc.clone();
    // 服务端 mock:读走真 manifest(2 文件),回 TAG_PLAN count=65,全
    // RESUME、每个 bmp 1 MiB;写完 finish() 半关——干净 FIN 保住已写数据,
    // 直接 drop 流会发 RESET_STREAM 作废在途位图。发送方解析即超限中止,
    // 迟到的写可能被 Stopped 击落——写失败属预期,忽略。
    let mock_task = tokio::spawn(async move {
        let (mut tx, mut rx) = rc.accept_bi().await.unwrap();
        let mut len = [0u8; 4];
        rx.read_exact(&mut len).await.unwrap();
        let mut mb = vec![0u8; u32::from_be_bytes(len) as usize];
        rx.read_exact(&mut mb).await.unwrap();
        tx.write_all(&[0x01]).await.unwrap();
        tx.write_all(&65u32.to_be_bytes()).await.unwrap();
        let bmp = vec![0u8; 1024 * 1024];
        for _ in 0..65 {
            let _ = tx.write_all(&[0x02]).await;
            let _ = tx.write_all(&(bmp.len() as u32).to_be_bytes()).await;
            let _ = tx.write_all(&bmp).await;
        }
        let _ = tx.finish();
    });

    let man = build_manifest(&items, &a).unwrap();
    let err = tokio::time::timeout(
        Duration::from_secs(30),
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default()),
    )
    .await
    .expect("send_files must return within 30s")
    .unwrap_err();
    mock_task.await.unwrap();
    // 断言完毕再放连接句柄(见上方 rc_keep 注释)
    drop(rc_keep);
    match err {
        SendError::Protocol { reason } => {
            assert!(
                reason.contains("total"),
                "reason must name the total cap: {reason}"
            );
        }
        e => panic!("expected Protocol, got {e:?}"),
    }
}

/// 收端 EOF 舞步 2s 有界化(M3 债务①):发送方收到 Final 后扣住控制流
/// 不半关(fault_no_eof_after_final,模拟发端静默死亡、连接不断、无 FIN)。
/// 修复前收端终局路径的无界 read 会挂到 QUIC idle 超时(或外层兜底);
/// 修复后必须等过 2s 宽限即放行:文件已落盘、终局帧已发出、干净返回。
/// 下界 ≥2s 证明确实等过宽限再放行(而非立即返回);上界 ≤2s+3s 容忍
/// 测试环境抖动。
#[tokio::test]
async fn recv_returns_within_grace_when_sender_withholds_eof() {
    /// 与 transfer.rs 私有常量 EOF_DANCE_GRACE 对齐(私有,测试侧字面对齐)。
    const GRACE: Duration = Duration::from_secs(2);
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "eof.bin", CHUNK_SIZE + 7);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&[(f.clone(), "eof.bin".to_string())], &a).unwrap();
    let items = vec![(f.clone(), "eof.bin".to_string())];
    // 发送方扣住 EOF 后永不返回:外层 timeout 兜底防测试挂死;正常路径下
    // 收端 2s 宽限先到,兜底不会被等待。
    let send_task = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(10),
            transfer::send_files(
                &sc,
                &man,
                &items,
                &|_| {},
                &SendOptions {
                    fault_no_eof_after_final: true,
                    ..Default::default()
                },
            ),
        )
        .await
    });
    let start = Instant::now();
    let out = transfer::recv_files_fault(&rc, dst.path(), &b, None).await;
    let elapsed = start.elapsed();

    // 终局帧已发出、文件已落盘:干净 Ok + 内容一致
    assert!(
        out.error.is_none(),
        "receiver must finish cleanly after the grace: {:?}",
        out.error.as_ref().map(|e| format!("{e:#}"))
    );
    assert_eq!(out.paths.len(), 1, "exactly one landed path: {:?}", out.paths);
    assert_eq!(std::fs::read(&out.paths[0]).unwrap(), data);
    assert!(
        elapsed >= GRACE,
        "receiver must actually wait out the grace before releasing (elapsed {elapsed:?})"
    );
    assert!(
        elapsed <= GRACE + Duration::from_secs(3),
        "receiver must return within grace + 3s, not until idle timeout (elapsed {elapsed:?})"
    );
    // 发送方有意扣住 EOF:不等其 10s 兜底,断言已过即中止任务
    send_task.abort();
}

// ---------- M3c:冲突策略 / 接收进度 / 双侧暂停门 ----------

/// 每文件最新累计字节表 + 求和(暂停测试的进度采样;Bytes.done 为单文件
/// 跨轮累计值,取各文件当前值之和即发送侧总推进)。
#[derive(Default)]
struct ByteTotals {
    per_file: HashMap<String, u64>,
}

impl ByteTotals {
    fn note(&mut self, rel: &str, done: u64) {
        self.per_file.insert(rel.to_string(), done);
    }

    fn total(&self) -> u64 {
        self.per_file.values().sum()
    }
}

/// 策略:接收端 skip=[a.txt] → 发送端照常收齐 FileDone(skip 文件计完成,
/// 否则发送方永远等不到它)、bytes_sent 不含 a.txt、目标盘无 a.txt、
/// RecvOutcome.paths 不含 a.txt。
#[tokio::test]
async fn policy_skip_is_counted_done() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_a = make_file(src.path(), "a.txt", 4096);
    let f_b = make_file(src.path(), "b.bin", 8192);
    let items = vec![
        (f_a.clone(), "a.txt".to_string()),
        (f_b.clone(), "b.bin".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let dones: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let dones_cl = dones.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &move |p| {
            if let SendProgress::FileDone { rel } = p {
                dones_cl.lock().unwrap().push(rel.to_string());
            }
        }, &SendOptions::default())
        .await
        .unwrap()
    });
    let ctl = RecvCtl {
        policy: TransferPolicy {
            skip: vec!["a.txt".to_string()],
            overwrite: vec![],
        },
        ..Default::default()
    };
    let out = transfer::recv_files_ctl(&rc, dst.path(), &ctl).await;
    assert!(
        out.error.is_none(),
        "recv error: {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    let report = send_task.await.unwrap();

    let mut d = dones.lock().unwrap().clone();
    d.sort();
    assert_eq!(
        d,
        vec!["a.txt".to_string(), "b.bin".to_string()],
        "skip 文件也必须计 FileDone: {d:?}"
    );
    assert_eq!(report.bytes_sent, 8192, "skip 文件不计入 bytes_sent");
    assert!(!dst.path().join("a.txt").exists(), "skip 文件不得落盘");
    assert_eq!(
        std::fs::read(dst.path().join("b.bin")).unwrap(),
        std::fs::read(&f_b).unwrap()
    );
    let names: Vec<String> = out
        .paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["b.bin".to_string()], "paths 穿出不含 skip 文件: {names:?}");
}

/// 策略:overwrite 命中 → 目标同名旧文件被顶替:内容=新、无 " (n)" 改名。
#[tokio::test]
async fn policy_overwrite_replaces_existing() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "a.txt", 123_456);
    std::fs::write(dst.path().join("a.txt"), b"stale old content").unwrap();
    let items = vec![(f.clone(), "a.txt".to_string())];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let ctl = RecvCtl {
        policy: TransferPolicy {
            skip: vec![],
            overwrite: vec!["a.txt".to_string()],
        },
        ..Default::default()
    };
    let out = transfer::recv_files_ctl(&rc, dst.path(), &ctl).await;
    assert!(
        out.error.is_none(),
        "recv error: {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    send_task.await.unwrap();

    assert_eq!(
        std::fs::read(dst.path().join("a.txt")).unwrap(),
        std::fs::read(&f).unwrap(),
        "覆盖后内容必须=新内容"
    );
    let names: Vec<String> = std::fs::read_dir(dst.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    assert_eq!(
        names,
        vec!["a.txt".to_string()],
        "覆盖语义不得触发冲突改名: {names:?}"
    );
}

/// 前缀必须落在路径段边界:skip=["ab"] 不命中 "abc" 与 "ab-c",命中
/// "ab/x" 与 "ab" 精确;被跳过文件计 FileDone、bytes_sent 不含。
#[tokio::test]
async fn policy_prefix_requires_separator() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_abc = make_file(src.path(), "abc-src", 1024);
    let f_dash = make_file(src.path(), "dash-src", 2048);
    let f_sub = make_file(src.path(), "sub-src", 4096);
    let f_exact = make_file(src.path(), "exact-src", 8192);
    let items = vec![
        (f_abc.clone(), "abc".to_string()),
        (f_dash.clone(), "ab-c".to_string()),
        (f_sub.clone(), "ab/x".to_string()),
        (f_exact.clone(), "ab".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let dones: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let dones_cl = dones.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &move |p| {
            if let SendProgress::FileDone { rel } = p {
                dones_cl.lock().unwrap().push(rel.to_string());
            }
        }, &SendOptions::default())
        .await
        .unwrap()
    });
    let ctl = RecvCtl {
        policy: TransferPolicy {
            skip: vec!["ab".to_string()],
            overwrite: vec![],
        },
        ..Default::default()
    };
    let out = transfer::recv_files_ctl(&rc, dst.path(), &ctl).await;
    assert!(
        out.error.is_none(),
        "recv error: {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    let report = send_task.await.unwrap();

    // 段边界:abc / ab-c 照收,ab/x 与 ab 跳过
    assert_eq!(
        std::fs::read(dst.path().join("abc")).unwrap(),
        std::fs::read(&f_abc).unwrap(),
        "abc 不得被前缀误伤"
    );
    assert_eq!(
        std::fs::read(dst.path().join("ab-c")).unwrap(),
        std::fs::read(&f_dash).unwrap(),
        "ab-c 不得被前缀误伤"
    );
    assert!(!dst.path().join("ab").exists(), "精确命中应跳过");
    assert!(
        !dst.path().join("ab").is_dir() && !dst.path().join("ab/x").exists(),
        "子路径命中应跳过(不建目录)"
    );
    let mut d = dones.lock().unwrap().clone();
    d.sort();
    assert_eq!(
        d,
        vec!["ab".to_string(), "ab-c".to_string(), "ab/x".to_string(), "abc".to_string()],
        "四个文件都计 FileDone: {d:?}"
    );
    assert_eq!(report.bytes_sent, (1024 + 2048) as u64, "只发未命中条目");
}

/// 覆盖的类型不匹配清障:目标预置**文件** "d"、清单 rel "d/e"(overwrite
/// 前缀 ["d"] 命中)→ 清掉 d 后终件 create_dir_all 成立,传输后 d 为
/// 目录且含 e;无 "d (1)" 之类的逃逸。
#[tokio::test]
async fn policy_overwrite_type_mismatch() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "e-src", 4096);
    let items = vec![(f.clone(), "d/e".to_string())];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;
    std::fs::write(dst.path().join("d"), b"file in the way").unwrap();

    let man = build_manifest(&items, &a).unwrap();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let ctl = RecvCtl {
        policy: TransferPolicy {
            skip: vec![],
            overwrite: vec!["d".to_string()],
        },
        ..Default::default()
    };
    let out = transfer::recv_files_ctl(&rc, dst.path(), &ctl).await;
    assert!(
        out.error.is_none(),
        "recv error: {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    send_task.await.unwrap();

    assert!(dst.path().join("d").is_dir(), "d 必须已成目录");
    assert_eq!(
        std::fs::read(dst.path().join("d").join("e")).unwrap(),
        std::fs::read(&f).unwrap(),
        "d/e 内容一致"
    );
    let escaped: Vec<String> = std::fs::read_dir(dst.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && n != "d")
        .collect();
    assert!(
        escaped.is_empty(),
        "不得出现改名逃逸(如 \"e (1)\"): {escaped:?}"
    );
}

/// 发送端暂停门:传输途中拉门 → 字节停走(两次采样零差,所有发送路径
/// 都被门挡住:波前 + 每条流 open_uni 前)、双侧任务未完;复位后完成,
/// 内容逐字节一致。拉门阈值取累计 ≥4MiB 而非墙钟 1s:环回上 32MiB 可
/// 能远快于 1s 跑完,墙钟暂停会错过中途;字节阈值确定性钉在中途。
#[tokio::test]
async fn pause_resume_mid_transfer() {
    const PER_FILE: usize = 4 * 1024 * 1024;
    const N: usize = 8;
    const PAUSE_AT: u64 = 4 * 1024 * 1024;
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let mut items = Vec::new();
    for i in 0..N {
        items.push((
            make_file(src.path(), &format!("p{i}.bin"), PER_FILE),
            format!("p{i}.bin"),
        ));
    }
    let expected_total = (PER_FILE * N) as u64;
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let gate = PauseGate::new();
    let totals: Arc<Mutex<ByteTotals>> = Arc::new(Mutex::new(ByteTotals::default()));
    let paused_once = Arc::new(AtomicBool::new(false));
    let opts = SendOptions {
        max_concurrent_files: 2,
        pause: Some(gate.clone()),
        ..Default::default()
    };
    let cb_totals = totals.clone();
    let cb_gate = gate.clone();
    let cb_once = paused_once.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &move |p| {
            if let SendProgress::Bytes { rel, done, .. } = p {
                let engaged = {
                    let mut t = cb_totals.lock().unwrap();
                    t.note(rel, *done);
                    !cb_once.load(Ordering::SeqCst) && t.total() >= PAUSE_AT
                };
                // 回调与发送同任务执行:置位即刻生效于下一次门检查
                if engaged && !cb_once.swap(true, Ordering::SeqCst) {
                    cb_gate.set(true);
                }
            }
        }, &opts)
        .await
        .unwrap()
    });
    let dst_path = dst.path().to_path_buf();
    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, &dst_path, &b).await });

    // 等拉门发生(有界)
    let deadline = Instant::now() + Duration::from_secs(30);
    while !paused_once.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "pause never engaged");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 停走断言:在途流排干后(留 700ms),两次采样零差
    tokio::time::sleep(Duration::from_millis(700)).await;
    let s1 = totals.lock().unwrap().total();
    tokio::time::sleep(Duration::from_millis(700)).await;
    let s2 = totals.lock().unwrap().total();
    assert_eq!(s1, s2, "暂停期间发送字节必须停走");
    assert!(
        s2 < expected_total,
        "必须停在中途(而非完成后): {s2}/{expected_total}"
    );
    assert!(!send_task.is_finished(), "发送任务不得在挂起期间完成");

    gate.set(false);
    let report = send_task.await.unwrap();
    let got = recv_task.await.unwrap().unwrap();
    assert_eq!(got.len(), N);
    assert_eq!(report.bytes_sent, expected_total, "暂停不丢不重,总量=整批");
    for i in 0..N {
        let name = format!("p{i}.bin");
        assert_eq!(
            std::fs::read(dst.path().join(&name)).unwrap(),
            std::fs::read(src.path().join(&name)).unwrap(),
            "{name} 内容一致"
        );
    }
}

/// 接收进度:Bytes.done 每文件严格单调、终值=total;FileDone 每文件恰
/// 一次;0 字节文件无 Bytes 事件。
#[tokio::test]
async fn recv_progress_counts() {
    #[derive(Default)]
    struct Log {
        bytes: Vec<(String, u64, u64)>,
        dones: Vec<String>,
    }
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_a = make_file(src.path(), "a.txt", 123_456);
    let f_b = make_file(src.path(), "b.bin", CHUNK_SIZE * 2 + 13);
    let f_e = src.path().join("empty.txt");
    std::fs::write(&f_e, b"").unwrap();
    let items = vec![
        (f_a.clone(), "a.txt".to_string()),
        (f_b.clone(), "docs/b.bin".to_string()),
        (f_e.clone(), "empty.txt".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let log: Arc<Mutex<Log>> = Arc::default();
    let log_cb = log.clone();
    let ctl = RecvCtl {
        progress: Some(Box::new(move |p: &RecvProgress<'_>| {
            match p {
                RecvProgress::Bytes { rel, done, total } => log_cb
                    .lock()
                    .unwrap()
                    .bytes
                    .push((rel.to_string(), *done, *total)),
                RecvProgress::FileDone { rel } => {
                    log_cb.lock().unwrap().dones.push(rel.to_string())
                }
            }
        })),
        ..Default::default()
    };
    let out = transfer::recv_files_ctl(&rc, dst.path(), &ctl).await;
    assert!(
        out.error.is_none(),
        "recv error: {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    send_task.await.unwrap();

    let log = log.lock().unwrap();
    for (rel, total) in [
        ("a.txt", 123_456u64),
        ("docs/b.bin", (CHUNK_SIZE * 2 + 13) as u64),
        ("empty.txt", 0),
    ] {
        let seq: Vec<u64> = log
            .bytes
            .iter()
            .filter(|(r, _, _)| r.as_str() == rel)
            .map(|(_, d, _)| *d)
            .collect();
        assert!(
            seq.iter().all(|d| *d <= total),
            "{rel}: done 超 total: {seq:?}"
        );
        if total == 0 {
            assert!(seq.is_empty(), "{rel}: 0 字节文件不得有 Bytes 事件");
        } else {
            assert!(!seq.is_empty(), "{rel}: 必须有 Bytes 事件");
            assert!(
                seq.windows(2).all(|w| w[0] < w[1]),
                "{rel}: done 必须严格单调: {seq:?}"
            );
            assert_eq!(*seq.last().unwrap(), total, "{rel}: 终值=total");
        }
        assert_eq!(
            log.dones.iter().filter(|r| r.as_str() == rel).count(),
            1,
            "{rel}: FileDone 恰一次"
        );
    }
    // 事件不得来自清单外的文件
    assert!(
        log.bytes
            .iter()
            .all(|(r, _, _)| ["a.txt", "docs/b.bin"].contains(&r.as_str())),
        "事件只属于清单内非空文件: {:?}",
        log.bytes
    );
    assert_eq!(log.dones.len(), 3, "FileDone 总数恰为文件数");
}

/// 接收端暂停门:收端先拉门再进场(计划照发、主循环即刻挂起,一条块流
/// 都不收)→ 发端在 MAX_STREAMS 硬顶上停走——quinn 默认 100 条并发单向
/// 流,收端不 accept 流就永无增额,发端 open_uni 死挡(吸收态:不受窗口
/// 抖动/计时影响);断言字节冻结且远未推完、双侧任务未完;复位后完成,
/// 内容一致。数据规模刻意选 150 流(>100):靠并发流上限而非接收窗口
/// 建立背压——默认连接窗近乎无界,窗口路在测试时序下不可靠(曾有
/// 停走-突刺假稳定)。
#[tokio::test]
async fn paused_receiver_backpressures() {
    const PER_FILE: usize = 64 * 1024;
    const N: usize = 150;
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let mut items = Vec::new();
    for i in 0..N {
        items.push((
            make_file(src.path(), &format!("q{i:03}.bin"), PER_FILE),
            format!("q{i:03}.bin"),
        ));
    }
    let expected_total = (PER_FILE * N) as u64;
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    // 发送端照常全速;收端的门先拉再进场
    let gate = PauseGate::new();
    gate.set(true);
    let totals: Arc<Mutex<ByteTotals>> = Arc::new(Mutex::new(ByteTotals::default()));
    let cb_totals = totals.clone();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &move |p| {
            if let SendProgress::Bytes { rel, done, .. } = p {
                cb_totals.lock().unwrap().note(rel, *done);
            }
        }, &SendOptions::default())
        .await
        .unwrap()
    });
    let ctl = RecvCtl {
        pause: Some(gate.clone()),
        ..Default::default()
    };
    let dst_path = dst.path().to_path_buf();
    let recv_task =
        tokio::spawn(async move { transfer::recv_files_ctl(&rc, &dst_path, &ctl).await });

    // 等停走成立(有界):发端计数稳定即吸收态——未收流占满 100 条并发
    // 名额后 open_uni 永挡,收端不进场就无任何增额来源。
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = totals.lock().unwrap().total();
    let mut quiet = Instant::now();
    loop {
        assert!(
            Instant::now() < deadline,
            "sender bytes never settled after pause"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = totals.lock().unwrap().total();
        if now == last {
            if quiet.elapsed() >= Duration::from_millis(800) {
                break;
            }
        } else {
            last = now;
            quiet = Instant::now();
        }
    }
    // 停走断言:冻结持续(再采样零差)、远未推完(并发流硬顶之下)、
    // 双侧任务都不得在挂起期间完成
    let c1 = totals.lock().unwrap().total();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let c2 = totals.lock().unwrap().total();
    assert_eq!(c1, c2, "收端挂起期间发送字节必须停走");
    assert!(
        c2 < expected_total,
        "必须被并发流上限挡在中途: {c2}/{expected_total}"
    );
    assert!(
        !send_task.is_finished() && !recv_task.is_finished(),
        "双侧任务都不得在挂起期间完成"
    );

    gate.set(false);
    let out = recv_task.await.unwrap();
    assert!(
        out.error.is_none(),
        "recv error: {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    let report = send_task.await.unwrap();
    assert_eq!(out.paths.len(), N);
    assert_eq!(report.bytes_sent, expected_total);
    for i in 0..N {
        let name = format!("q{i:03}.bin");
        assert_eq!(
            std::fs::read(dst.path().join(&name)).unwrap(),
            std::fs::read(src.path().join(&name)).unwrap(),
            "{name} 内容一致"
        );
    }
}

/// M3c Task 6(daemon 侧接收装配收敛为单点,Task 5 评审遗留闭合):注错
/// 钩子轮必须与生产轮共用同一 RecvCtl 装配——skip 策略在 close_after_files
/// 注错轮同样生效:被 skip 的文件不传输不落盘,fault 在首个真实 finalize
/// 后断连;paths 只含落盘文件。若注错轮丢 ctl(空策略),skip 文件会被
/// 传输落盘,本测试即翻车。
#[tokio::test]
async fn fault_round_carries_ctl_policy() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f_keep = make_file(src.path(), "keep.bin", 8192);
    let f_skip = make_file(src.path(), "skipme.bin", 4096);
    let items = vec![
        (f_keep.clone(), "keep.bin".to_string()),
        (f_skip.clone(), "skipme.bin".to_string()),
    ];
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = build_manifest(&items, &a).unwrap();
    let send_task = tokio::spawn(async move {
        // 注错断连后发送方必然报错:结果忽略,join 防任务泄漏
        let _ = transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default()).await;
    });
    let ctl = RecvCtl {
        policy: TransferPolicy {
            skip: vec!["skipme.bin".to_string()],
            overwrite: vec![],
        },
        ..Default::default()
    };
    // ctl + 注错钩子同一轮:首个真实 finalize(keep.bin;skip 无 finalize)
    // 后主动断连——recv 侧按 fault 语义干净返回已落盘路径(错误显形在发送侧)
    let out = transfer::recv_files_ctl_fault(&rc, dst.path(), &ctl, Some(1)).await;
    assert!(
        out.error.is_none(),
        "fault 钩子是干净返回(错误显形在发送侧): {:?}",
        out.error.map(|e| format!("{e:#}"))
    );
    let names: Vec<String> = out
        .paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["keep.bin"], "仅落盘文件随 paths 穿出: {names:?}");
    // ctl 生效的硬证据:skipme.bin 无终名也无 .part(空 ctl 下它是 Full 计划,
    // 断连时必留 .part 残迹)
    assert!(!dst.path().join("skipme.bin").exists(), "skip 文件不得落盘");
    assert!(
        !dst.path().join(".mtdss").join("skipme.bin.part").exists(),
        "skip 文件不得有 .part 残迹(空 ctl 下断连必留)"
    );
    assert_eq!(
        std::fs::read(dst.path().join("keep.bin")).unwrap(),
        std::fs::read(&f_keep).unwrap(),
        "keep.bin 内容一致"
    );
    send_task.await.unwrap();
}
