//! plain HTTP 监听模式(`--plain-http`):回环反代场景(Caddy 终止 TLS 后回源)。
//! TLS 模式(直连/过渡期/dev CA)不受影响,见 tests/tls.rs。

#[tokio::test]
async fn plain_server_serves_http_get_devices_401() {
    // serve_plain 内部自行 bind;先用 port 0 探一个空闲端口再交给它
    // (沿用 spawn_test_server_with 的 bind 形态),避免固定高端口在 CI 撞车。
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    let app = signaling::build_router(signaling::test_state().await);
    tokio::spawn(signaling::serve_plain(app, addr));

    // 401 = 明文可达且路由 + 鉴权全通(未带 Bearer 即拒),无需有效凭据
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/devices");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match client.get(&url).send().await {
            Ok(resp) => {
                assert_eq!(resp.status(), 401, "GET /devices without auth must 401");
                break;
            }
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => panic!("plain server unreachable at {url}: {e}"),
        }
    }
}

#[test]
fn plain_and_tls_flags_are_mutually_exclusive() {
    // 双旗标冲突必须非 0 退出并回显 usage(在任何 DB/文件副作用之前)
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_signaling-server"))
        .args([
            "--plain-http",
            "127.0.0.1:8787",
            "--tls-cert",
            "x.crt",
            "--tls-key",
            "x.key",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "--plain-http + --tls-cert/--tls-key must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("usage"),
        "conflict bail must print usage text, got: {stderr}"
    );
}
