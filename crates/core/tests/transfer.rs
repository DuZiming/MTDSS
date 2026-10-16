//! 传输引擎集成测试(v2 API):多文件计划传输 + 磁盘预检 + 同名重传改名。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::protocol::manifest::build_manifest;
use mtdss_core::transport::endpoint as ep;
use mtdss_core::transport::intake;
use mtdss_core::transport::transfer::{self, SendOptions};
use mtdss_core::transport::FilePlan;
use mtdss_proto::manifest::Manifest;

fn localhost0() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

fn make_file(dir: &std::path::Path, name: &str, size: usize) -> PathBuf {
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

/// 建立 A(sender/client) -> B(receiver/server) 的固定连接对。
/// 返回 (client 连接, server 连接)；a/b 所有权留在调用方。
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

/// 单文件清单(items 与源文件一一对应)。
fn single_man(f: &std::path::Path, rel: &str, id: &DeviceIdentity) -> Manifest {
    build_manifest(&[(f.to_path_buf(), rel.to_string())], id).unwrap()
}

/// 读一帧计划回包(tag 必须为 0x01)并解析为 FilePlan 列表。
async fn read_plan(rx: &mut quinn::RecvStream) -> Vec<FilePlan> {
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

/// 断言 `.mtdss/` 收件区内没有任何 `.part` 残留(成功传输后只应剩记录)。
fn assert_no_part_left(out_dir: &std::path::Path) {
    for e in std::fs::read_dir(out_dir.join(intake::INTAKE_DIR))
        .unwrap()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        assert!(!name.ends_with(".part"), "leftover part file: {name}");
    }
}

#[tokio::test]
async fn end_to_end_single_and_multichunk_and_unicode() {
    for (name, size) in [
        ("小.md", 100usize),
        ("empty.txt", 0usize),
        ("big.bin", transfer::CHUNK_SIZE * 2 + 555),
    ] {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let f = make_file(src.path(), name, size);
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        let mut allowed = HashMap::new();
        allowed.insert(a.device_id.clone(), a.public_key());
        let (sc, rc) = connected_pair(&a, &b, &allowed).await;

        let man = single_man(&f, name, &a);
        let items = vec![(f.clone(), name.to_string())];
        let send_task = tokio::spawn(async move {
            transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
                .await
                .unwrap()
        });
        let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
        let report = send_task.await.unwrap();
        assert_eq!(report.bytes_sent, size as u64);
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).unwrap(), std::fs::read(&f).unwrap());
        assert!(got[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(name.rsplit('/').next().unwrap()));
        assert_no_part_left(dst.path());
    }
}

#[tokio::test]
async fn zero_byte_file_transfers() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = src.path().join("empty");
    std::fs::write(&f, b"").unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;
    let man = single_man(&f, "empty", &a);
    let items = vec![(f.clone(), "empty".to_string())];
    tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap();
    });
    let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    assert_eq!(std::fs::read(&got[0]).unwrap(), Vec::<u8>::new());
}

#[tokio::test]
async fn disk_precheck_rejects_when_insufficient() {
    // 空间预检函数行为：free_bytes_at 真实查询；reject_for_space 阈值正确。
    // recv 侧在发计划前用 free_bytes_at(out_dir) 与 manifest 总大小比较。
    let tmp = tempfile::tempdir().unwrap();
    assert!(transfer::free_bytes_at(tmp.path()) > 0);
    assert!(transfer::reject_for_space(u64::MAX / 2, 1));
    assert!(!transfer::reject_for_space(1, u64::MAX));
}

