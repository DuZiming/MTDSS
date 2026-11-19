//! 缩略图桥端到端(V4-1 T9):host 双 daemon(A 观看端 / B 源端)+ 伪 App
//! 缩略图应答器(真实 IpcClient 订阅 `fs.request_thumb` + FdPassClient 回
//! `0x0B thumb_data`,JPEG 落 App 缩略图临时目录回路径)→ roots.set 注册
//! bridge 根 / photo_serve 开照片桥 → 观看端 `preview_thumb` 单帧校验。
//! fdpass 依赖 Unix 域套接字,整文件 cfg(unix)(CI 在 WSL 跑;Windows 编
//! 译单元整体缺席)。
//!
//! 断言面(brief 测试矩阵:thumb 桥路→T7 缓存落账;桥拒/超时→回退):
//! - **桥路主线**:bridge vpath thumb = 0x0B 交付 JPEG → 观看端收非空字节
//!   逐字节一致;源端 `{home}/thumb-tmp` 记账恰一条 + 缓存件在场(桥图入
//!   T7 源端缓存);App 请求级临时件随引擎读毕清理(thumb-serve 恒净);
//!   二次请求零重生成(缓存命中零广播——伪 App 收到的 fs.request_thumb
//!   恰一条)、零导出回退;
//! - **回退两条**(brief:桥拒/超时 → 既有回退,轨 T7/T8 已备):App 拒答
//!   (0x0B ok:false)与不应答(注入短超时)各落 0x09 导出件回退(seen
//!   serve_export;非 Windows 本机生成恒空 = 空字节 ok 帧,且空结果不入
//!   缓存);
//! - **照片 vpath**(photo_serve 开):`/照片图库/相册/件` 桥路同面;拒答 →
//!   照片导出件回退(0x06 单查填凭据 → 0x07 导出件)。
//!
//! 旧对端兼容:daemon 与 App 同 APK 同船,「连着但不识 fs.request_thumb」
//! 只出现在测试台——超时注入缝 `MTDSS_TEST_THUMB_TIMEOUT_MS` 全程压短。

#![cfg(unix)]

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{
    FdPassClient, KIND_ACK, KIND_PHOTO_EXPORT, KIND_PHOTO_LIST, KIND_SERVE_EXPORT, KIND_THUMB_DATA,
};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// 伪 App 事件等待上界。
const APP_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// thumb 应答等待窗注入(500ms;超时用例吃这个窗,不真等 10s)。
const INJECTED_THUMB_TIMEOUT_MS: u64 = 500;
static THUMB_TIMEOUT_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_thumb_timeout() {
    THUMB_TIMEOUT_INJECT.get_or_init(|| {
        std::env::set_var("MTDSS_TEST_THUMB_TIMEOUT_MS", INJECTED_THUMB_TIMEOUT_MS.to_string());
    });
}

/// 用例并发闸(打洞/信令负载;两用例并行足够)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE.clone().acquire_owned().await.expect("semaphore never closed")
}

/// 源端形态(photo_serve 按需开;测试不触更新托盘面)。
fn opts(home: &Path, photo_serve: bool) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        inbox_publish: false,
        photo_serve,
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

/// JPEG 夹具(image crate 现编,免外部二进制;纯色可复算——thumb 内容
/// 断言只看字节一致,解码面归 T7 单测/冒烟)。
fn fixture_jpeg(path: &Path, w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |_x, _y| {
        image::Rgb(color)
    }));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&img)
        .expect("JPEG 夹具现编");
    std::fs::write(path, &out).expect("夹具写盘");
    out
}

// ---------- 测试台(A 观看端 / B 源端)----------

struct Rig {
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    cb: IpcClient,
    dev_a: String,
    dev_b: String,
    scratch: tempfile::TempDir,
}

