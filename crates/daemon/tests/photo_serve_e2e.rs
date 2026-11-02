//! 照片图库虚拟根端到端(R7 Ruling P):host 双 daemon + 伪 App 照片桥
//! (真实 IpcClient 订阅 `fs.photo_list` / `fs.photo_export` + FdPassClient
//! 回 `0x06 photo_list` / `0x07 photo_export`)→ 守卫对端列根见「照片图库」
//! → 列相册 → 下载照片(引擎 0x07 导出临时件 → 读发 → 终态 unlink)→
//! md5/逐字节一致;对照片根的写/删/移/复制拒(只读来源);桥缺席 → 根
//! 隐藏;photo_serve 关 → 桌面语义不变。fdpass 依赖 Unix 域套接字,整文件
//! cfg(unix)(CI 在 WSL 跑;Windows 编译单元整体缺席)。
//!
//! 断言面(简报测试矩阵):
//! - 守卫对端列根:虚拟根与授权根并列(相册数 = children);列相册 =
//!   照片条目(名/尺寸);stat 走缓存(命中不再发请求)、miss 重列父目录;
//! - 下载单张/整相册/混合(真目录件 + 照片件同单)→ 接收端逐 rel 逐字节
//!   一致;任务终态后 `{home}/.mtdss/photo-serve` 引擎清扫为空;
//! - 对照片根 Delete/Move/Copy → 拒(只读文案);上传 INTO 照片根 →
//!   接收面回退默认收件目录(不写虚拟路径);
//! - 相册内不存在条目 stat → NOT_FOUND(miss 重列后仍无);
//! - 桥缺席(App 未授权/未跑):列根不含虚拟根、stat/list 报「不可用」;
//! - photo_serve 关(桌面 None):列根无探针延迟、`/照片图库` 走授权守卫
//!   INVALID_PATH;对照片根的变更面拒绝保持(只读文案不依赖开关)。

#![cfg(unix)]

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{FdPassClient, KIND_ACK, KIND_PHOTO_EXPORT, KIND_PHOTO_LIST};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时(下载编排含照片导出,留余量)。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// transfer.wait 长轮询上界。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);
/// 伪 App 事件等待上界。
const APP_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// 超时注入(生产 list 10s/export 60s,不真等):桥缺席用例的列根/浏览
/// 拒绝都吃这个窗,注入 1500ms(e2e 可承受,又不至于快到错过真实异步窗)。
const INJECTED_TIMEOUT_MS: u64 = 1500;
static TIMEOUT_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_timeout() {
    TIMEOUT_INJECT.get_or_init(|| {
        std::env::set_var("MTDSS_TEST_PHOTO_TIMEOUT_MS", INJECTED_TIMEOUT_MS.to_string());
    });
}

/// 用例并发闸(两用例同时跑打洞/信令,2 worker 足够)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE.clone().acquire_owned().await.expect("semaphore never closed")
}

/// 守卫 + 照片桥形态(A 端;授权根 = home/docs,照片桥开)。
fn photo_opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: Some(vec![home.join("docs")]),
        inbox_staging: None,
        inbox_publish: false,
        photo_serve: true,
    }
}

/// 桌面形态(B 端对照)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        inbox_publish: false,
        photo_serve: false,
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

fn fill_random_file(path: &Path, size: usize) -> Vec<u8> {
    use rand::RngCore as _;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(path, &data).unwrap();
    data
}

/// 进程内双 daemon 测试台(A = 照片桥形态;B = 桌面对照)。
struct Pair {
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    cb: IpcClient,
    dev_a: String,
}

