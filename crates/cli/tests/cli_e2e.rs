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
        mtdss_cli::flow::run_send(
            &sess_a,
            home_a.path(),
            std::slice::from_ref(&fpath),
            &dev_b,
            &|_| {},
        )
        .await
        .expect("run_send failed");
        eprintln!("e2e transfer took {:?}", t0.elapsed());

        // 接收方完成并返回落盘路径；内容字节级一致
        let landed = tokio::time::timeout(Duration::from_secs(15), b_task)
            .await
            .expect("receiver task did not finish in time")
            .unwrap();
        assert_eq!(
            landed[0].file_name().and_then(|n| n.to_str()),
            Some(name),
            "file name must be preserved end to end"
        );
        assert_eq!(
            std::fs::read(&landed[0]).unwrap(),
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
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .post(format!("{base}/auth/verify"))
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
            &server,
            &ca,
            "tls@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "side-A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest_tls(
            &server,
            &ca,
            "tls@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "side-B",
        )
        .await;

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
            assert!(
                std::time::Instant::now() < deadline,
                "receiver never online"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        let src = tempfile::tempdir().unwrap();
        let name = "tls 传输 📦.bin";
        let fpath = src.path().join(name);
        let mut data = vec![0u8; 8192];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        mtdss_cli::flow::run_send(
            &sess_a,
            home_a.path(),
            std::slice::from_ref(&fpath),
            &dev_b,
            &|_| {},
        )
        .await
        .expect("run_send failed");

        let landed = tokio::time::timeout(std::time::Duration::from_secs(15), b_task)
            .await
            .expect("receiver task did not finish in time")
            .unwrap();
        assert_eq!(landed[0].file_name().and_then(|n| n.to_str()), Some(name));
        assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
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
            reqwest::Certificate::from_pem(&std::fs::read(other.path().join("ca.crt")).unwrap())
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
        tokio::spawn(async move {
            let _ = stun::run_listener(sa, 100, 100).await;
        });
        tokio::spawn(async move {
            let _ = stun::run_listener(sb, 100, 100).await;
        });

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
        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "punch@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "punch@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "B",
        )
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
                mtdss_cli::flow::run_recv(&sess, &home, &out)
                    .await
                    .expect("run_recv")
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

        mtdss_cli::flow::run_send(
            &sess_a,
            home_a.path(),
            std::slice::from_ref(&fpath),
            &dev_b,
            &|_| {},
        )
        .await
        .expect("run_send");
        let landed = tokio::time::timeout(std::time::Duration::from_secs(15), b_task)
            .await
            .expect("recv timeout")
            .unwrap();
        assert_eq!(
            landed[0].file_name().and_then(|n| n.to_str()),
            Some(name),
            "file name must be preserved end to end"
        );
        assert_eq!(std::fs::read(&landed[0]).unwrap(), data);
    });
    run.await.expect("stun punch e2e exceeded 90s");
}

/// Task 6:传输中途断连 → 发送方重连续传。B 以 fault 注入在首个文件落盘后
/// 主动切断连接,靠 run_recv_with 的接收窗口重进吃下 A 的每一轮重试,
/// 最终全部落盘、run_send Ok。
#[tokio::test]
async fn reconnect_after_mid_transfer_close_completes() {
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "rc@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "rc@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "B",
        )
        .await;
        let sess_a = Session {
            server: server.clone(),
            jwt: jwt_a,
            refresh: ref_a,
            device_id: dev_a.clone(),
            ca: None,
        };
        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
            ca: None,
        };

        // B:首个文件落盘即被切断(fault 注入),run_recv_with 在接收窗口内
        // 重进等待 A 的重试,直至干净收尾轮(全 SKIP → Final)返回全部落盘路径
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv_with(
                    &sess,
                    &home,
                    &out,
                    &mtdss_cli::flow::RecvOpts {
                        fault_close_after_files: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .expect("run_recv")
            })
        };

        // 等 B 上线
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "B never online");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 3 个文件,尺寸错开:最小者最先落盘(fault 切断点),跨轮续传收敛
        let src = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, size) in [
            ("续传 甲.txt", 4096usize),
            ("续传 乙.bin", 256 * 1024),
            ("续传 丙.bin", 1024 * 1024),
        ] {
            let p = src.path().join(name);
            let mut data = vec![0u8; size];
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut data);
            std::fs::write(&p, &data).unwrap();
            files.push((p, data));
        }
        let paths: Vec<std::path::PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();

        mtdss_cli::flow::run_send_with(
            &sess_a,
            home_a.path(),
            &paths,
            &dev_b,
            &|_| {},
            &mtdss_cli::flow::SendOpts {
                retry: mtdss_cli::flow::RetryTuning {
                    backoff_base: Duration::from_millis(50),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect("run_send must complete after reconnects");

        let landed = tokio::time::timeout(Duration::from_secs(15), b_task)
            .await
            .expect("recv task did not finish in time")
            .unwrap();
        assert_eq!(
            landed.len(),
            3,
            "every file must land exactly once: {landed:?}"
        );
        for (p, data) in &files {
            let name = p.file_name().unwrap();
            let got = landed
                .iter()
                .find(|l| l.file_name() == Some(name))
                .unwrap_or_else(|| panic!("file {name:?} not in landed set"));
            assert_eq!(
                std::fs::read(got).unwrap(),
                *data,
                "bytes must survive reconnects"
            );
        }
    });
    run.await.expect("reconnect e2e exceeded 90s");
}

