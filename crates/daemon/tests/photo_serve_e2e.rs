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
//!   一致;任务终态后 `{home}/photo-serve` 引擎清扫为空(R8 终审 I-2:
//!   serve 目录在 `.mtdss` 暂存区命名空间之外);
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

/// 伪照片库:photo_id → (文件名, 原始字节);extra_albums = 标准档外的
/// 相册路由(相册名 → pids,特设用例自注入,不搅标准档的 children 断言)。
struct FakeLibrary {
    photos: HashMap<String, (&'static str, Vec<u8>)>,
    extra_albums: Vec<(String, Vec<String>)>,
}

impl FakeLibrary {
    /// 大相册条数(250 = App 侧单帧帽 200 的截断账面:实收 200 + skipped 50)。
    const BIG_ALBUM_LEN: usize = 250;
    /// 单帧帽(App 侧 PhotoResponder.listingEntryCap 同值;伪库按帽截断)。
    const CAP: usize = 200;

    /// 三张照片 + 大相册 250 张(确定性内容,md5 断言面)+ 越界件(pid-evil,
    /// 0x07 回前缀外路径——引擎拒读面)。
    fn new(dir: &Path) -> Self {
        use std::fmt::Write as _;
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
        // 大相册:确定性内容(逐张独立伪随机,收发 md5 面不依赖)
        for i in 0..Self::BIG_ALBUM_LEN {
            let mut pid = String::new();
            let _ = write!(pid, "pid-big-{i:03}");
            let data = fill_random_file(&dir.join(format!("big-{i:03}.bin")), 1024);
            let mut name = String::new();
            let _ = write!(name, "BIG_{i:03}.JPG");
            photos.insert(pid, (Box::leak(name.into_boxed_str()), data));
        }
        // 越界件:内容随意(引擎在路径门拒读,文件不进传输)
        let evil = fill_random_file(&dir.join("evil.bin"), 1024);
        photos.insert("pid-evil".to_string(), ("EVIL.JPG", evil));
        Self {
            photos,
            extra_albums: Vec::new(),
        }
    }

    /// 同名照片档(R8 终审 I-1):「重名相册」两张同名 DUP.JPG(不同
    /// pid/不同字节)——伪桥直供绕过 App 侧 0x06 批内唯一名收口,钉引擎
    /// 重复 rel 硬失败的契约兜底面(失败干净性);「去重相册」= 收口后
    /// 的线上形态对照(「DUP.JPG」+「DUP (1).JPG」distinct rel,两照片
    /// 逐字节完整交付)。
    fn duplicate_album(dir: &Path) -> Self {
        let d0 = fill_random_file(&dir.join("dup-0.bin"), 8 * 1024);
        let d1 = fill_random_file(&dir.join("dup-1.bin"), 8 * 1024);
        let d2 = fill_random_file(&dir.join("dedup-0.bin"), 8 * 1024);
        let d3 = fill_random_file(&dir.join("dedup-1.bin"), 8 * 1024);
        Self {
            photos: HashMap::from([
                ("pid-dup-0".to_string(), ("DUP.JPG", d0)),
                ("pid-dup-1".to_string(), ("DUP.JPG", d1)),
                ("pid-dedup-0".to_string(), ("DUP.JPG", d2)),
                ("pid-dedup-1".to_string(), ("DUP (1).JPG", d3)),
            ]),
            extra_albums: vec![
                (
                    "重名相册".to_string(),
                    vec!["pid-dup-0".to_string(), "pid-dup-1".to_string()],
                ),
                (
                    "去重相册".to_string(),
                    vec!["pid-dedup-0".to_string(), "pid-dedup-1".to_string()],
                ),
            ],
        }
    }

