use mtdss_cli::flow::local_candidates;
use mtdss_cli::session::PeersCache;

#[test]
fn tofu_first_use_records_then_locks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.json");
    let a = mtdss_core::identity::DeviceIdentity::generate();
    let b = mtdss_core::identity::DeviceIdentity::generate();

    let mut cache = PeersCache::load(&path).unwrap();
    cache.expected_peer("dev-remote", &a.public_key()).unwrap(); // 首次记录
    cache.expected_peer("dev-remote", &a.public_key()).unwrap(); // 一致通过
    cache.save(&path).unwrap();

    let mut cache2 = PeersCache::load(&path).unwrap();
    assert!(cache2.expected_peer("dev-remote", &b.public_key()).is_err()); // 变更 → 硬错误
}

#[test]
fn candidates_include_loopback_and_private() {
    let c = local_candidates(12345);
    assert!(c.contains(&"127.0.0.1:12345".to_string()));
    // 测试机至少有一个 IPv4 接口
    assert!(c.len() >= 1);
}