impl Rig {
    /// photo_serve 开时 B 侧带照片桥(照片臂用例)。
    async fn new(email: &str, photo_serve: bool) -> Self {
        inject_thumb_timeout();
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        preregister_identity(addr, home_a.path(), email, "A").await;
        preregister_identity(addr, home_b.path(), email, "B").await;
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path(), false)).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path(), photo_serve)).await.unwrap();
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
            _home_b: home_b,
            _state: state,
            daemon_a,
            daemon_b,
            cb,
            dev_a,
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

    /// 观看端 A 对 B 的一次缩略图请求(整体限时;单帧 ok + jpeg 字节)。
    async fn thumb(&self, path: &str, mtime: i64, size: u64, w: u32) -> (bool, Vec<u8>) {
        let outcome = tokio::time::timeout(
            CALL_TIMEOUT,
            self.daemon_a.preview_thumb(&self.dev_b, path, mtime, size as i64, w),
        )
        .await
        .expect("preview_thumb 整体超时(悬挂面)");
        let resp = outcome.expect("preview_thumb 发起应成功");
        // 缩略图面恒单帧成功形(失败=空字节,不回错误帧);错误帧即挂测试
        (true, resp.jpeg)
    }
}

// ---------- 伪 App(缩略图应答器)----------

/// thumb 应答模式(逐用例注入)。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ThumbMode {
    /// 生成 JPEG(夹具拷贝)落 thumb-serve 回 0x0B `{ok,path}`。
    Deliver,
    /// 落空文件回 0x0B `{ok,path}`(App「生成失败但 ok」面)。
    Empty,
    /// 0x0B `{error, ok:false}` 拒答(不可解/解码失败词面)。
    Refuse,
    /// 不应答(吃注入超时 → 引擎回退)。
    Ignore,
}

/// 伪 App 日志(用例体断言数据源)。
#[derive(Default)]
struct ThumbLog {
    seen_thumb: Mutex<Vec<(u64, String, i64)>>,
    seen_export: Mutex<Vec<(u64, String)>>,
    seen_photo_list: Mutex<Vec<(u64, String)>>,
    seen_photo_export: Mutex<Vec<(u64, String)>>,
}

impl ThumbLog {
    fn thumb_paths(&self) -> Vec<String> {
        self.seen_thumb.lock().unwrap().iter().map(|(_, p, _)| p.clone()).collect()
    }
}

/// 伪 App:订阅 B 的 fs.request_thumb / fs.serve_export / fs.photo_* 事件 →
/// FdPassClient 回 0x0B/0x09/0x06/0x07。事件流关闭(daemon 收摊)即退。
/// thumb 应答模式经共享句柄逐相位切换(单实例,无双应答竞速)。
struct MockThumbApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    thumb_dir: PathBuf,
    serve_dir: PathBuf,
    photo_dir: PathBuf,
    /// vpath → 真实文件路径(thumb「生成源」= 夹具原字节;photo 导出拷贝同源)。
    files: BTreeMap<String, PathBuf>,
    mode: Arc<Mutex<ThumbMode>>,
    log: Arc<ThumbLog>,
}

