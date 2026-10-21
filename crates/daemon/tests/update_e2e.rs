#![cfg(windows)]
//! daemon 更新核心 e2e(M4b Task 11):状态机/检查/下载/验签/空闲门。
//!
//! 全部场景经显式注入的 `UpdateConfig`(base_url = 本地 axum 测试服,
//! pubkey = 测试生成钥)驱动真 daemon,**零 env 依赖、零真实端点接触**。
//! 测试服路由:`/manifest.json`(force 检查)、`/win/{ver}/manifest.json`
//! (钉扎)、`/asset/{name}`(假 setup 字节,可配置慢速分块流)。
//!
//! 七场景矩阵(任务简报钉死):
//! ① happy:签好 manifest → check available → apply → ready + 文件 sha 复核;
//! ② 坏签名(换钥签)→ check Err、state=failed、无下载;
//! ③ sha 不符 → apply 在复核处 Err、临时文件已删;
//! ④ 公钥 None → check Err「更新未配置」,状态不翻(拒绝类不落 failed);
//! ⑤ 空闲门:paused 作业 → apply Err kind=JOBS_ACTIVE 文案精确 → 作业终态后重试过门;
//! ⑥ 降级:apply(Some("0.9.0")) 走钉扎 URL 且被接受(M4 无防回滚);
//! ⑦ 进度事件节流:相邻 progress 帧间距 ≥300ms(400ms 口径 + 接收容差),
//!    机制不等式断言(慢速分块流下逐 chunk 发帧必然违反),不断言绝对时长。

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ed25519_dalek::SigningKey;
use mtdss_core::update::{asset_sha256_hex, sign_manifest, PlatformAsset, UpdateManifest};
use mtdss_daemon::update::UpdateConfig;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 常规 RPC 调用上界(IpcClient::call 无内建超时)。
const CALL: Duration = Duration::from_secs(15);
/// 场景⑤暂停作业的体量:保证即便暂停竞速落败,复制也不可能瞬间完成。
const BIG_FILE: usize = 64 * 1024 * 1024;

/// 清单平台键(core::update 的键形态;M4 仅 windows-x86_64)。
const PLATFORM: &str = "windows-x86_64";

// ---------- 测试更新服务器(全内存,无文件系统) ----------

#[derive(Default)]
struct ServerState {
    /// force 检查(`{base}/manifest.json`)的清单;None → 404。
    force: Mutex<Option<Value>>,
    /// 钉扎(`{base}/win/{ver}/manifest.json`)的清单表。
    pinned: Mutex<HashMap<String, Value>>,
    /// 钉扎路由命中计数(场景⑥断言 apply(version) 走钉扎 URL)。
    pinned_hits: Mutex<HashMap<String, usize>>,
    /// 假 setup 字节表;delay = Some 时按 chunk 分块慢速流出(场景⑦)。
    assets: Mutex<HashMap<String, Asset>>,
}

#[derive(Clone)]
struct Asset {
    bytes: Arc<Vec<u8>>,
    chunk: usize,
    delay: Option<Duration>,
}

async fn spawn_update_server() -> (Arc<ServerState>, SocketAddr) {
    let st = Arc::new(ServerState::default());
    let app = Router::new()
        .route("/manifest.json", get(serve_force))
        .route("/win/{ver}/manifest.json", get(serve_pinned))
        .route("/asset/{name}", get(serve_asset))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (st, addr)
}