/// Task 6:重试耗尽后的按文件失败报告——文案必须含用户原始输入路径、
/// 已完成/未完成清单与文件名。B 落下首个文件后被测试 abort 整个 recv
/// 任务(模拟对端彻底消失),A 以 2 次重试 + 300ms 打洞预算耗尽告终。
#[tokio::test]
async fn failure_report_lists_original_paths_after_retries() {
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "fr@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "fr@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "B",
        )
        .await;
        let sess_a = Session {
            server: server.clone(),
            jwt: jwt_a,
            refresh: ref_a,
            device_id: dev_a.clone(),
            ca: None,
        };
        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
            ca: None,
        };

        // B:首个文件落盘即被切断(fault 注入)后停在接收窗口等待
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv_with(
                    &sess,
                    &home,
                    &out,
                    &mtdss_cli::flow::RecvOpts {
                        fault_close_after_files: Some(1),
                        ..Default::default()
                    },
                )
                .await
                .expect("run_recv")
            })
        };

        // 等 B 上线
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "B never online");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 3 个文件,尺寸 4KiB/4MiB/8MiB:最小者先落盘(abort 触发点),
        // 其余两个在切断(fault 路径 µs 级生效)与 abort 之间绝无完成窗口
        let src = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, size) in [
            ("败报 甲.bin", 4 * 1024usize),
            ("败报 乙.bin", 4 * 1024 * 1024),
            ("败报 丙.bin", 8 * 1024 * 1024),
        ] {
            let p = src.path().join(name);
            let mut data = vec![0u8; size];
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut data);
            std::fs::write(&p, &data).unwrap();
            files.push((p, data));
        }
        let paths: Vec<std::path::PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();

        // A 发送与"B 消失"并行:轮询到首个落盘文件即刻 abort 整个 recv 任务
        let send_task = {
            let sess = sess_a.clone();
            let home = home_a.path().to_path_buf();
            let to = dev_b.clone();
            let paths = paths.clone();
            tokio::spawn(async move {
                mtdss_cli::flow::run_send_with(
                    &sess,
                    &home,
                    &paths,
                    &to,
                    &|_| {},
                    &mtdss_cli::flow::SendOpts {
                        retry: mtdss_cli::flow::RetryTuning {
                            max_reconnects: 2,
                            backoff_base: Duration::from_millis(50),
                            ..Default::default()
                        },
                        punch: mtdss_core::transport::punch::PunchConfig {
                            budget: Duration::from_millis(300),
                            ..Default::default()
                        },
                    },
                )
                .await
            })
        };
        let gone = Instant::now() + Duration::from_secs(30);
        loop {
            let any = std::fs::read_dir(out_b.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.path().is_file());
            if any {
                b_task.abort(); // B 上线接受首个请求、落下首个文件后整体消失
                break;
            }
            assert!(Instant::now() < gone, "first file never landed");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let err = tokio::time::timeout(Duration::from_secs(60), send_task)
            .await
            .expect("send task did not finish")
            .unwrap()
            .expect_err("run_send must fail after retries");
        let report = format!("{err:#}");

        // 文案断言:用户原始输入路径、已完成/未完成清单、文件名
        for p in &paths {
            assert!(
                report.contains(&p.display().to_string()),
                "report must name the original input path {:#}: {report}",
                p.display()
            );
        }
        let i_done = report
            .find("已完成")
            .expect("report must have a done section");
        let i_partial = report
            .find("部分传输")
            .expect("report must have a partial section");
        let i_never = report
            .find("未完成")
            .expect("report must have a never section");
        assert!(
            i_done < i_partial && i_partial < i_never,
            "sections must be ordered: {report}"
        );
        // 已完成栏可以为空:切断在 finalize 后立即发生,B 的 FileDone 确认帧
        // 常常来不及抵达 A(字节数已满却未确认 → 归入部分传输,如实呈现)。
        // 结构性断言:任何文件都不得虚报进"已完成"——最大文件必在其后。
        let i_big = report
            .find(&paths[2].display().to_string())
            .expect("big file path in report");
        assert!(
            i_big > i_partial,
            "big file must not be listed as done: {report}"
        );
        // 文件名(独立于路径文本)出现
        assert!(
            report.contains("败报 甲.bin"),
            "report must contain file names: {report}"
        );
        assert!(
            report.contains("败报 丙.bin"),
            "report must contain file names: {report}"
        );
    });
    run.await.expect("failure report e2e exceeded 90s");
}

