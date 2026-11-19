//! 转码控制桥端到端(V4-1 T13):host 双 daemon(A 观看端 / B 源端)+
//! 伪 App 转码应答器(真实 IpcClient 订阅 `fs.rendition_open/cancel` +
//! FdPassClient 回 `0x0C rendition_data`:open/cancel 回执 + 段路径回告,
//! 段文件为伪 App 真实落盘字节)→ B 侧 doc(hidden) 钩子装桥后端
//! (与 Android 生产装配同语;非 Android unix 的模拟形态)→ A 侧
//! `preview_rendition_open/segment/cancel` 逐字节校验。fdpass 依赖 Unix
//! 域套接字,整文件 cfg(unix)(CI 在 WSL 跑;Windows 编译单元整体缺席)。
//!
//! 断言面(brief 测试矩阵:Rust 侧模拟桥 e2e——会话 open → 段文件路径
//! 回告 → 引擎读段字节;cancel/超时回收):
//! - **转码主线**:open 单帧(session/segment_ms/mime)→ 段 0(init)/1
//!   流式逐字节 = 伪 App 落盘字节(「读已产段」);未产段(index 7)→
//!   segment_pending 携「段未就绪」词(T15-M2 等待稳定词,与止步词可
//!   区分;有界等待/503 的 Err 面原料);导出回退不触发(seek fd 先行
//!   证词:serve-tmp 恒空);
//! - **cancel**:空 ok 单帧幂等 → 伪 App 收 fs.rendition_cancel(session
//!   原样回传)→ 清段目录;回收后段读 = NOT_FOUND(会话不存在/已回收);
//! - **App 拒开**(0x0C ok:false)→ 工厂落 None → 稳定词 unsupported
//!   (查看端回退原流+海报;T13 工厂缝无错误通道的钉面);
//! - **App 不应答**(注入短窗)→ 超时 → 同落 unsupported。
//!
//! 旧对端兼容:daemon 与 App 同 APK 同船,「连着但不识 fs.rendition_*」
//! 只出现在测试台——注入缝 `MTDSS_TEST_RENDITION_TIMEOUT_MS` 全程压短。

#![cfg(unix)]

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{
    FdPassClient, KIND_ACK, KIND_RENDITION_DATA, KIND_SEEK_FD,
};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 伪 App 事件等待上界。
const APP_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// open 应答等待窗注入(1s;不应答用例吃这个窗,不真等 10s)。
const INJECTED_RENDITION_TIMEOUT_MS: u64 = 1_000;
static RENDITION_TIMEOUT_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_rendition_timeout() {
    RENDITION_TIMEOUT_INJECT.get_or_init(|| {
        std::env::set_var(
            "MTDSS_TEST_RENDITION_TIMEOUT_MS",
            INJECTED_RENDITION_TIMEOUT_MS.to_string(),
        );
    });
}

/// 用例并发闸(打洞/信令负载;两用例并行足够)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE.clone().acquire_owned().await.expect("semaphore never closed")
}

/// 源端形态(测试不触更新托盘面;photo_serve 关)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        inbox_publish: false,
        photo_serve: false,
        ctrl_tuning: None,
    }
}

async fn call_bounded(
    client: &IpcClient,
    method: &str,
    params: Value,
    t: Duration,
) -> Result<Value, RpcError> {
    match tokio::time::timeout(t, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("call {method} 超时({t:?})"),
    }
}

async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    call_bounded(client, method, params, CALL_TIMEOUT).await
}

async fn ipc_login(client: &IpcClient, server: &str, email: &str, name: &str) -> Value {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("result 必含 code 字符串");
    call(client, "login.verify", json!({ "email": email, "code": code, "name": name }))
        .await
        .expect("login.verify 应成功")
}

/// V3-1 审批状态机:任何上线发生前 REST 预置设备行(与 seek e2e 同款)。
async fn preregister_identity(addr: std::net::SocketAddr, home: &Path, email: &str, name: &str) {
    let id = mtdss_core::identity::DeviceIdentity::generate();
    id.save_seed(&home.join("identity.seed")).unwrap();
    let http = reqwest::Client::new();
    let c: Value = http
        .post(format!("http://{addr}/auth/request-code"))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: Value = http
        .post(format!("http://{addr}/auth/verify"))
        .json(&json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": hex::encode(id.public_key().as_bytes()),
            "name": name,
            "platform": "test"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["status"], "ok", "预置注册必须直入 ok: {v}");
}

