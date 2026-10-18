use mtdss_daemon::session::PeersCache;

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

#[tokio::test]
async fn relay_timeout_returns_error_on_stall() {
    // 契约：relay 的有界包装（flow::bounded）对永不完成的 future
    // （= WS sink 停滞时 SignalingClient::relay 的行为）在注入的超时后
    // 返回 Err，绝不永久挂起。外层 1s 超时保证：若包装层回归为无界，
    // 本测试显式失败而不是挂死测试套件。
    let start = std::time::Instant::now();
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mtdss_daemon::flow::bounded(
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

#[test]
fn session_ca_roundtrip_and_legacy_load() {
    let dir = tempfile::tempdir().unwrap();
    let s = mtdss_daemon::session::Session {
        server: "https://x".into(),
        jwt: "j".into(),
        refresh: "r".into(),
        device_id: "d".into(),
        ca: Some(dir.path().join("ca.crt")),
    };
    s.save(dir.path()).unwrap();
    let l = mtdss_daemon::session::Session::load(dir.path()).unwrap();
    assert_eq!(l.ca, s.ca);
    // 旧 session.json(无 ca 字段)仍可加载
    std::fs::write(
        dir.path().join("session.json"),
        r#"{"server":"http://x","jwt":"j","refresh":"r","device_id":"d"}"#,
    )
    .unwrap();
    assert!(mtdss_daemon::session::Session::load(dir.path()).unwrap().ca.is_none());
}