/// Task 7:CLI 多路径发送端到端。两侧都驱动真实二进制:B 先起
/// `mtdss recv --out`(一轮收完自然退出),A 再以
/// `mtdss send <目录> <单文件> --to <B>` 一次发送目录(含子目录)与单文件。
/// 断言:目录结构还原(dir/a.txt、dir/sub/b.bin)与单文件名、内容字节一致、
/// 收件区 .mtdss/ 存在且仅含完成记录(无 .part 残留)。
#[tokio::test]
async fn cli_send_directory_and_files_multi_path() {
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "mp@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "mp@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "B",
        )
        .await;

        // 子进程靠 MTDSS_HOME 定位数据目录:两侧 session 必须先落盘
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

        // B:真实 `mtdss recv --out` 子进程(kill_on_drop 防测试失败时残留挂死进程)。
        // 手持管道而非 .output():30s 内未退出时,把已捕获的 stdout/stderr
        // (含 tracing 诊断)打进 panic 文案——CI 偶发挂起时日志直接指认卡点,
        // 同时 start_kill 收掉子进程不再泄漏。
        let mut b_child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mtdss"))
            .env("MTDSS_HOME", home_b.path())
            .env("RUST_LOG", "info")
            .kill_on_drop(true)
            .args(["recv", "--out"])
            .arg(out_b.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn mtdss recv");
        let mut b_out_pipe = b_child.stdout.take().unwrap();
        let mut b_err_pipe = b_child.stderr.take().unwrap();
        let mut b_out_buf = Vec::new();
        let mut b_err_buf = Vec::new();

        // 等 B 上线(WS presence 已注册)再发起发送
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "B never online");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 输入:目录(顶层 a.txt + 子目录 sub/b.bin)+ 单文件
        let src = tempfile::tempdir().unwrap();
        let dir = src.path().join("目录");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let fill = |p: &std::path::Path, size: usize| -> Vec<u8> {
            let mut data = vec![0u8; size];
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut data);
            std::fs::write(p, &data).unwrap();
            data
        };
        let data_a = fill(&dir.join("a.txt"), 4096);
        let data_b = fill(&dir.join("sub").join("b.bin"), 128 * 1024);
        let single = src.path().join("多路径 单文件 📄.bin");
        let data_c = fill(&single, 8192);

        // A:真实 `mtdss send <目录> <单文件> --to` 子进程(多位置参数)
        let sent = tokio::process::Command::new(env!("CARGO_BIN_EXE_mtdss"))
            .env("MTDSS_HOME", home_a.path())
            .kill_on_drop(true)
            .args(["send"])
            .arg(&dir)
            .arg(&single)
            .args(["--to", &dev_b])
            .output()
            .await
            .expect("spawn mtdss send");
        assert!(
            sent.status.success(),
            "mtdss send failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&sent.stdout),
            String::from_utf8_lossy(&sent.stderr)
        );

        // B 子进程须在一轮收完后自然退出;管道持续抽干(防写满假死),
        // 30s 未退出即杀并转储捕获输出(诊断 CI 偶发挂起)
        use tokio::io::AsyncReadExt as _;
        let drained = tokio::time::timeout(Duration::from_secs(30), async {
            let _ = tokio::join!(
                b_out_pipe.read_to_end(&mut b_out_buf),
                b_err_pipe.read_to_end(&mut b_err_buf),
            );
        })
        .await;
        if drained.is_err() {
            let _ = b_child.start_kill();
            panic!(
                "recv child did not exit in 30s\n\
                 --- captured stdout ({}) ---\n{}\n\
                 --- captured stderr ({}) ---\n{}",
                b_out_buf.len(),
                String::from_utf8_lossy(&b_out_buf),
                b_err_buf.len(),
                String::from_utf8_lossy(&b_err_buf),
            );
        }
        let b_status = b_child.wait().await.expect("wait mtdss recv");
        assert!(
            b_status.success(),
            "mtdss recv failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&b_out_buf),
            String::from_utf8_lossy(&b_err_buf)
        );

        // 结构还原 + 内容一致:目录内容落在 out/<目录名>/...,单文件在 out/<文件名>
        let restored_dir = out_b.path().join(dir.file_name().unwrap());
        let checks = [
            (restored_dir.join("a.txt"), &data_a),
            (restored_dir.join("sub").join("b.bin"), &data_b),
            (out_b.path().join(single.file_name().unwrap()), &data_c),
        ];
        for (got, data) in checks {
            assert_eq!(
                std::fs::read(&got).unwrap(),
                *data,
                "content mismatch at {}",
                got.display()
            );
        }

        // 收件区:.mtdss/ 存在且仅含完成记录(无 .part/.tmp 残留)
        let intake = out_b.path().join(".mtdss");
        assert!(intake.is_dir(), "intake area must exist under out");
        let mut names: Vec<String> = std::fs::read_dir(&intake)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names.len(),
            3,
            "one record per received file expected: {names:?}"
        );
        assert!(
            names.iter().all(|n| n.ends_with(".json")),
            "intake must hold only finished records (no .part): {names:?}"
        );
    });
    run.await.expect("multi-path e2e exceeded 90s");
}

