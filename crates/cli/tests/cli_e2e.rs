//! CLI 编排端到端：真实 `run_send`/`run_recv` 走真实信令服务器（WS relay）
//! 与 QUIC 直连，两台"设备"在同进程并发运行，各自使用独立 home 目录
//! （home 显式传参注入，不依赖进程环境变量——同进程并发改 env 会互相踩）。
//! 覆盖：身份生成落盘、REST 注册、session.json、TOFU 固定、presence 等待、
//! ConnectRequest/Accept/Candidates 信令、逐候选连接、分块校验传输、落盘。
//! Task 6 另有真实子进程硬杀的静默断链用例：进程内 drop 必发隐式
//! CONNECTION_CLOSE,模拟不了静默;kill 真子进程(OS 回收 socket、零
//! CLOSE 帧)才能验证注入 idle 的两端显形与跨进程续传。

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
        // 复用 Task 6 的 spawn_mtdss_child:抽干任务自 spawn 即并发运行——
        // 自 ff08a81 起本用例的手持管道要等 send 完成才起抽干,传输期 recv
        // 的 info 日志无人读,写满 OS 管道缓冲(Windows ~64KB / Linux 管道
        // 64KB)会把 recv 冻结在日志写上、收尾轮不退出(本轮加固的头号嫌疑)。
        // 30s 未退出仍杀并转储已捕获输出(ff08a81 诊断在位)。
        let (mut b_child, mut b_pipes) = spawn_mtdss_child(
            home_b.path(),
            &[
                "recv".into(),
                "--out".into(),
                out_b.path().as_os_str().to_owned(),
            ],
            &[],
        );

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

        // B 子进程须在一轮收完后自然退出;抽干任务已自 spawn 起并发运行,
        // 这里只等子进程本身,30s 未退出即杀并转储已捕获输出(诊断 CI 偶发挂起)
        let b_status = match tokio::time::timeout(Duration::from_secs(30), b_child.wait()).await {
            Ok(status) => status.expect("wait mtdss recv"),
            Err(_) => {
                let _ = b_child.start_kill();
                let (b_out, b_err) = b_pipes.snapshot().await;
                panic!(
                    "recv child did not exit in 30s\n\
                     --- captured stdout ({}) ---\n{}\n\
                     --- captured stderr ({}) ---\n{}",
                    b_out.len(),
                    b_out,
                    b_err.len(),
                    b_err
                );
            }
        };
        // 断言文本取共享缓冲:等抽干任务到 EOF(子进程已退出)收尾,保证不缺尾
        let (b_out, b_err) = b_pipes.snapshot_after_exit().await;
        assert!(
            b_status.success(),
            "mtdss recv failed\nstdout: {}\nstderr: {}",
            b_out,
            b_err
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

// ---------- Task 6:真实子进程硬杀的静默断链 e2e ----------
//
// quinn 对 Connection 句柄 drop 会发隐式 CONNECTION_CLOSE——进程内断链
// 模拟不了"静默"。只有对真实子进程 start_kill(Windows TerminateProcess /
// Unix SIGKILL)才会让 OS 直接回收 socket、零 QUIC CLOSE 帧离场,对端
// 只能靠注入 idle 超时(Task 5 的 MTDSS_TEST_* env 面)在限时内显形。

/// 起一个真实 `mtdss` 子进程,注入 `MTDSS_TEST_*` 调优 env(Task 5 面)。
/// 手持管道由后台任务持续抽干(info 日志写满管道会假死),缓冲供诊断
/// 转储;kill_on_drop 保证测试失败也不残留进程。
/// `args` 形如 `["recv", "--out", <out 路径>]`。
fn spawn_mtdss_child(
    home: &std::path::Path,
    args: &[std::ffi::OsString],
    env: &[(&str, &str)],
) -> (tokio::process::Child, ChildPipes) {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mtdss"));
    cmd.env("MTDSS_HOME", home)
        .env("RUST_LOG", "info")
        .args(args)
        // stdin 显式置空:recv/send 不读 stdin(交互输码只在 register 子命令);
        // 置 null 让任何未来引入的 stdin 读取立即 EOF 显形,而非阻塞在
        // 继承的终端/管道上(挂点排查时的保险丝)。
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn mtdss child");
    let mut pipes = ChildPipes {
        out_buf: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        err_buf: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        drains: Vec::new(),
    };
    let stdout = child.stdout.take().expect("child stdout piped");
    let stderr = child.stderr.take().expect("child stderr piped");
    pipes
        .drains
        .push(tokio::spawn(drain_into(stdout, pipes.out_buf.clone())));
    pipes
        .drains
        .push(tokio::spawn(drain_into(stderr, pipes.err_buf.clone())));
    (child, pipes)
}

/// 单条管道的持续抽干:读到 EOF(子进程退出)为止,分片写共享缓冲。
async fn drain_into(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    buf: std::sync::Arc<tokio::sync::Mutex<Vec<u8>>>,
) {
    use tokio::io::AsyncReadExt as _;
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.lock().await.extend_from_slice(&chunk[..n]),
        }
    }
}

