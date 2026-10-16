//! CLI 编排端到端：真实 `run_send`/`run_recv` 走真实信令服务器（WS relay）
//! 与 QUIC 直连，两台"设备"在同进程并发运行，各自使用独立 home 目录
//! （home 显式传参注入，不依赖进程环境变量——同进程并发改 env 会互相踩）。
//! 覆盖：身份生成落盘、REST 注册、session.json、TOFU 固定、presence 等待、
//! ConnectRequest/Accept/Candidates 信令、逐候选连接、分块校验传输、落盘。

use mtdss_cli::session::Session;
use signaling::spawn_test_server;
use std::time::{Duration, Instant};

/// dev-mode 验证码直接回带，直接走 REST 注册（flow::register 是交互式 stdin 流程）。
async fn register_rest(
    addr: std::net::SocketAddr,
    email: &str,
    pubkey_hex: &str,
    name: &str,
) -> (String, String, String) {
    let client = reqwest::Client::new();
    let c: serde_json::Value = client
        .post(format!("http://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .post(format!("http://{addr}/auth/verify"))
        .json(&serde_json::json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": pubkey_hex,
            "name": name,
            "platform": "test"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        v["jwt"].as_str().unwrap().into(),
        v["refresh"].as_str().unwrap().into(),
        v["device_id"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn cli_send_recv_end_to_end() {
    // 整体兜底超时：任何环节退化成挂死都让测试显式失败而非卡死套件
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");

        // 两侧 home 隔离：身份走真实 load_identity（首次自动生成并落盘 0600）
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();

        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "e2e@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "side-A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "e2e@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "side-B",
        )
        .await;
        assert_ne!(dev_a, dev_b);

        let sess_a = Session {
            server: server.clone(),
            jwt: jwt_a,
            refresh: ref_a,
            device_id: dev_a.clone(),
            ca: None,
        };
        sess_a.save(home_a.path()).unwrap();
        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
            ca: None,
        };
        sess_b.save(home_b.path()).unwrap();

        // 持久化层自检：两侧目录互串不了（session.json 各自落在各自 home）
        assert_eq!(
            Session::load(home_a.path()).unwrap().device_id,
            dev_a,
            "session.json must round-trip in side A's home"
        );
        assert_eq!(Session::load(home_b.path()).unwrap().device_id, dev_b);

        // B 先收：真实 run_recv 后台任务（owned 数据 'static）
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv(&sess, &home, &out)
                    .await
                    .expect("run_recv failed")
            })
        };

        // A 轮询设备目录直到 B 上线（WS presence 已注册），再发起发送
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            let b_online = devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            });
            if b_online {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "receiver never came online; devices: {devices:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // A 发送小随机文件，unicode 文件名（真实 rel 路径校验 + 分块传输）
        let src = tempfile::tempdir().unwrap();
        let name = "季报 报告 🚀.txt";
        let fpath = src.path().join(name);
        let mut data = vec![0u8; 4096];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        let t0 = Instant::now();
        mtdss_cli::flow::run_send(&sess_a, home_a.path(), &fpath, &dev_b, &|_| {})
            .await
            .expect("run_send failed");
        eprintln!("e2e transfer took {:?}", t0.elapsed());

        // 接收方完成并返回落盘路径；内容字节级一致
        let landed = tokio::time::timeout(Duration::from_secs(15), b_task)
            .await
            .expect("receiver task did not finish in time")
            .unwrap();
        assert_eq!(
            landed.file_name().and_then(|n| n.to_str()),
            Some(name),
            "file name must be preserved end to end"
        );
        assert_eq!(
            std::fs::read(&landed).unwrap(),
            data,
            "received bytes must be identical"
        );
    });
    run.await.expect("cli e2e exceeded overall 90s timeout");
}

