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
    for (name, size) in [("小.md", 100usize), ("big.bin", transfer::CHUNK_SIZE * 2 + 555)] {
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
