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

// ===== T5:中继启动旗标解析(parse 沿 parse_stun_ports 先例提为可测函数)=====

#[test]
fn relay_port_parse_rejects_zero_and_non_numeric() {
    assert_eq!(signaling::relay::parse_relay_port("8788").unwrap(), 8788);
    assert!(signaling::relay::parse_relay_port("0").is_err(), "0 端口不可绑定");
    assert!(signaling::relay::parse_relay_port("abc").is_err(), "非数字拒");
    assert!(signaling::relay::parse_relay_port("").is_err());
    assert!(signaling::relay::parse_relay_port("70000").is_err(), "超 u16 范围拒");
    assert!(signaling::relay::parse_relay_port("-1").is_err());
}

#[test]
fn relay_host_parse_rejects_host_port_composite() {
    // main 追加 :{port} 拼通告地址:值里带 `:` 会产出畸形通告,必须拒
    assert!(signaling::relay::parse_relay_host("relay.example.com").is_ok());
    assert!(signaling::relay::parse_relay_host("198.51.100.10").is_ok());
    let err = signaling::relay::parse_relay_host("1.2.3.4:9000").unwrap_err();
    assert!(err.to_string().contains("bare host"), "报错要说明收裸 host: {err}");
    assert!(signaling::relay::parse_relay_host("[::1]:8788").is_err());
}

#[test]
fn relay_global_kbps_env_parse_defaults_and_fallback() {
    // env 缺省 → 默认 2500;坏值 → 回退默认(解析函数内打 warn);合法值原样;0=不限
    assert_eq!(signaling::relay::parse_global_kbps(None), 2500);
    assert_eq!(signaling::relay::parse_global_kbps(Some("not-a-number")), 2500);
    assert_eq!(signaling::relay::parse_global_kbps(Some("4096")), 4096);
    assert_eq!(signaling::relay::parse_global_kbps(Some("0")), 0);
}