async fn wait_online(client: &IpcClient, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = call(client, "status", json!({})).await.unwrap();
        if s["online"].as_bool() == Some(want) {
            return;
        }
        assert!(Instant::now() < deadline, "status.online 未在 {secs}s 内到 {want}: {s}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_peer_online(client: &IpcClient, dev: &str, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let devs = call(client, "devices", json!({})).await.unwrap();
        let hit = devs["devices"]
            .as_array()
            .map(|l| {
                l.iter()
                    .any(|d| d["device_id"].as_str() == Some(dev) && d["online"].as_bool() == Some(want))
            })
            .unwrap_or(false);
        if hit {
            return;
        }
        assert!(Instant::now() < deadline, "devices 快照未在 {secs}s 内看到 {dev} online={want}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 确定性字节图样(伪段内容;逐字节可复算)。
fn pattern_byte(i: u64) -> u8 {
    (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8
}

/// 伪转码产物:段 0 = init 段(定词面),段 n = 图样字节(长度随序可复算)。
fn pseudo_init_bytes() -> Vec<u8> {
    b"REND-E2E-FMP4-INIT".to_vec()
}

fn pseudo_segment_bytes(index: u32) -> Vec<u8> {
    let len = 256 + index as u64 * 64;
    (0..len).map(pattern_byte).collect()
}

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}

// ---------- 测试台(A 观看端 / B 源端)----------

struct Rig {
    _home_a: tempfile::TempDir,
    home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    cb: IpcClient,
    /// 观看端设备 id(对偶面与家族 Rig 同形;本用例直驱即 dev_b 侧,保留
    /// 字段为家族形态一致,读面缺位由下划线前缀声明)。
    _dev_a: String,
    dev_b: String,
    scratch: tempfile::TempDir,
}

impl Rig {
    async fn new(email: &str) -> Self {
        inject_rendition_timeout();
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        preregister_identity(addr, home_a.path(), email, "A").await;
        preregister_identity(addr, home_b.path(), email, "B").await;
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        // 桥后端装配(非 Android unix 的模拟形态;与 Daemon::start 的
        // target_os=android 装配同语单点——doc(hidden) 测试直驱面)
        daemon_b.install_bridge_rendition_factory();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        ipc_login(&ca, &server, email, "A").await;
        ipc_login(&cb, &server, email, "B").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let ra = call(&ca, "status", json!({})).await.unwrap();
        let dev_a = ra["device"]["device_id"].as_str().unwrap().to_string();
        let rb = call(&cb, "status", json!({})).await.unwrap();
        let dev_b = rb["device"]["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&ca, &dev_b, true, 30).await;
        wait_peer_online(&cb, &dev_a, true, 30).await;
        let scratch = tempfile::tempdir().unwrap();
        Self {
            _home_a: home_a,
            home_b,
            _state: state,
            daemon_a,
            daemon_b,
            cb,
            _dev_a: dev_a,
            dev_b,
            scratch,
        }
    }

    /// B 源端 roots.set 注册 bridge 根(收件箱,可见)。
    async fn register_bridge_root(&self) {
        let r = call(
            &self.cb,
            "roots.set",
            json!({
                "roots": [
                    {"name": "收件箱", "path": "/收件箱", "source": "bridge", "visible": true},
                ]
            }),
        )
        .await
        .expect("roots.set 应成功");
        assert_eq!(r, json!({"ok": true}));
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }

    /// A 对 B 的转码会话开启(整体限时;单帧错即 Err 词面)。
    async fn rendition_open(
        &self,
        path: &str,
        size: u64,
    ) -> Result<mtdss_proto::ctrl::RenditionOpenResponse, String> {
        tokio::time::timeout(
            CALL_TIMEOUT,
            self.daemon_a.preview_rendition_open(&self.dev_b, path, 0, size as i64),
        )
        .await
        .expect("rendition_open 整体超时(悬挂面)")
    }

    /// A 对 B 的段拉取:Stream 臂收齐字节;Frame 臂 = 单帧错(code,词)。
    async fn rendition_segment(
        &self,
        session: u64,
        index: u32,
    ) -> Result<Vec<u8>, (String, String)> {
        use mtdss_core::transport::ctrl::CtrlStreamOutcome;
        let outcome = tokio::time::timeout(
            CALL_TIMEOUT,
            self.daemon_a.preview_rendition_segment(&self.dev_b, session, index),
        )
        .await
        .expect("rendition_segment 整体超时(悬挂面)");
        match outcome.expect("preview_rendition_segment 发起应成功") {
            CtrlStreamOutcome::Stream(mut s) => {
                let mut bytes = Vec::new();
                while let Some(item) = s.next_chunk().await {
                    match item {
                        Ok(c) => bytes.extend_from_slice(&c),
                        Err(e) => return Err((e.code, e.message)),
                    }
                }
                Ok(bytes)
            }
            // 单帧错:投影 (code, 词)(ok 面 = 异常形态,panic)
            CtrlStreamOutcome::Frame(r) => {
                assert!(!r.ok, "段拉取单帧 ok 面(用例未预期): {r:?}");
                let (code, message) = match r.body {
                    Some(mtdss_proto::ctrl::ctrl_response::Body::Error(e)) => (e.code, e.message),
                    other => panic!("单帧无错误体: {other:?}"),
                };
                Err((code, message))
            }
        }
    }
}

// ---------- 伪 App(转码桥应答器)----------

/// 转码应答模式(逐用例注入)。
#[derive(Clone, Copy, PartialEq, Eq)]
enum RenditionMode {
    /// 受理:落段真件 → 0x0C 回执 + 段路径回告(0=init,1..=2 内容)。
    Deliver,
    /// 0x0C ok:false 拒开。
    Refuse,
    /// 不应答(吃注入超时)。
    Ignore,
}

/// 伪 App 日志(用例体断言数据源)。
#[derive(Default)]
struct RenditionLog {
    seen_open: Mutex<Vec<(u64, String, u64)>>,   // (request_id, path, start_at)
    seen_cancel: Mutex<Vec<(u64, u64)>>,         // (request_id, session)
    seen_seek: Mutex<Vec<(u64, String)>>,
}

/// 伪 App:订阅 B 的 fs.request_seek / fs.rendition_* 事件 → FdPassClient
/// 回 0x0D/0x0C。事件流关闭(daemon 收摊)即退。段文件 = 真实落盘字节
/// (init + 2 内容段;段 3 起不产 = 「未产段」Err 面)。
struct MockRenditionApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    rendition_dir: PathBuf,
    /// vpath → 真实文件路径(seek fd 开这个)。
    files: BTreeMap<String, PathBuf>,
    mode: Arc<Mutex<RenditionMode>>,
    log: Arc<RenditionLog>,
}

impl MockRenditionApp {
    /// 装配 + 起跑;返回任务句柄与模式切换句柄(逐相位用)。
    async fn spawn(
        home: &Path,
        files: BTreeMap<String, PathBuf>,
        mode: RenditionMode,
        log: Arc<RenditionLog>,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<RenditionMode>>) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        // serve-tmp 目录在场但保持空(用例断言「导出回退未触发」的锚;
        // daemon 侧从不建此目录——空目录由本伪 App 预置)
        std::fs::create_dir_all(home.join("serve-tmp")).unwrap();
        let rendition_dir = home.join("rendition-serve");
        std::fs::create_dir_all(&rendition_dir).unwrap();
        let mode = Arc::new(Mutex::new(mode));
        let app = Self {
            ipc,
            fdpass,
            rendition_dir,
            files,
            mode: mode.clone(),
            log,
        };
        (tokio::spawn(app.run()), mode)
    }

    async fn run(mut self) {
        let mut evs = self.ipc.events();
        loop {
            let ev = match tokio::time::timeout(APP_EVENT_TIMEOUT, evs.recv()).await {
                Ok(Ok(f)) => f,
                Ok(Err(broadcast::error::RecvError::Closed)) => return,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Err(_) => continue,
            };
            match ev.method.as_str() {
                "fs.request_seek" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let path = ev.params["path"].as_str().unwrap().to_string();
                    self.log.seen_seek.lock().unwrap().push((request_id, path.clone()));
                    let Some(file) = self.files.get(&path) else {
                        let ack = self
                            .fdpass
                            .send_msg(
                                KIND_SEEK_FD,
                                json!({"error": "源已不存在", "ok": false, "request_id": request_id}),
                                None,
                            )
                            .await;
                        assert_ack(ack, "0x0D 缺席拒开");
                        continue;
                    };
                    let fd = std::fs::File::open(file).expect("伪 App 开源文件");
                    let ack = self
                        .fdpass
                        .send_msg(KIND_SEEK_FD, json!({"request_id": request_id}), Some(fd.into()))
                        .await;
                    assert_ack(ack, "0x0D 交付");
                }
                "fs.rendition_open" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let path = ev.params["path"].as_str().unwrap().to_string();
                    let start_at = ev.params["start_at"].as_u64().unwrap();
                    self.log
                        .seen_open
                        .lock()
                        .unwrap()
                        .push((request_id, path.clone(), start_at));
                    let mode = *self.mode.lock().unwrap();
                    match mode {
                        RenditionMode::Ignore => continue,
                        RenditionMode::Refuse => {
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_RENDITION_DATA,
                                    json!({"error": "App 拒转码(测试注入)", "ok": false, "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0C 拒开");
                        }
                        RenditionMode::Deliver => {
                            // 回执先行(管线受理),段回告随后(0=init;
                            // 1..=2 内容;3 起不产 = 「未产段」面)
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_RENDITION_DATA,
                                    json!({"ok": true, "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0C 受理");
                            let dir = self.rendition_dir.join(request_id.to_string());
                            let init = (0u32, pseudo_init_bytes());
                            let contents: Vec<(u32, Vec<u8>)> = (1..=2u32)
                                .map(|i| (i, pseudo_segment_bytes(i)))
                                .collect();
                            for (index, bytes) in std::iter::once(init).chain(contents) {
                                let p = dir.join(format!("{index}.m4s"));
                                write_file(&p, &bytes);
                                let ack = self
                                    .fdpass
                                    .send_msg(
                                        KIND_RENDITION_DATA,
                                        json!({
                                            "index": index, "ok": true,
                                            "path": p.to_string_lossy(),
                                            "request_id": request_id,
                                        }),
                                        None,
                                    )
                                    .await;
                                assert_ack(ack, "0x0C 段回告");
                            }
                        }
                    }
                }
                "fs.rendition_cancel" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let session = ev.params["session"].as_u64().unwrap();
                    self.log.seen_cancel.lock().unwrap().push((request_id, session));
                    // 会话段目录清理(真实 App 语义:cancel 即清,残件由
                    // 挂载清扫兜底)
                    let dir = self.rendition_dir.join(session.to_string());
                    if dir.is_dir() {
                        std::fs::remove_dir_all(&dir).unwrap();
                    }
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_RENDITION_DATA,
                            json!({"ok": true, "request_id": request_id}),
                            None,
                        )
                        .await;
                    assert_ack(ack, "0x0C 取消回执");
                }
                _ => {}
            }
        }
    }
}

