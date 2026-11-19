//! 可 seek fd 源桥端到端(V4-1 T8):host 双 daemon(A 观看端 / B 源端)+
//! 伪 App seek 源桥应答器(真实 IpcClient 订阅 `fs.request_seek` +
//! FdPassClient 回 `0x0D seek_fd`,fd = 真开文件描述符)→ roots.set 注册
//! bridge 根 / photo_serve 开照片桥 → 观看端 `preview_read_range` 逐字节
//! 校验。fdpass 依赖 Unix 域套接字,整文件 cfg(unix)(CI 在 WSL 跑;
//! Windows 编译单元整体缺席)。
//!
//! 断言面(brief 测试矩阵:Rust 侧 e2e 模拟桥):
//! - **seek 直读**:bridge vpath 读 = 0x0D fd 交付 → 中段定点读逐字节一致
//!   (seek 语义:非零 offset 一读即中),且 `serve-tmp` 保持为空(导出
//!   回退未触发——seek 先行的证词)、伪 App 收到全量 vpath;
//! - **回退面三条**(brief:不可用 → 既有导出再读回退,轨已备):App 拒开
//!   (无 fd 0x0D)、fd 不可 seek(管道流)、App 不应答(注入短超时)——
//!   三路都落 0x09 导出回退,字节仍逐字节一致;
//! - **照片 vpath**(SAF/MediaStore 两源):`/照片图库/相册/件` 同面——
//!   seek 直读;拒开 → 0x07 照片导出回退(照片桥单查 photo_id → 导出件
//!   → ExportFileSource)。
//! - **iCloud 未卸载错误形**(V4-1 T10 iOS 桥):App 拒开词面 = 稳定词
//!   `icloud_not_local`(T25 UI「原件在 iCloud,需先下载」提示语义的线上
//!   形)→ 词面透传进引擎 Err,导出回退照常兜底(字节仍逐字节一致)——
//!   词面不破回退链,iOS 桥与 0x0D 契约同轨的证词。
//!
//! 旧对端兼容:daemon 与 App 同 APK 同船,「连着但不识 fs.request_seek」
//! 只出现在测试台——超时注入缝 `MTDSS_TEST_SEEK_TIMEOUT_MS` 全程压短。

