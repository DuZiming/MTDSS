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

#[tokio::test]
async fn relay_timeout_returns_error_on_stall() {
    // 契约：relay 的有界包装（flow::bounded）对永不完成的 future
    // （= WS sink 停滞时 SignalingClient::relay 的行为）在注入的超时后
    // 返回 Err，绝不永久挂起。外层 1s 超时保证：若包装层回归为无界，
    // 本测试显式失败而不是挂死测试套件。
    let start = std::time::Instant::now();
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mtdss_cli::flow::bounded(
            "relay to peer",
            std::time::Duration::from_millis(50),
            std::future::pending::<anyhow::Result<()>>(),
        ),
    )
    .await
    .expect("bounded() must not hang on a never-completing future");
    assert!(res.is_err(), "stalled future must surface as Err");
    let msg = format!("{}", res.unwrap_err());
    assert!(msg.contains("timed out"), "error should name the timeout: {msg}");
    assert!(msg.contains("relay to peer"), "error should carry the label: {msg}");
    assert!(start.elapsed() >= std::time::Duration::from_millis(50));
}