/// 真实子进程的 stdout/stderr 捕获:失败文案直接内嵌,CI 挂起时日志指认卡点。
struct ChildPipes {
    out_buf: std::sync::Arc<tokio::sync::Mutex<Vec<u8>>>,
    err_buf: std::sync::Arc<tokio::sync::Mutex<Vec<u8>>>,
    drains: Vec<tokio::task::JoinHandle<()>>,
}

impl ChildPipes {
    /// 即时快照:子进程仍存活时用(可能缺结尾一小段),只作诊断。
    async fn snapshot(&self) -> (String, String) {
        (
            String::from_utf8_lossy(&self.out_buf.lock().await).into_owned(),
            String::from_utf8_lossy(&self.err_buf.lock().await).into_owned(),
        )
    }

    /// 完整快照:子进程已死后调用——等抽干任务收尾(管道 EOF)再取,
    /// 保证断言用文本(如 stderr 里的错误文案)不缺尾。
    async fn snapshot_after_exit(&mut self) -> (String, String) {
        for h in self.drains.drain(..) {
            let _ = h.await;
        }
        self.snapshot().await
    }
}

/// 随机填充并落盘,返回内容副本(收发两端字节一致性断言用)。
fn fill_random_file(path: &std::path::Path, size: usize) -> Vec<u8> {
    use rand::RngCore as _;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(path, &data).unwrap();
    data
}

/// 双端准备:两侧独立 home、真实身份、REST 注册、session 各自落盘。
/// 返回 (home_a, home_b, A 的 session, B 的 device_id);子进程靠
/// MTDSS_HOME 各自读盘,session 对象只在本测试进程用(A 的 presence 轮询)。
async fn two_party_setup(
    addr: std::net::SocketAddr,
    email: &str,
) -> (tempfile::TempDir, tempfile::TempDir, Session, String) {
    let home_a = tempfile::tempdir().unwrap();
    let home_b = tempfile::tempdir().unwrap();
    let id_a = mtdss_cli::flow::load_identity(home_a.path()).unwrap();
    let id_b = mtdss_cli::flow::load_identity(home_b.path()).unwrap();
    let (jwt_a, ref_a, dev_a) =
        register_rest(addr, email, &hex::encode(id_a.public_key().as_bytes()), "A").await;
    let (jwt_b, ref_b, dev_b) =
        register_rest(addr, email, &hex::encode(id_b.public_key().as_bytes()), "B").await;
    assert_ne!(dev_a, dev_b);
    let sess_a = Session {
        server: format!("http://{addr}"),
        jwt: jwt_a,
        refresh: ref_a,
        device_id: dev_a,
        ca: None,
    };
    sess_a.save(home_a.path()).unwrap();
    let sess_b = Session {
        server: format!("http://{addr}"),
        jwt: jwt_b,
        refresh: ref_b,
        device_id: dev_b.clone(),
        ca: None,
    };
    sess_b.save(home_b.path()).unwrap();
    (home_a, home_b, sess_a, dev_b)
}

