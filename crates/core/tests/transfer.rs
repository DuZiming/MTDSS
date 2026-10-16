//! 传输引擎集成测试：分块校验传输 + 磁盘预检 + 同名并发隔离。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::endpoint as ep;
use mtdss_core::transport::transfer;

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

        let f_path = f.clone();
        let send_task = tokio::spawn(async move {
            transfer::send_file(&sc, &f_path, name, &a, &|_, _| {}).await.unwrap()
        });
        let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
        send_task.await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).unwrap(), std::fs::read(&f).unwrap());
        assert!(got[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(name.rsplit('/').next().unwrap()));
        // .part 清理：成功传输后不残留
        assert!(std::fs::read_dir(dst.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| !e.file_name().to_string_lossy().contains(".part")));
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
    let f2 = f.clone();
    tokio::spawn(async move {
        transfer::send_file(&sc, &f2, "empty", &a, &|_, _| {})
            .await
            .unwrap();
    });
    let got = transfer::recv_files(&rc, dst.path(), &b).await.unwrap();
    assert_eq!(std::fs::read(&got[0]).unwrap(), Vec::<u8>::new());
}

#[tokio::test]
async fn disk_precheck_rejects_when_insufficient() {
    // 空间预检函数行为：free_bytes_at 真实查询；reject_for_space 阈值正确。
    // recv 侧在 accept 前用 free_bytes_at(out_dir) 与 manifest 总大小比较。
    let tmp = tempfile::tempdir().unwrap();
    assert!(transfer::free_bytes_at(tmp.path()) > 0);
    assert!(transfer::reject_for_space(u64::MAX / 2, 1));
    assert!(!transfer::reject_for_space(1, u64::MAX));
}

#[tokio::test]
async fn concurrent_same_name_outputs_do_not_collide() {
    // 同一接收端、两个并发传输、同名文件：transfer_id 隔离 .part，
    // 冲突改名保证两个 same* 都落盘。
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let a1 = DeviceIdentity::generate();
    let a2 = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let b_seed = b.signing.to_bytes();
    let mut allowed = HashMap::new();
    allowed.insert(a1.device_id.clone(), a1.public_key());
    allowed.insert(a2.device_id.clone(), a2.public_key());
    let ep_b = ep::make_server_endpoint(localhost0(), &b, &allowed).unwrap();
    let addr = ep_b.local_addr().unwrap();
    let ep1 = ep::make_endpoint(localhost0()).unwrap();
    let ep2 = ep::make_endpoint(localhost0()).unwrap();

    let f1 = make_file(src.path(), "same.txt", 1000);
    let f2b = make_file(src.path(), "same.txt", 2000);
    // VerifyingKey 是 Copy：两个发送任务各自固定 B 的公钥
    let b_pk = b.public_key();

    let allowed1 = allowed.clone();
    let allowed2 = allowed.clone();
    let dst_path = dst.path().to_path_buf();
    let server = tokio::spawn(async move {
        let (c1, _) = ep::accept_pinned(&ep_b, &allowed1).await.unwrap();
        let me1 = DeviceIdentity::from_seed(&b_seed);
        let d1 = dst_path.clone();
        let h1 =
            tokio::spawn(async move { transfer::recv_files(&c1, &d1, &me1).await.unwrap() });
        let (c2, _) = ep::accept_pinned(&ep_b, &allowed2).await.unwrap();
        let me2 = DeviceIdentity::from_seed(&b_seed);
        let d2 = dst_path.clone();
        let h2 =
            tokio::spawn(async move { transfer::recv_files(&c2, &d2, &me2).await.unwrap() });
        let r1 = h1.await.unwrap();
        let r2 = h2.await.unwrap();
        (r1, r2)
    });
    let s1 = tokio::spawn(async move {
        let c = ep::connect_pinned(&ep1, addr, &b_pk, &a1).await.unwrap();
        transfer::send_file(&c, &f1, "same.txt", &a1, &|_, _| {})
            .await
            .unwrap();
    });
    let s2 = tokio::spawn(async move {
        let c = ep::connect_pinned(&ep2, addr, &b_pk, &a2).await.unwrap();
        transfer::send_file(&c, &f2b, "same.txt", &a2, &|_, _| {})
            .await
            .unwrap();
    });
    s1.await.unwrap();
    s2.await.unwrap();
    // 等两个接收端都完成（send 完成=已收到最终确认，双保险）
    server.await.unwrap();

    // 落盘为 same.txt 与自动改名的副本，且无 .part 残留
    let names: Vec<String> = std::fs::read_dir(dst.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.iter().filter(|n| n.starts_with("same")).count(), 2);
    assert!(names.iter().all(|n| !n.contains(".part")));
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

    let f_path = f.clone();
    // TempDir 留在主任务,只把路径移交 recv 任务:任务结束时临时目录若随
    // 任务析构,主任务将读不到落盘文件。
    let dst_path = dst.path().to_path_buf();
    let send_task = tokio::spawn(async move {
        transfer::send_file(&sc, &f_path, "large.bin", &a, &|_, _| {}).await.unwrap()
    });
    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, &dst_path, &b).await.unwrap() });
    send_task.await.unwrap();
    let landed = recv_task.await.unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}

