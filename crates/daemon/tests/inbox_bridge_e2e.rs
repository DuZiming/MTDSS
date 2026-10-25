//! 收件箱桥接端到端(v2-P3 Task 6):host 双 daemon + 伪 provider(真实
//! IpcClient 订阅 `fs.request_dest` + FdPassClient 回 `0x02 dest_fd`)→
//! A `transfer.start {kind:upload, files, to, dir:"inbox"}` → B(收件箱侧)
//! 经 BridgeDestIo 落 provider 暂存。fdpass 依赖 Unix 域套接字,整文件
//! cfg(unix)(CI 在 WSL 跑;Windows 编译单元整体缺席,桌面回归面不受
//! 影响)。
//!
//! 拓扑注:实机收件箱是 android 端 download(dir="inbox")——本 e2e 以
//! **发送端 upload 直指 dir="inbox"** 驱动同一接收面(ConnectRequest
//! target_dir="inbox" → B 的 resident loop 哨兵路由 → BridgeDestIo);
//! download 侧的哨兵放行是 android cfg 分支(桌面拒绝面由契约 golden
//! `transfer.start.download-inbox` 钉死),不在 host 可跑面。
//!
//! 断言面(任务简报四支):
//! - **① 全新收**:provider 应 fd=临时暂存文件 → 内容逐字节 + 记录
//!   (`{home}/inbox-records`,done/final_rel/位图)+ `0x03 dest_commit
//!   done:true` 恰每文件一次;`.mtdss` 键空间不被触碰(防串台);
//! - **② 中断续传**:暂停冻结 + 取消中断(记录已落 ≥N 块位)→ 重新
//!   发起 → provider 应 resume=true 重开同一暂存(重开时已完整在场
//!   的首部块数 ≥N)→ bitmap 续传(缺块集补齐)→ done:true;中断轮
//!   的已交付未提交请求收 `0x03 done:false`;
//! - **③ resume 撞 NOT_FOUND**:预置部分记录(provider 暂存已失)→
//!   provider 对 resume 请求答 ok:false → daemon 清陈旧记录 → 轮失败
//!   → 发送端重试轮对账无记录 → **FULL 重收**(provider 见
//!   resume=false 请求 + 全新暂存)→ done:true;
//! - **④ request_dest 超时**(注入 1500ms,不真等 10s):provider 不
//!   应答 → 任务 failed(B 侧匿名卡 error 含「目的地不可用」「超时」);
//!   迟到 0x02 应答按未知 request_id 拒(ack ok:false),fd 就地释放。

#![cfg(unix)]

use mtdss_core::transport::intake::record_key;
use mtdss_core::transport::transfer::CHUNK_SIZE;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{FdPassClient, KIND_ACK, KIND_DEST_COMMIT, KIND_DEST_FD};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use sha2::Digest as _;
use signaling::spawn_test_server;
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot};

/// 常规调用超时(transfer.* 除 wait 外都是快操作)。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// transfer.wait 长轮询上界(④ 的重试收敛 ~20s,留足余量)。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);
/// fd 计数回落的轮询上界。
const FD_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// 超时注入(生产 10s,不真等):`MTDSS_TEST_REQUEST_DEST_TIMEOUT_MS`(沿
/// MTDSS_TEST_* 家族口径,daemon 构造期一次读定)。全部用例共享测试进程,
/// 首例起跑前经 OnceLock 一次性设置。
const INJECTED_TIMEOUT_MS: u64 = 1500;
/// 发送端重试退避注入(`MTDSS_TEST_BACKOFF_MS`,flow 既有旋钮):③④ 的
/// 重试轮收敛提速(默认 1s 基数 × 2^n 会让 ④ 拖到 ~40s)。
const INJECTED_BACKOFF_MS: u64 = 300;
static ENV_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_env() {
    ENV_INJECT.get_or_init(|| {
        std::env::set_var(
            "MTDSS_TEST_REQUEST_DEST_TIMEOUT_MS",
            INJECTED_TIMEOUT_MS.to_string(),
        );
        std::env::set_var("MTDSS_TEST_BACKOFF_MS", INJECTED_BACKOFF_MS.to_string());
    });
}