#![cfg(unix)]

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{
    FdPassClient, KIND_ACK, KIND_PHOTO_EXPORT, KIND_PHOTO_LIST, KIND_SEEK_FD, KIND_SERVE_EXPORT,
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

/// seek 应答等待窗注入(500ms;超时用例吃这个窗,不真等 10s)。
const INJECTED_SEEK_TIMEOUT_MS: u64 = 500;
static SEEK_TIMEOUT_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_seek_timeout() {
    SEEK_TIMEOUT_INJECT.get_or_init(|| {
        std::env::set_var("MTDSS_TEST_SEEK_TIMEOUT_MS", INJECTED_SEEK_TIMEOUT_MS.to_string());
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

/// V3-1 审批状态机:任何上线发生前 REST 预置设备行(与 serve_e2e 同款)。
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

/// 确定性字节图样(与 ctrl_e2e/source 单测同款函数式图样,逐字节可复算)。
fn pattern_byte(i: u64) -> u8 {
    (i.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8
}

fn write_pattern_file(path: &Path, size: u64) {
    let mut f = std::fs::File::create(path).unwrap();
    let mut off = 0u64;
    while off < size {
        let want = (size - off).min(1024 * 1024) as usize;
        let block: Vec<u8> = (0..want as u64).map(|k| pattern_byte(off + k)).collect();
        f.write_all(&block).unwrap();
        off += want as u64;
    }
    f.sync_all().unwrap();
}

fn expect_segment(offset: u64, len: usize) -> Vec<u8> {
    (0..len as u64).map(|k| pattern_byte(offset + k)).collect()
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
    /// photo_serve 开时 B 侧带照片桥(photo 用例)。
    async fn new(email: &str, photo_serve: bool) -> Self {
        inject_seek_timeout();
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

    /// 观看端 A 对 B 的一段按需读(整体限时;返回全量字节;单帧错即 panic)。
    async fn read_range(
        &self,
        path: &str,
        size: u64,
        offset: u64,
        len: u32,
    ) -> Vec<u8> {
        use mtdss_core::transport::ctrl::CtrlStreamOutcome;
        let outcome = tokio::time::timeout(
            CALL_TIMEOUT,
            self.daemon_a.preview_read_range(&self.dev_b, path, 0, size as i64, offset, len),
        )
        .await
        .expect("preview_read_range 整体超时(悬挂面)");
        let mut s = match outcome.expect("preview_read_range 发起应成功") {
            CtrlStreamOutcome::Stream(s) => s,
            CtrlStreamOutcome::Frame(r) => panic!("预览读必须进流式面,得单帧: {r:?}"),
        };
        let mut got = Vec::new();
        while let Some(item) = s.next_chunk().await {
            got.extend_from_slice(&item.expect("预览流全程无中途错"));
        }
        got
    }
}

// ---------- 伪 App(seek 源桥应答器)----------

/// seek 应答模式(逐用例注入)。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SeekMode {
    /// 开真文件 fd 回 0x0D。
    Deliver,
    /// 无 fd 0x0D 拒开。
    Refuse,
    /// 管道读端 fd 回 0x0D(不可 seek 流)。
    Pipe,
    /// 不应答(吃注入超时 → 引擎回退)。
    Ignore,
    /// 无 fd 0x0D 拒开,error 词面钉稳定词 `icloud_not_local`(V4-1 T10
    /// iOS 桥 iCloud 未卸载形;iOS SeekResponder 同词面)。
    Icloud,
}

/// 伪 App 日志(用例体断言数据源)。
#[derive(Default)]
struct SeekLog {
    seen_seek: Mutex<Vec<(u64, String)>>,
    seen_export: Mutex<Vec<(u64, String)>>,
    seen_photo_export: Mutex<Vec<(u64, String)>>,
    seen_photo_list: Mutex<Vec<(u64, String)>>,
}

/// 伪 App:订阅 B 的 fs.request_seek / fs.serve_export / fs.photo_* 事件 →
/// FdPassClient 回 0x0D/0x09/0x06/0x07。事件流关闭(daemon 收摊)即退。
/// seek 应答模式经共享句柄逐相位切换(单实例,无双应答竞速)。
struct MockSeekApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    serve_dir: PathBuf,
    photo_dir: PathBuf,
    /// vpath → 真实文件路径(seek fd 开这个;serve/photo 导出拷贝同源)。
    files: BTreeMap<String, PathBuf>,
    mode: Arc<Mutex<SeekMode>>,
    log: Arc<SeekLog>,
}

impl MockSeekApp {
    /// 装配 + 起跑;返回任务句柄与模式切换句柄(逐相位用)。
    async fn spawn(
        home: &Path,
        files: BTreeMap<String, PathBuf>,
        mode: SeekMode,
        log: Arc<SeekLog>,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<SeekMode>>) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        // 两处导出临时目录锚(与 daemon serve_dir/photo_dir 同名;回传
        // 路径前缀检查按各自锚收口)
        let serve_dir = home.join("serve-tmp");
        let photo_dir = home.join("photo-serve");
        std::fs::create_dir_all(&serve_dir).unwrap();
        std::fs::create_dir_all(&photo_dir).unwrap();
        let mode = Arc::new(Mutex::new(mode));
        let app = Self {
            ipc,
            fdpass,
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
                "fs.request_seek" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let path = ev.params["path"].as_str().unwrap().to_string();
                    self.log.seen_seek.lock().unwrap().push((request_id, path.clone()));
                    let mode = *self.mode.lock().unwrap();
                    match mode {
                        SeekMode::Ignore => continue,
                        SeekMode::Refuse => {
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_SEEK_FD,
                                    json!({"error": "App 拒开(测试注入)", "ok": false, "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0D 拒开");
                        }
                        SeekMode::Icloud => {
                            // T10 iOS 桥 iCloud 未卸载形:error = 稳定词
                            // icloud_not_local(无 fd;字母序拒形)
                            let ack = self
                                .fdpass
                                .send_msg(
                                    KIND_SEEK_FD,
                                    json!({"error": "icloud_not_local", "ok": false, "request_id": request_id}),
                                    None,
                                )
                                .await;
                            assert_ack(ack, "0x0D iCloud 拒开");
                        }
                        SeekMode::Pipe => {
                            let (rx, tx) = std::os::unix::net::UnixStream::pair().unwrap();
                            drop(tx); // 写端丢弃:读端无数据且不可 seek
                            let ack = self
                                .fdpass
                                .send_msg(KIND_SEEK_FD, json!({"request_id": request_id}), Some(rx.into()))
                                .await;
                            assert_ack(ack, "0x0D 管道");
                        }
                        SeekMode::Deliver => {
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
                    // 相册清单:单一照片项(photo_id = 0x07/seek 解析凭据)
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

/// seek 直读主线:bridge vpath 中段定点读(非零 offset = seek 语义)逐字节
/// 一致;导出回退未触发(serve-tmp 空 + 伪 App 零 0x09);EOF 探测零数据帧。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seek_fd_source_serves_read_range_without_export() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("seek-main@test", false).await;
        rig.register_bridge_root().await;
        let file = rig.scratch.path().join("video.bin");
        let size: u64 = 300_000;
        write_pattern_file(&file, size);
        let log = Arc::new(SeekLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/视频.bin".to_string(), file.clone());
        let _app = MockSeekApp::spawn(&rig._home_b.path().to_path_buf(), files, SeekMode::Deliver, log.clone()).await;
        // 中段定点读:非零 offset 一读即中(seek 直读,非顺序导轨)
        let mid = rig.read_range("/收件箱/视频.bin", size, 100_000, 1_000).await;
        assert_eq!(mid, expect_segment(100_000, 1_000), "中段 seek 读逐字节一致");

        // 头段
        let head = rig.read_range("/收件箱/视频.bin", size, 0, 500).await;
        assert_eq!(head, expect_segment(0, 500), "头段一致");

        // EOF 探测(offset == size)→ 零数据帧(空流)
        let eof = rig.read_range("/收件箱/视频.bin", size, size, 10).await;
        assert!(eof.is_empty(), "EOF 探测零数据帧: {}", eof.len());

        // seek 先行的证词:伪 App 见 fs.request_seek(全量 vpath)、零 0x09
        let seen = log.seen_seek.lock().unwrap().clone();
        assert!(
            seen.iter().any(|(_, p)| p == "/收件箱/视频.bin"),
            "伪 App 应收到全量 vpath: {seen:?}"
        );
        assert!(log.seen_export.lock().unwrap().is_empty(), "导出回退不得触发");

        // 引擎侧 serve-tmp 保持为空(导出件从未落盘)
        let tmp = rig._home_b.path().join("serve-tmp");
        assert!(
            !tmp.exists() || std::fs::read_dir(&tmp).unwrap().next().is_none(),
            "serve-tmp 必空(seek 直读不走导出轨)"
        );

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}

/// 回退面三条(brief:不可用 → 既有导出再读):App 拒开 / 管道 fd(不可
/// seek 流)/ 不应答(注入超时)——三路都落 0x09 导出回退,字节逐字节一致。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seek_unavailable_falls_back_to_export_rail() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("seek-fallback@test", false).await;
        rig.register_bridge_root().await;
        let file = rig.scratch.path().join("video.bin");
        let size: u64 = 100_000;
        write_pattern_file(&file, size);
        let log = Arc::new(SeekLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/视频.bin".to_string(), file.clone());

        // 1) 拒开 → 回退
        let (_app, mode) = MockSeekApp::spawn(
            &rig._home_b.path().to_path_buf(),
            files.clone(),
            SeekMode::Refuse,
            log.clone(),
        )
        .await;
        let got = rig.read_range("/收件箱/视频.bin", size, 40_000, 2_000).await;
        assert_eq!(got, expect_segment(40_000, 2_000), "拒开回退导出轨,字节一致");
        assert!(!log.seen_export.lock().unwrap().is_empty(), "拒开必落导出回退");

        // 2) 管道 fd(不可 seek)→ 回退(单实例切相位,无双应答竞速)
        *mode.lock().unwrap() = SeekMode::Pipe;
        let got = rig.read_range("/收件箱/视频.bin", size, 7_000, 1_000).await;
        assert_eq!(got, expect_segment(7_000, 1_000), "管道 fd 回退导出轨,字节一致");
        assert!(log.seen_export.lock().unwrap().len() >= 2, "管道 fd 必落导出回退");

        // 3) 不应答(注入 500ms 窗)→ 回退
        *mode.lock().unwrap() = SeekMode::Ignore;
        let got = rig.read_range("/收件箱/视频.bin", size, 9_000, 1_000).await;
        assert_eq!(got, expect_segment(9_000, 1_000), "超时回退导出轨,字节一致");
        assert!(log.seen_export.lock().unwrap().len() >= 3, "超时必落导出回退");

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}

/// 照片 vpath(SAF/MediaStore 两源):seek 直读;拒开 → 0x07 照片导出回退
/// (照片桥单查 photo_id → 导出件 → ExportFileSource)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_vpath_seek_and_export_fallback() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("seek-photo@test", true).await;
        let file = rig.scratch.path().join("photo.jpg");
        let size: u64 = 50_000;
        write_pattern_file(&file, size);
        let log = Arc::new(SeekLog::default());
        let mut files = BTreeMap::new();
        files.insert("/照片图库/相册/x.jpg".to_string(), file.clone());

        // seek 直读臂
        let (_app, mode) = MockSeekApp::spawn(
            &rig._home_b.path().to_path_buf(),
            files.clone(),
            SeekMode::Deliver,
            log.clone(),
        )
        .await;
        let got = rig.read_range("/照片图库/相册/x.jpg", size, 20_000, 1_000).await;
        assert_eq!(got, expect_segment(20_000, 1_000), "照片 vpath seek 直读逐字节一致");
        assert!(log.seen_seek.lock().unwrap().iter().any(|(_, p)| p == "/照片图库/相册/x.jpg"));
        assert!(log.seen_photo_export.lock().unwrap().is_empty(), "seek 命中不走照片导出");

        // 拒开 → 照片导出回退(0x06 单查填 photo_id 缓存 → 0x07 导出件)
        *mode.lock().unwrap() = SeekMode::Refuse;
        let got = rig.read_range("/照片图库/相册/x.jpg", size, 30_000, 1_000).await;
        assert_eq!(got, expect_segment(30_000, 1_000), "照片拒开回退导出件,字节一致");
        let exports = log.seen_photo_export.lock().unwrap().clone();
        assert!(
            exports.iter().any(|(_, id)| id == "img:1"),
            "照片导出凭据 = 0x06 清单的 photo_id: {exports:?}"
        );

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}

/// iCloud 未卸载错误形(V4-1 T10 iOS 桥):App 拒开词面 = 稳定词
/// `icloud_not_local`(iOS SeekResponder 对未卸载 iCloud 优化项的恒拒词;
/// T25 UI「原件在 iCloud,需先下载」提示语义的线上形)→ 引擎词面透传落
/// 既有导出回退(回退已备勿重写),字节仍逐字节一致——词面不破回退链。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn icloud_not_local_refuse_falls_back_to_export() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let rig = Rig::new("seek-icloud@test", false).await;
        rig.register_bridge_root().await;
        let file = rig.scratch.path().join("video.bin");
        let size: u64 = 100_000;
        write_pattern_file(&file, size);
        let log = Arc::new(SeekLog::default());
        let mut files = BTreeMap::new();
        files.insert("/收件箱/视频.bin".to_string(), file.clone());
        let (_app, _mode) = MockSeekApp::spawn(
            &rig._home_b.path().to_path_buf(),
            files,
            SeekMode::Icloud,
            log.clone(),
        )
        .await;
        let got = rig.read_range("/收件箱/视频.bin", size, 15_000, 1_000).await;
        assert_eq!(
            got,
            expect_segment(15_000, 1_000),
            "iCloud 拒开回退导出轨,字节一致"
        );
        assert!(log.seen_seek.lock().unwrap().iter().any(|(_, p)| p == "/收件箱/视频.bin"));
        assert!(!log.seen_export.lock().unwrap().is_empty(), "iCloud 拒开必落导出回退");

        rig.shutdown().await;
    })
    .await
    .expect("用例整体超时(悬挂面)");
}
