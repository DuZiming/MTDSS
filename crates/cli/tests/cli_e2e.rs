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
        };
        sess_a.save(home_a.path()).unwrap();
        let sess_b = Session {
            server: server.clone(),
            jwt: jwt_b,
            refresh: ref_b,
            device_id: dev_b.clone(),
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
