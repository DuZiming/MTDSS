//! iOS 布局参数化端到端(v2-P5 T8,spec §5.2):收件暂存区经
//! `DaemonOpts::inbox_staging` 注入(iOS 形态 `{home}/.mtdss`,App 私有;
//! 发布目标 = Documents 由 App 侧发布应答消费),收件箱/授权根 = Documents。
//!
//! 与 inbox_staging_e2e(P4a,默认 `{home}/inbox-staging`)互为参数两型:
//! 本文件钉「参数化不破语义」——
//! - **布局锚**:暂存真实落在注入目录(默认布局目录 `{home}/inbox-staging`
//!   **不得**被创建);传输中暂存有终件、发布根(Documents)**无中间态**;
//! - **发布两步**(App 侧 move+rename 的 host 桩):应答即 move 暂存终件
//!   进 Documents(逐字节一致),完成后暂存空、记录 done+published;
//! - **杀停收纳**:传中优雅杀 B → 残件只在暂存区(`.part`),Documents
//!   零残留;同 home 重启 → reinitiate 位图续传 → 发布 → Documents 本名
//!   md5 一致(T2-① 同构,参数化形态);
//! - **清除回收**:failed「保存失败」态的收件记录 transfer.clear → 暂存
//!   记录+终件回收,Documents 不被触碰(P4a cleanup_inbox 语义,注入形态);
//! - **授权根接线**:authorized_roots=[Documents] → 远端 list Documents
//!   放行、集外(home 根)拒——发布根即可浏览收件箱(spec §3)。
//!
//! fdpass 依赖 Unix 域套接字,整文件 `#![cfg(unix)]`(CI 在 WSL 跑)。

#![cfg(unix)]

use mtdss_core::transport::intake::{record_key, INTAKE_DIR};
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{FdPassClient, KIND_ACK, KIND_DEST_QUERY, KIND_PUBLISH_RESULT};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot};

/// 常规调用超时(transfer.* 除 wait 外都是快操作)。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// transfer.wait 长轮询上界。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);
/// 发布/查询超时注入(不真等 30s/3s)。
const INJECTED_TIMEOUT_MS: u64 = 1500;
/// 发送端重试退避注入(重试轮收敛提速)。
const INJECTED_BACKOFF_MS: u64 = 300;
static ENV_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_env() {
    ENV_INJECT.get_or_init(|| {
        std::env::set_var("MTDSS_TEST_PUBLISH_TIMEOUT_MS", INJECTED_TIMEOUT_MS.to_string());
        std::env::set_var("MTDSS_TEST_QUERY_DEST_TIMEOUT_MS", INJECTED_TIMEOUT_MS.to_string());
        std::env::set_var("MTDSS_TEST_BACKOFF_MS", INJECTED_BACKOFF_MS.to_string());
    });
}

/// 同文件用例并发闸(限并发到 2 防打洞/信令预算被饿穿)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE.clone().acquire_owned().await.expect("semaphore never closed")
}

/// iOS 形态启动选项(spec §5.2):home = 注入 home;暂存 = `{home}/.mtdss`;
/// 发布根/收件箱/授权根 = `{home 父}/Documents`(容器布局:Library 与
/// Documents 同级,桥派生同款规则)。
fn layout_opts(home: &Path) -> DaemonOpts {
    let documents = home.parent().expect("home 必有父(容器)").join("Documents");
    DaemonOpts {
        recv_dir: documents.clone(),
        tray: false,
        authorized_roots: Some(vec![documents]),
        inbox_staging: Some(home.join(".mtdss")),
        // 移动语义钉面(iOS 布局参数化,spec §5.2):哨兵路由 InboxDestIo。
        inbox_publish: true,
    }
}

/// 暂存区(注入形态:`{home}/.mtdss`)。
fn staging(home: &Path) -> PathBuf {
    home.join(".mtdss")
}

/// 发布根(`{home 父}/Documents`)。
fn documents(home: &Path) -> PathBuf {
    home.parent().unwrap().join("Documents")
}