async fn serve_force(State(st): State<Arc<ServerState>>) -> Response {
    match st.force.lock().unwrap().clone() {
        Some(m) => Json(m).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_pinned(State(st): State<Arc<ServerState>>, Path(ver): Path<String>) -> Response {
    *st.pinned_hits.lock().unwrap().entry(ver.clone()).or_insert(0) += 1;
    match st.pinned.lock().unwrap().get(&ver).cloned() {
        Some(m) => Json(m).into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_asset(State(st): State<Arc<ServerState>>, Path(name): Path<String>) -> Response {
    let asset = st.assets.lock().unwrap().get(&name).cloned();
    let Some(a) = asset else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    match a.delay {
        None => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            a.bytes.to_vec(),
        )
            .into_response(),
        Some(delay) => {
            // 慢速分块流:首块即发,其后每块前睡 delay——字节可用性被拉开,
            // 下载侧逐块收到(超合并也只可能更少,不会更快)。
            let chunks: Vec<axum::body::Bytes> =
                a.bytes.chunks(a.chunk).map(|c| c.to_vec().into()).collect();
            let stream = futures_util::stream::unfold(
                (chunks, 0usize, delay),
                |(chunks, i, delay)| async move {
                    if !chunks.is_empty() && i < chunks.len() {
                        if i > 0 {
                            tokio::time::sleep(delay).await;
                        }
                        Some((
                            Ok::<_, std::io::Error>(chunks[i].clone()),
                            (chunks, i + 1, delay),
                        ))
                    } else {
                        None
                    }
                },
            );
            axum::body::Body::from_stream(stream).into_response()
        }
    }
}

// ---------- 装配助手 ----------

/// 签好名的清单 JSON;`sha_override` 供场景③构造「声明 ≠ 实际」。
fn signed_manifest(
    sk: &SigningKey,
    ver: &str,
    asset_name: &str,
    addr: SocketAddr,
    bytes: &[u8],
    sha_override: Option<String>,
) -> Value {
    let mut platforms = std::collections::BTreeMap::new();
    platforms.insert(
        PLATFORM.to_string(),
        PlatformAsset {
            url: format!("http://{addr}/asset/{asset_name}"),
            sha256: sha_override.unwrap_or_else(|| asset_sha256_hex(bytes)),
            size: bytes.len() as u64,
        },
    );
    let mut m = UpdateManifest {
        version: ver.to_string(),
        notes: format!("{ver} 测试更新"),
        pubdate: "2026-10-22T00:00:00Z".to_string(),
        platforms,
        signature: String::new(),
    };
    sign_manifest(&mut m, sk);
    serde_json::to_value(&m).unwrap()
}

fn asset_entry(bytes: Vec<u8>, chunk: usize, delay: Option<Duration>) -> Asset {
    Asset { bytes: Arc::new(bytes), chunk, delay }
}

/// force 清单 + 假 setup 字节一键上桌。
fn put_force(srv: &ServerState, manifest: Value, asset_name: &str, bytes: Vec<u8>) {
    srv.assets
        .lock()
        .unwrap()
        .insert(asset_name.to_string(), asset_entry(bytes, 64 * 1024, None));
    *srv.force.lock().unwrap() = Some(manifest);
}

/// 钉扎清单 + 假 setup 字节一键上桌。
fn put_pinned(srv: &ServerState, ver: &str, manifest: Value, asset_name: &str, bytes: Vec<u8>) {
    srv.assets
        .lock()
        .unwrap()
        .insert(asset_name.to_string(), asset_entry(bytes, 64 * 1024, None));
    srv.pinned.lock().unwrap().insert(ver.to_string(), manifest);
}

struct Rig {
    _home: tempfile::TempDir,
    daemon: Daemon,
    client: IpcClient,
}

/// 注入 UpdateConfig 启动真 daemon(零 env)。
async fn rig_with(cfg: UpdateConfig) -> Rig {
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start_with_update(
        home.path(),
        DaemonOpts { recv_dir: home.path().join("inbox"), tray: false },
        cfg,
    )
    .await
    .unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    Rig { _home: home, daemon, client }
}

fn cfg_for(addr: SocketAddr, pubkey: Option<ed25519_dalek::VerifyingKey>) -> UpdateConfig {
    UpdateConfig {
        base_url: format!("http://{addr}"),
        ca: None,
        pubkey,
        current: "1.0.0".to_string(),
    }
}

async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    match tokio::time::timeout(CALL, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("RPC {method} 超时({CALL:?})"),
    }
}

async fn update_state(client: &IpcClient) -> Value {
    call(client, "update.state", json!({})).await.unwrap()
}

// ---------- 场景①:happy — check available → apply → ready + sha 复核 ----------

#[tokio::test(flavor = "multi_thread")]
async fn happy_check_apply_ready_and_file_verified() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        put_force(
            &srv,
            signed_manifest(&sk, "1.0.1", "s101", addr, &bytes, None),
            "s101",
            bytes.clone(),
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        let r = call(&rig.client, "update.check", json!({})).await.unwrap();
        assert_eq!(r["state"], "available", "check 结果: {r}");
        assert_eq!(r["latest"], "1.0.1", "{r}");
        assert_eq!(r["current"], "1.0.0", "{r}");
        assert!(r["notes"].is_string(), "{r}");
        assert!(r["progress"].is_null(), "{r}");

        let r = call(&rig.client, "update.apply", json!({})).await.unwrap();
        assert_eq!(r["state"], "ready", "apply 结果: {r}");
        assert_eq!(r["latest"], "1.0.1", "{r}");

        // 下载落盘:sha 复核过的 setup 文件;临时名不残留
        let final_path = rig._home.path().join("downloads").join("mtdss-setup-1.0.1.exe");
        let got = std::fs::read(&final_path).expect("下载文件必须存在");
        assert_eq!(asset_sha256_hex(&got), asset_sha256_hex(&bytes));
        assert!(!rig
            ._home
            .path()
            .join("downloads")
            .join("mtdss-setup-1.0.1.exe.part")
            .exists());

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 1 exceeded 60s");
}

// ---------- 场景②:坏签名 — check Err、failed、无下载 ----------

#[tokio::test(flavor = "multi_thread")]
async fn bad_signature_fails_check_without_download() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let wrong = SigningKey::from_bytes(&[9; 32]); // 换钥签
        let bytes = vec![1u8; 1024];
        put_force(
            &srv,
            signed_manifest(&wrong, "1.0.1", "sbad", addr, &bytes, None),
            "sbad",
            bytes,
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        let err = call(&rig.client, "update.check", json!({})).await.unwrap_err();
        assert_eq!(err.code, 1, "验签失败是业务错: {err:?}");
        let st = update_state(&rig.client).await;
        assert_eq!(st["state"], "failed", "Err 必落 failed: {st}");
        assert!(
            st["error"].as_str().unwrap_or_default().contains("验签"),
            "error 须可辨: {st}"
        );
        // 系统零改动:下载面无任何残留
        let dl = rig._home.path().join("downloads");
        let leftovers: Vec<_> = std::fs::read_dir(&dl)
            .map(|d| d.flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "失败路径不得有下载残留: {leftovers:?}");

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 2 exceeded 60s");
}

// ---------- 场景③:sha 不符 — apply 复核 Err、临时文件已删 ----------

#[tokio::test(flavor = "multi_thread")]
async fn sha_mismatch_fails_apply_and_deletes_tmp() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let actual = vec![2u8; 2048];
        let declared = vec![3u8; 2048]; // 清单声明与实际不符
        put_force(
            &srv,
            signed_manifest(
                &sk,
                "1.0.1",
                "ssha",
                addr,
                &actual,
                Some(asset_sha256_hex(&declared)),
            ),
            "ssha",
            actual,
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        call(&rig.client, "update.check", json!({})).await.unwrap();
        let err = call(&rig.client, "update.apply", json!({})).await.unwrap_err();
        assert_eq!(err.code, 1, "复核失败是业务错: {err:?}");
        let st = update_state(&rig.client).await;
        assert_eq!(st["state"], "failed", "{st}");
        // 临时文件已删:downloads 目录必须为空(不存在或无条目)
        let dl = rig._home.path().join("downloads");
        let leftovers: Vec<_> = std::fs::read_dir(&dl)
            .map(|d| d.flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "复核失败后 downloads 必须为空: {leftovers:?}");

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 3 exceeded 60s");
}

// ---------- 场景④:公钥 None — check Err「更新未配置」,状态不翻 ----------

#[tokio::test(flavor = "multi_thread")]
async fn missing_pubkey_fails_closed() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (_srv, addr) = spawn_update_server().await;
        let rig = rig_with(cfg_for(addr, None)).await;

        let err = call(&rig.client, "update.check", json!({})).await.unwrap_err();
        assert_eq!(err.code, 1);
        assert!(
            err.message.contains("更新未配置"),
            "文案: {}",
            err.message
        );
        // 拒绝类错误(未配置)不落 failed:状态保持 idle;后台循环在未配置
        // 下也不查不装(否则 .invalid/无钥构建每次启动都翻 failed)。
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st = update_state(&rig.client).await;
        assert_eq!(st["state"], "idle", "未配置不得翻 failed: {st}");

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 4 exceeded 60s");
}

// ---------- 场景⑤:空闲门 — JOBS_ACTIVE 精确文案 → 作业终态后重试过门 ----------

#[tokio::test(flavor = "multi_thread")]
async fn idle_gate_blocks_then_job_terminal_retries_pass() {
    let run = tokio::time::timeout(Duration::from_secs(90), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let bytes = vec![0x5Au8; 4096];
        put_force(
            &srv,
            signed_manifest(&sk, "1.0.1", "sgate", addr, &bytes, None),
            "sgate",
            bytes.clone(),
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        call(&rig.client, "update.check", json!({})).await.unwrap();

        // 预置一个 paused 作业(纯本地复制不要求登录):大文件 + 立即暂停。
        // 暂停 RPC 返回即门已置、可见状态已翻 paused;64MiB 复制不可能已完。
        let payload = tempfile::tempdir().unwrap();
        let src = payload.path().join("big.bin");
        let dst = payload.path().join("dst");
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(&src, vec![0u8; BIG_FILE]).unwrap();
        let job = call(
            &rig.client,
            "transfer.start",
            json!({
                "kind": "local_copy",
                "paths": [src.to_string_lossy()],
                "dir": dst.to_string_lossy(),
            }),
        )
        .await
        .unwrap();
        let id = job["job_id"].as_str().unwrap().to_string();
        call(&rig.client, "transfer.pause", json!({ "id": id })).await.unwrap();
        let jobs = call(&rig.client, "transfer.list", json!({})).await.unwrap();
        let js = jobs["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .unwrap();
        assert_eq!(js["state"], "paused", "作业须呈现暂停: {js}");

        // 空闲门:apply 被拒 → code 1 + data.kind=JOBS_ACTIVE + 文案精确
        let err = call(&rig.client, "update.apply", json!({})).await.unwrap_err();
        assert_eq!(err.code, 1, "{err:?}");
        assert_eq!(err.kind(), Some("JOBS_ACTIVE"), "类型化判定面: {err:?}");
        assert_eq!(
            err.message, "1 个传输中(含已暂停),可先取消作业后重试",
            "文案精确: {}",
            err.message
        );
        assert_eq!(
            update_state(&rig.client).await["state"],
            "pending_install",
            "门内拒绝转 pending_install"
        );

        // 作业转终态(resume → 复制完成 done)后重试过门
        call(&rig.client, "transfer.resume", json!({ "id": id })).await.unwrap();
        let fin = call(&rig.client, "transfer.wait", json!({ "id": id })).await.unwrap();
        assert_eq!(fin["state"], "done", "作业终态: {fin}");

        let r = call(&rig.client, "update.apply", json!({})).await.unwrap();
        assert_eq!(r["state"], "ready", "重试过门 → ready: {r}");
        let final_path = rig._home.path().join("downloads").join("mtdss-setup-1.0.1.exe");
        assert_eq!(std::fs::read(&final_path).unwrap(), bytes);

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 5 exceeded 90s");
}

// ---------- 场景⑥:降级 — apply(version) 走钉扎 URL 且被接受 ----------

#[tokio::test(flavor = "multi_thread")]
async fn downgrade_version_via_pinned_url_accepted() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let bytes = vec![0x11u8; 1024];
        put_pinned(
            &srv,
            "0.9.0",
            signed_manifest(&sk, "0.9.0", "s090", addr, &bytes, None),
            "s090",
            bytes.clone(),
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        let r = call(
            &rig.client,
            "update.apply",
            json!({ "version": "0.9.0" }),
        )
        .await
        .unwrap();
        assert_eq!(r["state"], "ready", "M4 无防回滚,降级被接受: {r}");
        assert_eq!(r["latest"], "0.9.0", "{r}");
        let hits = *srv.pinned_hits.lock().unwrap().get("0.9.0").unwrap_or(&0);
        assert!(hits >= 1, "apply(version) 必须走 /win/0.9.0/manifest.json");
        let final_path = rig._home.path().join("downloads").join("mtdss-setup-0.9.0.exe");
        assert_eq!(std::fs::read(&final_path).unwrap(), bytes);

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 6 exceeded 60s");
}

// ---------- 场景⑦:进度事件节流 — 机制不等式(相邻帧距),不断言绝对时长 ----------

#[tokio::test(flavor = "multi_thread")]
async fn progress_events_throttled_by_min_gap() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        // 8 chunk × 64KiB、块间 150ms:下载约 1.05s;若无节流,逐 chunk 发帧
        // 的相邻帧距 ≈150ms,必然违反 ≥300ms 断言(节流 400ms + 接收容差)。
        let bytes = vec![0x77u8; 8 * 64 * 1024];
        let manifest = signed_manifest(&sk, "1.0.1", "sslow", addr, &bytes, None);
        srv.assets.lock().unwrap().insert(
            "sslow".to_string(),
            asset_entry(bytes.clone(), 64 * 1024, Some(Duration::from_millis(150))),
        );
        *srv.force.lock().unwrap() = Some(manifest);
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        // 采集器:先订阅再触发下载,progress 帧到达即记时刻;1.5s 静默 =
        // 下载窗口已过(末帧后等满静默窗收尾)。
        let mut rx = rig.daemon.events.subscribe();
        let collector = tokio::spawn(async move {
            let mut stamps = Vec::new();
            while let Ok(Ok(f)) =
                tokio::time::timeout(Duration::from_millis(1500), rx.recv()).await
            {
                if f.method == "event.update" && !f.params["progress"].is_null() {
                    stamps.push(Instant::now());
                }
            }
            stamps
        });

        call(&rig.client, "update.check", json!({})).await.unwrap();
        let r = call(&rig.client, "update.apply", json!({})).await.unwrap();
        assert_eq!(r["state"], "ready", "{r}");
        let stamps = collector.await.unwrap();

        assert!(stamps.len() >= 2, "首帧即发 + 至少一次节流后续: {stamps:?}");
        for w in stamps.windows(2) {
            let gap = w[1] - w[0];
            assert!(
                gap >= Duration::from_millis(300),
                "相邻 progress 帧距 {gap:?} 违反 400ms 节流口径(容差内): {stamps:?}"
            );
        }

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 7 exceeded 60s");
}