fn assert_ack(ack: Result<mtdss_ipc::fdpass::FdMsg, anyhow::Error>, what: &str) {
    assert_eq!(
        ack.as_ref().map(|a| a.kind).ok(),
        Some(KIND_ACK),
        "{what} 应受理(ack): {ack:?}"
    );
}

/// 连接 fds.sock(重试兜底启动竞速;上限 5s)。
async fn connect_fdpass_retry(home: &Path) -> FdPassClient {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match FdPassClient::connect(home).await {
            Ok(c) => return c,
            Err(_) => {
                assert!(Instant::now() < deadline, "fds.sock 连接超窗");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

// ---------- 用例 ----------

/// 转码主线:open 单帧 → 段 0(init)/1 流式逐字节 = 伪 App 落盘字节;
/// 未产段(3)→ segment_pending 携「段未就绪」(T15-M2 等待稳定词);cancel
/// → 伪 App 收取消事件 + 清目录;回收后段读 NOT_FOUND;serve-tmp 恒空
/// (seek fd 先行,导出回退未触)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rendition_bridge_open_segment_bytes_and_cancel() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("rendition-main@test").await;
        rig.register_bridge_root().await;
        let file = rig.scratch.path().join("movie.mp4");
        let size: u64 = 200_000;
        let content: Vec<u8> = (0..size).map(pattern_byte).collect();
        write_file(&file, &content);
        let log = Arc::new(RenditionLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/movie.mp4".to_string(), file.clone());
        let (_app, _mode) =
            MockRenditionApp::spawn(rig.home_b.path(), files, RenditionMode::Deliver, log.clone())
                .await;

        // open:单帧 ok(会话元信息齐)
        let open = rig.rendition_open("/收件箱/movie.mp4", size).await.expect("open 应成");
        assert_eq!(open.segment_ms, 4000, "段长 4000ms(常量锚透传)");
        assert_eq!(open.mime, "video/iso.segment", "fMP4 MIME");
        let session = open.session_id;

        // 伪 App 词证:open 事件(vpath + start_at=0 原样)
        let seen = log.seen_open.lock().unwrap().clone();
        assert!(
            seen.iter().any(|(_, p, s)| p == "/收件箱/movie.mp4" && *s == 0),
            "伪 App 应收到 open 事件(全量 vpath + start_at): {seen:?}"
        );

        // 段 0(init)/1:逐字节 = 伪 App 落盘件。回执先行的产段异步窗:
        // 段回告在途(伪 App 真 App 同序:受理即回,逐段落盘回告),段读
        // 对「未就绪」有界重试(T15 跳段等待语义的原料面)
        async fn pull_until_ready(
            rig: &Rig,
            session: u64,
            index: u32,
        ) -> Vec<u8> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match rig.rendition_segment(session, index).await {
                    Ok(bytes) => return bytes,
                    Err((code, msg)) => {
                        assert!(
                            code == "segment_pending" && msg.contains("未就绪"),
                            "重试面仅限未就绪(T15-M2 等待词): {code} {msg}"
                        );
                    }
                }
                assert!(Instant::now() < deadline, "段 {index} 回告未在窗内到达");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        assert_eq!(
            pull_until_ready(&rig, session, 0).await,
            pseudo_init_bytes(),
            "init 段逐字节一致"
        );
        assert_eq!(
            pull_until_ready(&rig, session, 1).await,
            pseudo_segment_bytes(1),
            "内容段 1 逐字节一致"
        );
        assert_eq!(
            pull_until_ready(&rig, session, 2).await,
            pseudo_segment_bytes(2),
            "内容段 2 逐字节一致"
        );

        // 未产段(3 起)→ segment_pending 携「段未就绪」(T15-M2 等待稳定词,
        // 与「段越界(止步)」的 segment_range 不得共词)
        let (code, msg) = rig.rendition_segment(session, 3).await.expect_err("未产段必单帧错");
        assert_eq!(code, "segment_pending", "未产段稳定词: {msg}");
        assert!(msg.contains("未就绪"), "等待词面: {msg}");
        assert!(!msg.contains("越界"), "等待词不得携止步词(不得共词): {msg}");

        // 导出回退未触发(seek fd 先行证词)
        assert!(log.seen_seek.lock().unwrap().iter().any(|(_, p)| p == "/收件箱/movie.mp4"));
        let tmp = rig.home_b.path().join("serve-tmp");
        assert!(
            !tmp.exists() || std::fs::read_dir(&tmp).unwrap().next().is_none(),
            "serve-tmp 必空(源轨道走 seek fd,不走导出回退)"
        );

        // cancel:空 ok 单帧;伪 App 收 fs.rendition_cancel(session 原样)清目录
        rig.daemon_a
            .preview_rendition_cancel(&rig.dev_b, session)
            .await
            .expect("cancel 应成(空 ok)");
        let cancels = log.seen_cancel.lock().unwrap().clone();
        assert!(
            cancels.iter().any(|&(_, s)| s == session),
            "伪 App 应收到取消事件(会话键原样): {cancels:?}"
        );
        assert!(
            !rig.home_b.path().join("rendition-serve").join(session.to_string()).exists(),
            "伪 App cancel 即清段目录"
        );

        // 回收后段读 = NOT_FOUND(会话不存在/已回收,重开面)
        let (code, msg) = rig.rendition_segment(session, 0).await.expect_err("回收后段读必错");
        assert_eq!(code, "NOT_FOUND", "回收稳定词: {msg}");
        assert!(msg.contains("已回收") || msg.contains("不存在"), "词面: {msg}");

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}

/// 拒开/不应答两路(brief 超时回收面):App 拒开(0x0C ok:false)→ 工程
/// 厂落 None → 稳定词 unsupported;App 不应答(注入 1s 窗)→ 超时 → 同落
/// unsupported——查看端单一降级分支(回退原流+海报)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rendition_bridge_refuse_and_silence_fall_back_to_unsupported() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("rendition-fallback@test").await;
        rig.register_bridge_root().await;
        let file = rig.scratch.path().join("movie.mp4");
        write_file(&file, &[0u8; 4096]);
        let log = Arc::new(RenditionLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/movie.mp4".to_string(), file.clone());

        // 1) 拒开 → unsupported
        let (_app, mode) = MockRenditionApp::spawn(
            rig.home_b.path(),
            files.clone(),
            RenditionMode::Refuse,
            log.clone(),
        )
        .await;
        let err = rig
            .rendition_open("/收件箱/movie.mp4", 4096)
            .await
            .expect_err("拒开必 Err");
        assert!(err.contains("unsupported"), "拒开词面: {err}");

        // 2) 不应答(注入 1s 窗)→ 超时 → unsupported(单实例切相位)
        *mode.lock().unwrap() = RenditionMode::Ignore;
        let started = Instant::now();
        let err = rig
            .rendition_open("/收件箱/movie.mp4", 4096)
            .await
            .expect_err("不应答必 Err");
        assert!(err.contains("unsupported"), "超时词面: {err}");
        assert!(
            started.elapsed() >= Duration::from_millis(INJECTED_RENDITION_TIMEOUT_MS),
            "超时窗须真走完(注入窗生效): {:?}",
            started.elapsed()
        );

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}