/// Fix I-1:接收窗口必须在等待中被强制执行。B 在一次切断(fault 钩子)后
/// 重进等待,此后 A 静默放弃(单次尝试);注入 600ms recv_window,
/// run_recv_with 必须在窗口(+宽限)内返回 Err,文案点名"接收窗口超时"
/// 与窗口时长——而不是永远阻塞在 wait_connect_request 上。
#[tokio::test]
async fn recv_window_expires_while_waiting_after_drop() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "iw@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "iw@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "B",
        )
        .await;
        let sess_a = Session {
            server: server.clone(),
            jwt: jwt_a,
            refresh: ref_a,
            device_id: dev_a.clone(),
            ca: None,
        };
        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
            ca: None,
        };

        // B:首文件落盘即被切断 → 起窗重进等待;600ms 窗口必须在等待中到期
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv_with(
                    &sess,
                    &home,
                    &out,
                    &mtdss_cli::flow::RecvOpts {
                        fault_close_after_files: Some(1),
                        retry: mtdss_cli::flow::RetryTuning {
                            recv_window: Duration::from_millis(600),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .await
            })
        };

        // 等 B 上线
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "B never online");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 单文件 + A max_reconnects=0:一次尝试,B 落盘即切断,A 随即放弃
        // → B 侧此后静默,只能靠窗口超时退出
        let src = tempfile::tempdir().unwrap();
        let name = "窗口 甲.bin";
        let fpath = src.path().join(name);
        let mut data = vec![0u8; 64 * 1024];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut data);
        std::fs::write(&fpath, &data).unwrap();

        mtdss_cli::flow::run_send_with(
            &sess_a,
            home_a.path(),
            std::slice::from_ref(&fpath),
            &dev_b,
            &|_| {},
            &mtdss_cli::flow::SendOpts {
                retry: mtdss_cli::flow::RetryTuning {
                    max_reconnects: 0,
                    backoff_base: Duration::from_millis(50),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect_err("single-attempt send must fail after the cut");

        // B 必须在窗口(+轮间小歇与调度宽限)内返回 Err,而不是永久阻塞
        let t0 = Instant::now();
        let b_err = tokio::time::timeout(Duration::from_secs(10), b_task)
            .await
            .expect("run_recv_with must return once the window expires (was blocked waiting)")
            .unwrap()
            .expect_err("expired window must surface as Err");
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "window enforcement too slow: {:?}",
            t0.elapsed()
        );
        let msg = format!("{b_err:#}");
        assert!(
            msg.contains("接收窗口超时"),
            "error must name the expired window: {msg}"
        );
        assert!(
            msg.contains("600ms"),
            "error must carry the window value: {msg}"
        );
        // 切断前落盘的文件仍在盘上(磁盘事实不丢)
        assert_eq!(
            std::fs::read(out_b.path().join(name)).unwrap(),
            data,
            "file landed before the cut must stay on disk"
        );
    });
    run.await.expect("recv window e2e exceeded 60s");
}

