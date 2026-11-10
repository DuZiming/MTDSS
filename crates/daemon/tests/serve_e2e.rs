//! 服务桥端到端(V3-4 §4.1 安卓服务桥):host 双 daemon + 伪 App 服务桥
//! 应答器(真实 IpcClient 订阅 `fs.serve_list` / `fs.serve_export` /
//! `fs.serve_delete` + FdPassClient 回 `0x08 serve_list` / `0x09
//! serve_export` / `0x0A serve_delete`)→ roots.set 注册 bridge 根 →
//! 对端列根见「收件箱」(隐藏根不见)→ 下钻列表 → 下载(引擎 0x09 导出
//! `{home}/serve-tmp/` 临时件 → 读发 → 终态 unlink)→ 逐字节一致 →
//! 删除后列表缺行;对桥根的写/移/复制拒(只读来源);桥死 → 根隐藏 +
//! 浏览/删除诚实报错。fdpass 依赖 Unix 域套接字,整文件 cfg(unix)
//! (CI 在 WSL 跑;Windows 编译单元整体缺席)。
//!
//! 断言面(简报测试矩阵):
//! - roots.set 注册(可见 收件箱 + 隐藏 暂存区)→ 列根见可见桥根
//!   (children 惰性 None)、不见隐藏根;隐藏根 vpath 仍可路由(可授权
//!   而不可见);
//! - 下钻列表 = SAF 树条目(名/尺寸/合成 vpath);stat 子路径/根自身;
//! - 下载整目录(任意深度递归展开)与单文件 → 接收端逐 rel 逐字节一致;
//!   任务终态后 `{home}/serve-tmp` 引擎清扫为空;
//! - delete 桥 vpath → App 侧整树删 → 列表缺行;
//! - Move/Copy 触桥根 → 只读拒;
//! - 桥缺席:列根不含桥根、list/stat 报「不可用」、delete 逐条诚实失败
//!   (批不半途弃废)、下载编排 failed。

#![cfg(unix)]

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{
    FdPassClient, KIND_ACK, KIND_SERVE_DELETE, KIND_SERVE_EXPORT, KIND_SERVE_LIST,
};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时(下载编排含导出与真传输,留余量)。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// transfer.wait 长轮询上界。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);
/// 伪 App 事件等待上界。
const APP_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// 超时注入(生产 list/delete 10s / export 60s,不真等):桥缺席用例的
/// 列根探针/浏览/删除失败都吃这个窗,注入 1500ms。
const INJECTED_TIMEOUT_MS: u64 = 1500;
static TIMEOUT_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_timeout() {
    TIMEOUT_INJECT.get_or_init(|| {
        std::env::set_var("MTDSS_TEST_SERVE_TIMEOUT_MS", INJECTED_TIMEOUT_MS.to_string());
    });
}

/// 用例并发闸(两用例同时跑打洞/信令,2 worker 足够)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE.clone().acquire_owned().await.expect("semaphore never closed")
}

/// 桌面形态(两侧同形;A 侧 bridge 根经 roots.set 运行时注册)。
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

/// V3-1 审批状态机:任何上线发生前 REST 预置一台设备行(预生成身份落 seed,
/// daemon IPC 登录幂等收敛同一 device_id),登录恒走老终端直入。
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
    assert_eq!(v["status"], "ok", "预置注册必须直入 ok(夹具在任何上线前调用): {v}");
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

