#![cfg(windows)]
//! daemon 更新核心 e2e(M4b Task 11/12):状态机/检查/下载/验签/空闲门/
//! 安装契约(标记/installing 序列/注入桩/开机消费)。
//!
//! 全部场景经显式注入的 `UpdateConfig`(base_url = 本地 axum 测试服,
//! pubkey = 测试生成钥)驱动真 daemon,**零 env 依赖、零真实端点接触**。
//! 测试服路由:`/manifest.json`(force 检查)、`/win/{ver}/manifest.json`
//! (钉扎)、`/asset/{name}`(假 setup 字节,可配置慢速分块流)。
//!
//! 场景矩阵(Task 11 简报钉死 ①-⑦;Task 12 增 ⑧-⑪):
//! ① happy:签好 manifest → check available → apply → installing(经
//!    Installing 序列)+ 文件 sha 复核 + 标记/桩;
//! ② 坏签名(换钥签)→ check Err、state=failed、无下载;
//! ③ sha 不符 → apply 在复核处 Err、临时文件已删;
//! ④ 公钥 None → check Err「更新未配置」,状态不翻(拒绝类不落 failed);
//! ⑤ 空闲门:paused 作业 → apply Err kind=JOBS_ACTIVE 文案精确 → 作业终态后重试过门;
//! ⑥ 降级:apply(Some("0.9.0")) 走钉扎 URL 且被接受(M4 无防回滚);
//! ⑦ 进度事件节流:相邻 progress 帧间距 ≥300ms(400ms 口径 + 接收容差),
//!    机制不等式断言(慢速分块流下逐 chunk 发帧必然违反),不断言绝对时长;
//! ⑧ Installing 序列:ready→installing 事件链、标记先于事件落盘、客户端
//!    归零后注入桩才被调用且收到已验签 setup 路径;
//! ⑨ 开机消费:等版本标记(= cfg.current)启动即清除(「已更新到 X」);
//! ⑩ 开机消费:不等版本标记(≠ cfg.current)同样清除(未完成告警路径);
//! ⑪ 开机消费:restart-ui 标记启动即清除(主窗唤回;MTDSS_UI_BIN 指向
//!    不存在路径,唤起失败仅留痕——真窗面由 Task 15 冒烟承担)。
//! ⑫ 钉扎版本不符:钉扎位放着**别的版本**的合法签名清单 → apply(Some(v))
//!    Err「钉扎清单版本不符」、落 failed、零下载(不得静默装错版)。

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ed25519_dalek::SigningKey;
use mtdss_core::update::{asset_sha256_hex, sign_manifest, PlatformAsset, UpdateManifest};
use mtdss_daemon::update::UpdateConfig;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{restart_ui_marker_path, update_marker_path, IpcClient, RpcError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
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
    /// 注入 spawn 桩收到的安装器路径(Task 12;各 rig 独立记录)。
    spawned: Arc<Mutex<Vec<PathBuf>>>,
}

/// 注入 UpdateConfig 启动真 daemon(零 env),自动注入 spawn 桩(记录路径
/// ——生产桩位是 detached Command,测试恒注入,绝不真拉安装器)。
async fn rig_with(cfg: UpdateConfig) -> Rig {
    rig_with_in(tempfile::tempdir().unwrap(), cfg).await
}

/// 指定 home 的装配(开机标记消费测试需要预置标记现场的 home)。
async fn rig_with_in(home: tempfile::TempDir, mut cfg: UpdateConfig) -> Rig {
    let spawned = Arc::new(Mutex::new(Vec::new()));
    let rec = spawned.clone();
    cfg.spawner = Some(Arc::new(move |p: PathBuf| {
        rec.lock().unwrap().push(p);
    }));
    let daemon = Daemon::start_with_update(
        home.path(),
        DaemonOpts { recv_dir: home.path().join("inbox"), tray: false, authorized_roots: None },
        cfg,
    )
    .await
    .unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    Rig { _home: home, daemon, client, spawned }
}