#[tokio::test]
async fn corrupt_chunk_never_written() {
    // 手工驱动协议:manifest 合法,但块数据与清单哈希不符 →
    // 接收方必须拒块,坏字节不得落盘(.part 该偏移保持预分配零)。
    use prost::Message as _;

    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let f = make_file(src.path(), "victim.bin", transfer::CHUNK_SIZE);
    let a = DeviceIdentity::generate();
    let b = DeviceIdentity::generate();
    let mut allowed: HashMap<String, ed25519_dalek::VerifyingKey> = HashMap::new();
    allowed.insert(a.device_id.clone(), a.public_key());
    let (sc, rc) = connected_pair(&a, &b, &allowed).await;

    let man = mtdss_core::protocol::manifest::build_manifest(&[(f.clone(), "victim.bin".to_string())], &a)
        .unwrap();
    let tid = man.transfer_id.clone();
    let part_path = out.path().join(format!("{tid}.mtdss-part"));
    // TempDir 留在主任务,只把路径移交 recv 任务
    let out_path = out.path().to_path_buf();

    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, &out_path, &b).await });

    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let mb = man.encode_to_vec();
    ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
    ctl_tx.write_all(&mb).await.unwrap();
    let mut verdict = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut ctl_rx, &mut verdict).await.unwrap();
    assert_eq!(verdict[0], 1, "receiver must accept (space is fine)");

    // 唯一一块,发坏数据
    let mut uni = sc.open_uni().await.unwrap();
    uni.write_all(&0u64.to_be_bytes()).await.unwrap();
    uni.write_all(&(transfer::CHUNK_SIZE as u64).to_be_bytes()).await.unwrap();
    uni.write_all(&vec![0xDEu8; transfer::CHUNK_SIZE]).await.unwrap();
    uni.finish().unwrap();

    let err = recv_task.await.unwrap().unwrap_err();
    assert!(
        err.to_string().contains("chunk hash mismatch"),
        "unexpected error: {err:#}"
    );
    let part = std::fs::read(&part_path).unwrap();
    assert_eq!(part.len(), transfer::CHUNK_SIZE, "preallocated size");
    assert!(
        part.iter().all(|&b| b == 0),
        "corrupt chunk bytes must never hit the .part file"
    );
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

    let man = mtdss_core::protocol::manifest::build_manifest(&[(f.clone(), "ooo.bin".to_string())], &a)
        .unwrap();
    let chunks = man.files[0].chunks.clone();
    // TempDir 留在主任务,只把路径移交 recv 任务
    let out_path = out.path().to_path_buf();

    let recv_task =
        tokio::spawn(async move { transfer::recv_files(&rc, &out_path, &b).await });

    let (mut ctl_tx, mut ctl_rx) = sc.open_bi().await.unwrap();
    let mb = man.encode_to_vec();
    ctl_tx.write_all(&(mb.len() as u32).to_be_bytes()).await.unwrap();
    ctl_tx.write_all(&mb).await.unwrap();
    let mut verdict = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut ctl_rx, &mut verdict).await.unwrap();
    assert_eq!(verdict[0], 1);

    async fn send_chunk(
        conn: &quinn::Connection,
        id: u64,
        data: &[u8],
    ) {
        let mut uni = conn.open_uni().await.unwrap();
        uni.write_all(&id.to_be_bytes()).await.unwrap();
        uni.write_all(&(data.len() as u64).to_be_bytes()).await.unwrap();
        uni.write_all(data).await.unwrap();
        uni.finish().unwrap();
        // 不等待:先开的流流 ID 更低,QUIC 按流交付,accept_uni 顺序确定
    }

    // 后块先发
    let second = &data[(chunks[1].offset as usize)..(chunks[1].offset + chunks[1].length) as usize];
    send_chunk(&sc, chunks[1].offset, second).await;
    let first = &data[..(chunks[0].length as usize)];
    send_chunk(&sc, chunks[0].offset, first).await;

    // 完成标记 + 最终确认
    ctl_tx.write_all(&[0xFF]).await.unwrap();
    let mut ack = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut ctl_rx, &mut ack).await.unwrap();
    assert_eq!(ack[0], 0x01);

    let landed = recv_task.await.unwrap().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
}