impl MockThumbApp {
    /// 装配 + 起跑;返回任务句柄与模式切换句柄(逐相位用)。
    async fn spawn(
        home: &Path,
        files: BTreeMap<String, PathBuf>,
        mode: ThumbMode,
        log: Arc<ThumbLog>,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<ThumbMode>>) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        // App 侧目录锚(与引擎侧同名同构:thumb-serve 生成件 / serve-tmp
        // 与 photo-serve 导出件;引擎回传路径前缀检查按各自锚收口)
        let thumb_dir = home.join("thumb-serve");
        let serve_dir = home.join("serve-tmp");
        let photo_dir = home.join("photo-serve");
        std::fs::create_dir_all(&thumb_dir).unwrap();
        std::fs::create_dir_all(&serve_dir).unwrap();
        std::fs::create_dir_all(&photo_dir).unwrap();
        let mode = Arc::new(Mutex::new(mode));
        let app = Self {
            ipc,
            fdpass,
            thumb_dir,
            serve_dir,
            photo_dir,
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
                "fs.request_thumb" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let path = ev.params["path"].as_str().unwrap().to_string();
                    let w = ev.params["w"].as_i64().unwrap_or(0);
                    self.log.seen_thumb.lock().unwrap().push((request_id, path.clone(), w));
                    let mode = *self.mode.lock().unwrap();
                    match mode {
                        ThumbMode::Ignore => continue,
                        ThumbMode::Refuse => {
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_THUMB_DATA,
                                    json!({"error": "App 不可解(测试注入)", "ok": false, "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0B 拒答");
                        }
                        ThumbMode::Empty => {
                            let dest = self.thumb_dir.join(format!("{request_id}-empty.jpg"));
                            std::fs::write(&dest, b"").unwrap();
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_THUMB_DATA,
                                    json!({"ok": true, "path": dest.to_string_lossy(), "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0B 空件");
                        }
                        ThumbMode::Deliver => {
                            let Some(file) = self.files.get(&path) else {
                                let ack = self
                                    .fdpass
                                    .send_msg(
                                        KIND_THUMB_DATA,
                                        json!({"error": "源已不存在", "ok": false, "request_id": request_id}),
                                        None,
                                    )
                                    .await;
                                assert_ack(ack, "0x0B 缺席拒答");
                                continue;
                            };
                            let dest = self.thumb_dir.join(format!("{request_id}-x.jpg"));
                            std::fs::copy(file, &dest).unwrap();
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_THUMB_DATA,
                                    json!({"ok": true, "path": dest.to_string_lossy(), "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0B 交付");
                        }
                    }
                }
                "fs.serve_export" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let root = ev.params["root"].as_str().unwrap().to_string();
                    let sub = ev.params["sub"].as_str().unwrap().to_string();
                    let vpath = format!("{root}/{sub}");
                    self.log.seen_export.lock().unwrap().push((request_id, vpath.clone()));
                    let Some(file) = self.files.get(&vpath) else {
                        let _ = self
                            .fdpass
                            .send_msg(
                                KIND_SERVE_EXPORT,
                                json!({"error": "源已不存在", "ok": false, "request_id": request_id}),
                                None,
                            )
                            .await;
                        continue;
                    };
                    let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_string();
                    let dest = self.serve_dir.join(format!("{request_id}-{name}"));
                    std::fs::copy(file, &dest).unwrap();
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_SERVE_EXPORT,
                            json!({"ok": true, "path": dest.to_string_lossy(), "request_id": request_id}),
                            None,
                        )
                        .await;
                    assert_ack(ack, "0x09 回退");
                }
                "fs.photo_list" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let path = ev.params["path"].as_str().unwrap().to_string();
                    self.log.seen_photo_list.lock().unwrap().push((request_id, path.clone()));
                    if path != "相册" {
                        let _ = self
                            .fdpass
                            .send_msg(
                                KIND_PHOTO_LIST,
                                json!({"error": "无此相册", "ok": false, "request_id": request_id}),
                                None,
                            )
                            .await;
                        continue;
                    }
                    // 相册清单:单一照片项(photo_id = 0x07 导出凭据)
                    let size = std::fs::metadata(self.files.get("/照片图库/相册/x.jpg").unwrap())
                        .unwrap()
                        .len();
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_PHOTO_LIST,
                            json!({
                                "entries": [
                                    {"is_dir": false, "mtime_ms": 111i64, "name": "x.jpg", "photo_id": "img:1", "size": size},
                                ],
                                "ok": true,
                                "request_id": request_id,
                                "truncated": false,
                            }),
                            None,
                        )
                        .await;
                    assert_ack(ack, "0x06 清单");
                }
                "fs.photo_export" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let photo_id = ev.params["photo_id"].as_str().unwrap().to_string();
                    self.log.seen_photo_export.lock().unwrap().push((request_id, photo_id));
                    let Some(file) = self.files.get("/照片图库/相册/x.jpg") else {
                        let _ = self
                            .fdpass
                            .send_msg(
                                KIND_PHOTO_EXPORT,
                                json!({"error": "资产不可用", "ok": false, "request_id": request_id}),
                                None,
                            )
                            .await;
                        continue;
                    };
                    let dest = self.photo_dir.join(format!("{request_id}-x.jpg"));
                    std::fs::copy(file, &dest).unwrap();
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_PHOTO_EXPORT,
                            json!({"ok": true, "path": dest.to_string_lossy(), "request_id": request_id}),
                            None,
                        )
                        .await;
                    assert_ack(ack, "0x07 照片回退");
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