/// TLS 版注册:信任 spawn_test_tls_server 的 CA。
async fn register_rest_tls(
    base: &str,
    ca_path: &std::path::Path,
    email: &str,
    pubkey_hex: &str,
    name: &str,
) -> (String, String, String) {
    let pem = std::fs::read(ca_path).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap())
        .build()
        .unwrap();
    let c: serde_json::Value = client
        .post(format!("{base}/auth/request-code"))
        .json(&serde_json::json!({ "email": email }))
        .send().await.unwrap().json().await.unwrap();
    let v: serde_json::Value = client
        .post(format!("{base}/auth/verify"))
        .json(&serde_json::json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": pubkey_hex,
            "name": name,
            "platform": "test"
        }))
        .send().await.unwrap().json().await.unwrap();
    (
        v["jwt"].as_str().unwrap().into(),
        v["refresh"].as_str().unwrap().into(),
        v["device_id"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn cli_send_recv_end_to_end_over_tls() {
    let run = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        let (addr, ca, _dir) = signaling::spawn_test_tls_server().await;
        let server = format!("https://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();

        let (jwt_a, ref_a, dev_a) = register_rest_tls(
            &server, &ca, "tls@x.y",
            &hex::encode(id_a.public_key().as_bytes()), "side-A",
        ).await;
        let (jwt_b, ref_b, dev_b) = register_rest_tls(
            &server, &ca, "tls@x.y",
            &hex::encode(id_b.public_key().as_bytes()), "side-B",
        ).await;

        let mk = |jwt: String, refresh: String, dev: String| Session {
            server: server.clone(),
            jwt,
            refresh,
            device_id: dev,
            ca: Some(ca.clone()),
        };
        let sess_a = mk(jwt_a, ref_a, dev_a.clone());
        let sess_b = mk(jwt_b, ref_b, dev_b.clone());
        sess_a.save(home_a.path()).unwrap();
        sess_b.save(home_b.path()).unwrap();

        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv(&sess, &home, &out)
                    .await
                    .expect("run_recv failed")
            })
        };

        // 等 B 上线(经 https 的 devices 查询)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "receiver never online");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        let src = tempfile::tempdir().unwrap();
        let name = "tls 传输 📦.bin";
        let fpath = src.path().join(name);
        let mut data = vec![0u8; 8192];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        mtdss_cli::flow::run_send(&sess_a, home_a.path(), &fpath, &dev_b, &|_| {})
            .await
            .expect("run_send failed");

        let landed = tokio::time::timeout(std::time::Duration::from_secs(15), b_task)
            .await
            .expect("receiver task did not finish in time")
            .unwrap();
        assert_eq!(
            landed.file_name().and_then(|n| n.to_str()),
            Some(name)
        );
        assert_eq!(std::fs::read(&landed).unwrap(), data);
    });
    run.await.expect("tls e2e exceeded 90s");
}

#[tokio::test]
async fn wrong_ca_is_rejected() {
    let (addr, _ca, _dir) = signaling::spawn_test_tls_server().await;
    // 另一套独立 CA:不受信
    let other = tempfile::tempdir().unwrap();
    signaling::tls::generate_dev_tls(other.path()).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_pem(
                &std::fs::read(other.path().join("ca.crt")).unwrap(),
            )
            .unwrap(),
        )
        .build()
        .unwrap();
    let res = client
        .post(format!("https://{addr}/auth/request-code"))
        .json(&serde_json::json!({ "email": "x@y.z" }))
        .send()
        .await;
    assert!(res.is_err(), "untrusted CA must fail the TLS handshake");
}

/// M2b:信令服务器通告本地回环 STUN(真实起 stun-server),全链路
/// 候选收集(srflx=回环映射)→ 双向候选交换 → punch_connect → 传输。
#[tokio::test]
async fn cli_send_recv_with_stun_punch() {
    let run = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        // 真实 STUN 双端口(回环)
        let sa = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let sb = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let (pa, pb) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
        tokio::spawn(async move { let _ = stun::run_listener(sa, 100, 100).await; });
        tokio::spawn(async move { let _ = stun::run_listener(sb, 100, 100).await; });

        let (addr, _state) = signaling::spawn_test_server_with(
            signaling::ws::Keepalive::default(),
            vec![pa.to_string(), pb.to_string()],
        )
        .await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) =
            register_rest(addr, "punch@x.y", &hex::encode(id_a.public_key().as_bytes()), "A")
                .await;
        let (jwt_b, ref_b, dev_b) =
            register_rest(addr, "punch@x.y", &hex::encode(id_b.public_key().as_bytes()), "B")
                .await;

        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
            ca: None,
        };
        sess_b.save(home_b.path()).unwrap();
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv(&sess, &home, &out).await.expect("run_recv")
            })
        };

        // 等 B 上线
        let sess_a = Session {
            server: server.clone(),
            jwt: jwt_a,
            refresh: ref_a,
            device_id: dev_a.clone(),
            ca: None,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "B never online");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        let src = tempfile::tempdir().unwrap();
        let name = "打洞 成功 🕳.bin";
        let fpath = src.path().join(name);
        // 3 MiB + 尾块(CHUNK_SIZE = 1 MiB):4 块,真正跨块经打洞路径传输
        let mut data = vec![0u8; 3 * 1024 * 1024 + 777];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        mtdss_cli::flow::run_send(&sess_a, home_a.path(), &fpath, &dev_b, &|_| {})
            .await
            .expect("run_send");
        let landed = tokio::time::timeout(std::time::Duration::from_secs(15), b_task)
            .await
            .expect("recv timeout")
            .unwrap();
        assert_eq!(
            landed.file_name().and_then(|n| n.to_str()),
            Some(name),
            "file name must be preserved end to end"
        );
        assert_eq!(std::fs::read(&landed).unwrap(), data);
    });
    run.await.expect("stun punch e2e exceeded 90s");
}