impl Pair {
    /// a_opts 覆盖 A 端启动选项(照片桥开/关两形态共用)。
    async fn new(email: &str, a_opts: impl Fn(&Path) -> DaemonOpts) -> Self {
        inject_timeout();
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), a_opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        ipc_login(&ca, &server, email, "A").await;
        ipc_login(&cb, &server, email, "B").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let ra = call(&ca, "status", json!({})).await.unwrap();
        let dev_a = ra["device"]["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&ca, &dev_a, true, 30).await;
        let rb = call(&cb, "status", json!({})).await.unwrap();
        let _dev_b = rb["device"]["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&cb, &dev_a, true, 30).await;
        Self {
            _home_a: home_a,
            _home_b: home_b,
            _state: state,
            daemon_a,
            daemon_b,
            cb,
            dev_a,
        }
    }

    fn home_a(&self) -> PathBuf {
        self._home_a.path().to_path_buf()
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

// ---------- 伪 App 照片桥(镜像 iOS 侧 PhotoResponder 的职责面) ----------

/// 伪照片库:photo_id → (文件名, 原始字节)。
struct FakeLibrary {
    photos: HashMap<String, (&'static str, Vec<u8>)>,
}

impl FakeLibrary {
    /// 三张照片(内容随机,逐字节断言用)。
    fn new(dir: &Path) -> Self {
        let mut photos = HashMap::new();
        for (i, (pid, name)) in [
            ("pid-img1", "IMG_0001.JPG"),
            ("pid-img2", "IMG_0002.JPG"),
            ("pid-img3", "IMG_0003.JPG"),
        ]
        .into_iter()
        .enumerate()
        {
            let data = fill_random_file(&dir.join(format!("src-{i}.bin")), 24 * 1024 * (i + 1));
            photos.insert(pid.to_string(), (name, data));
        }
        Self { photos }
    }

    /// 相册列表(0x06 根臂)。
    fn albums(&self) -> Vec<Value> {
        vec![
            json!({"children": 2, "is_dir": true, "mtime_ms": 1760000000000i64, "name": "最近项目", "size": 0}),
            json!({"children": 1, "is_dir": true, "mtime_ms": 1760000000000i64, "name": "相册B", "size": 0}),
        ]
    }

    /// 相册内照片(0x06 相册臂;mtime = 固定值,尺寸 = 原始字节长)。
    fn album_photos(&self, ids: &[&str]) -> Vec<Value> {
        ids.iter()
            .map(|pid| {
                let (name, data) = &self.photos[*pid];
                json!({
                    "is_dir": false, "mtime_ms": 1760000000000i64,
                    "name": name, "photo_id": pid, "size": data.len(),
                })
            })
            .collect()
    }
}

/// 伪 App 日志(用例体断言数据源)。
#[derive(Default)]
struct PhotoLog {
    seen_list: Mutex<Vec<(u64, String)>>,
    seen_export: Mutex<Vec<(u64, String)>>,
}

/// 伪 App:订阅 fs.photo_list / fs.photo_export → 查伪库/写导出临时件 →
/// FdPassClient 回 0x06/0x07。事件流关闭(daemon 收摊)即退。
struct MockPhotoApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    serve_dir: PathBuf,
    library: Arc<FakeLibrary>,
    log: Arc<PhotoLog>,
}

impl MockPhotoApp {
    async fn spawn(
        home: &Path,
        library: Arc<FakeLibrary>,
    ) -> (tokio::task::JoinHandle<()>, Arc<PhotoLog>) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        let serve_dir = home.join(".mtdss").join("photo-serve");
        std::fs::create_dir_all(&serve_dir).unwrap();
        let log = Arc::new(PhotoLog::default());
        let app = Self {
            ipc,
            fdpass,
            serve_dir,
            library,
            log: log.clone(),
        };
        let task = tokio::spawn(app.run());
        (task, log)
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
                "fs.photo_list" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let path = ev.params["path"].as_str().unwrap().to_string();
                    self.log.seen_list.lock().unwrap().push((request_id, path.clone()));
                    let entries: Vec<Value> = match path.as_str() {
                        "" => self.library.albums(),
                        "最近项目" => self.library.album_photos(&["pid-img1", "pid-img2"]),
                        "相册B" => self.library.album_photos(&["pid-img3"]),
                        _ => {
                            // 未知子路径 = 拒答(桥「不可用」等价面)
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
                    };
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_PHOTO_LIST,
                            json!({
                                "entries": entries,
                                "ok": true,
                                "request_id": request_id,
                                "truncated": false,
                            }),
                            None,
                        )
                        .await;
                    assert_eq!(
                        ack.as_ref().map(|a| a.kind).ok(),
                        Some(KIND_ACK),
                        "0x06 应受理(ack): {ack:?}"
                    );
                }
                "fs.photo_export" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let photo_id = ev.params["photo_id"].as_str().unwrap().to_string();
                    self.log.seen_export.lock().unwrap().push((request_id, photo_id.clone()));
                    let Some((name, data)) = self.library.photos.get(&photo_id) else {
                        let _ = self
                            .fdpass
                            .send_msg(
                                KIND_PHOTO_EXPORT,
                                json!({"error": "照片已不可用", "ok": false, "request_id": request_id}),
                                None,
                            )
                            .await;
                        continue;
                    };
                    // 导出 = 原始字节落 App 临时目录(请求级唯一名,防在途覆写)
                    let dest = self.serve_dir.join(format!("{request_id}-{name}"));
                    std::fs::write(&dest, data).unwrap();
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_PHOTO_EXPORT,
                            json!({
                                "ok": true,
                                "path": dest.to_string_lossy(),
                                "request_id": request_id,
                            }),
                            None,
                        )
                        .await;
                    assert_eq!(
                        ack.as_ref().map(|a| a.kind).ok(),
                        Some(KIND_ACK),
                        "0x07 应受理(ack): {ack:?}"
                    );
                }
                _ => {}
            }
        }
    }
}