/// 桥路主线:bridge vpath thumb = 0x0B 交付,字节逐字节一致;T7 源端缓存
/// 落账(记账恰一条 + 缓存件在场);请求级临时件随引擎读毕清理(thumb-serve
/// 恒净);二次请求零重生成(零广播)零导出;w 原样到达伪 App(降采样参数
/// 面由 App 侧落地,e2e 钉「引擎发什么 App 收什么」)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumb_bridge_serves_and_caches() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("thumb-main@test", false).await;
        rig.register_bridge_root().await;
        let jpeg = fixture_jpeg(&rig.scratch.path().join("p.jpg"), 96, 64, [200, 30, 40]);
        let log = Arc::new(ThumbLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/图.jpg".to_string(), rig.scratch.path().join("p.jpg"));
        let _app = MockThumbApp::spawn(
            &rig._home_b.path().to_path_buf(),
            files,
            ThumbMode::Deliver,
            log.clone(),
        )
        .await;

        // 首取:桥交付,字节一致
        let (ok, got) = rig.thumb("/收件箱/图.jpg", 7, jpeg.len() as u64, 64).await;
        assert!(ok, "thumb 恒成功帧");
        assert_eq!(got, jpeg, "桥图逐字节一致");

        // T7 缓存落账:记账恰一条 + 缓存件在场
        let ledger = rig._home_b.path().join("thumb-tmp").join("ledger.json");
        let body = std::fs::read_to_string(&ledger).expect("记账应在场(桥图入缓存)");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["entries"].as_array().unwrap().len(), 1, "记账恰一条: {v}");
        let cached: Vec<_> = std::fs::read_dir(rig._home_b.path().join("thumb-tmp"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".jpg"))
            .collect();
        assert_eq!(cached.len(), 1, "缓存件恰一枚(请求级临时件已清理): {cached:?}");

        // thumb-serve 恒净(引擎读毕 unlink;0x0B 交付 → 读 → 删)
        let serve = rig._home_b.path().join("thumb-serve");
        assert!(
            !serve.exists() || std::fs::read_dir(&serve).unwrap().next().is_none(),
            "thumb-serve 必空(引擎侧清理义务)"
        );

        // 二次请求:缓存命中零重生成(零广播)、字节一致、零导出
        let (ok, got2) = rig.thumb("/收件箱/图.jpg", 7, jpeg.len() as u64, 64).await;
        assert!(ok);
        assert_eq!(got2, jpeg, "命中字节一致");
        let seen = log.thumb_paths();
        assert_eq!(seen.len(), 1, "二次请求零广播(缓存命中): {seen:?}");
        assert_eq!(seen[0], "/收件箱/图.jpg", "事件携全量 vpath");
        let raw = log.seen_thumb.lock().unwrap().clone();
        assert_eq!(raw[0].2, 64, "事件携 w(降采样参数原样广播): {raw:?}");
        assert!(log.seen_export.lock().unwrap().is_empty(), "桥命中不落导出回退");

        // 异 w = 新键 = 重取(键敏感性:桥臂同 T7 键口径)
        let (_ok, got3) = rig.thumb("/收件箱/图.jpg", 7, jpeg.len() as u64, 128).await;
        assert_eq!(got3, jpeg, "异 w 桥重取仍一致");
        assert_eq!(log.thumb_paths().len(), 2, "异 w = 新键重取(缓存键含 w)");

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}