/// 进程内双 daemon 测试台(A = bridge 根形态经 roots.set;B = 对端)。
struct Pair {
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    ca: IpcClient,
    cb: IpcClient,
    dev_a: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        inject_timeout();
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        preregister_identity(addr, home_a.path(), email, "A").await;
        preregister_identity(addr, home_b.path(), email, "B").await;
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
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
        wait_peer_online(&cb, &dev_a, true, 30).await;
        Self {
            _home_a: home_a,
            _home_b: home_b,
            _state: state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_a,
        }
    }

    fn home_a(&self) -> PathBuf {
        self._home_a.path().to_path_buf()
    }

    /// A 侧 roots.set 注册标准档:可见 收件箱 + 隐藏 暂存区。
    async fn register_serve_roots(&self) {
        let r = call(
            &self.ca,
            "roots.set",
            json!({
                "roots": [
                    {"name": "收件箱", "path": "/收件箱", "source": "bridge", "visible": true},
                    {"name": "暂存", "path": "/暂存区", "source": "bridge", "visible": false},
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
}

// ---------- 伪 SAF(内存树;list/export/delete 三原语) ----------

/// 伪 SAF 树:files = 完整 vpath → 字节;dirs = 完整 vpath 集(目录)。
/// 根自身隐含(roots.set 注册的 vpath 即根)。
#[derive(Clone, Default)]
struct FakeSaf {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    dirs: Arc<Mutex<HashSet<String>>>,
}

impl FakeSaf {
    /// 标准档:两级子树 + 深层嵌套(任意深度递归展开面)+ 隐藏根文件。
    /// 返回 (树, 内容表;键 = 完整 vpath)。
    fn standard(scratch: &Path) -> (Self, BTreeMap<String, Vec<u8>>) {
        let mut contents = BTreeMap::new();
        for (full, size) in [
            ("/收件箱/报告.pdf", 16 * 1024usize),
            ("/收件箱/说明.txt", 4 * 1024),
            ("/收件箱/子目录/图.png", 24 * 1024),
            ("/收件箱/子目录/深层/嵌套.bin", 8 * 1024),
            // 隐藏根内文件(可授权而不可见:不在对端根列表,直接路由可达)
            ("/暂存区/机密.pdf", 2 * 1024),
        ] {
            let data = fill_random_file(&scratch.join(format!("src-{}.bin", contents.len())), size);
            contents.insert(full.to_string(), data);
        }
        let mut files = HashMap::new();
        for (k, v) in &contents {
            files.insert(k.clone(), v.clone());
        }
        let dirs: HashSet<String> = ["/收件箱/子目录", "/收件箱/子目录/深层", "/暂存区"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        (
            Self {
                files: Arc::new(Mutex::new(files)),
                dirs: Arc::new(Mutex::new(dirs)),
            },
            contents,
        )
    }

    /// 0x08 列表(直接子项;名序,目录 children = 直接子项数)。
    fn list(&self, root: &str, sub: &str) -> Option<Vec<Value>> {
        let dir_path = if sub.is_empty() {
            root.to_string()
        } else {
            format!("{root}/{sub}")
        };
        let prefix = format!("{dir_path}/");
        let files = self.files.lock().unwrap();
        let dirs = self.dirs.lock().unwrap();
        // 名 → (is_dir, size);children 按目录前缀现算
        let mut names: BTreeMap<String, (bool, u64)> = BTreeMap::new();
        for k in files.keys() {
            if let Some(rest) = k.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') {
                    names.insert(rest.to_string(), (false, files[k].len() as u64));
                }
            }
        }
        for d in dirs.iter() {
            if let Some(rest) = d.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.contains('/') {
                    names.insert(rest.to_string(), (true, 0));
                }
            }
        }
        let out = names
            .into_iter()
            .map(|(name, (is_dir, size))| {
                let mut e = json!({
                    "is_dir": is_dir, "mtime_ms": 1760000000000i64,
                    "name": name, "size": size,
                });
                if is_dir {
                    let dprefix = format!("{dir_path}/{name}/");
                    // 直接子项(不含更深层):strip 前缀后无 '/'
                    let children = files
                        .keys()
                        .filter_map(|k| k.strip_prefix(&dprefix))
                        .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                        .count()
                        + dirs
                            .iter()
                            .filter_map(|x| x.strip_prefix(&dprefix))
                            .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                            .count();
                    e["children"] = json!(children);
                }
                e
            })
            .collect();
        Some(out)
    }

    /// 0x09 导出寻址:文件字节。
    fn bytes(&self, root: &str, sub: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(&format!("{root}/{sub}")).cloned()
    }

    /// 0x0A 删除:整树删(文件 + 目录),返回是否有任何删除。
    fn delete(&self, root: &str, sub: &str) -> bool {
        let target = if sub.is_empty() {
            root.to_string()
        } else {
            format!("{root}/{sub}")
        };
        let prefix = format!("{target}/");
        let mut files = self.files.lock().unwrap();
        let mut dirs = self.dirs.lock().unwrap();
        let before = files.len() + dirs.len();
        files.retain(|k, _| !(k == &target || k.starts_with(&prefix)));
        dirs.retain(|d| !(d == &target || d.starts_with(&prefix)));
        files.len() + dirs.len() != before
    }
}

/// 伪 App 日志(用例体断言数据源)。
#[derive(Default)]
struct ServeLog {
    seen_list: Mutex<Vec<(u64, String, String)>>,
    seen_export: Mutex<Vec<(u64, String, String)>>,
    seen_delete: Mutex<Vec<(u64, String, String)>>,
}

/// 伪 App:订阅 fs.serve_* → 查伪 SAF / 写导出临时件 → FdPassClient 回
/// 0x08/0x09/0x0A。事件流关闭(daemon 收摊)即退。
struct MockServeApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    serve_dir: PathBuf,
    saf: FakeSaf,
    log: Arc<ServeLog>,
}

impl MockServeApp {
    async fn spawn(home: &Path, saf: FakeSaf) -> (tokio::task::JoinHandle<()>, Arc<ServeLog>) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        let serve_dir = home.join("serve-tmp");
        std::fs::create_dir_all(&serve_dir).unwrap();
        let log = Arc::new(ServeLog::default());
        let app = Self {
            ipc,
            fdpass,
            serve_dir,
            saf,
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
                "fs.serve_list" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let root = ev.params["root"].as_str().unwrap().to_string();
                    let sub = ev.params["sub"].as_str().unwrap().to_string();
                    self.log
                        .seen_list
                        .lock()
                        .unwrap()
                        .push((request_id, root.clone(), sub.clone()));
                    let Some(entries) = self.saf.list(&root, &sub) else {
                        let _ = self
                            .fdpass
                            .send_msg(
                                KIND_SERVE_LIST,
                                json!({"error": "无此目录", "ok": false, "request_id": request_id}),
                                None,
                            )
                            .await;
                        continue;
                    };
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_SERVE_LIST,
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
                        "0x08 应受理(ack): {ack:?}"
                    );
                }
                "fs.serve_export" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let root = ev.params["root"].as_str().unwrap().to_string();
                    let sub = ev.params["sub"].as_str().unwrap().to_string();
                    self.log
                        .seen_export
                        .lock()
                        .unwrap()
                        .push((request_id, root.clone(), sub.clone()));
                    let Some(data) = self.saf.bytes(&root, &sub) else {
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
                    // 导出 = 原始字节落 App 临时目录(请求级唯一名)
                    let name = sub.rsplit('/').next().unwrap_or(&sub);
                    let dest = self.serve_dir.join(format!("{request_id}-{name}"));
                    std::fs::write(&dest, &data).unwrap();
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_SERVE_EXPORT,
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
                        "0x09 应受理(ack): {ack:?}"
                    );
                }
                "fs.serve_delete" => {
                    let request_id = ev.params["request_id"].as_u64().unwrap();
                    let root = ev.params["root"].as_str().unwrap().to_string();
                    let sub = ev.params["sub"].as_str().unwrap().to_string();
                    self.log
                        .seen_delete
                        .lock()
                        .unwrap()
                        .push((request_id, root.clone(), sub.clone()));
                    let deleted = self.saf.delete(&root, &sub);
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_SERVE_DELETE,
                            json!({
                                "deleted": deleted,
                                "ok": true,
                                "request_id": request_id,
                            }),
                            None,
                        )
                        .await;
                    assert_eq!(
                        ack.as_ref().map(|a| a.kind).ok(),
                        Some(KIND_ACK),
                        "0x0A 应受理(ack): {ack:?}"
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

/// 全链:roots.set → 列根(可见桥根 children None/隐藏根不见)→ 下钻列
/// 表(合成 vpath)→ stat → 下载整目录(任意深度)+ 单文件逐字节一致 →
/// 引擎清扫导出临时件 → delete 后列表缺行 → 变更面拒绝 → 隐藏根路由
/// 可达。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_bridge_full_chain() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let p = Pair::new("serve1@x.y").await;
        p.register_serve_roots().await;
        let (saf, contents) = FakeSaf::standard(&tempfile::tempdir().unwrap().keep());
        let (app, log) = MockServeApp::spawn(&p.home_a(), saf.clone()).await;

        // ① 列根:可见桥根在列(is_dir、children 惰性 None),隐藏根不见
        let root = call(&p.cb, "list", json!({"device": p.dev_a, "path": ""})).await.unwrap();
        let entries = root["entries"].as_array().unwrap();
        let inbox = entries
            .iter()
            .find(|e| e["path"].as_str() == Some("/收件箱"))
            .unwrap_or_else(|| panic!("列根应含可见桥根 /收件箱: {root}"));
        assert_eq!(inbox["is_dir"], json!(true));
        assert!(
            inbox.get("children").map(|c| c.is_null()).unwrap_or(true),
            "桥根条目 children 惰性 None: {inbox}"
        );
        assert!(
            entries.iter().all(|e| e["path"].as_str() != Some("/暂存区")),
            "隐藏桥根不得出现在对端根列表: {root}"
        );

        // ② 下钻列表:条目 = SAF 直接子项,路径 = 合成 vpath(可直接再下钻)
        let l1 = call(&p.cb, "list", json!({"device": p.dev_a, "path": "/收件箱"}))
            .await
            .unwrap();
        let names: Vec<&str> = l1["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["子目录", "报告.pdf", "说明.txt"], "目录优先+名序: {l1}");
        let report = &l1["entries"][1];
        assert_eq!(report["path"], json!("/收件箱/报告.pdf"));
        assert_eq!(report["size"], json!(contents["/收件箱/报告.pdf"].len() as u64));
        let sub = &l1["entries"][0];
        assert_eq!(sub["children"], json!(2), "子目录直接子项 = 2(图.png + 深层): {sub}");

        // ③ 深层下钻(三级)
        let l3 = call(
            &p.cb,
            "list",
            json!({"device": p.dev_a, "path": "/收件箱/子目录/深层"}),
        )
        .await
        .unwrap();
        let names: Vec<&str> = l3["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["嵌套.bin"]);

        // ④ stat:子路径(列父目录找条目)与根自身(探活合成)
        let st = call(
            &p.cb,
            "stat",
            json!({"device": p.dev_a, "path": "/收件箱/报告.pdf"}),
        )
        .await
        .unwrap();
        assert_eq!(st["entry"]["size"], json!(contents["/收件箱/报告.pdf"].len() as u64));
        let st = call(&p.cb, "stat", json!({"device": p.dev_a, "path": "/收件箱"}))
            .await
            .unwrap();
        assert_eq!(st["entry"]["is_dir"], json!(true));
        // 隐藏根 vpath 照常路由(可授权而不可见)
        let st = call(
            &p.cb,
            "stat",
            json!({"device": p.dev_a, "path": "/暂存区/机密.pdf"}),
        )
        .await
        .unwrap();
        assert_eq!(
            st["entry"]["size"],
            json!(contents["/暂存区/机密.pdf"].len() as u64),
            "隐藏根 stat 应照常路由: {st}"
        );

        // ⑤ 下载整目录(递归展开:根输入 /收件箱 → rel = 收件箱/...)
        let target = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/收件箱"],
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
        assert_eq!(fin["state"].as_str(), Some("done"), "整目录下载终态: {fin}");
        // 逐 rel 逐字节一致(目录行不入清单——空子树不发行,photo 先例;
        // 深层文件按内部相对路径落位)
        for (vpath, data) in &contents {
            if !vpath.starts_with("/收件箱/") {
                continue;
            }
            let rel = vpath.trim_start_matches('/');
            let landed = target.path().join(rel);
            assert_eq!(
                &std::fs::read(&landed)
                    .unwrap_or_else(|e| panic!("读 {landed:?} 失败: {e}")),
                data,
                "{rel} 逐字节一致"
            );
        }
        // ⑥ 下载单文件(rel = 文件名)
        let target2 = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/收件箱/子目录/图.png"],
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
        assert_eq!(fin["state"].as_str(), Some("done"), "单文件下载终态: {fin}");
        assert_eq!(
            std::fs::read(target2.path().join("图.png")).unwrap(),
            contents["/收件箱/子目录/图.png"],
            "单文件逐字节一致"
        );

        // ⑦ 引擎清扫:任务终态后 serve-tmp 无导出残件
        let serve_dir = p.home_a().join("serve-tmp");
        let leftover: Vec<_> = std::fs::read_dir(&serve_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(leftover.is_empty(), "引擎终态后应清空导出临时件: {leftover:?}");
        assert!(!log.seen_export.lock().unwrap().is_empty(), "下载应触发导出请求");

        // ⑧ delete:App 侧整树删 → 列表缺行
        let r = call(
            &p.cb,
            "delete",
            json!({"device": p.dev_a, "paths": ["/收件箱/报告.pdf"], "force": true}),
        )
        .await
        .unwrap();
        assert_eq!(r["deleted"], json!(1), "桥删除应计 1: {r}");
        assert_eq!(r["failed"].as_array().map(Vec::len), Some(0), "无逐条失败: {r}");
        let l1 = call(&p.cb, "list", json!({"device": p.dev_a, "path": "/收件箱"}))
            .await
            .unwrap();
        let names: Vec<&str> = l1["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["子目录", "说明.txt"], "删除后列表缺行: {l1}");
        // 再删同一路径:App 明言未删(deleted:false)→ NOT_FOUND 逐条失败
        let r = call(
            &p.cb,
            "delete",
            json!({"device": p.dev_a, "paths": ["/收件箱/报告.pdf"], "force": true}),
        )
        .await
        .unwrap();
        assert_eq!(r["deleted"], json!(0));
        let failed = r["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["code"], json!("NOT_FOUND"), "缺席删除 = NOT_FOUND: {r}");

        // ⑨ 变更面拒绝(只读来源;delete 唯一例外已证)
        let r = call(
            &p.cb,
            "move",
            json!({
                "device": p.dev_a,
                "pairs": [{"from": "/收件箱/说明.txt", "to": "/收件箱/改名.txt"}],
            }),
        )
        .await;
        assert!(r.expect_err("对桥根 Move 应拒").message.contains("只读"));
        let r = call(
            &p.cb,
            "copy",
            json!({
                "device": p.dev_a,
                "paths": ["/收件箱/说明.txt"],
                "dir": "/tmp-serve-dst",
            }),
        )
        .await;
        assert!(r.expect_err("对桥根 Copy 应拒").message.contains("只读"));

        // ⑩ 桥死:App 应答器退出 → 根消失 + 浏览/删除诚实报错
        app.abort();
        // 等伪 App 的 IPC 连接真正落下(receiver_count 归零或探针超时)
        tokio::time::sleep(Duration::from_millis(300)).await;
        let root = call(&p.cb, "list", json!({"device": p.dev_a, "path": ""})).await.unwrap();
        assert!(
            root["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["path"].as_str() != Some("/收件箱")),
            "桥死列根不得出现桥根(探针超时隐藏): {root}"
        );
        let r = call(&p.cb, "list", json!({"device": p.dev_a, "path": "/收件箱"})).await;
        let err = r.expect_err("桥死浏览应报错");
        assert!(
            err.message.contains("服务目录不可用"),
            "桥死浏览诚实报错: {err:?}"
        );
        let r = call(
            &p.cb,
            "delete",
            json!({"device": p.dev_a, "paths": ["/收件箱/说明.txt"], "force": true}),
        )
        .await
        .unwrap();
        assert_eq!(r["deleted"], json!(0), "桥死删除计 0: {r}");
        assert_eq!(
            r["failed"].as_array().map(Vec::len),
            Some(1),
            "桥死删除逐条诚实失败: {r}"
        );

        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// 桥缺席(应答器从未起):列根不含桥根(探针超时隐藏)、浏览/单查报
/// 「不可用」、删除逐条诚实失败、下载编排 failed(源端解析失败)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_bridge_absent_degrades() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("serve2@x.y").await;
        p.register_serve_roots().await;

        // 列根:探针超时(注入窗 1.5s)→ 桥根隐藏
        let root = call(&p.cb, "list", json!({"device": p.dev_a, "path": ""})).await.unwrap();
        assert!(
            root["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["path"].as_str() != Some("/收件箱")),
            "桥缺席列根不得出现桥根: {root}"
        );

        // 浏览/单查:报不可用(不挂死)
        for method in ["list", "stat"] {
            let r = call(&p.cb, method, json!({"device": p.dev_a, "path": "/收件箱"})).await;
            let err = r.expect_err("桥缺席浏览应报错");
            assert!(
                err.message.contains("服务目录不可用"),
                "{method} 诚实报错: {err:?}"
            );
        }

        // 删除:批语义 ok + 逐条诚实失败
        let r = call(
            &p.cb,
            "delete",
            json!({"device": p.dev_a, "paths": ["/收件箱/x.pdf"], "force": true}),
        )
        .await
        .unwrap();
        assert_eq!(r["deleted"], json!(0));
        assert_eq!(r["failed"].as_array().map(Vec::len), Some(1));

        // 下载编排:源端解析失败(桥不可达)→ 操作端 failed
        let target = tempfile::tempdir().unwrap();
        let r = call(
            &p.cb,
            "transfer.start",
            json!({
                "kind": "download", "device": p.dev_a,
                "paths": ["/收件箱/报告.pdf"],
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
        // 接收端零落盘
        let landed: Vec<_> = std::fs::read_dir(target.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(landed.is_empty(), "桥缺席不得有部分落盘: {landed:?}");

        p.shutdown().await;
    });
    run.await.expect("用例超时");
}
