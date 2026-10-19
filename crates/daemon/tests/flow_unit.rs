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

#[test]
fn collect_items_skips_reserved_entries_case_insensitive() {
    // 目录内大小写变体保留段(.MTDSS 目录、嵌套 .Mtdss 文件)一律跳过,
    // 与 core fs 列表过滤、manifest 首段校验三处同口径;精确匹配语义保留:
    // 后缀形 x.mtdss-part 非保留名本体,照常入清单
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("dir");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    std::fs::write(dir.join("sub").join("b.bin"), b"b").unwrap();
    std::fs::create_dir_all(dir.join(".MTDSS")).unwrap();
    std::fs::write(dir.join(".MTDSS").join("x.bin"), b"x").unwrap();
    std::fs::write(dir.join("sub").join(".Mtdss"), b"f").unwrap();
    std::fs::write(dir.join("x.mtdss-part"), b"keep").unwrap();

    let items = mtdss_daemon::flow::collect_items(&[dir]).unwrap();
    let rels: Vec<&str> = items.iter().map(|(_, r)| r.as_str()).collect();
    // walk_dir 按名排序:保留变体跳过后剩余 a.txt < sub < x.mtdss-part
    assert_eq!(
        rels,
        vec!["dir/a.txt", "dir/sub/b.bin", "dir/x.mtdss-part"],
        "case-variant reserved entries must be skipped, suffix names kept: {items:?}"
    );
}

#[test]
fn collect_items_top_level_reserved_input_case_insensitive() {
    // 顶层输入名 .MTDSS(目录)与 .Mtdss(文件)同 Err——与 .mtdss 同一
    // 保留语义。两者分置不同 tempdir(Windows 大小写不敏感 FS 上同名变体
    // 会相撞);目录内放内容,保证 Err 只能来自保留名检查而非"nothing to send"
    let b1 = tempfile::tempdir().unwrap();
    let d = b1.path().join(".MTDSS");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("x.bin"), b"x").unwrap();
    assert!(mtdss_daemon::flow::collect_items(&[d]).is_err());

    let b2 = tempfile::tempdir().unwrap();
    let f = b2.path().join(".Mtdss");
    std::fs::write(&f, b"x").unwrap();
    assert!(mtdss_daemon::flow::collect_items(&[f]).is_err());
}