/// v2 intake 记录按 rel 键控：同一输出目录的**并发**同名传输会互相踩
/// `.part`（两份记录同键），属设计外场景。同名冲突落盘语义由顺序重传
/// 覆盖：第二传内容/尺寸不同 → 记录不符 → 重收 → 冲突改名。
#[tokio::test]
async fn same_name_retransfer_lands_renamed() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let a1 = DeviceIdentity::generate();
    let a2 = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    // 两个源分居不同子目录:同路径会互相覆盖,内容将不再不同
    let sub1 = src.path().join("s1");
    let sub2 = src.path().join("s2");
    std::fs::create_dir_all(&sub1).unwrap();
    std::fs::create_dir_all(&sub2).unwrap();
    let f1 = make_file(&sub1, "same.txt", 1000);
    let f2 = make_file(&sub2, "same.txt", 2000);

    let mut allowed = HashMap::new();
    allowed.insert(a1.device_id.clone(), a1.public_key());
    allowed.insert(a2.device_id.clone(), a2.public_key());

    // 第一传：a1 → b
    let (sc1, rc1) = connected_pair(&a1, &b, &allowed).await;
    let man1 = single_man(&f1, "same.txt", &a1);
    let items1 = vec![(f1.clone(), "same.txt".to_string())];
    let send1 = tokio::spawn(async move {
        transfer::send_files(&sc1, &man1, &items1, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let got1 = transfer::recv_files(&rc1, dst.path(), &b).await.unwrap();
    send1.await.unwrap();
    assert_eq!(got1.len(), 1);
    assert_eq!(std::fs::read(&got1[0]).unwrap(), std::fs::read(&f1).unwrap());

    // 第二传：a2 → 同一 out_dir，同名不同内容
    let (sc2, rc2) = connected_pair(&a2, &b, &allowed).await;
    let man2 = single_man(&f2, "same.txt", &a2);
    let items2 = vec![(f2.clone(), "same.txt".to_string())];
    let send2 = tokio::spawn(async move {
        transfer::send_files(&sc2, &man2, &items2, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let got2 = transfer::recv_files(&rc2, dst.path(), &b).await.unwrap();
    send2.await.unwrap();
    assert_eq!(got2.len(), 1);

    // 落盘为 same.txt 与自动改名的副本，且无 .part 残留
    let p_plain = dst.path().join("same.txt");
    let p_renamed = dst.path().join("same (1).txt");
    let mut bodies = [
        std::fs::read(&p_plain).unwrap_or_default(),
        std::fs::read(&p_renamed).unwrap_or_default(),
    ];
    bodies.sort_by_key(|v| v.len());
    assert_eq!(bodies[0].len(), 1000);
    assert_eq!(bodies[1].len(), 2000);
    assert_no_part_left(dst.path());
}

#[tokio::test]
async fn large_file_streams_correctly() {
    // 16 MiB(16 块)覆盖定位写 + 读回校验
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "large.bin", transfer::CHUNK_SIZE * 16);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = single_man(&f, "large.bin", &a);
    let items = vec![(f.clone(), "large.bin".to_string())];
    // TempDir 留在主任务,只把路径移交 recv 任务:任务结束时临时目录若随
    // 任务析构,主任务将读不到落盘文件。
    let dst_path = dst.path().to_path_buf();
    let send_task = tokio::spawn(async move {
        transfer::send_files(&sc, &man, &items, &|_| {}, &SendOptions::default())
            .await
            .unwrap()
    });
    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, &dst_path, &b).await.unwrap() });
    let report = send_task.await.unwrap();
    let landed = recv_task.await.unwrap();
    assert_eq!(report.bytes_sent, (transfer::CHUNK_SIZE * 16) as u64);
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}

#[tokio::test]
async fn corrupt_chunk_never_written() {
    // 手工驱动协议:manifest 合法,但块数据与清单哈希不符 →
    // 接收方必须拒块(不落盘、不进 bitmap,spec §4.4),坏字节留在
    // .part 的预分配零位;块自然落入下一轮 checkpoint 的重请求
    // (Resume 位图),补发好块后传输干净完成。
    use prost::Message as _;

    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "victim.bin", transfer::CHUNK_SIZE);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = single_man(&f, "victim.bin", &a);
    let part_path = intake::part_path(out.path(), "victim.bin");
    // TempDir 留在主任务,只把路径移交 recv 任务
    let out_path = out.path().to_path_buf();

    let recv_task = tokio::spawn(async move { transfer::recv_files(&rc, &out_path, &b).await });

    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let mb = man.encode_to_vec();
    ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
    ctl_tx.write_all(&mb).await.unwrap();
    let plans = read_plan(&mut ctl_rx).await;
    assert_eq!(plans, vec![FilePlan::Full], "fresh receive must plan Full");

    // 唯一块(file_idx=0 前缀),发坏数据 → 接收方静默丢弃(不得 Err)
    let mut uni = sc.open_uni().await.unwrap();
    uni.write_all(&0u32.to_be_bytes()).await.unwrap();
    uni.write_all(&0u64.to_be_bytes()).await.unwrap();
    uni.write_all(&(transfer::CHUNK_SIZE as u64).to_be_bytes()).await.unwrap();
    uni.write_all(&vec![0xDEu8; transfer::CHUNK_SIZE]).await.unwrap();
    uni.finish().unwrap();

    // checkpoint:坏块未进 bitmap → 计划必须退化为 Resume(位 0 置位)
    ctl_tx.write_all(&[0xFF]).await.unwrap();
    let plans = read_plan(&mut ctl_rx).await;
    match &plans[..] {
        [FilePlan::Resume(bmp)] => {
            assert_eq!(bmp.len(), 1, "single chunk → 1 bitmap byte");
            assert_eq!(bmp[0], 0b0000_0001, "missing chunk 0 must be re-requested");
        }
        other => panic!("expected Resume plan after bad chunk, got {other:?}"),
    }
    // 坏字节确实未落盘:.part 保持预分配零
    let part = std::fs::read(&part_path).unwrap();
    assert_eq!(part.len(), transfer::CHUNK_SIZE, "preallocated size");
    assert!(
        part.iter().all(|&b| b == 0),
        "corrupt chunk bytes must never hit the .part file"
    );

    // 补发好块 → checkpoint → FileDone + 终局 + EOF 舞步 → 干净完成
    let mut uni = sc.open_uni().await.unwrap();
    uni.write_all(&0u32.to_be_bytes()).await.unwrap();
    uni.write_all(&0u64.to_be_bytes()).await.unwrap();
    uni.write_all(&(transfer::CHUNK_SIZE as u64).to_be_bytes()).await.unwrap();
    uni.write_all(&data).await.unwrap();
    uni.finish().unwrap();
    ctl_tx.write_all(&[0xFF]).await.unwrap();
    let mut ack = [0u8; 1];
    loop {
        ctl_rx.read_exact(&mut ack).await.unwrap();
        if ack[0] == 0x03 {
            break; // 终局
        }
        assert_eq!(ack[0], 0x02, "expected FileDone then Final, got 0x{:02x}", ack[0]);
        let mut idx = [0u8; 4]; // FileDone 携带 u32 文件序号,排干
        ctl_rx.read_exact(&mut idx).await.unwrap();
    }
    ctl_tx.finish().unwrap();
    let landed = recv_task.await.unwrap().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data, "re-requested good chunk must complete the file");
    assert_no_part_left(out.path());
}