/// 回退两条(brief:桥拒/超时 → 既有回退,轨 T7 已备):0x0B 拒答与不应答
/// (注入 500ms 窗)各落 0x09 导出件回退;非 Windows 本机生成恒空 = 空字节
/// ok 帧;空结果不入缓存(记账零增)。空件 ok 形(0x0B `{ok,空件}`)同空
/// 字节面 —— App「生成失败但 ok」亦落回退。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumb_bridge_refuse_timeout_empty_fall_back_to_export() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("thumb-fallback@test", false).await;
        rig.register_bridge_root().await;
        let jpeg = fixture_jpeg(&rig.scratch.path().join("p.jpg"), 96, 64, [10, 20, 30]);
        let log = Arc::new(ThumbLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/图.jpg".to_string(), rig.scratch.path().join("p.jpg"));

        // 1) 拒答 → 0x09 导出回退,空字节 ok 帧
        let (_app, mode) = MockThumbApp::spawn(
            &rig._home_b.path().to_path_buf(),
            files.clone(),
            ThumbMode::Refuse,
            log.clone(),
        )
        .await;
        let (ok, got) = rig.thumb("/收件箱/图.jpg", 7, jpeg.len() as u64, 64).await;
        assert!(ok, "回退面仍恒成功帧");
        assert!(got.is_empty(), "非 Windows 本机生成恒空 = 空字节: {}", got.len());
        assert!(!log.thumb_paths().is_empty(), "桥先行证词:fs.request_thumb 已发");
        assert!(!log.seen_export.lock().unwrap().is_empty(), "拒答必落导出回退");

        // 2) 不应答(注入 500ms 窗)→ 同落导出回退
        *mode.lock().unwrap() = ThumbMode::Ignore;
        let (ok, got) = rig.thumb("/收件箱/图.jpg", 7, jpeg.len() as u64, 64).await;
        assert!(ok);
        assert!(got.is_empty(), "超时回退空字节");
        assert!(log.seen_export.lock().unwrap().len() >= 2, "超时必落导出回退");

        // 3) 空件 ok 形(生成失败但 ok)→ 同空字节面,落回退
        *mode.lock().unwrap() = ThumbMode::Empty;
        let (ok, got) = rig.thumb("/收件箱/图.jpg", 7, jpeg.len() as u64, 64).await;
        assert!(ok);
        assert!(got.is_empty(), "空件 = 空字节(不入缓存,落回退)");
        assert!(log.seen_export.lock().unwrap().len() >= 3, "空件必落导出回退");

        // 空结果不入缓存(T7 语义):记账零条目
        let ledger = rig._home_b.path().join("thumb-tmp").join("ledger.json");
        assert!(
            !ledger.exists(),
            "失败面不入缓存(记账不应在场): {:?}",
            std::fs::read_to_string(&ledger)
        );

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}

/// 照片 vpath(photo_serve 开):桥路同面(字节一致 + 缓存落账);拒答 →
/// 照片导出件回退(0x06 单查填凭据 → 0x07 导出件)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thumb_photo_vpath_bridge_and_export_fallback() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("thumb-photo@test", true).await;
        let jpeg = fixture_jpeg(&rig.scratch.path().join("p.jpg"), 96, 64, [66, 77, 88]);
        let log = Arc::new(ThumbLog::default());
        let mut files = BTreeMap::new();
        files.insert("/照片图库/相册/x.jpg".to_string(), rig.scratch.path().join("p.jpg"));

        // 桥路臂
        let (_app, mode) = MockThumbApp::spawn(
            &rig._home_b.path().to_path_buf(),
            files.clone(),
            ThumbMode::Deliver,
            log.clone(),
        )
        .await;
        let (ok, got) = rig.thumb("/照片图库/相册/x.jpg", 111, jpeg.len() as u64, 64).await;
        assert!(ok);
        assert_eq!(got, jpeg, "照片 vpath 桥图逐字节一致");
        assert!(log.thumb_paths().iter().any(|p| p == "/照片图库/相册/x.jpg"));
        assert!(log.seen_photo_export.lock().unwrap().is_empty(), "桥命中不走照片导出");
        let ledger = rig._home_b.path().join("thumb-tmp").join("ledger.json");
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(&ledger).expect("桥图应入缓存")).unwrap();
        assert_eq!(v["entries"].as_array().unwrap().len(), 1, "照片桥图落账恰一条");

        // 拒答 → 照片导出件回退(0x06 单查 → 0x07 导出;异 mtime = 新键
        // 强制桥臂重广播,照片树条目同件仍可单查)
        *mode.lock().unwrap() = ThumbMode::Refuse;
        let (ok, got) = rig.thumb("/照片图库/相册/x.jpg", 222, jpeg.len() as u64, 64).await;
        assert!(ok);
        assert!(got.is_empty(), "回退面空字节(非 Windows 本机生成恒空)");
        let exports = log.seen_photo_export.lock().unwrap().clone();
        assert!(
            exports.iter().any(|(_, id)| id == "img:1"),
            "照片导出凭据 = 0x06 清单的 photo_id: {exports:?}"
        );
        // 缓存命中面不破:原键仍命中(零广播)
        let before = log.thumb_paths().len();
        let (ok, got2) = rig.thumb("/照片图库/相册/x.jpg", 111, jpeg.len() as u64, 64).await;
        assert!(ok && got2 == jpeg, "原键缓存命中不回退");
        assert_eq!(log.thumb_paths().len(), before, "命中零广播");

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}