/// Fix R2-F2:接收窗口必须按"最近一次断连"重起算。5 文件多轮重连
/// (B 每轮切断一个文件)整段历时远超首个窗口(2s),但每轮 A 的重连
/// 间隔(100/200/400/800/1600ms 退避)都小于窗口——若 deadline 只在
/// 首次断连时设置,最后一轮重连(≈3.3s)会撞上过期窗口,B 提前退出、
/// A 找不到接收方;正确行为:每个新 ConnectRequest 清窗,断连重新
/// 起算,最终全部落盘。
#[tokio::test]
async fn recv_window_rearms_after_each_reconnect() {
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");

        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let out_b = tempfile::tempdir().unwrap();
        let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
        let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
        let (jwt_a, ref_a, dev_a) = register_rest(
            addr,
            "rw@x.y",
            &hex::encode(id_a.public_key().as_bytes()),
            "A",
        )
        .await;
        let (jwt_b, ref_b, dev_b) = register_rest(
            addr,
            "rw@x.y",
            &hex::encode(id_b.public_key().as_bytes()),
            "B",
        )
        .await;
        let sess_a = Session {
            server: server.clone(),
            jwt: jwt_a,
            refresh: ref_a,
            device_id: dev_a.clone(),
            ca: None,
        };
        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
            ca: None,
        };

        // B:每轮首个新 finalize 即切断;2s 窗口 < 整段重连历时(退避累计
        // 3.1s+),但 > 任一单轮重连间隔(最大 1.6s)——窗口必须每轮重
        // 起算才能活着等到收尾轮
        let b_task = {
            let sess = sess_b.clone();
            let home = home_b.path().to_path_buf();
            let out = out_b.path().to_path_buf();
            tokio::spawn(async move {
                mtdss_cli::flow::run_recv_with(
                    &sess,
                    &home,
                    &out,
                    &mtdss_cli::flow::RecvOpts {
                        fault_close_after_files: Some(1),
                        retry: mtdss_cli::flow::RetryTuning {
                            recv_window: Duration::from_millis(2000),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .await
                .expect("run_recv must survive the multi-round reconnect saga")
            })
        };

        // 等 B 上线
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let devices = mtdss_cli::flow::fetch_devices(&sess_a).await.unwrap();
            if devices.iter().any(|d| {
                d["device_id"].as_str() == Some(dev_b.as_str())
                    && d["online"].as_bool().unwrap_or(false)
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "B never online");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 5 个文件,尺寸数量级错开:每轮恰好落地最小剩余者(fault 阈值 1
        // 的切断点),5 轮切断 + 1 轮干净收尾
        let src = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, size) in [
            ("重窗 甲.bin", 4 * 1024usize),
            ("重窗 乙.bin", 256 * 1024),
            ("重窗 丙.bin", 1024 * 1024),
            ("重窗 丁.bin", 2 * 1024 * 1024),
            ("重窗 戊.bin", 4 * 1024 * 1024),
        ] {
            let p = src.path().join(name);
            let mut data = vec![0u8; size];
            use rand::RngCore;
            rand::thread_rng().fill_bytes(&mut data);
            std::fs::write(&p, &data).unwrap();
            files.push((p, data));
        }
        let paths: Vec<std::path::PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();

        mtdss_cli::flow::run_send_with(
            &sess_a,
            home_a.path(),
            &paths,
            &dev_b,
            &|_| {},
            &mtdss_cli::flow::SendOpts {
                retry: mtdss_cli::flow::RetryTuning {
                    backoff_base: Duration::from_millis(100),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect("run_send must complete: window must re-arm per break, not pin to the first");

        let landed = tokio::time::timeout(Duration::from_secs(15), b_task)
            .await
            .expect("recv task did not finish in time")
            .unwrap();
        assert_eq!(
            landed.len(),
            5,
            "every file must land exactly once: {landed:?}"
        );
        for (p, data) in &files {
            let name = p.file_name().unwrap();
            let got = landed
                .iter()
                .find(|l| l.file_name() == Some(name))
                .unwrap_or_else(|| panic!("file {name:?} not in landed set"));
            assert_eq!(std::fs::read(got).unwrap(), *data);
        }
    });
    run.await.expect("window re-arm e2e exceeded 90s");
}
