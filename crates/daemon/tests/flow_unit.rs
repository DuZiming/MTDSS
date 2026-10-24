use mtdss_core::transport::transfer::SendError;
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

// ---------- Task 5:中继兜底错误面 ----------

/// Relay 终局错误外层 retryable 恒 false(裁定 G:配额尽/未开通不空耗重试
/// 轮次;修复轮裁定:超并发拒 6 为容量瞬态——服务端 120s 空闲回收自愈,
/// 经 RelayFailed 归 Network 可重试,不在此列,钉于 relay_e2e 的
/// refusal_cap_limit_retryable);死因映射产物同口径;Network 对照仍可重试;
/// Display 文案沿 #[error("relay: {message}")]。
#[test]
fn relay_error_not_retryable() {
    let denied = SendError::Relay {
        code: 5,
        message: "中继配额已耗尽".into(),
    };
    assert!(
        !mtdss_daemon::flow::retryable(&denied),
        "Relay 分配拒/终局死因不得进重试: {denied}"
    );
    let dead = mtdss_daemon::fallback::relay_dead_err("quota");
    assert!(
        !mtdss_daemon::flow::retryable(&dead),
        "死因映射的终局错误不得进重试: {dead}"
    );
    assert!(mtdss_daemon::flow::retryable(&SendError::Network("x".into())));
    assert_eq!(denied.to_string(), "relay: 中继配额已耗尽");
}

/// 死因 → 错误分类映射:quota → code 5 配额终局文案;disabled → 停用终局
/// 文案;idle(空闲回收)/空(旧服务器不带死因)→ Network 可重试
/// 「中继会话已中断」。
#[test]
fn relay_dead_err_maps_reasons() {
    match mtdss_daemon::fallback::relay_dead_err("quota") {
        SendError::Relay { code, message } => {
            assert_eq!(code, 5, "配额杀死因映射 code 5(文案钉死口径)");
            assert_eq!(message, "中继配额已耗尽");
        }
        other => panic!("quota 须映射终局 Relay: {other:?}"),
    }
    match mtdss_daemon::fallback::relay_dead_err("disabled") {
        SendError::Relay { message, .. } => assert_eq!(message, "中继已被停用"),
        other => panic!("disabled 须映射终局 Relay: {other:?}"),
    }
    for reason in ["idle", ""] {
        match mtdss_daemon::fallback::relay_dead_err(reason) {
            SendError::Network(msg) => {
                assert_eq!(msg, "中继会话已中断");
                assert!(
                    mtdss_daemon::flow::retryable(&SendError::Network(msg)),
                    "{reason:?} 须可重试"
                );
            }
            other => panic!("{reason:?} 须映射可重试 Network: {other:?}"),
        }
    }
}