/// 暂存记录 JSON(缺席 → None)。
fn staging_record(home: &Path, rel: &str) -> Option<Value> {
    let p = staging(home)
        .join(INTAKE_DIR)
        .join(format!("{}.json", record_key(rel)));
    let raw = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 记录 final_rel 的暂存终件路径。
fn staged_final(home: &Path, final_rel: &str) -> PathBuf {
    staging(home).join(final_rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// 暂存终件路径(Documents 侧,平台分隔符)。
fn published_final(home: &Path, final_rel: &str) -> PathBuf {
    documents(home).join(final_rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Documents 树内全部文件(中间态断言原料)。
fn documents_files(home: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&documents(home), &mut out);
    out
}

/// 暂存区文件(排除 `.mtdss` 记账目录)。
fn staging_finals(home: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&staging(home), &mut out);
    out.retain(|p| !p.starts_with(staging(home).join(INTAKE_DIR)));
    out
}

/// 暂存记录位图已收块数(hex 位图 → 置位数;1MiB 块)。
fn bitmap_popcount(rec: &Value) -> u64 {
    rec["bitmap"]
        .as_str()
        .map(|hex| {
            hex.chars()
                .map(|c| u8::from_str_radix(&c.to_string(), 16).unwrap().count_ones() as u64)
                .sum()
        })
        .unwrap_or(0)
}

// ---------- 测试台:A 发送端 / B 收件端(iOS 布局)+ 伪发布 App ----------

/// 伪 App 对 fs.publish 的处置。
#[derive(Clone)]
enum PublishMode {
    /// move 暂存终件进 Documents(两步发布的 App 桩)后应答 ok。
    Answer,
    /// 收下请求但不应答(超时路径 → failed 保存失败,暂存保留)。
    Ignore,
    /// 握住不应答,等 SetMode 放行(传输中中间态可观察)。
    Hold,
}

/// 伪 App 共享日志。
#[derive(Default)]
struct AppLog {
    seen: Mutex<Vec<(u64, String, PathBuf, u64)>>,
    answered: Mutex<Vec<(u64, bool)>>,
    /// 已发布 rel → size(0x04 在场应答的事实面)。
    published: Mutex<std::collections::HashMap<String, u64>>,
    queries: Mutex<Vec<(u64, String)>>,
}

enum AppCmd {
    SetMode { mode: PublishMode, reply: oneshot::Sender<()> },
}

/// 伪发布 App(iOS 形态):fs.publish → move 暂存件进 Documents(rel 逐级
/// 建目录)→ 0x05 ok;fs.query_dest → 按 Documents 真实行应答。事件流关闭即退。
struct MockApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    home: PathBuf,
    log: Arc<AppLog>,
    mode: Arc<Mutex<PublishMode>>,
    held: Arc<Mutex<Vec<(u64, String, u64)>>>,
}

impl MockApp {
    async fn spawn(home: &Path, mode: PublishMode) -> (tokio::task::JoinHandle<()>, Arc<AppLog>, mpsc::Sender<AppCmd>) {
        inject_env();
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        let log = Arc::new(AppLog::default());
        let app = Self {
            ipc,
            fdpass,
            home: home.to_path_buf(),
            log: log.clone(),
            mode: Arc::new(Mutex::new(mode)),
            held: Arc::new(Mutex::new(Vec::new())),
        };
        let (cmd_tx, cmd_rx) = mpsc::channel(4);
        let task = tokio::spawn(app.run(cmd_rx));
        (task, log, cmd_tx)
    }

    async fn run(mut self, mut cmd_rx: mpsc::Receiver<AppCmd>) {
        let mut evs = self.ipc.events();
        loop {
            tokio::select! {
                ev = evs.recv() => match ev {
                    Ok(f) => self.handle_event(f).await,
                    Err(broadcast::error::RecvError::Closed) => return,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                },
                cmd = cmd_rx.recv() => match cmd {
                    Some(AppCmd::SetMode { mode, reply }) => {
                        *self.mode.lock().unwrap() = mode;
                        let held: Vec<(u64, String, u64)> =
                            std::mem::take(&mut *self.held.lock().unwrap());
                        for (id, rel, size) in held {
                            self.publish_one(id, &rel, size).await;
                        }
                        let _ = reply.send(());
                    }
                    None => return,
                },
            }
        }
    }

    async fn handle_event(&mut self, ev: mtdss_ipc::EventFrame) {
        if ev.method == "fs.query_dest" {
            return self.handle_query(&ev).await;
        }
        if ev.method != "fs.publish" {
            return;
        }
        let id = ev.params["request_id"].as_u64().expect("request_id u64");
        let rel = ev.params["rel"].as_str().expect("rel 字符串").to_string();
        let path = PathBuf::from(ev.params["staging_path"].as_str().expect("staging_path 字符串"));
        let size = ev.params["size"].as_u64().expect("size u64");
        self.log.seen.lock().unwrap().push((id, rel.clone(), path.clone(), size));
        // 发布素材真实性:暂存件在场且 size 吻合(协议前提)。
        match std::fs::metadata(&path) {
            Ok(m) => assert_eq!(m.len(), size, "fs.publish 的 size 必须与暂存件一致: {path:?}"),
            Err(e) => panic!("fs.publish 的暂存件必须在场: {path:?}: {e}"),
        }
        let mode = self.mode.lock().unwrap().clone();
        match mode {
            PublishMode::Answer => self.publish_one(id, &rel, size).await,
            PublishMode::Ignore => {}
            PublishMode::Hold => self.held.lock().unwrap().push((id, rel, size)),
        }
    }

    /// App 侧两步发布桩:move 暂存终件 → Documents/<rel>(逐级建目录),
    /// 应答 0x05 ok。
    async fn publish_one(&mut self, id: u64, rel: &str, size: u64) {
        let target = documents(&self.home).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("发布目标父目录创建");
        }
        // 见证面:move 前源必须还在暂存区(fs.publish 的 staging_path)。
        let staging_path = self
            .log
            .seen
            .lock()
            .unwrap()
            .iter()
            .find(|(i, r, _, _)| *i == id && *r == rel)
            .expect("发布请求已登记")
            .2
            .clone();
        assert!(
            staging_path.starts_with(staging(&self.home)),
            "发布素材必须来自注入暂存区: {staging_path:?}"
        );
        std::fs::rename(&staging_path, &target).expect("move 暂存件进 Documents");
        let ack = self
            .fdpass
            .send_msg(KIND_PUBLISH_RESULT, json!({ "ok": true, "request_id": id }), None)
            .await
            .expect("0x05 发送应成功");
        assert_eq!(ack.kind, KIND_ACK, "0x05 应答应为 ack: {ack:?}");
        assert_eq!(ack.payload["ok"].as_bool(), Some(true), "0x05 应受理: {ack:?}");
        self.log.answered.lock().unwrap().push((id, true));
        self.log.published.lock().unwrap().insert(rel.to_string(), size);
    }

    /// fs.query_dest 处置:按 Documents 真实行应答(在且 size 吻合 → exists)。
    async fn handle_query(&mut self, ev: &mtdss_ipc::EventFrame) {
        let id = ev.params["request_id"].as_u64().expect("request_id u64");
        let rel = ev.params["rel"].as_str().expect("rel 字符串").to_string();
        let size = ev.params["size"].as_u64().expect("size u64");
        self.log.queries.lock().unwrap().push((id, rel.clone()));
        let target = documents(&self.home).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let actual = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        let exists = actual == size && target.is_file();
        let ack = self
            .fdpass
            .send_msg(
                KIND_DEST_QUERY,
                json!({ "exists": exists, "request_id": id, "size": actual }),
                None,
            )
            .await
            .expect("0x04 发送应成功");
        assert_eq!(ack.kind, KIND_ACK, "0x04 应答应为 ack: {ack:?}");
        assert_eq!(ack.payload["ok"].as_bool(), Some(true), "0x04 应受理: {ack:?}");
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

/// iOS 布局双 daemon 台:A 发送端(默认布局,不相干)、B 收件端(注入布局)。
/// B 的引擎 home = `{container}/home`,发布根 = `{container}/Documents`——
/// 容器 tempdir 由台持有,drop 即整树清理。
struct Pair {
    /// A 的 home tempdir(drop 即删 home——必须握到用例尾,守护进程的
    /// 身份种子/会话在其下,丢弃即「identity seed does not match」)。
    _home_a: tempfile::TempDir,
    /// B 的容器 tempdir(home = `{container}/home` 同理必须握到尾)。
    _container: tempfile::TempDir,
    home_b: PathBuf,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    ca: IpcClient,
    cb: IpcClient,
    dev_b: String,
}

impl Pair {
    /// B 的引擎 home 固定 `{tempdir}/home`(父目录 = 容器根, Documents 与
    /// 之同级——桥派生同款布局)。`pre_tasks` = 启动前直写 tasks.json 的
    /// 记录(clear 用例的记录面)。
    async fn new(email: &str, pre_tasks: &[Value]) -> Self {
        inject_env();
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let container = tempfile::tempdir().unwrap();
        let home_dir = container.path().join("home");
        std::fs::create_dir_all(&home_dir).unwrap();
        if !pre_tasks.is_empty() {
            append_tasks_json(&home_dir, pre_tasks);
        }
        let daemon_a = Daemon::start(
            home_a.path(),
            DaemonOpts {
                recv_dir: home_a.path().join("inbox"),
                tray: false,
                authorized_roots: None,
                inbox_staging: None,
                // 桌面形态:无收件箱发布面(v2-P5 T5b,哨兵回退默认收件目录)
                inbox_publish: false,
            },
        )
        .await
        .unwrap();
        let daemon_b = Daemon::start(&home_dir, layout_opts(&home_dir)).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(&home_dir).await.unwrap();
        login(&ca, &server, email, "A").await;
        login(&cb, &server, email, "B").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let rb = call(&cb, "status", json!({})).await.unwrap();
        let dev_b = rb["device"]["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&ca, &dev_b, true, 30).await;
        Self {
            _home_a: home_a,
            _container: container,
            home_b: home_dir,
            _state: state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_b,
        }
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

async fn call_bounded(client: &IpcClient, method: &str, params: Value, t: Duration) -> Result<Value, RpcError> {
    match tokio::time::timeout(t, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("call {method} 超时({t:?})"),
    }
}

async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    call_bounded(client, method, params, CALL_TIMEOUT).await
}

/// IPC 完整登录:request_code(dev 回显取码)→ verify。
async fn login(client: &IpcClient, server: &str, email: &str, name: &str) {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("code 字符串");
    call(client, "login.verify", json!({ "email": email, "code": code, "name": name }))
        .await
        .expect("login.verify 应成功");
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
            .map(|l| l.iter().any(|d| d["device_id"].as_str() == Some(dev) && d["online"].as_bool() == Some(want)))
            .unwrap_or(false);
        if hit {
            return;
        }
        assert!(Instant::now() < deadline, "devices 未在 {secs}s 内看到 {dev} online={want}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 随机填充并落盘,返回内容副本。
fn fill_random_file(path: &Path, size: usize) -> Vec<u8> {
    use rand::RngCore as _;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(path, &data).unwrap();
    data
}

/// A 侧起收件箱 upload(dir="inbox"),返回 job_id。
async fn start_inbox_upload(client: &IpcClient, files: &[PathBuf], to: &str) -> String {
    let files: Vec<String> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let r = call(
        client,
        "transfer.start",
        json!({ "kind": "upload", "files": files, "to": to, "dir": "inbox", "skip": [], "overwrite": [], "cut": false }),
    )
    .await
    .expect("transfer.start upload(dir=inbox) 应成功");
    r["job_id"].as_str().expect("job_id 字符串").to_string()
}

/// 轮询直至谓词命中(记录面观察;deadline 到即 panic)。
async fn wait_record<F: Fn(&Value) -> bool>(home: &Path, rel: &str, what: &str, pred: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(v) = staging_record(home, rel) {
            if pred(&v) {
                return v;
            }
        }
        assert!(Instant::now() < deadline, "记录面未在预算内满足 {what}: {:?}", staging_record(home, rel));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// B 侧匿名 incoming 卡快照(列表尾向找最新命中卡)。
async fn find_incoming<F: Fn(&Value) -> bool>(cb: &IpcClient, pred: F) -> Option<Value> {
    let lst = call(cb, "transfer.list", json!({})).await.unwrap();
    lst["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|j| j["kind"].as_str() == Some("incoming") && pred(j))
        .cloned()
}

async fn wait_incoming<F: Fn(&Value) -> bool>(cb: &IpcClient, what: &str, pred: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(j) = find_incoming(cb, &pred).await {
            return j;
        }
        assert!(Instant::now() < deadline, "B 侧 incoming 卡未在预算内满足 {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 按 id 找卡。
async fn find_card_by_id(client: &IpcClient, id: &str) -> Option<Value> {
    let lst = call(client, "transfer.list", json!({})).await.ok()?;
    lst["jobs"].as_array()?.iter().find(|j| j["id"].as_str() == Some(id)).cloned()
}

async fn wait_card<F: Fn(&Value) -> bool>(client: &IpcClient, id: &str, what: &str, pred: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(j) = find_card_by_id(client, id).await {
            if pred(&j) {
                return j;
            }
        }
        assert!(Instant::now() < deadline, "卡片 {id} 未在预算内满足 {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn set_mode(cmd: &mpsc::Sender<AppCmd>, mode: PublishMode) {
    let (tx, rx) = oneshot::channel();
    cmd.send(AppCmd::SetMode { mode, reply: tx }).await.unwrap();
    rx.await.unwrap();
}

/// 往 tasks.json 追加记录(B 停机窗独占写;文件缺席自空表起)。
fn append_tasks_json(home: &Path, recs: &[Value]) {
    let path = home.join("tasks.json");
    let mut all: Vec<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    all.extend(recs.iter().cloned());
    std::fs::write(&path, serde_json::to_string_pretty(&all).unwrap()).unwrap();
}

/// 收件 download 记录直写(重放构形;state 写 running 让 load 翻 interrupted)。
fn inbox_download_rec(id: &str, from_dev: &str, to_dev: &str, rels: &[&str], src: &[PathBuf], state: &str) -> Value {
    json!({
        "id": id,
        "kind": "download",
        "name": rels[0],
        "from_dev": from_dev,
        "to_dev": to_dev,
        "src_paths": src.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "target_dir": "inbox",
        "policy_skip": [],
        "policy_overwrite": [],
        "cut": false,
        "rels": rels,
        "state": state,
    })
}

/// reinitiate 的翻新 id 发现(RPC 回 `{}`,差集恰一)。
async fn reinitiate_and_find_new_id(client: &IpcClient, old_id: &str) -> String {
    let before: Vec<String> = call(client, "transfer.list", json!({}))
        .await
        .unwrap()["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|j| j["id"].as_str().map(|s| s.to_string()))
        .collect();
    let r = call(client, "transfer.reinitiate", json!({ "id": old_id }))
        .await
        .expect("transfer.reinitiate 应成功");
    assert_eq!(r, json!({}), "响应面由 golden 钉死");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let lst = call(client, "transfer.list", json!({})).await.unwrap();
        let fresh: Vec<&str> = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|j| j["id"].as_str())
            .filter(|id| !before.iter().any(|b| b == id))
            .collect();
        if fresh.len() == 1 {
            return fresh[0].to_string();
        }
        assert!(Instant::now() < deadline, "翻新新卡未在预算内出现(差集 {fresh:?}): {lst}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------- 用例 ----------

/// 匿名重发可续(sim E2E 的 host 钉面):传中杀 B → 同 home 重启 → **重新
/// upload 同文件(无任务记录,匿名来件)**→ 短推续传收口(不整件重收——
/// A 侧字节显著小于全量)、恰一次发布、记录 done+published。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anonymous_resend_after_kill_resumes_and_publishes() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let p = Pair::new("t8probe@x.y", &[]).await;
        let home = p.home_b.clone();
        let src = tempfile::tempdir().unwrap();
        let rel = "探针 续传.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 64 * 1024 * 1024);

        let (app, _log, _cmd) = MockApp::spawn(&home, PublishMode::Answer).await;
        let id1 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let rec = staging_record(&home, rel);
            if let Some(v) = &rec {
                let n = bitmap_popcount(v);
                if v["done"].as_bool() != Some(true) && (1..=48).contains(&n) {
                    break;
                }
                assert!(v["done"].as_bool() != Some(true) || n > 48, "杀窗未赶上: {v}");
            }
            assert!(Instant::now() < deadline, "记录未出现: {rec:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        call(&p.ca, "transfer.pause", json!({ "id": id1 })).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let received_before = {
            let v = staging_record(&home, rel).unwrap();
            bitmap_popcount(&v) * 1024 * 1024
        };
        assert!(received_before > 0, "续传起点必须已收部分字节: {received_before}");
        let Pair { _home_a, _container, home_b, _state, daemon_a, daemon_b, ca, cb, dev_b } = p;
        drop(cb);
        daemon_b.shutdown().await.expect("杀 B");
        call(&ca, "transfer.cancel", json!({ "id": id1 })).await.unwrap();
        let home = home_b.clone();
        assert!(
            mtdss_core::transport::intake::part_path(&staging(&home), rel).exists(),
            ".part 必须在场(杀停残件)"
        );
        assert!(documents_files(&home).is_empty(), "发布根零残留");

        let daemon_b2 = Daemon::start(&home, layout_opts(&home)).await.expect("重启 B");
        let cb2 = IpcClient::connect(&home).await.expect("重连");
        wait_online(&cb2, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;
        app.abort();
        let (app2, log2, _cmd2) = MockApp::spawn(&home, PublishMode::Answer).await;

        // 重新 upload 同文件(匿名来件,无任务记录):续传/发布收口。
        let id2 = start_inbox_upload(&ca, std::slice::from_ref(&path), &dev_b).await;
        wait_card(&ca, &id2, "A 回显卡", |_| true).await;
        let fin = call_bounded(&ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("A 侧终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "重发终态: {fin}");

        // 短推续传:A 侧推送字节显著小于全量(重收即全量;实测 <40%,钉上界
        // 保守 90% 防调度抖动)。
        let pushed = fin["bytes_done"].as_u64().unwrap_or(0);
        let total = data.len() as u64;
        assert!(
            pushed < total * 9 / 10,
            "重发必须是短推续传(推送 {pushed} / 全量 {total})"
        );
        // 恰一次发布;记录 done+published;暂存 .part 消费;发布根本名逐字
        // 节一致。
        {
            let seen = log2.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 1, "恰一次发布: {seen:?}");
            assert_eq!(seen[0].1, rel);
        }
        let rec = wait_record(&home, rel, "done+published", |v| {
            v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
        })
        .await;
        assert!(
            !mtdss_core::transport::intake::part_path(&staging(&home), rel).exists(),
            ".part 已消费"
        );
        assert!(staging_finals(&home).is_empty());
        let got = std::fs::read(published_final(&home, rel)).expect("读发布终件");
        assert_eq!(got.len(), data.len());
        assert!(got.iter().zip(data.iter()).all(|(a, b)| a == b), "发布终件逐字节一致");

        app2.abort();
        let _ = daemon_b2.shutdown().await;
        let _ = daemon_a.shutdown().await;
    });
    run.await.expect("用例超时");
}


/// 布局锚 + 发布两步全链:Hold 下传输中暂存有终件、Documents 无中间态;
/// 放行后 App move 落位(逐字节一致),暂存空、记录 done+published;默认
/// 布局目录与「暂存入发布根」两漂移形态都不出现;重发走 0x04 在场 Skip。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn layout_full_chain_publishes_into_documents() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("t8layout1@x.y", &[]).await;
        let home = p.home_b.clone();
        let src = tempfile::tempdir().unwrap();

        let f1 = src.path().join("发布 甲.bin");
        let data1 = fill_random_file(&f1, 16 * 1024);
        let nested = src.path().join("嵌套");
        std::fs::create_dir_all(&nested).unwrap();
        let f2 = nested.join("乙.bin");
        let data2 = fill_random_file(&f2, 48 * 1024);
        let files = vec![f1.clone(), nested.clone()];
        let contents = vec![
            ("发布 甲.bin".to_string(), data1),
            ("嵌套/乙.bin".to_string(), data2),
        ];

        let (app, log, cmd) = MockApp::spawn(&home, PublishMode::Hold).await;
        let id = start_inbox_upload(&p.ca, &files, &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("A 侧网络面终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "A 侧网络面: {fin}");

        // 传输中中间态(Hold 窗):两件暂存终件在注入暂存区;发布根零可见。
        for (rel, _) in &contents {
            let rec = wait_record(&home, rel, "暂存 done 记录", |v| v["done"].as_bool() == Some(true)).await;
            let fin_rel = rec["final_rel"].as_str().unwrap().to_string();
            assert!(
                staged_final(&home, &fin_rel).exists(),
                "传输中暂存终件必须在暂存区: {fin_rel}"
            );
        }
        assert!(
            documents_files(&home).is_empty(),
            "传输中发布根必须零中间态: {:?}",
            documents_files(&home)
        );

        // 放行:App move 两步发布 → done。
        set_mode(&cmd, PublishMode::Answer).await;
        wait_incoming(&p.cb, "done 终态", |j| j["state"].as_str() == Some("done")).await;

        // 发布面:恰两次、全 ok;发布根本名逐字节一致;暂存空。
        {
            let seen = log.seen.lock().unwrap().clone();
            let rels: Vec<&str> = seen.iter().map(|(_, r, _, _)| r.as_str()).collect();
            for (rel, _) in &contents {
                assert!(rels.contains(&rel.as_str()), "发布名=final_rel: {rels:?}");
            }
            assert!(log.answered.lock().unwrap().iter().all(|(_, ok)| *ok), "应答全 ok");
        }
        for (rel, data) in &contents {
            let got = std::fs::read(published_final(&home, rel))
                .unwrap_or_else(|e| panic!("读发布终件 {:?}: {e}", published_final(&home, rel)));
            assert_eq!(&got, data, "发布终件逐字节一致: {rel}");
            let rec = wait_record(&home, rel, "done+published", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
            })
            .await;
            assert!(!staged_final(&home, rec["final_rel"].as_str().unwrap()).exists(), "发布后暂存件删除");
        }
        assert!(staging_finals(&home).is_empty(), "暂存区无残留: {:?}", staging_finals(&home));

        // 布局漂移守卫:默认暂存目录不得被创建;暂存不得落进发布根。
        assert!(!home.join("inbox-staging").exists(), "默认布局目录不得存在(参数化必须生效)");
        assert!(!documents(&home).join(".mtdss").exists(), "暂存区不得落进发布根");

        // 重发:0x04 在场(App 按 Documents 实行应答)→ Skip 维持,无新发布。
        let rel0 = contents[0].0.clone();
        let path0 = f1.clone();
        let id2 = start_inbox_upload(&p.ca, std::slice::from_ref(&path0), &p.dev_b).await;
        let fin2 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("重发终态");
        assert_eq!(fin2["state"].as_str(), Some("done"), "重发终态: {fin2}");
        assert_eq!(fin2["files_done"].as_u64(), Some(1), "Skip 计完成: {fin2}");
        let card2 = wait_incoming(&p.cb, "重发 done", |j| j["state"].as_str() == Some("done")).await;
        assert_eq!(card2["bytes_done"].as_u64(), Some(0), "零网络字节(Skip): {card2}");
        {
            let seen = log.seen.lock().unwrap().clone();
            let rel0_publishes = seen.iter().filter(|(_, r, _, _)| *r == rel0).count();
            assert_eq!(rel0_publishes, 1, "Skip 不得再发 fs.publish: {seen:?}");
            let queries = log.queries.lock().unwrap().clone();
            assert!(
                queries.iter().any(|(_, r)| *r == rel0),
                "0x04 在场查询必须发生(锚=发布根实行): {queries:?}"
            );
        }
        assert_eq!(
            std::fs::read(published_final(&home, &rel0)).unwrap().len(),
            contents[0].1.len(),
            "发布根正身不被重发扰动"
        );

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// 杀停收纳 + 续传 + 发布(T2-① 参数化同构):传中杀 B → 残件只在暂存区
/// (Documents 零残留)→ 收件记录直写 + 同 home 重启 → reinitiate 位图续传
/// → 发布 → Documents 本名 md5 一致。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_mid_receive_residue_confined_then_resume_publishes() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(300), async {
        let p = Pair::new("t8layout2@x.y", &[]).await;
        let src = tempfile::tempdir().unwrap();
        let rel = "杀停 续传.bin";
        let path = src.path().join(rel);
        // 64 块 × 1MiB(与 P4a T2-① 同账本):落盘批量 16 块/2s 先到者——
        // 杀窗 1..=48 必接住首个落盘点(24 块小件会 0→16→done 跳过小窗)。
        let data = fill_random_file(&path, 64 * 1024 * 1024);
        let ra = call(&p.ca, "status", json!({})).await.unwrap();
        let dev_a = ra["device"]["device_id"].as_str().unwrap().to_string();
        let home = p.home_b.clone();

        let (app, _log, _cmd) = MockApp::spawn(&home, PublishMode::Answer).await;
        let id1 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        // 杀点:记录出现且 1..=48 块已收、未 done。
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let rec = staging_record(&home, rel);
            if let Some(v) = &rec {
                let n = bitmap_popcount(v);
                if v["done"].as_bool() != Some(true) && (1..=48).contains(&n) {
                    break;
                }
                assert!(v["done"].as_bool() != Some(true) || n > 48, "杀窗未赶上(已 done): {v}");
            }
            assert!(Instant::now() < deadline, "首传记录未在预算内出现: {rec:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        call(&p.ca, "transfer.pause", json!({ "id": id1 })).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await; // 在途块排干窗

        // 杀停收纳断言:残件只在暂存区——发布根零文件;暂存 .part 在场。
        // 解构持有 A home 与容器 tempdir(下划线绑定仍持有值,drop 即整树
        // 删除,必须握到用例尾)。
        let Pair { _home_a, _container, home_b, _state, daemon_a, daemon_b, ca, cb, dev_b } = p;
        drop(cb);
        daemon_b.shutdown().await.expect("杀 B(传中)");
        call(&ca, "transfer.cancel", json!({ "id": id1 })).await.unwrap();

        let home = home_b.clone();
        assert!(
            documents_files(&home).is_empty(),
            "杀停后发布根必须零残留: {:?}",
            documents_files(&home)
        );
        let has_part = mtdss_core::transport::intake::part_path(&staging(&home), rel).exists();
        assert!(
            has_part,
            "杀停后暂存区必须有 .part 残件({{staging}}/.mtdss/<key>.part): {:?}",
            staging(&home).join(INTAKE_DIR)
        );
        let stabilized = staging_record(&home, rel).expect("杀停后记录在场");
        assert_ne!(stabilized["done"].as_bool(), Some(true), "杀点必须传中: {stabilized}");
        let received_before = bitmap_popcount(&stabilized) * 1024 * 1024;
        assert!((1..=64).contains(&(received_before / 1024 / 1024)), "续传起点在窗内: {received_before}");

        // 收件记录直写 + 同 home 重启 → reinitiate 续传。
        append_tasks_json(
            &home,
            &[inbox_download_rec("devB:9001", &dev_a, &dev_b, &[rel], std::slice::from_ref(&path), "running")],
        );
        let daemon_b2 = Daemon::start(&home, layout_opts(&home)).await.expect("B 同 home 重启");
        let cb2 = IpcClient::connect(&home).await.expect("重连 B IPC");
        wait_online(&cb2, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        // 伪 App 重连(旧实例随旧 daemon IPC 关闭退出)。
        app.abort();
        let (app2, log2, _cmd2) = MockApp::spawn(&home, PublishMode::Answer).await;
        let id2 = reinitiate_and_find_new_id(&cb2, "devB:9001").await;
        // A 回显卡先于 wait 在场(wait 对缺席 id 立即报「作业不存在」,T2-①
        // 同款竞窗;编排到达有信令往返)。
        wait_card(&ca, &id2, "A 回显卡", |_| true).await;
        let fin_a = call_bounded(&ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("A 回显卡终态");
        assert_eq!(fin_a["state"].as_str(), Some("done"), "A 侧终态: {fin_a}");
        wait_card(&cb2, &id2, "B done", |j| j["state"].as_str() == Some("done")).await;

        // 发布面:发布名零改名(续传件不重收);Documents 本名逐字节一致;
        // 暂存区空。
        {
            let seen = log2.seen.lock().unwrap().clone();
            let rels: Vec<&str> = seen.iter().map(|(_, r, _, _)| r.as_str()).collect();
            assert_eq!(rels, vec![rel], "恰一次发布、零改名: {seen:?}");
        }
        let got = std::fs::read(published_final(&home, rel)).expect("读发布终件");
        assert_eq!(got.len(), data.len(), "发布终件尺寸一致");
        assert!(
            got.iter().zip(data.iter()).all(|(a, b)| a == b),
            "发布终件逐字节一致(续传无腐坏)"
        );
        assert!(staging_finals(&home).is_empty(), "暂存区无残留: {:?}", staging_finals(&home));

        app2.abort();
        let _ = daemon_b2.shutdown().await;
        let _ = daemon_a.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// 清除回收 + 授权根接线:failed「保存失败」(Ignore 不应答)暂存保留 →
/// 收件记录 clear → 暂存记录+终件回收、发布根不触碰;authorized_roots=
/// [Documents] → 远端 list 发布根放行、home 根(集外)拒。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_reclaims_staging_and_documents_root_is_browsable() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        // 收件记录先于启动直写(clear 的记录面;failed 终态可清)。
        let p = Pair::new(
            "t8layout3@x.y",
            &[inbox_download_rec("devB:9001", "devA-x", "devB-y", &["清除 件.bin"], &[], "failed")],
        )
        .await;
        let home = p.home_b.clone();
        let src = tempfile::tempdir().unwrap();
        let rel = "清除 件.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 16 * 1024);

        // 授权根接线面:B 的发布根放一颗正身,home 根放一颗集外件。
        std::fs::create_dir_all(documents(&home)).unwrap();
        std::fs::write(documents(&home).join("browsable.txt"), b"ok").unwrap();
        std::fs::write(home.join("secret.txt"), b"x").unwrap();

        let (app, _log, _cmd) = MockApp::spawn(&home, PublishMode::Ignore).await;
        let id = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("A 侧网络面终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "网络面终态: {fin}");
        wait_incoming(&p.cb, "failed 保存失败", |j| {
            j["state"].as_str() == Some("failed")
                && j["error"].as_str().unwrap_or_default().contains("保存失败")
        })
        .await;
        let rec = wait_record(&home, rel, "done 未发布记录", |v| {
            v["done"].as_bool() == Some(true) && v["published"].as_bool() != Some(true)
        })
        .await;
        let staged = staged_final(&home, rec["final_rel"].as_str().unwrap());
        assert_eq!(std::fs::read(&staged).unwrap(), data, "暂存终件保留(失败保留面)");

        // ---- 授权根:发布根可远端浏览;home 根(集外)拒。 ----
        let browse = |target: String| {
            let ca = &p.ca;
            let dev = p.dev_b.clone();
            async move { call(ca, "list", json!({ "device": dev, "path": target })).await }
        };
        let docs_path = documents(&home).to_string_lossy().into_owned();
        let ls = browse(docs_path).await.expect("发布根(授权集内)应可浏览");
        let names: Vec<String> = ls["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(names.contains(&"browsable.txt".to_string()), "发布根条目在列: {ls}");
        let err = browse(home.to_string_lossy().into_owned())
            .await
            .expect_err("home 根(集外)必须被拒");
        assert!(
            err.message.contains("授权") || err.message.contains("INVALID_PATH") || err.code != 0,
            "集外路径拒绝: {err:?}"
        );

        // ---- 清除:failed 收件记录 clear → 暂存回收,发布根不触碰。 ----
        call(&p.cb, "transfer.clear", json!({ "id": "devB:9001" }))
            .await
            .expect("failed 记录应可清除");
        assert!(
            staging_record(&home, rel).is_none(),
            "清除后暂存记录必须回收"
        );
        assert!(!staged.exists(), "清除后暂存终件必须回收: {staged:?}");
        assert!(
            documents_files(&home)
                == vec![documents(&home).join("browsable.txt")],
            "清除不得触碰发布根: {:?}",
            documents_files(&home)
        );

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}