/// 轮询设备目录直到 `device_id` 在 presence 上线(WS 已注册,可收 ConnectRequest)。
async fn wait_online(sess: &Session, device_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let devices = mtdss_cli::flow::fetch_devices(sess).await.unwrap();
        if devices.iter().any(|d| {
            d["device_id"].as_str() == Some(device_id) && d["online"].as_bool().unwrap_or(false)
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{device_id} never online; devices: {devices:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 轮询收件区(.mtdss)直到传输已实际开动(首个 .part/记录条目出现)。
/// 3ms 轮距。注意:不能等 out 首个终名文件再杀——loopback 上 sender 在
/// "终名可见→杀死"的间隙(~5ms)内就能把剩余 1.25MB 全部推进内核缓冲,
/// 接收端照样从缓冲收完、拿到 Final 干净退出,场景失真(实测必现)。
/// .part 是接收开写的最早信号:此刻动手,硬杀必然落在传输中途。
async fn wait_transfer_started(intake: &std::path::Path, budget: Duration) {
    let gone = Instant::now() + budget;
    loop {
        // 收件区目录随传输才建:尚不存在视作"未开动"继续等
        let started = std::fs::read_dir(intake)
            .map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.path().is_file()))
            .unwrap_or(false);
        if started {
            return;
        }
        assert!(
            Instant::now() < gone,
            "transfer never started (no intake entry) in {}",
            intake.display()
        );
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
}

/// Task 6①:硬杀发送方。OS 回收 socket、零 QUIC CLOSE 帧——接收端只能
/// 靠注入 idle(800ms)显形断连,再靠接收窗口(3000ms)到点退出。断言
/// ≤8s(0.8s idle 显形 + 3s 窗口 + 轮间小歇/调度宽限)内**非零**退出且
/// stderr 点名"接收窗口超时":默认 30s idle / 300s 窗口的旧语义都会撞破
/// 该时间界——时间本身就是检测生效的证明。
#[tokio::test]
async fn silent_sender_death_detected_receiver_window_expires() {
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (addr, _state) = spawn_test_server().await;
        let (home_a, home_b, sess_a, dev_b) = two_party_setup(addr, "sd@x.y").await;
        let out_b = tempfile::tempdir().unwrap();

        // 接收端:IDLE=800ms + 窗口 3000ms
        let recv_args: Vec<std::ffi::OsString> = vec![
            "recv".into(),
            "--out".into(),
            out_b.path().as_os_str().to_owned(),
        ];
        let (mut recv_child, mut recv_pipes) = spawn_mtdss_child(
            home_b.path(),
            &recv_args,
            &[
                ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
                ("MTDSS_TEST_RECV_WINDOW_MS", "3000"),
            ],
        );
        wait_online(&sess_a, &dev_b).await;

        // 发送端 3 文件(尺寸数量级错开):BACKOFF=200ms, MAX_RECONNECTS=2, IDLE=800ms
        let src = tempfile::tempdir().unwrap();
        let files: Vec<(std::path::PathBuf, Vec<u8>)> = [
            ("静杀 甲.bin", 4 * 1024usize),
            ("静杀 乙.bin", 256 * 1024),
            ("静杀 丙.bin", 1024 * 1024),
        ]
        .into_iter()
        .map(|(name, size)| {
            let p = src.path().join(name);
            let data = fill_random_file(&p, size);
            (p, data)
        })
        .collect();
        let mut send_args: Vec<std::ffi::OsString> = vec!["send".into()];
        send_args.extend(files.iter().map(|(p, _)| p.as_os_str().to_owned()));
        send_args.extend(["--to".into(), dev_b.as_str().into()]);
        let (mut send_child, send_pipes) = spawn_mtdss_child(
            home_a.path(),
            &send_args,
            &[
                ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
                ("MTDSS_TEST_BACKOFF_MS", "200"),
                ("MTDSS_TEST_MAX_RECONNECTS", "2"),
            ],
        );

        // 传输开动(收件区出现首个 .part)即刻硬杀发送方:断连无 CLOSE 帧,
        // 只能靠 idle 显形
        wait_transfer_started(&out_b.path().join(".mtdss"), Duration::from_secs(30)).await;
        let t0 = Instant::now();
        send_child.start_kill().expect("kill sender");
        let send_status = send_child.wait().await.expect("reap sender");
        assert!(
            !send_status.success(),
            "killed sender must not exit clean\n--- send stderr ---\n{}",
            send_pipes.snapshot().await.1
        );

        // 接收端须 ≤8s 内非零退出,stderr 点名"接收窗口超时"
        let waited = tokio::time::timeout(Duration::from_secs(8), recv_child.wait()).await;
        let (recv_out, recv_err, recv_success, elapsed) = match waited {
            Ok(s) => {
                let status = s.expect("wait recv child");
                let (o, e) = recv_pipes.snapshot_after_exit().await;
                (o, e, status.success(), t0.elapsed())
            }
            Err(_) => {
                let diag = recv_pipes.snapshot().await;
                panic!(
                    "recv child did not exit within 8s (800ms idle detect + 3s window + grace)\n\
                     --- recv stdout ---\n{}\n--- recv stderr ---\n{}",
                    diag.0, diag.1
                );
            }
        };
        assert!(
            elapsed >= Duration::from_millis(800),
            "receiver surfaced before the injected idle could fire: {elapsed:?}"
        );
        let diag_ctx = format!("--- recv stdout ---\n{recv_out}\n--- recv stderr ---\n{recv_err}");
        assert!(
            !recv_success,
            "window expiry must be a failure exit\n{diag_ctx}"
        );
        assert!(
            recv_err.contains("接收窗口超时"),
            "stderr must name the expired window\n{diag_ctx}"
        );
    });
    run.await.expect("sender-death e2e exceeded 90s");
}

/// Task 6②:硬杀接收方。发送端靠注入 idle(800ms)显形断连,按退避
/// (300ms 基数,MAX_RECONNECTS=8)重连;接收端#1 被杀后同 home/out 重启
/// #2(同 env,首次等待无期限),经 intake 对账(已完 Skip / .part Resume)
/// 跨进程续传。断言:发送端最终 exit 0,3 文件字节逐一一致,接收端#2
/// 干净退出且收件区只剩完成记录(无 .part 残留)。
#[tokio::test]
async fn silent_receiver_death_sender_reconnects_and_resumes() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let (addr, _state) = spawn_test_server().await;
        let (home_a, home_b, sess_a, dev_b) = two_party_setup(addr, "rd@x.y").await;
        let out_b = tempfile::tempdir().unwrap();

        // 接收端#1:IDLE=800ms + 窗口 30000ms(本场景窗口只是同 env 重启
        // 的一致性;真实断链靠硬杀,不靠窗口退出)
        let recv_args: Vec<std::ffi::OsString> = vec![
            "recv".into(),
            "--out".into(),
            out_b.path().as_os_str().to_owned(),
        ];
        let recv_env = [
            ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
            ("MTDSS_TEST_RECV_WINDOW_MS", "30000"),
        ];
        let (mut recv1, recv1_pipes) = spawn_mtdss_child(home_b.path(), &recv_args, &recv_env);
        wait_online(&sess_a, &dev_b).await;

        // 发送端 3 文件:BACKOFF=300ms, MAX_RECONNECTS=8, IDLE=800ms
        let src = tempfile::tempdir().unwrap();
        let files: Vec<(std::path::PathBuf, Vec<u8>)> = [
            ("续传 甲.bin", 4 * 1024usize),
            ("续传 乙.bin", 256 * 1024),
            ("续传 丙.bin", 1024 * 1024),
        ]
        .into_iter()
        .map(|(name, size)| {
            let p = src.path().join(name);
            let data = fill_random_file(&p, size);
            (p, data)
        })
        .collect();
        let mut send_args: Vec<std::ffi::OsString> = vec!["send".into()];
        send_args.extend(files.iter().map(|(p, _)| p.as_os_str().to_owned()));
        send_args.extend(["--to".into(), dev_b.as_str().into()]);
        let (mut send_child, send_pipes) = spawn_mtdss_child(
            home_a.path(),
            &send_args,
            &[
                ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
                ("MTDSS_TEST_BACKOFF_MS", "300"),
                ("MTDSS_TEST_MAX_RECONNECTS", "8"),
            ],
        );

        // 传输开动(收件区出现首个 .part)即刻硬杀接收端#1:杀死必然落在
        // 传输中途,留下 .part/部分位图给 #2 对账(比"首文件已完"更能练
        // 到 Resume 路径)。收割句柄,释放 .part 文件句柄
        wait_transfer_started(&out_b.path().join(".mtdss"), Duration::from_secs(30)).await;
        recv1.start_kill().expect("kill recv#1");
        let recv1_status = recv1.wait().await.expect("reap recv#1");
        assert!(
            !recv1_status.success(),
            "killed recv#1 must not exit clean\n--- recv#1 stderr ---\n{}",
            recv1_pipes.snapshot().await.1
        );

        // 2s 静默:发送端在此期间显形断连并进入退避重连循环
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 同 home/out 重启接收端#2(同 env):首次等待无期限,等重连轮
        let (mut recv2, recv2_pipes) = spawn_mtdss_child(home_b.path(), &recv_args, &recv_env);

        // 发送端最终 exit 0(重连成功 + 续传完成)
        let send_waited = tokio::time::timeout(Duration::from_secs(75), send_child.wait()).await;
        let send_success = match send_waited {
            Ok(s) => s.expect("wait send child").success(),
            Err(_) => {
                let (so, se) = send_pipes.snapshot().await;
                let (_, r1e) = recv1_pipes.snapshot().await;
                let (r2o, r2e) = recv2_pipes.snapshot().await;
                panic!(
                    "sender did not exit within 75s after the resume window\n\
                     --- send stdout ---\n{so}\n--- send stderr ---\n{se}\n\
                     --- recv#1 stderr ---\n{r1e}\n\
                     --- recv#2 stdout ---\n{r2o}\n--- recv#2 stderr ---\n{r2e}"
                );
            }
        };
        let (send_out, send_err) = send_pipes.snapshot().await;
        let send_diag = format!("--- send stdout ---\n{send_out}\n--- send stderr ---\n{send_err}");
        assert!(
            send_success,
            "sender must exit 0 after resume across the receiver restart\n{send_diag}"
        );

        // 接收端#2 一轮收完自然退出(干净)
        let recv2_waited = tokio::time::timeout(Duration::from_secs(30), recv2.wait()).await;
        let recv2_success = match recv2_waited {
            Ok(s) => s.expect("wait recv#2").success(),
            Err(_) => {
                let (r2o, r2e) = recv2_pipes.snapshot().await;
                panic!(
                    "recv#2 did not exit within 30s\n\
                     --- recv#2 stdout ---\n{r2o}\n--- recv#2 stderr ---\n{r2e}"
                );
            }
        };
        let (recv2_out, recv2_err) = recv2_pipes.snapshot().await;
        let recv2_diag =
            format!("--- recv#2 stdout ---\n{recv2_out}\n--- recv#2 stderr ---\n{recv2_err}");
        assert!(recv2_success, "recv#2 must exit clean\n{recv2_diag}");

        // 跨进程续传的字节事实:3 文件逐一与源一致
        for (p, data) in &files {
            let got = out_b.path().join(p.file_name().unwrap());
            assert_eq!(
                std::fs::read(&got).unwrap(),
                *data,
                "content mismatch at {}",
                got.display()
            );
        }
        // 收件区:.mtdss/ 只剩完成记录(续传轮收尾后无 .part 残留)
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
            "one finished record per file expected: {names:?}"
        );
        assert!(
            names.iter().all(|n| n.ends_with(".json")),
            "intake must hold only finished records (no .part): {names:?}"
        );
    });
    run.await.expect("receiver-death resume e2e exceeded 120s");
}