/// 连接 fds.sock(重试兜底启动竞速;上限 5s)。
async fn connect_fdpass_retry(home: &Path) -> FdPassClient {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match FdPassClient::connect(home).await {
            Ok(c) => return c,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => panic!("连接 fds.sock 失败: {e}"),
        }
    }
}

// ---------- 用例 ----------

/// 全链:列根见虚拟根 → 列相册 → stat 缓存 → 单张/整相册/混合下载逐字节
/// 一致 → 引擎清扫导出临时件 → 变更面拒绝 → 上传 INTO 回退默认收件目录。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_root_browse_download_full_chain() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("photo1@x.y", photo_opts).await;
        std::fs::create_dir_all(p.home_a().join("docs")).unwrap();
        let real = fill_random_file(&p.home_a().join("docs").join("mixed-real.bin"), 8 * 1024);
        let library = Arc::new(FakeLibrary::new(&tempfile::tempdir().unwrap().keep()));
        let (app, log) = MockPhotoApp::spawn(&p.home_a(), library.clone()).await;

        let img1 = &library.photos["pid-img1"];
        let img3 = &library.photos["pid-img3"];
        let photo_root = "/照片图库";

        // ① 列根:授权根 + 虚拟根并列(children = 相册数)
        let root = call(&p.cb, "list", json!({"device": p.dev_a, "path": ""})).await.unwrap();
        let entries = root["entries"].as_array().unwrap();
        let photo_entry = entries
            .iter()
            .find(|e| e["path"].as_str() == Some(photo_root))
            .unwrap_or_else(|| panic!("列根应含照片图库虚拟根: {root}"));
        assert_eq!(photo_entry["is_dir"], json!(true));
        assert_eq!(photo_entry["children"], json!(2));
        assert!(
            entries.iter().any(|e| e["name"].as_str() == Some("docs")),
            "授权根仍在列: {root}"
        );

        // ② 列相册列表(虚拟根下钻)
        let albums = call(&p.cb, "list", json!({"device": p.dev_a, "path": photo_root}))
            .await
            .unwrap();
        let names: Vec<&str> = albums["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["最近项目", "相册B"], "相册列表(目录优先/名升序): {albums}");

        // ③ 列相册内照片(名/尺寸来自 0x06)
        let album = call(
            &p.cb,
            "list",
            json!({"device": p.dev_a, "path": format!("{photo_root}/最近项目")}),
        )
        .await
        .unwrap();
        let album_entries = album["entries"].as_array().unwrap();
        assert_eq!(album_entries.len(), 2);
        assert_eq!(album_entries[0]["name"], json!("IMG_0001.JPG"));
        assert_eq!(album_entries[0]["size"], json!(img1.1.len() as u64));
        assert_eq!(album_entries[0]["path"], json!(format!("{photo_root}/最近项目/IMG_0001.JPG")));

        // ④ stat:相册照片走缓存(命中不再发列表请求);miss 重列父目录
        let st = call(
            &p.cb,
            "stat",
            json!({"device": p.dev_a, "path": format!("{photo_root}/最近项目/IMG_0001.JPG")}),
        )
        .await
        .unwrap();
        assert_eq!(st["entry"]["size"], json!(img1.1.len() as u64));
        {
            let before = log.seen_list.lock().unwrap().len();
            let st_root = call(&p.cb, "stat", json!({"device": p.dev_a, "path": photo_root}))
                .await
                .unwrap();
            assert_eq!(st_root["entry"]["is_dir"], json!(true));
            assert_eq!(st_root["entry"]["children"], json!(2));
            assert_eq!(
                log.seen_list.lock().unwrap().len(),
                before,
                "stat 命中缓存不得再发列表请求"
            );
        }
        // 不存在的照片:miss 重列后仍无 → NOT_FOUND(对端 FsError 透传,
        // RPC 码位恒 1 + kind 数据面,api.rs ctrl_rpc_err 口径)
        let miss = call(
            &p.cb,
            "stat",
            json!({"device": p.dev_a, "path": format!("{photo_root}/最近项目/NOPE.JPG")}),
        )
        .await;
        let err = miss.expect_err("不存在的照片应 NOT_FOUND");
        assert_eq!(err.code, 1, "对端错误码位: {err:?}");
        assert!(err.message.contains("NOT_FOUND"), "应透传 NOT_FOUND: {err:?}");

        // ⑤ 下载单张(虚拟文件输入 → rel = 文件名)
        let target1 = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": [format!("{photo_root}/最近项目/IMG_0001.JPG")],
                "dir": target1.path().to_string_lossy(),
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await
        .unwrap();
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.cb, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "单张下载终态: {fin}");
        let got1 = std::fs::read(target1.path().join("IMG_0001.JPG")).unwrap();
        assert_eq!(&got1, &img1.1, "单张下载逐字节一致");

        // ⑥ 下载整相册(虚拟目录输入 → rel = 相册名/文件名)
        let target2 = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": [format!("{photo_root}/相册B")],
                "dir": target2.path().to_string_lossy(),
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await
        .unwrap();
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.cb, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "相册下载终态: {fin}");
        let got3 = std::fs::read(target2.path().join("相册B").join("IMG_0003.JPG")).unwrap();
        assert_eq!(&got3, &img3.1, "相册下载逐字节一致");

        // ⑦ 混合单(真目录件 + 照片件)
        let target3 = tempfile::tempdir().unwrap();
        let docs = p.home_a().join("docs");
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": [
                    docs.join("mixed-real.bin").to_string_lossy(),
                    format!("{photo_root}/最近项目/IMG_0002.JPG"),
                ],
                "dir": target3.path().to_string_lossy(),
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await
        .unwrap();
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.cb, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "混合下载终态: {fin}");
        assert_eq!(std::fs::read(target3.path().join("mixed-real.bin")).unwrap(), real);
        // 照片件为**文件输入**:rel = 文件名(与真路径文件输入同语义,落
        // 目标根;目录输入才带目录段,见 ⑥)
        assert_eq!(
            std::fs::read(target3.path().join("IMG_0002.JPG")).unwrap(),
            library.photos["pid-img2"].1,
        );

        // ⑧ 引擎清扫:任务终态后 photo-serve 无导出残件
        let serve_dir = p.home_a().join(".mtdss").join("photo-serve");
        let leftover: Vec<_> = std::fs::read_dir(&serve_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(leftover.is_empty(), "引擎终态后应清空导出临时件: {leftover:?}");

        // ⑨ 变更面拒绝(只读来源;码位 INVALID_PATH)
        for method_params in [
            json!({"device": p.dev_a, "paths": [format!("{photo_root}/最近项目/IMG_0001.JPG")], "force": true}),
        ] {
            let r = call(&p.cb, "delete", method_params).await;
            let err = r.expect_err("对照片根 Delete 应拒");
            assert!(err.message.contains("只读"), "Delete 拒因应点名只读: {err:?}");
        }
        let r = call(
            &p.cb,
            "move",
            json!({
                "device": p.dev_a,
                "pairs": [{"from": format!("{photo_root}/最近项目/IMG_0001.JPG"), "to": format!("{photo_root}/x.JPG")}],
            }),
        )
        .await;
        assert!(r.expect_err("对照片根 Move 应拒").message.contains("只读"));
        let r = call(
            &p.cb,
            "copy",
            json!({
                "device": p.dev_a,
                "paths": [format!("{photo_root}/最近项目/IMG_0001.JPG")],
                "dir": p.home_a().join("docs").to_string_lossy(),
            }),
        )
        .await;
        assert!(r.expect_err("对照片根 Copy 应拒").message.contains("只读"));

        // ⑩ 上传 INTO 照片根:接收面回退默认收件目录(不落虚拟路径)
        let up_src = tempfile::tempdir().unwrap();
        let up_data = fill_random_file(&up_src.path().join("upload-into-photo.bin"), 4 * 1024);
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "upload", "files": [up_src.path().join("upload-into-photo.bin").to_string_lossy()],
                "to": p.dev_a, "dir": photo_root,
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await
        .unwrap();
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.cb, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "上传 INTO 回退后照常收: {fin}");
        let landed = p.home_a().join("inbox").join("upload-into-photo.bin");
        assert_eq!(std::fs::read(&landed).unwrap(), up_data, "回退默认收件目录落位");
        assert!(!p.home_a().join("照片图库").exists(), "不得创建真实 照片图库 目录");

        // 请求面:单张下载 = 恰一次 export;列根+相册浏览 = 列表请求在案
        assert!(!log.seen_export.lock().unwrap().is_empty(), "下载应触发导出请求");

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// 桥缺席(App 未授权/应答器未跑):列根不含虚拟根;list/stat 报不可用。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_root_hidden_when_bridge_absent() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("photo2@x.y", photo_opts).await;
        std::fs::create_dir_all(p.home_a().join("docs")).unwrap();

        // 列根:探针超时 → 虚拟根隐藏(授权根照旧)
        let root = call(&p.cb, "list", json!({"device": p.dev_a, "path": ""})).await.unwrap();
        let entries = root["entries"].as_array().unwrap();
        assert!(
            entries.iter().all(|e| e["path"].as_str() != Some("/照片图库")),
            "桥缺席列根不得出现照片图库: {root}"
        );
        assert!(entries.iter().any(|e| e["name"].as_str() == Some("docs")));

        // 显式浏览/单查:报不可用(注入窗 1.5s)
        let r = call(&p.cb, "list", json!({"device": p.dev_a, "path": "/照片图库"})).await;
        assert!(r.is_err(), "桥缺席列虚拟根应报错: {r:?}");
        let r = call(&p.cb, "stat", json!({"device": p.dev_a, "path": "/照片图库"})).await;
        assert!(r.is_err(), "桥缺席 stat 虚拟根应报错: {r:?}");

        // 下载编排:源端拒绝(accepted=false → 本地 failed)
        let target = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/照片图库/最近项目/IMG_0001.JPG"],
                "dir": target.path().to_string_lossy(),
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await
        .unwrap();
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.cb, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("failed"), "桥缺席下载终态: {fin}");

        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// photo_serve 关(桌面/Android 形态):列根无探针(零延迟)、`/照片图库`