fn cfg_for(addr: SocketAddr, pubkey: Option<ed25519_dalek::VerifyingKey>) -> UpdateConfig {
    UpdateConfig {
        base_url: format!("http://{addr}"),
        ca: None,
        pubkey,
        current: "1.0.0".to_string(),
        spawner: None,
    }
}

/// 离线配置(开机消费测试用):pubkey None → check/apply 一律「未配置」
/// 拒绝且后台循环静默,base_url 不被触达(端口 9 = discard,即便误连也
/// 立即拒绝)。开机消费不依赖更新配置,只消费标记。
fn offline_cfg() -> UpdateConfig {
    UpdateConfig {
        base_url: "http://127.0.0.1:9".into(),
        ca: None,
        pubkey: None,
        current: "1.0.0".to_string(),
        spawner: None,
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

// ---------- 场景①:happy — check available → apply → Installing + sha 复核 ----------

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
        let home = rig._home.path().to_path_buf();

        let r = call(&rig.client, "update.check", json!({})).await.unwrap();
        assert_eq!(r["state"], "available", "check 结果: {r}");
        assert_eq!(r["latest"], "1.0.1", "{r}");
        assert_eq!(r["current"], "1.0.0", "{r}");
        assert!(r["notes"].is_string(), "{r}");
        assert!(r["progress"].is_null(), "{r}");

        // 调用方保持连接(生产同形:UI 不退出)→ 序列等待归零 5s 超时后照常
        // 继续(超时不候):本测断言的是终态与落盘契约,不是等待机制(⑧)。
        let r = call(&rig.client, "update.apply", json!({})).await.unwrap();
        assert_eq!(r["state"], "installing", "apply 结果: {r}");
        assert_eq!(r["latest"], "1.0.1", "{r}");

        // 下载落盘:sha 复核过的 setup 文件;临时名不残留
        let final_path = home.join("downloads").join("mtdss-setup-1.0.1.exe");
        let got = std::fs::read(&final_path).expect("下载文件必须存在");
        assert_eq!(asset_sha256_hex(&got), asset_sha256_hex(&bytes));
        assert!(!home.join("downloads").join("mtdss-setup-1.0.1.exe.part").exists());

        // Task 12 安装契约:update-in-progress 标记 = 目标版本(原子落盘),
        // 注入桩恰好收到一次已验签 setup 路径
        assert_eq!(
            std::fs::read_to_string(update_marker_path(&home)).unwrap(),
            "1.0.1",
            "标记内容必须是目标版本"
        );
        let spawned = rig.spawned.lock().unwrap().clone();
        assert_eq!(spawned.len(), 1, "桩必须恰好收到一次 spawn: {spawned:?}");
        assert_eq!(spawned[0], final_path, "桩收到的必须是已验签 setup 路径");

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
        assert_eq!(r["state"], "installing", "重试过门 → Installing 序列: {r}");
        let final_path = rig._home.path().join("downloads").join("mtdss-setup-1.0.1.exe");
        assert_eq!(std::fs::read(&final_path).unwrap(), bytes);
        // 过门后走完整安装契约:标记 + 桩
        assert_eq!(
            std::fs::read_to_string(update_marker_path(rig._home.path())).unwrap(),
            "1.0.1"
        );
        assert_eq!(rig.spawned.lock().unwrap().len(), 1);

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
        assert_eq!(r["state"], "installing", "M4 无防回滚,降级被接受: {r}");
        assert_eq!(r["latest"], "0.9.0", "{r}");
        let hits = *srv.pinned_hits.lock().unwrap().get("0.9.0").unwrap_or(&0);
        assert!(hits >= 1, "apply(version) 必须走 /win/0.9.0/manifest.json");
        let final_path = rig._home.path().join("downloads").join("mtdss-setup-0.9.0.exe");
        assert_eq!(std::fs::read(&final_path).unwrap(), bytes);
        // 降级同样走完整安装契约:标记 = 显式 version,桩收到对应 setup
        assert_eq!(
            std::fs::read_to_string(update_marker_path(rig._home.path())).unwrap(),
            "0.9.0"
        );
        assert_eq!(rig.spawned.lock().unwrap().last(), Some(&final_path));

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
        assert_eq!(r["state"], "installing", "{r}");
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

// ---------- 场景⑧:Installing 序列 — ready→installing 事件链 + 归零后 spawn ----------

/// 生产同形:UI 经 RPC 发起 apply,收到 installing 事件后自行退出(掉连接)
/// → 序列等 connected_clients 归零后继续,注入桩才被调用。断言四件事:
/// ready 先于 installing 的事件链;标记先于事件落盘(事件可见时标记必在);
/// 桩收到的恰为已验签 setup 路径;收尾状态停在 installing。
#[tokio::test(flavor = "multi_thread")]
async fn install_sequence_ready_then_installing_and_stub_after_clients_gone() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let bytes = vec![0x33u8; 2048];
        put_force(
            &srv,
            signed_manifest(&sk, "1.0.1", "sinst", addr, &bytes, None),
            "sinst",
            bytes,
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;
        let home = rig._home.path().to_path_buf();

        call(&rig.client, "update.check", json!({})).await.unwrap();

        // 订阅器直连 daemon.events(不经 IPC,不占连接计数),先于 apply 挂上
        let mut rx = rig.daemon.events.subscribe();
        let client = rig.client;
        let applier = tokio::spawn(async move {
            // UI 同形掉线后响应大概率收不到:结果不参与断言
            let _ = client.call("update.apply", json!({})).await;
        });

        let mut saw_ready = false;
        let mut saw_installing = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !saw_installing {
            match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Ok(Ok(f)) if f.method == "event.update" => {
                    match f.params["state"].as_str() {
                        Some("ready") => saw_ready = true,
                        Some("installing") => saw_installing = true,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        assert!(saw_ready, "下载复核完成必须先翻转 ready");
        assert!(saw_installing, "必须广播 installing 事件");

        // 序列内标记(③)先于事件(④):事件可见时标记必已在盘
        let marker = update_marker_path(&home);
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "1.0.1",
            "installing 事件可见时标记必须已落盘"
        );

        // UI 同形退出:掉连接。 apply 自身所在的读半工正在分发中,连接死亡
        // 由「下一帧事件写失败」暴露——补一帧广播即写半工收尾、计数归零
        // (生产同机制:任何后续事件都会冲走死连接)。
        applier.abort();
        let _ = rig.daemon.events.send(mtdss_ipc::EventFrame {
            method: "tick".into(),
            params: json!({}),
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.spawned.lock().unwrap().is_empty() {
            assert!(
                Instant::now() < deadline,
                "客户端归零后注入桩必须被调用(安装序列不得停在等待)"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            rig.spawned.lock().unwrap()[0],
            home.join("downloads").join("mtdss-setup-1.0.1.exe"),
            "桩收到的必须是已验签 setup 路径"
        );

        // 收尾状态:installing(换新连接查询;apply 调用方的连接已掉)
        let fresh = IpcClient::connect(&home).await.unwrap();
        let st = call(&fresh, "update.state", json!({})).await.unwrap();
        assert_eq!(st["state"], "installing", "{st}");

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 8 exceeded 60s");
}

// ---------- 场景⑨/⑩:开机消费 update-in-progress(等版本 / 不等版本) ----------

/// 等版本标记(= 运行中 daemon 版本):启动即清除(「已更新到 X」通知路径,
/// 通知本身不可观测,消费以标记清除为准)。
#[tokio::test(flavor = "multi_thread")]
async fn boot_consumes_equal_version_marker() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(update_marker_path(home.path()), "1.0.0").unwrap();
    let rig = rig_with_in(home, offline_cfg()).await;
    assert!(
        !update_marker_path(rig._home.path()).exists(),
        "等版本标记必须在开机时被消费清除"
    );
    rig.daemon.shutdown().await.unwrap();
}

/// 不等版本标记(上次更新未完成):同样清除(告警路径),不拦启动。
#[tokio::test(flavor = "multi_thread")]
async fn boot_consumes_mismatched_version_marker() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(update_marker_path(home.path()), "9.9.9").unwrap();
    let rig = rig_with_in(home, offline_cfg()).await;
    assert!(
        !update_marker_path(rig._home.path()).exists(),
        "不等版本标记必须在开机时被消费清除"
    );
    rig.daemon.shutdown().await.unwrap();
}

// ---------- 场景⑪:开机消费 restart-ui 标记(主窗唤回) ----------

/// restart-ui 标记存在 → 启动即清除 + 唤起主窗。MTDSS_UI_BIN 指向不存在
/// 路径:唤起层 spawn 失败仅 warn(既有口径),测试只钉标记消费;真窗面
/// 由 Task 15 冒烟承担。env 是进程级全局:持锁串行 + 现场还原。
#[tokio::test(flavor = "multi_thread")]
async fn boot_consumes_restart_ui_marker() {
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _g = ENV_LOCK.lock().await;
    let home = tempfile::tempdir().unwrap();
    std::fs::write(restart_ui_marker_path(home.path()), "").unwrap();
    let saved = std::env::var("MTDSS_UI_BIN").ok();
    std::env::set_var("MTDSS_UI_BIN", home.path().join("no-such-ui.exe"));

    let rig = rig_with_in(home, offline_cfg()).await;

    match saved {
        Some(v) => std::env::set_var("MTDSS_UI_BIN", v),
        None => std::env::remove_var("MTDSS_UI_BIN"),
    }
    assert!(
        !restart_ui_marker_path(rig._home.path()).exists(),
        "restart-ui 标记必须在开机时被消费清除"
    );
    rig.daemon.shutdown().await.unwrap();
}

// ---------- 场景⑫:钉扎清单版本不符 — apply(version) Err、failed、零下载 ----------

/// 钉扎位被塞了**别的版本**的清单(签名合法、版本形态合法,唯独 version ≠
/// 请求):不得静默安装别的版本而 UI/CLI 仍打印请求版本——版本一致校验按
/// 操作失败拒绝(落 failed),且零下载。
#[tokio::test(flavor = "multi_thread")]
async fn pinned_manifest_version_mismatch_rejected_without_download() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (srv, addr) = spawn_update_server().await;
        let sk = SigningKey::from_bytes(&[7; 32]);
        let bytes = vec![0x44u8; 1024];
        // 钉扎位 win/1.0.2/ 放 9.9.9 的清单(合法签名,但版本 ≠ 请求的 1.0.2)
        put_pinned(
            &srv,
            "1.0.2",
            signed_manifest(&sk, "9.9.9", "smix", addr, &bytes, None),
            "smix",
            bytes,
        );
        let rig = rig_with(cfg_for(addr, Some(sk.verifying_key()))).await;

        let err = call(
            &rig.client,
            "update.apply",
            json!({ "version": "1.0.2" }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, 1, "版本不符是业务错(操作失败): {err:?}");
        assert!(
            err.message.contains("钉扎清单版本不符")
                && err.message.contains("1.0.2")
                && err.message.contains("9.9.9"),
            "文案须点明请求与清单两个版本: {}",
            err.message
        );
        let st = update_state(&rig.client).await;
        assert_eq!(st["state"], "failed", "操作失败必须落 failed: {st}");
        // 钉扎位确被命中(拒绝发生在验签之后的本地比对,不是网络前短路)
        let hits = *srv.pinned_hits.lock().unwrap().get("1.0.2").unwrap_or(&0);
        assert!(hits >= 1, "必须真拉过 /win/1.0.2/manifest.json: {hits}");
        // 零下载:downloads 目录不存在或为空(9.9.9 的资产未被触达)
        let dl = rig._home.path().join("downloads");
        let leftovers: Vec<_> = std::fs::read_dir(&dl)
            .map(|d| d.flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "拒绝路径不得有下载残留: {leftovers:?}");

        rig.daemon.shutdown().await.unwrap();
    });
    run.await.expect("scenario 12 exceeded 60s");
}