/// 同文件用例并发闸(限并发到 2 防打洞/信令预算被饿穿)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE
        .clone()
        .acquire_owned()
        .await
        .expect("semaphore never closed")
}

/// 临时 home + 显式 recv_dir 的启动选项(不碰真实 mtdss_home)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
    }
}

/// 限时 IPC 调用;超时视为测试失败(暴露悬挂)。
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

/// 常规限时调用。
async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    call_bounded(client, method, params, CALL_TIMEOUT).await
}

/// IPC 完整登录:request_code(dev 回显取码)→ verify。
async fn ipc_login(client: &IpcClient, server: &str, email: &str, name: &str) {
    let code = call(
        client,
        "login.request_code",
        json!({ "server": server, "email": email }),
    )
    .await
    .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("result 必含 code 字符串");
    call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
    .expect("login.verify 应成功");
}

/// 轮询 status.online 直到 want(在线循环异步起连)。
async fn wait_online(client: &IpcClient, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = call(client, "status", json!({})).await.unwrap();
        if s["online"].as_bool() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "status.online 未在 {secs}s 内到 {want}: {s}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 轮询 devices 快照直到 peer 的 online 等于 want。
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
        assert!(
            Instant::now() < deadline,
            "devices 快照未在 {secs}s 内看到 {dev} online={want}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 随机填充并落盘,返回内容副本(收发两端字节一致性断言用)。
fn fill_random_file(path: &Path, size: usize) -> Vec<u8> {
    use rand::RngCore as _;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(path, &data).unwrap();
    data
}

/// 指向任一暂存文件的进程 fd 计数(/proc/self/fd 逐条 readlink;进程内
/// daemon + 测试共用同一 fd 表)。
fn fds_at(paths: &[PathBuf]) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("读 /proc/self/fd")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let target = std::fs::read_link(e.path()).ok();
            matches!(target, Some(t) if paths.contains(&t))
        })
        .count()
}