/// 走授权守卫 INVALID_PATH;变更面拒绝保持(只读文案不依赖开关)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_serve_off_keeps_desktop_semantics() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        // A 关照片桥(守卫仍在,授权根 = docs)
        let p = Pair::new(
            "photo3@x.y",
            |home| DaemonOpts {
                recv_dir: home.join("inbox"),
                tray: false,
                authorized_roots: Some(vec![home.join("docs")]),
                inbox_staging: None,
                inbox_publish: false,
                photo_serve: false,
            },
        )
        .await;
        std::fs::create_dir_all(p.home_a().join("docs")).unwrap();

        // 列根:仅授权根,无虚拟根(photo_serve 关 → 无探针面;时延含
        // 对端首连 punch,不作计时断言)
        let root = call(&p.cb, "list", json!({"device": p.dev_a, "path": ""})).await.unwrap();
        assert!(
            root["entries"].as_array().unwrap().iter().all(|e| e["path"].as_str() != Some("/照片图库")),
            "关形态列根不得出现照片图库: {root}"
        );

        // 显式浏览:授权守卫拒(INVALID_PATH,非「不可用」)
        let r = call(&p.cb, "list", json!({"device": p.dev_a, "path": "/照片图库"})).await;
        let err = r.expect_err("关形态列虚拟根应按集外拒");
        assert_eq!(err.code, 1, "INVALID_PATH 码位: {err:?}");
        assert!(err.message.contains("授权"), "关形态拒因走授权守卫: {err:?}");

        // 变更面:只读拒绝保持(与开关无关)
        let r = call(
            &p.cb,
            "delete",
            json!({"device": p.dev_a, "paths": ["/照片图库/x.JPG"], "force": true}),
        )
        .await;
        assert!(r.expect_err("照片根变更面应拒").message.contains("只读"));

        p.shutdown().await;
    });
    run.await.expect("用例超时");
}