    /// 相册列表(0x06 根臂;标准档 + 特设注入档)。
    fn albums(&self) -> Vec<Value> {
        let mut out = vec![
            json!({"children": 2, "is_dir": true, "mtime_ms": 1760000000000i64, "name": "最近项目", "size": 0}),
            json!({"children": 1, "is_dir": true, "mtime_ms": 1760000000000i64, "name": "相册B", "size": 0}),
            json!({"children": Self::BIG_ALBUM_LEN, "is_dir": true, "mtime_ms": 1760000000000i64, "name": "大相册", "size": 0}),
            json!({"children": 1, "is_dir": true, "mtime_ms": 1760000000000i64, "name": "恶相册", "size": 0}),
        ];
        for (name, pids) in &self.extra_albums {
            out.push(json!({
                "children": pids.len(), "is_dir": true,
                "mtime_ms": 1760000000000i64, "name": name, "size": 0,
            }));
        }
        out
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
        let serve_dir = home.join("photo-serve");
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
                    let mut truncated = false;
                    let entries: Vec<Value> = match path.as_str() {
                        "" => self.library.albums(),
                        "最近项目" => self.library.album_photos(&["pid-img1", "pid-img2"]),
                        "相册B" => self.library.album_photos(&["pid-img3"]),
                        "恶相册" => self.library.album_photos(&["pid-evil"]),
                        "大相册" => {
                            // 单帧帽(R7 评审 I-2 面):帽截断 + truncated 上达
                            let mut v = self.library.album_photos(
                                &(0..FakeLibrary::BIG_ALBUM_LEN)
                                    .map(|i| {
                                        let mut s = String::new();
                                        use std::fmt::Write as _;
                                        let _ = write!(s, "pid-big-{i:03}");
                                        Box::leak(s.into_boxed_str()) as &str
                                    })
                                    .collect::<Vec<_>>(),
                            );
                            v.truncate(FakeLibrary::CAP);
                            truncated = true;
                            v
                        }
                        _ => {
                            // 特设注入档(相册名 → pids);未命中 = 未知子
                            // 路径拒答(桥「不可用」等价面)
                            match self
                                .library
                                .extra_albums
                                .iter()
                                .find(|(n, _)| n == &path)
                            {
                                Some((_, ids)) => {
                                    let refs: Vec<&str> =
                                        ids.iter().map(String::as_str).collect();
                                    self.library.album_photos(&refs)
                                }
                                None => {
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
                            }
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
                                "truncated": truncated,
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
                    // 导出 = 原始字节落 App 临时目录(请求级唯一名,防在途覆写);
                    // pid-evil(R7 评审 cheap-a 面)= 故意写前缀外并回其路径
                    let dest = if photo_id == "pid-evil" {
                        self.serve_dir
                            .parent()
                            .map(|p| p.join(format!("evil-{request_id}.bin")))
                            .unwrap_or_else(|| self.serve_dir.join(format!("{request_id}-{name}")))
                    } else {
                        self.serve_dir.join(format!("{request_id}-{name}"))
                    };
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
        assert_eq!(photo_entry["children"], json!(4), "根 = 全部相册(含截断账/恶相册测试档)");
        assert!(
            entries.iter().any(|e| e["name"].as_str() == Some("docs")),
            "授权根仍在列: {root}"
        );

        // ② 列相册列表(虚拟根下钻)
        let albums = call(&p.cb, "list", json!({"device": p.dev_a, "path": photo_root}))
            .await
            .unwrap();
        let mut names: Vec<&str> = albums["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["大相册", "恶相册", "最近项目", "相册B"],
            "相册列表(全档在列): {albums}"
        );

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
            assert_eq!(st_root["entry"]["children"], json!(4), "根 children = 全部相册数");
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

        // ⑧ 引擎清扫:任务终态后 photo-serve 无导出残件(R8 终审 I-2:
        // serve 目录 = {home}/photo-serve,暂存区命名空间外)
        let serve_dir = p.home_a().join("photo-serve");
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

/// 截断账(R7 评审 I-2):250 张大相册 → App 侧帽截断实收 200 → 作业
/// done 且 skipped==50(卡片「跳过 50 项」),200 张逐字节落位;
/// children 留真值(250,列根面如实)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_album_truncation_accounts_skipped() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let p = Pair::new("photo4@x.y", photo_opts).await;
        std::fs::create_dir_all(p.home_a().join("docs")).unwrap();
        let library = Arc::new(FakeLibrary::new(&tempfile::tempdir().unwrap().keep()));
        let (app, _log) = MockPhotoApp::spawn(&p.home_a(), library.clone()).await;
        let target = tempfile::tempdir().unwrap();

        // 列照片图库:大相册 children 留真值 250(children 谎言面 = 留真值
        // 的裁定,差额由跳过行解释)
        let albums = call(&p.cb, "list", json!({"device": p.dev_a, "path": "/照片图库"}))
            .await
            .unwrap();
        let big = albums["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"].as_str() == Some("大相册"))
            .unwrap_or_else(|| panic!("照片图库应含大相册: {albums}"));
        assert_eq!(big["children"], json!(250), "children 留真值(跳过行解释差额)");

        // 列大相册:帽截断 200(truncated 经 proto 透传)
        let album = call(
            &p.cb,
            "list",
            json!({"device": p.dev_a, "path": "/照片图库/大相册"}),
        )
        .await
        .unwrap();
        let entries = album["entries"].as_array().unwrap();
        assert_eq!(entries.len(), FakeLibrary::CAP, "帽截断面: {album}");
        assert_eq!(album["truncated"], json!(true), "truncated 上达");

        // 下载整册:done + skipped==50(实收 200,差额入跳过面)
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/照片图库/大相册"],
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
        assert_eq!(fin["state"].as_str(), Some("done"), "截断册终态: {fin}");
        assert_eq!(
            fin["skipped"].as_u64(),
            Some(50),
            "广告 250 − 实收 200 = 跳过 50(静默缩水可见): {fin}"
        );
        // 下载卡 files_total 恒 0(建卡 rels 空,Task 8 面),实收数在
        // files_done
        assert_eq!(fin["files_done"].as_u64(), Some(200), "实收 200 张");

        // 落位面:200 张逐 rel 在(目录段/文件名),抽 3 张 md5 对账
        let dir = target.path().join("大相册");
        let got: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(got.len(), 200, "实收 200 张");
        for i in [0usize, 100, 199] {
            let (_, data) = &library.photos[&format!("pid-big-{i:03}")];
            let landed = std::fs::read(dir.join(format!("BIG_{i:03}.JPG"))).unwrap();
            assert_eq!(&landed, data, "BIG_{i:03} 逐字节一致");
        }

        // 引擎清扫:终态后 photo-serve 零文件({home}/photo-serve,R8 I-2)
        let serve_dir = p.home_a().join("photo-serve");
        let leftover: Vec<_> = std::fs::read_dir(&serve_dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_type().unwrap().is_file())
            .collect();
        assert!(leftover.is_empty(), "引擎终态后应清空导出临时件");

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// 同名照片(R8 终审 I-1)两钉:①相册内两张同名照片(伪桥直供,绕过
/// App 侧 0x06 批内唯一名收口)→ 相册下载整单 failed(文案点名重复
/// rel),**无部分传输泄漏**——失败在编排期(orchestrate accepted=false,
/// 源表物化之前):零 0x07 导出请求、接收端零落盘;②对照面(收口后的
/// 线上形态):桥答去重名「DUP.JPG」+「DUP (1).JPG」→ 相册下载 done,
/// 两照片各自 rel 逐字节落位。生产 App 侧批内唯一名(` (n)` 顺延)后,
/// 引擎硬失败兜底仅对非照片形态的真重复可达,①钉住兜底失败面的干净性
/// (契约存档),②钉收口名走通全链。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_album_duplicate_names_fail_cleanly() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("photo6@x.y", photo_opts).await;
        std::fs::create_dir_all(p.home_a().join("docs")).unwrap();
        let library = Arc::new(FakeLibrary::duplicate_album(
            &tempfile::tempdir().unwrap().keep(),
        ));
        let lib = library.clone();
        let (app, log) = MockPhotoApp::spawn(&p.home_a(), library).await;
        let target = tempfile::tempdir().unwrap();

        // 浏览面照常:两张同名条目如实呈现(去重是 App 侧命名职责,非引擎列表职责)
        let album = call(
            &p.cb,
            "list",
            json!({"device": p.dev_a, "path": "/照片图库/重名相册"}),
        )
        .await
        .unwrap();
        let names: Vec<&str> = album["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["DUP.JPG", "DUP.JPG"], "列表如实呈现同名条目: {album}");

        // ① 相册下载:编排期整单 failed,文案点名重复 rel
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/照片图库/重名相册"],
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
        assert_eq!(fin["state"].as_str(), Some("failed"), "同名相册下载终态: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(err.contains("重复 rel"), "失败文案点名重复 rel: {err}");

        // 无部分传输泄漏:零导出请求(失败先于源表物化)、接收端零落盘
        assert!(
            log.seen_export.lock().unwrap().is_empty(),
            "编排期失败不得触发任何导出请求: {:?}",
            log.seen_export.lock().unwrap()
        );
        let landed: Vec<_> = std::fs::read_dir(target.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(landed.is_empty(), "接收端不得有部分落盘: {landed:?}");

        // ② 对照面:桥答批内唯一名(「DUP.JPG」「DUP (1).JPG」)→ done,
        //    两照片各自 rel 逐字节落位(` (n)` 后缀原样过引擎落接收端)
        let target2 = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/照片图库/去重相册"],
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
        assert_eq!(fin["state"].as_str(), Some("done"), "去重相册下载终态: {fin}");
        let got0 = std::fs::read(target2.path().join("去重相册").join("DUP.JPG")).unwrap();
        let got1 = std::fs::read(target2.path().join("去重相册").join("DUP (1).JPG")).unwrap();
        assert_eq!(&got0, &lib.photos["pid-dedup-0"].1, "首名照片逐字节一致");
        assert_eq!(&got1, &lib.photos["pid-dedup-1"].1, "顺延名照片逐字节一致");

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// 回传路径前缀约束(R7 评审 cheap-a):0x07 path 越出 photo-serve →
/// 作业 failed(拒读),前缀外文件**不被清理**(清理面同前缀约束)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn photo_export_outside_serve_dir_rejected() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("photo5@x.y", photo_opts).await;
        std::fs::create_dir_all(p.home_a().join("docs")).unwrap();
        let library = Arc::new(FakeLibrary::new(&tempfile::tempdir().unwrap().keep()));
        let (app, _log) = MockPhotoApp::spawn(&p.home_a(), library.clone()).await;
        let target = tempfile::tempdir().unwrap();

        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/照片图库/恶相册/EVIL.JPG"],
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
        assert_eq!(fin["state"].as_str(), Some("failed"), "越界回传终态: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("越出照片服务目录"),
            "失败文案点名前缀约束: {err}"
        );

        // 前缀外文件存活(引擎清理不得触碰):serve_dir.parent() = home
        // (R8 终审 I-2:serve 目录 = {home}/photo-serve,home 级兄弟位)
        let evil_path = p
            .home_a()
            .join("photo-serve")
            .parent()
            .map(|p| p.join("evil-0.bin"));
        // 请求 id 0..N 均可能;逐个探测 home/evil-*.bin 任一存活即可
        let home = p.home_a();
        let mut evil_alive = false;
        if let Ok(rd) = std::fs::read_dir(&home) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("evil-") {
                    evil_alive = true;
                }
            }
        }
        assert!(evil_alive, "前缀外文件应存活: {evil_path:?}");
        assert_eq!(
            std::fs::read_dir(p.home_a().join("photo-serve"))
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_type().unwrap().is_file())
                .count(),
            0,
            "前缀内无任何导出件(拒读前即拦)"
        );

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