/// 轮询直至指向暂存文件的 fd 计数归零(终态后句柄 Drop 有微小滞后)。
async fn wait_fds_released(paths: &[PathBuf]) {
    let deadline = Instant::now() + FD_SETTLE_TIMEOUT;
    loop {
        let n = fds_at(paths);
        if n == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "任务终态后暂存 fd 未释放(泄漏 {n} 条)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 进程内双 daemon 测试台:A = 发送端(源文件 + upload 作业),B = 收件
/// 箱侧(伪 provider 挂 B 的 home)。双方已登录且 A 已见 B 在线。
struct Pair {
    _home_a: tempfile::TempDir,
    home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    ca: IpcClient,
    cb: IpcClient,
    dev_b: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        inject_env();
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        ipc_login(&ca, &server, email, "A").await;
        ipc_login(&cb, &server, email, "B").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let rb = call(&cb, "status", json!({})).await.unwrap();
        let dev_b = rb["device"]["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&ca, &dev_b, true, 30).await;
        Self {
            _home_a: home_a,
            home_b,
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

// ---------- 伪 provider(镜像 Kotlin T9 InboxProvider 的职责面) ----------

/// 伪 provider 对单个 fs.request_dest 的处置。
#[derive(Clone)]
enum DestStep {
    /// 打开暂存文件回 fd(resume 按请求标志重开同一文件/全新截断)。
    Answer,
    /// 拒开:0x02 无 fd,payload 带 ok:false + error 文案(实机「暂存已
    /// 失/NOT_FOUND」路径的 host 模拟)。
    Refuse(&'static str),
    /// 收下请求但不应答(超时路径)。
    Ignore,
}

/// 伪 provider 的共享日志(断言数据源)。
#[derive(Default)]
struct ProviderLog {
    /// 到达的全部请求 (request_id, rel, size, resume)。
    seen: Mutex<Vec<(u64, String, u64, bool)>>,
    /// 已应答的请求 (request_id, ack.ok)——Answer/Refuse 都算已应答。
    answered: Mutex<Vec<(u64, bool)>>,
    /// 收到的全部 0x03 (request_id, done)——ack 交错与排空两路并入。
    commits: Mutex<Vec<(u64, bool)>>,
    /// resume 应答时暂存文件已完整在场的首部块数(重开同一文件的证明)。
    resume_leading: Mutex<Vec<(u64, usize)>>,
}

/// 伪 provider 的运行时指令(测试体 → 应答循环)。
enum AppCmd {
    /// 排空 0x03 至期望数(含 ack 交错已暂存的),完成即回执。
    DrainCommits { expected: usize, reply: oneshot::Sender<()> },
}

/// 伪 provider:订阅 fs.request_dest → 按脚本开暂存文件回 0x02(+1 fd);
/// 0x03 推送与 ack 在同一连接交错到达,读面容错排序。事件流关闭即退。
struct MockProvider {
    ipc: IpcClient,
    fdpass: FdPassClient,
    staging: PathBuf,
    /// rel → 源文件(重开快照的内容基准)。
    sources: HashMap<String, PathBuf>,
    script: Vec<DestStep>,
    default_step: DestStep,
    log: Arc<ProviderLog>,
}

impl MockProvider {
    /// 暂存路径(rel 中 `/` 归一,防嵌套 rel 落进子目录)。
    fn staging_path(&self, rel: &str) -> PathBuf {
        self.staging.join(rel.replace('/', "__"))
    }

    async fn spawn(
        home: &Path,
        staging: PathBuf,
        sources: HashMap<String, PathBuf>,
        script: Vec<DestStep>,
        default_step: DestStep,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<ProviderLog>,
        mpsc::Sender<AppCmd>,
    ) {
        let ipc = IpcClient::connect(home).await.expect("伪 provider 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        let log = Arc::new(ProviderLog::default());
        let app = Self {
            ipc,
            fdpass,
            staging,
            sources,
            script,
            default_step,
            log: log.clone(),
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
                    Some(AppCmd::DrainCommits { expected, reply }) => {
                        while self.log.commits.lock().unwrap().len() < expected {
                            let msg = match tokio::time::timeout(
                                Duration::from_secs(15),
                                self.fdpass.recv_msg(),
                            )
                            .await
                            {
                                Err(_) => panic!("等待 0x03 dest_commit 超时"),
                                Ok(Err(e)) => panic!("fdpass 推送读失败: {e}"),
                                Ok(Ok(m)) => m,
                            };
                            self.record_commit(msg);
                        }
                        let _ = reply.send(());
                    }
                    None => return,
                },
            }
        }
    }

    async fn handle_event(&mut self, ev: mtdss_ipc::EventFrame) {
        if ev.method != "fs.request_dest" {
            return;
        }
        let request_id = ev.params["request_id"].as_u64().expect("request_id u64");
        let rel = ev.params["rel"].as_str().expect("rel 字符串").to_string();
        let size = ev.params["size"].as_u64().expect("size u64");
        let resume = ev.params["resume"].as_bool().expect("resume bool");
        self.log
            .seen
            .lock()
            .unwrap()
            .push((request_id, rel.clone(), size, resume));
        let step = self
            .script
            .get(self.log.answered.lock().unwrap().len())
            .cloned()
            .unwrap_or_else(|| self.default_step.clone());
        match step {
            DestStep::Answer => {
                let src = self
                    .sources
                    .get(&rel)
                    .unwrap_or_else(|| panic!("未知 rel: {rel}"))
                    .clone();
                let staging = self.staging_path(&rel);
                let mut open = std::fs::OpenOptions::new();
                open.read(true).write(true);
                let file = if resume {
                    // 重开同一暂存(不截断):先快照在场块,再交 fd。
                    let f = open
                        .open(&staging)
                        .unwrap_or_else(|e| panic!("重开暂存 {staging:?}: {e}"));
                    let leading = leading_matching_chunks(&staging, &src);
                    self.log.resume_leading.lock().unwrap().push((request_id, leading));
                    f
                } else {
                    open.create(true)
                        .truncate(true)
                        .open(&staging)
                        .unwrap_or_else(|e| panic!("新建暂存 {staging:?}: {e}"))
                };
                let fd: OwnedFd = file.into();
                let ack_ok = self
                    .send_expect_ack(
                        KIND_DEST_FD,
                        json!({ "request_id": request_id }),
                        Some(fd),
                    )
                    .await;
                self.log.answered.lock().unwrap().push((request_id, ack_ok));
            }
            DestStep::Refuse(reason) => {
                let ack_ok = self
                    .send_expect_ack(
                        KIND_DEST_FD,
                        json!({ "request_id": request_id, "ok": false, "error": reason }),
                        None,
                    )
                    .await;
                self.log.answered.lock().unwrap().push((request_id, ack_ok));
            }
            DestStep::Ignore => {} // 不应答:仅 seen 记录
        }
    }

    /// 发 0x02 应答并等 ack;ack 之前可能先收到交错的 0x03 推送(同连接
    /// 的服务端主动帧)——先记下再继续等 ack。
    async fn send_expect_ack(&mut self, kind: u8, payload: Value, fd: Option<OwnedFd>) -> bool {
        let reply = self
            .fdpass
            .send_msg(kind, payload, fd)
            .await
            .expect("fdpass 发送应答");
        let mut msg = reply;
        loop {
            match msg.kind {
                KIND_ACK => return msg.payload["ok"].as_bool().unwrap_or(false),
                KIND_DEST_COMMIT => {
                    self.record_commit(msg);
                    msg = tokio::time::timeout(Duration::from_secs(15), self.fdpass.recv_msg())
                        .await
                        .expect("等待交错帧后的 ack 超时")
                        .expect("fdpass 推送读失败");
                }
                other => panic!("预期 ack/dest_commit,收到 kind 0x{other:02x}"),
            }
        }
    }

    fn record_commit(&self, msg: mtdss_ipc::fdpass::FdMsg) {
        assert_eq!(msg.kind, KIND_DEST_COMMIT, "推送帧必须是 0x03: {msg:?}");
        let request_id = msg.payload["request_id"].as_u64().expect("request_id");
        let done = msg.payload["done"].as_bool().expect("done");
        self.log.commits.lock().unwrap().push((request_id, done));
    }
}

/// 暂存文件与源文件按块对齐比对的「已完整在场首部块数」(续传素材证明)。
fn leading_matching_chunks(staging: &Path, src: &Path) -> usize {
    let got = std::fs::read(staging).expect("读暂存");
    let want = std::fs::read(src).expect("读源");
    let mut n = 0usize;
    while (n + 1) * CHUNK_SIZE <= got.len().min(want.len())
        && got[n * CHUNK_SIZE..(n + 1) * CHUNK_SIZE]
            == want[n * CHUNK_SIZE..(n + 1) * CHUNK_SIZE]
    {
        n += 1;
    }
    n
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

// ---------- 断言助手 ----------

/// A 侧起一个指向收件箱的 upload(dir="inbox"),返回 job_id。
async fn start_inbox_upload(client: &IpcClient, files: &[PathBuf], to: &str) -> String {
    let files: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let r = call(
        client,
        "transfer.start",
        json!({
            "kind": "upload", "files": files, "to": to, "dir": "inbox",
            "skip": [], "overwrite": [], "cut": false,
        }),
    )
    .await
    .expect("transfer.start upload(dir=inbox) 应成功");
    r["job_id"].as_str().expect("job_id 字符串").to_string()
}

/// B 侧收件箱记录({home}/inbox-records/<sha256(rel)>.json)→ 解析 JSON;
/// 缺席 → None。
fn inbox_record(home_b: &Path, rel: &str) -> Option<Value> {
    let p = home_b
        .join("inbox-records")
        .join(format!("{}.json", record_key(rel)));
    let raw = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 记录 JSON 的位图置位数;缺席/畸形 → 0。
fn value_bits(v: &Value) -> usize {
    v["bitmap"]
        .as_str()
        .and_then(|h| hex::decode(h).ok())
        .map(|b| b.iter().map(|x| x.count_ones() as usize).sum())
        .unwrap_or(0)
}

/// 全 1 位图 hex(chunk_count 块,与 intake FileRecord 的已收位图口径一致)。
fn full_bitmap_hex(chunk_count: usize) -> String {
    let mut b = vec![0u8; chunk_count.div_ceil(8)];
    for i in 0..chunk_count {
        b[i / 8] |= 1 << (i % 8);
    }
    hex::encode(b)
}

/// 轮询直至谓词命中(记录面观察;deadline 到即 panic)。
async fn wait_record<F: Fn(&Value) -> bool>(home_b: &Path, rel: &str, what: &str, pred: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(v) = inbox_record(home_b, rel) {
            if pred(&v) {
                return v;
            }
        }
        assert!(
            Instant::now() < deadline,
            "记录面未在预算内满足 {what}: {:?}",
            inbox_record(home_b, rel)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------- 用例 ----------

/// ① 全新收:两件(含嵌套 rel)→ 暂存逐字节 + 记录 done/final_rel/位图 +
/// 0x03 done:true 恰两次;`.mtdss` 键空间不被触碰;暂存 fd 全释放。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_fresh_receive_delivers_content_record_and_commit() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("inbox1@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        // 件面(顶层文件 → rel = 文件名;顶层目录 → rel = 目录名/内部
        // 相对路径——collect_items 口径,嵌套 rel 由目录输入产生)。
        let f1 = src.path().join("桥接 收件 甲.bin");
        let data1 = fill_random_file(&f1, 16 * 1024);
        let nested = src.path().join("嵌套");
        std::fs::create_dir_all(&nested).unwrap();
        let f2 = nested.join("乙.bin");
        let data2 = fill_random_file(&f2, 48 * 1024);
        let files: Vec<PathBuf> = vec![f1.clone(), nested.clone()];
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        sources.insert("桥接 收件 甲.bin".to_string(), f1);
        sources.insert("嵌套/乙.bin".to_string(), f2);
        let contents: Vec<(String, Vec<u8>)> = vec![
            ("桥接 收件 甲.bin".to_string(), data1),
            ("嵌套/乙.bin".to_string(), data2),
        ];

        let (app, log, cmd) = MockProvider::spawn(
            p.home_b.path(),
            staging.path().to_path_buf(),
            sources,
            vec![],
            DestStep::Answer,
        )
        .await;

        let id = start_inbox_upload(&p.ca, &files, &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        assert_eq!(fin["files_done"].as_u64(), Some(2), "完成计数: {fin}");

        // 暂存内容逐字节(provider 侧就是终件落点)。
        for (rel, data) in &contents {
            let got = std::fs::read(staging.path().join(rel.replace('/', "__")))
                .unwrap_or_else(|e| panic!("读暂存 {rel}: {e}"));
            assert_eq!(&got, data, "暂存内容必须逐字节一致: {rel}");
        }

        // 记录:{home}/inbox-records 下 done + final_rel=rel + 位图全 1。
        for (rel, data) in &contents {
            let rec = wait_record(p.home_b.path(), rel, "done 记录", |v| {
                v["done"].as_bool() == Some(true)
            })
            .await;
            let chunks = data.len().div_ceil(CHUNK_SIZE);
            assert_eq!(rec["rel"].as_str(), Some(rel.as_str()), "记录 rel: {rec}");
            assert_eq!(
                rec["final_rel"].as_str(),
                Some(rel.as_str()),
                "final_rel = rel(provider 侧终名,无冲突改名面): {rec}"
            );
            assert_eq!(rec["size"].as_u64(), Some(data.len() as u64), "size: {rec}");
            assert_eq!(
                rec["bitmap"].as_str(),
                Some(full_bitmap_hex(chunks).as_str()),
                "位图全 1: {rec}"
            );
        }

        // 请求面:两件各恰一次,均为全新(resume=false),ack 全受理。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "每文件恰一次 request_dest: {seen:?}");
            assert!(
                seen.iter().all(|(_, _, _, resume)| !resume),
                "全新收的请求必须 resume=false: {seen:?}"
            );
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 2, "两件全应答: {answered:?}");
            assert!(
                answered.iter().all(|(_, ok)| *ok),
                "正常交付应 ack ok:true: {answered:?}"
            );
        }

        // 0x03:恰每文件一次 done:true,request_id 与 seen 对得上。
        {
            let (tx, rx) = oneshot::channel();
            cmd.send(AppCmd::DrainCommits { expected: 2, reply: tx })
                .await
                .unwrap();
            rx.await.unwrap();
            let commits = log.commits.lock().unwrap().clone();
            assert_eq!(commits.len(), 2, "恰两次 dest_commit: {commits:?}");
            assert!(
                commits.iter().all(|(_, done)| *done),
                "全新收的提交必须 done:true: {commits:?}"
            );
            let seen_ids: Vec<u64> = log.seen.lock().unwrap().iter().map(|(i, _, _, _)| *i).collect();
            assert!(
                commits.iter().all(|(id, _)| seen_ids.contains(id)),
                "dest_commit 的 request_id 必须对应已见请求: {commits:?}"
            );
        }

        // 防串台:默认收件目录的 `.mtdss` 键空间不被收件箱轮触碰。
        assert!(
            !p.home_b.path().join("inbox").join(".mtdss").exists(),
            "收件箱轮不得在默认收件目录建 .mtdss"
        );

        // fd 生命周期:终态后暂存 fd 全释放。
        let staging_paths: Vec<PathBuf> = contents
            .iter()
            .map(|(rel, _)| staging.path().join(rel.replace('/', "__")))
            .collect();
        wait_fds_released(&staging_paths).await;

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ② 中断续传:暂停冻结 + 取消(记录已落 ≥24/64 块位)→ 重新发起 →
/// provider resume=true 重开同一暂存(重开时首部 ≥24 块已完整在场)→
/// 缺块补齐 → done:true;中断轮的请求收 0x03 done:false(失败收尾)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_interrupted_transfer_resumes_on_reopened_fd() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("inbox2@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        // 64 MiB(64 块):中断门 = 记录位 ≥24(已冲盘),暂停冻结后余量
        // ≥ 64-24-15(未冲盘)-4(在途)块,不可能在中断前完成。
        // rel = 文件名(顶层文件口径)。
        let rel = "断点 大件.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 64 * CHUNK_SIZE);
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        sources.insert(rel.to_string(), path.clone());

        let (app, log, cmd) = MockProvider::spawn(
            p.home_b.path(),
            staging.path().to_path_buf(),
            sources,
            vec![],
            DestStep::Answer,
        )
        .await;

        // 第一传:跑到记录位 ≥24 即暂停(冻结)→ 取消(中断)。
        let id1 = start_inbox_upload(&p.ca, &[path.clone()], &p.dev_b).await;
        let rec = wait_record(p.home_b.path(), rel, "中断门(位 ≥24)", |v| {
            v["done"].as_bool() != Some(true) && value_bits(v) >= 24
        })
        .await;
        assert_eq!(rec["done"].as_bool(), Some(false), "中断前必须未完成: {rec}");
        call(&p.ca, "transfer.pause", json!({ "id": id1 }))
            .await
            .expect("暂停应成功(冻结传输面)");
        tokio::time::sleep(Duration::from_millis(300)).await; // 在途块落地
        call(&p.ca, "transfer.cancel", json!({ "id": id1 }))
            .await
            .expect("取消应成功");
        let fin1 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id1 }), WAIT_TIMEOUT)
            .await
            .expect("中断轮应到终态");
        assert_eq!(fin1["state"].as_str(), Some("cancelled"), "中断: {fin1}");

        // 重新发起(同源同 rel 同 dir)→ 对账见部分记录 → Resume 计划 →
        // provider 收 resume=true 请求并重开同一暂存。
        let id2 = start_inbox_upload(&p.ca, &[path.clone()], &p.dev_b).await;
        let fin2 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("重发轮应到终态");
        assert_eq!(fin2["state"].as_str(), Some("done"), "重发终态: {fin2}");

        // 请求面:两轮各一次;第二轮必须 resume=true(断点续传决策)。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "每轮恰一次 request_dest: {seen:?}");
            assert!(!seen[0].3, "首轮全新收 resume=false: {seen:?}");
            assert!(seen[1].3, "重发轮必须 resume=true(对账得 Resume 计划): {seen:?}");
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 2, "两轮全应答: {answered:?}");
            assert!(
                answered.iter().all(|(_, ok)| *ok),
                "两轮交付均被受理(ack ok:true): {answered:?}"
            );
        }

        // 重开快照:resume 应答时暂存首部 ≥24 块已完整在场(同一文件续传
        // 的素材证明——重开文件保留了中断轮的已收块)。
        {
            let leading = log.resume_leading.lock().unwrap().clone();
            assert_eq!(leading.len(), 1, "恰一次重开快照: {leading:?}");
            assert!(leading[0].1 >= 24, "重开时已完整在场 ≥24 块: {leading:?}");
        }

        // 终件:暂存内容与源逐字节一致;记录 done。
        let got = std::fs::read(staging.path().join(rel.replace('/', "__")))
            .expect("读暂存终件");
        assert_eq!(&got, &data, "续传终件必须逐字节一致");
        let rec = wait_record(p.home_b.path(), rel, "终局 done 记录", |v| {
            v["done"].as_bool() == Some(true)
        })
        .await;
        assert_eq!(rec["bitmap"].as_str(), Some(full_bitmap_hex(64).as_str()), "终局位图全 1: {rec}");

        // 0x03:中断轮 done:false(失败收尾)+ 重发轮 done:true,恰两次。
        {
            let (tx, rx) = oneshot::channel();
            cmd.send(AppCmd::DrainCommits { expected: 2, reply: tx })
                .await
                .unwrap();
            rx.await.unwrap();
            let commits = log.commits.lock().unwrap().clone();
            assert_eq!(commits.len(), 2, "恰两次 dest_commit: {commits:?}");
            let by_req: HashMap<u64, bool> = commits.into_iter().collect();
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(by_req.get(&seen[0].0), Some(&false), "中断轮请求必须 done:false");
            assert_eq!(by_req.get(&seen[1].0), Some(&true), "重发轮请求必须 done:true");
        }

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ③ resume 撞 NOT_FOUND:预置部分记录(provider 暂存已失)→ provider 对
/// resume 请求答 ok:false → daemon 清陈旧记录 → 轮失败 → 发送端重试轮
/// 对账无记录 → FULL 重收(resume=false 请求 + 全新暂存)→ done:true。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_resume_not_found_falls_back_to_full_receive() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("inbox3@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        // 8 MiB(8 块);预置 3 块已收的部分记录(暂存已失——provider 侧
        // 从未建过,重开必撞 NOT_FOUND)。
        let rel = "失锚 续传件.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 8 * CHUNK_SIZE);
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        sources.insert(rel.to_string(), path.clone());
        let records = p.home_b.path().join("inbox-records");
        std::fs::create_dir_all(&records).unwrap();
        let planted = json!({
            "rel": rel,
            "size": data.len(),
            "sha256": hex::encode(sha2::Sha256::digest(&data)),
            "chunk_count": 8,
            "bitmap": "e0",       // 块 0-2 已收(0b111_00000)
            "done": false,
            "final_rel": serde_json::Value::Null,
        });
        std::fs::write(
            records.join(format!("{}.json", record_key(rel))),
            serde_json::to_vec(&planted).unwrap(),
        )
        .unwrap();

        // 脚本:首个请求(resume=true)拒开;其后默认应答(resume=false
        // 全新暂存)。
        let (app, log, _cmd) = MockProvider::spawn(
            p.home_b.path(),
            staging.path().to_path_buf(),
            sources,
            vec![DestStep::Refuse("NOT_FOUND: staging cleaned")],
            DestStep::Answer,
        )
        .await;

        let id = start_inbox_upload(&p.ca, &[path.clone()], &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "降级重收后终态: {fin}");

        // 请求面:首请求 resume=true 被拒;重试轮请求 resume=false(FULL
        // 决策随 provider 应答)。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "两轮各一次请求: {seen:?}");
            assert!(seen[0].3, "首请求必须 resume=true(部分记录对账): {seen:?}");
            assert!(!seen[1].3, "拒开后的重试请求必须 resume=false(FULL 重收): {seen:?}");
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 2, "拒开与交付都受理(ack ok:true): {answered:?}");
            assert!(answered.iter().all(|(_, ok)| *ok), "受理面: {answered:?}");
        }

        // 陈旧记录被清除:拒开后记录先缺席(再由 FULL 轮重建)——以
        // 「内容不再是预置形态」收口(缺席或位图归零都证明清除)。
        {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let cur = inbox_record(p.home_b.path(), rel);
                let cleared = match &cur {
                    None => true,
                    Some(v) => v["bitmap"].as_str() != Some("e0"),
                };
                if cleared {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "拒开后陈旧记录必须被清除: {cur:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        // 终件:FULL 重收后暂存与源逐字节一致;记录 done + 位图全 1。
        let got = std::fs::read(staging.path().join(rel.replace('/', "__")))
            .expect("读暂存终件");
        assert_eq!(&got, &data, "FULL 重收终件必须逐字节一致");
        let rec = wait_record(p.home_b.path(), rel, "终局 done 记录", |v| {
            v["done"].as_bool() == Some(true)
        })
        .await;
        assert_eq!(rec["bitmap"].as_str(), Some(full_bitmap_hex(8).as_str()), "终局位图全 1: {rec}");

        // B 侧匿名卡:拒开轮落 failed 且 error 含「目的地不可用」「拒开」。
        {
            let lst = call(&p.cb, "transfer.list", json!({})).await.unwrap();
            let hit = lst["jobs"].as_array().unwrap().iter().any(|j| {
                j["kind"].as_str() == Some("incoming")
                    && j["state"].as_str() == Some("failed")
                    && j["error"]
                        .as_str()
                        .map(|e| e.contains("目的地不可用") && e.contains("拒开"))
                        .unwrap_or(false)
            });
            assert!(hit, "拒开轮的匿名卡必须 failed 且带拒开终因: {lst}");
        }

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ④ request_dest 超时(注入 1500ms):provider 永不应答 → 任务 failed
/// (B 侧匿名卡 error 含「目的地不可用」「超时」);迟到 0x02 应答按未知
/// request_id 拒(ack ok:false),fd 就地释放。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_request_timeout_fails_task_and_rejects_late_answer() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("inbox4@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();

        let rel = "超时 件.bin";
        let path = src.path().join(rel);
        fill_random_file(&path, 8 * 1024);
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        sources.insert(rel.to_string(), path.clone());

        // 默认 Ignore:收下请求但永不应答。
        let (app, log, _cmd) = MockProvider::spawn(
            p.home_b.path(),
            staging.path().to_path_buf(),
            sources,
            vec![],
            DestStep::Ignore,
        )
        .await;

        let id = start_inbox_upload(&p.ca, &[path.clone()], &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("failed"), "超时终态: {fin}");

        // 请求面:发送端重试轮各发新请求(≥2 次),provider 全未应答。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert!(seen.len() >= 2, "重试轮必须重新请求(≥2 次): {seen:?}");
            assert!(
                log.answered.lock().unwrap().is_empty(),
                "provider 全程未应答"
            );
            assert!(
                log.commits.lock().unwrap().is_empty(),
                "无交付即无 dest_commit"
            );
        }

        // B 侧匿名卡:failed 且 error 含「目的地不可用」「超时」。
        {
            let lst = call(&p.cb, "transfer.list", json!({})).await.unwrap();
            let hit = lst["jobs"].as_array().unwrap().iter().any(|j| {
                j["kind"].as_str() == Some("incoming")
                    && j["state"].as_str() == Some("failed")
                    && j["error"]
                        .as_str()
                        .map(|e| e.contains("目的地不可用") && e.contains("超时"))
                        .unwrap_or(false)
            });
            assert!(hit, "超时轮的匿名卡必须 failed 且带超时终因: {lst}");
        }

        // 迟到应答:超时请求已从 pending 摘除 → 未知 request_id,ack
        // ok:false(第二条连接直发,顺带覆盖多连接面);fd 随拒收释放。
        let request_id = log.seen.lock().unwrap()[0].0;
        let late_path = staging.path().join("late.bin");
        std::fs::write(&late_path, b"late").unwrap();
        let mut late = connect_fdpass_retry(p.home_b.path()).await;
        let fd: OwnedFd = std::fs::File::open(&late_path).unwrap().into();
        let ack = late
            .send_msg(KIND_DEST_FD, json!({ "request_id": request_id }), Some(fd))
            .await
            .expect("迟到应答发送不应断连");
        assert_eq!(ack.kind, KIND_ACK);
        assert_eq!(
            ack.payload["ok"].as_bool(),
            Some(false),
            "迟到应答按未知 request_id 拒: {ack:?}"
        );
        drop(late);
        wait_fds_released(&[late_path]).await;

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}