#[tokio::test]
async fn reverse_order_chunks_assemble_correctly() {
    // 两个块,后块先完整发送:接收方按任意顺序定位写仍能重组正确文件。
    use prost::Message as _;

    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "ooo.bin", transfer::CHUNK_SIZE * 2);
    let data = std::fs::read(&f).unwrap();
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = single_man(&f, "ooo.bin", &a);
    let chunks = man.files[0].chunks.clone();
    // TempDir 留在主任务,只把路径移交 recv 任务
    let out_path = out.path().to_path_buf();

    let recv_task = tokio::spawn(async move { transfer::recv_files(&rc, &out_path, &b).await });

    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let mb = man.encode_to_vec();
    ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
    ctl_tx.write_all(&mb).await.unwrap();
    let plans = read_plan(&mut ctl_rx).await;
    assert_eq!(plans, vec![FilePlan::Full]);

    async fn send_chunk(conn: &quinn::Connection, file_idx: u32, id: u64, data: &[u8]) {
        let mut uni = conn.open_uni().await.unwrap();
        uni.write_all(&file_idx.to_be_bytes()).await.unwrap();
        uni.write_all(&id.to_be_bytes()).await.unwrap();
        uni.write_all(&(data.len() as u64).to_be_bytes()).await.unwrap();
        uni.write_all(data).await.unwrap();
        uni.finish().unwrap();
        // 不等待:先开的流流 ID 更低,QUIC 按流交付,accept_uni 顺序确定
    }

    // 后块先发
    let second =
        &data[(chunks[1].offset as usize)..(chunks[1].offset + chunks[1].length) as usize];
    send_chunk(&sc, 0, chunks[1].offset, second).await;
    let first = &data[..(chunks[0].length as usize)];
    send_chunk(&sc, 0, chunks[0].offset, first).await;

    // 完成标记 + 终局确认(先到的 FileDone 帧要排干)+ EOF 舞步
    ctl_tx.write_all(&[0xFF]).await.unwrap();
    let mut ack = [0u8; 1];
    loop {
        ctl_rx.read_exact(&mut ack).await.unwrap();
        if ack[0] == 0x02 {
            let mut idx = [0u8; 4];
            ctl_rx.read_exact(&mut idx).await.unwrap();
            continue; // FileDone:继续等终局
        }
        assert_eq!(ack[0], 0x03);
        break;
    }
    ctl_tx.finish().unwrap();

    let landed = recv_task.await.unwrap().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}
