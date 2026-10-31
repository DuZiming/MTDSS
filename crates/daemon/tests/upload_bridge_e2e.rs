//! bridge 上行端到端(v2-P3 Task 4):host 双 daemon + 伪 App(真实
//! IpcClient 订阅 `fs.request_source` + FdPassClient 回 `0x01 source_fd`)
//! → `transfer.start {kind:upload, source:"bridge", items, to, dir}` → 接收
//! 端逐字节断言。fdpass 依赖 Unix 域套接字,整文件 cfg(unix)(CI 在 WSL
//! 跑;Windows 编译单元整体缺席,桌面回归面不受影响)。
//!
//! 断言面(计划评审焦点 #1/#3 + 任务简报):
//! - **逐文件逐个 request_source**:每文件恰一次请求,串行(下一文件的
//!   请求只在上一文件 fd 应答落地后才发出),request_id 单调不复用;
//! - **fd 生命周期**:驱动建表持有源 fd(hash+send 两段共用),任务终态
//!   (done/failed)后指向源文件的 fd 计数回落 0——泄漏(逐文件开/关中
//!   任一环节漏关)以计数不归零显形;在途快照(收到第 N 个请求时,前
//!   N-1 个文件的 fd 已在引擎手里)作对照,防「从未真正持有」的空过;
//! - **10s 超时路径**(注入 1500ms 测试值,不真等 10s):伪 App 不应答 →
//!   任务 failed「源不可用」;驱动在首个失败即收束(不再请求第二件);
//!   超时后的迟到 fd 应答按未知 request_id 拒(ack ok:false);
//! - **拒开路径**(评审焦点 #3,实机「授权中途被撤」的 host 模拟):伪
//!   App 第二次拒开(0x01 无 fd)→ 任务 failed「源不可用」,已解析的
//!   第一件 fd 随失败收尾释放;
//! - **taskstore 序列化**:src_paths 落 `bridge://{ref}` 串;重发
//!   (transfer.reinitiate)按「源已不存在」优雅失败(P2 语义原样承袭:
//!   记录保留可再试,不 panic)。

#![cfg(unix)]

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{FdPassClient, KIND_ACK, KIND_SOURCE_FD};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时(transfer.* 除 wait 外都是快操作)。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// transfer.wait 长轮询上界(传输全程在驱动内)。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);
/// 伪 App 应答的 fs.request_source 事件等待上界(驱动起步即发,快到达)。
const APP_EVENT_TIMEOUT: Duration = Duration::from_secs(30);
/// fd 计数回落的轮询上界:驱动表 Drop 发生在终态翻转之后(wait 返回时
/// fd 可能尚在),轮询至归零而非立即断言。
const FD_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

/// 超时注入(生产 10s,不真等):daemon 构造期读
/// `MTDSS_TEST_REQUEST_SOURCE_TIMEOUT_MS`(沿 MTDSS_TEST_* 家族口径)。
/// 全部用例共享测试进程,首例起跑前经 OnceLock 一次性设置(并发用例在
/// 锁上等初始化完成,读定时序无竞态);注入值须远大于伪 App 的本地
/// 应答时延(µs 级),正常路径不受影响。
const INJECTED_TIMEOUT_MS: u64 = 1500;
static TIMEOUT_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_timeout() {
    TIMEOUT_INJECT.get_or_init(|| {
        std::env::set_var(
            "MTDSS_TEST_REQUEST_SOURCE_TIMEOUT_MS",
            INJECTED_TIMEOUT_MS.to_string(),
        );
    });
}

/// 同文件用例并发闸(jobs_e2e 先例):每用例自带 2-worker reactor + 进程内
/// 双 daemon + 独立信令服务器,限并发到 2 防打洞/信令预算被饿穿。
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
        authorized_roots: None,
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
async fn ipc_login(client: &IpcClient, server: &str, email: &str, name: &str) -> Value {
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
    .expect("login.verify 应成功")
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
                l.iter().any(
                    |d| d["device_id"].as_str() == Some(dev) && d["online"].as_bool() == Some(want),
                )
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

/// 指向任一源文件的进程 fd 计数:/proc/self/fd 逐条 readlink,目标命中
/// 源文件集合即计入(进程内 daemon + 测试共用同一 fd 表,精确隔离与传输
/// 无关的句柄扰动;多条 fd 指向同一文件按条计数)。
fn fds_at_sources(paths: &[PathBuf]) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("读 /proc/self/fd")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let target = std::fs::read_link(e.path()).ok();
            matches!(target, Some(t) if paths.contains(&t))
        })
        .count()
}

/// 轮询直至指向源文件的 fd 计数归零(终态后驱动表 Drop 有微小滞后)。
async fn wait_fds_released(paths: &[PathBuf]) {
    let deadline = Instant::now() + FD_SETTLE_TIMEOUT;
    loop {
        let n = fds_at_sources(paths);
        if n == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "任务终态后源 fd 未释放(泄漏 {n} 条)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 进程内双 daemon 测试台(jobs_e2e::Pair 同款):独立信令服务器 + 两侧
/// 独立 home/IPC 客户端,双方已登录且 A 已见 B 在线。
struct Pair {
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    ca: IpcClient,
    dev_b: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        inject_timeout();
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
            _home_b: home_b,
            _state: state,
            daemon_a,
            daemon_b,
            ca,
            dev_b,
        }
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

// ---------- 伪 App(镜像 Kotlin T10 SourceProvider 的职责面) ----------

/// 伪 App 对单个 fs.request_source 的处置。
#[derive(Clone)]
enum AppStep {
    /// 打开源文件回 fd(默认)。
    Answer,
    /// 拒开:0x01 无 fd,payload 带 ok:false + error 文案(实机「授权被
    /// 撤」路径的 host 模拟)。
    Refuse(&'static str),
    /// 收下请求但不应答(超时路径)。
    Ignore,
}

/// 伪 App 的共享日志(断言数据源;spawn 任务里的断言不外显,一律落日志
/// 由用例体核验)。
#[derive(Default)]
struct AppLog {
    /// 到达的全部请求 (request_id, ref)。
    seen: Mutex<Vec<(u64, String)>>,
    /// 已应答的请求 (request_id, ref, ack.ok)——Answer/Refuse 都算已应答。
    answered: Mutex<Vec<(u64, String, bool)>>,
    /// 每次请求到达时的源 fd 在途计数快照(第 N 个请求到达时,前 N-1 件
    /// 的 fd 应已在引擎手里)。
    inflight: Mutex<Vec<usize>>,
}

/// 伪 App:订阅 fs.request_source → 按 ref 打开本地源文件 → FdPassClient
/// 回 0x01(+1 fd);逐请求脚本注入处置。事件流关闭(daemon 收摊)即退。
struct MockApp {
    ipc: IpcClient,
    fdpass: FdPassClient,
    /// ref → 本地源文件。
    sources: HashMap<String, PathBuf>,
    /// 逐请求脚本(按到达序;不足回落 Answer)。
    script: Vec<AppStep>,
    log: Arc<AppLog>,
}

impl MockApp {
    /// 连接 daemon(IPC 事件流 + fds.sock)并起应答循环。fdpass 的 bind 与
    /// 首连存在启动竞速,重试兜底(fdpass_test 同款)。
    async fn spawn(
        home: &Path,
        sources: HashMap<String, PathBuf>,
        script: Vec<AppStep>,
    ) -> (tokio::task::JoinHandle<()>, Arc<AppLog>) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        let log = Arc::new(AppLog::default());
        let app = Self {
            ipc,
            fdpass,
            sources,
            script,
            log: log.clone(),
        };
        let task = tokio::spawn(app.run());
        (task, log)
    }

    async fn run(mut self) {
        let src_paths: Vec<PathBuf> = self.sources.values().cloned().collect();
        let mut evs = self.ipc.events();
        loop {
            let ev = match tokio::time::timeout(APP_EVENT_TIMEOUT, evs.recv()).await {
                Ok(Ok(f)) => f,
                // daemon 收摊/通道关闭:退出
                Ok(Err(broadcast::error::RecvError::Closed)) => return,
                // 滞后:继续等(容量 256,不应触达)
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                // 静默窗:无新请求,继续守
                Err(_) => continue,
            };
            if ev.method != "fs.request_source" {
                continue;
            }
            let request_id = ev.params["request_id"].as_u64().expect("request_id u64");
            let r#ref = ev.params["ref"].as_str().expect("ref 字符串").to_string();
            self.log.seen.lock().unwrap().push((request_id, r#ref.clone()));
            self.log
                .inflight
                .lock()
                .unwrap()
                .push(fds_at_sources(&src_paths));
            let step = self
                .script
                .get(self.log.answered.lock().unwrap().len())
                .cloned()
                .unwrap_or(AppStep::Answer);
            let ack_ok = match step {
                AppStep::Answer => {
                    let path = self
                        .sources
                        .get(&r#ref)
                        .unwrap_or_else(|| panic!("未知 ref: {ref}"))
                        .clone();
                    let fd: std::os::fd::OwnedFd = std::fs::File::open(&path)
                        .unwrap_or_else(|e| panic!("开源文件 {path:?}: {e}"))
                        .into();
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_SOURCE_FD,
                            json!({ "request_id": request_id }),
                            Some(fd),
                        )
                        .await;
                    match ack {
                        Ok(a) => a.payload["ok"].as_bool().unwrap_or(false),
                        Err(_) => false,
                    }
                }
                AppStep::Refuse(reason) => {
                    let ack = self
                        .fdpass
                        .send_msg(
                            KIND_SOURCE_FD,
                            json!({ "request_id": request_id, "ok": false, "error": reason }),
                            None,
                        )
                        .await;
                    match ack {
                        Ok(a) => a.payload["ok"].as_bool().unwrap_or(false),
                        Err(_) => false,
                    }
                }
                AppStep::Ignore => continue, // 不应答:仅 seen 记录
            };
            self.log
                .answered
                .lock()
                .unwrap()
                .push((request_id, r#ref, ack_ok));
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

/// transfer.start(bridge 形态)速构;返回 (job_id, start 结果)。
async fn start_bridge_upload(
    client: &IpcClient,
    items: &[Value],
    to: &str,
    dir: &str,
) -> Value {
    call(
        client,
        "transfer.start",
        json!({
            "kind": "upload", "source": "bridge", "items": items,
            "to": to, "dir": dir, "skip": [], "overwrite": [], "cut": false,
        }),
    )
    .await
    .expect("transfer.start(bridge) 应成功")
}

/// 读 {home}/tasks.json 里指定 id 的记录(无则 panic)。
fn task_record(home: &Path, id: &str) -> Value {
    let raw = std::fs::read_to_string(home.join("tasks.json")).expect("读 tasks.json");
    let recs: Vec<Value> = serde_json::from_str(&raw).expect("tasks.json 解析");
    recs.into_iter()
        .find(|r| r["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("tasks.json 无记录 {id}"))
}

// ---------- 用例 ----------

/// ① 全链路:三件(含嵌套 rel)逐个 request_source → 逐字节落盘 → fd 释放。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_upload_delivers_per_file_and_releases_fds() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("bridge1@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();

        // 三件源文件(随机内容,收发两端逐字节断言);ref → 路径供伪 App 开。
        let spec = [
            ("mock-ref-1", "桥接 甲.bin", 16 * 1024usize),
            ("mock-ref-2", "嵌套/乙.bin", 48 * 1024usize),
            ("mock-ref-3", "桥接 丙.bin", 8 * 1024usize),
        ];
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        let mut contents: Vec<(&'static str, String, Vec<u8>)> = Vec::new();
        let mut src_paths: Vec<PathBuf> = Vec::new();
        let mut items: Vec<Value> = Vec::new();
        for (i, (r, rel, size)) in spec.iter().enumerate() {
            let path = src.path().join(format!("src-{i}.bin"));
            let data = fill_random_file(&path, *size);
            let meta = std::fs::metadata(&path).unwrap();
            sources.insert(r.to_string(), path.clone());
            src_paths.push(path);
            contents.push((r, rel.to_string(), data));
            items.push(json!({
                "ref": r, "rel": rel,
                "size": meta.len(),
                "mtime": meta
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            }));
        }

        // 伪 App 全程应答(默认 Answer 脚本)。
        let (app, log) = MockApp::spawn(p._home_a.path(), sources, vec![]).await;

        let r = start_bridge_upload(&p.ca, &items, &p.dev_b, &target.path().to_string_lossy())
            .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        assert_eq!(fin["files_total"].as_u64(), Some(3), "预扫总数(items 元数据): {fin}");
        assert_eq!(fin["files_done"].as_u64(), Some(3), "完成计数: {fin}");
        let expect_total: u64 = spec.iter().map(|(_, _, s)| *s as u64).sum();
        assert_eq!(fin["bytes_total"].as_u64(), Some(expect_total), "字节总数: {fin}");

        // 接收端逐 rel 逐字节(定向目录,嵌套 rel 原样落盘)。
        for (r, rel, data) in &contents {
            let got = std::fs::read(target.path().join(rel))
                .unwrap_or_else(|e| panic!("读 {rel}({r}): {e}"));
            assert_eq!(&got, data, "接收端内容必须逐字节一致: {rel}");
        }

        // 逐文件逐个 request_source:三件恰三请求,request_id 单调,ref 按
        // items 序(串行:第 N+1 件的请求只在第 N 件应答落地后发出)。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 3, "每文件恰一次 request_source: {seen:?}");
            let ids: Vec<u64> = seen.iter().map(|(i, _)| *i).collect();
            assert!(
                ids.windows(2).all(|w| w[0] < w[1]),
                "request_id 单调不复用: {ids:?}"
            );
            let refs: Vec<&str> = seen.iter().map(|(_, r)| r.as_str()).collect();
            assert_eq!(refs, vec!["mock-ref-1", "mock-ref-2", "mock-ref-3"]);
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 3, "三件全应答: {answered:?}");
            assert!(
                answered.iter().all(|(_, _, ok)| *ok),
                "正常交付应 ack ok:true: {answered:?}"
            );
        }
        // 在途持有:第 2/3 个请求到达时,前 1/2 件的 fd 已在引擎手里
        //(串行解析的持有证明;防「从未真正持有」的空过断言)。
        {
            let inflight = log.inflight.lock().unwrap().clone();
            assert_eq!(inflight.len(), 3);
            assert!(inflight[1] >= 1, "第 2 请求时至少持有第 1 件 fd: {inflight:?}");
            assert!(inflight[2] >= 2, "第 3 请求时至少持有前两件 fd: {inflight:?}");
        }
        // fd 生命周期:任务终态后源 fd 全释放(泄漏以计数不归零显形)。
        wait_fds_released(&src_paths).await;

        // taskstore 序列化:src_paths = bridge://{ref} 串(按 items 序)。
        let rec = task_record(p._home_a.path(), &id);
        assert_eq!(
            rec["src_paths"],
            json!(["bridge://mock-ref-1", "bridge://mock-ref-2", "bridge://mock-ref-3"]),
            "bridge 条目按 bridge://ref 串落记录: {rec}"
        );
        assert_eq!(rec["kind"], "upload");
        assert_eq!(rec["state"], "done");

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ② 超时路径(注入 1500ms,不真等 10s):伪 App 不应答 → 任务 failed
/// 「源不可用」;驱动在首件失败即收束;迟到应答按未知 request_id 拒;
/// 重发(reinitiate)按「源已不存在」优雅失败且记录保留。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_upload_timeout_fails_source_unavailable() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("bridge2@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let path = src.path().join("src.bin");
        let data = fill_random_file(&path, 8 * 1024);
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        sources.insert("mock-ref-t".to_string(), path.clone());
        let items = vec![json!({
            "ref": "mock-ref-t", "rel": "超时 件.bin", "size": data.len(), "mtime": 1760000000000i64,
        })];

        // 伪 App 收下请求但永不应答(脚本仅一条 Ignore;第二件本用例预期
        // 到不了请求面——见下方 seen 断言)。
        let (app, log) = MockApp::spawn(
            p._home_a.path(),
            sources.clone(),
            vec![AppStep::Ignore, AppStep::Ignore],
        )
        .await;

        let r = start_bridge_upload(&p.ca, &items, &p.dev_b, &target.path().to_string_lossy())
            .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("failed"), "终态: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(err.contains("源不可用"), "失败文案含「源不可用」: {err}");
        assert!(err.contains("超时"), "失败文案点名超时: {err}");

        // 首件失败即收束:恰一次请求,第二件的请求从未发出(串行逐个)。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 1, "首件失败即收束,无第二请求: {seen:?}");
            assert!(log.answered.lock().unwrap().is_empty(), "伪 App 未应答");
        }

        // 迟到应答:超时请求已从 pending 摘除 → 未知 request_id,ack
        // ok:false(第二条连接直发,顺带覆盖多连接面)。
        let request_id = log.seen.lock().unwrap()[0].0;
        let mut late = connect_fdpass_retry(p._home_a.path()).await;
        let fd: std::os::fd::OwnedFd = std::fs::File::open(sources["mock-ref-t"].as_path())
            .unwrap()
            .into();
        let ack = late
            .send_msg(KIND_SOURCE_FD, json!({ "request_id": request_id }), Some(fd))
            .await
            .expect("迟到应答发送不应断连");
        assert_eq!(ack.kind, KIND_ACK);
        assert_eq!(
            ack.payload["ok"].as_bool(),
            Some(false),
            "迟到应答按未知 request_id 拒: {ack:?}"
        );

        // fd 生命周期:失败收尾后无残留(含迟到应答被拒后丢弃的 fd)。
        wait_fds_released(&[path]).await;

        // P2 重发语义承袭:bridge 引用不可当路径预扫 → 重发按源已不存在
        // 优雅失败(Err + 原记录保留可再试),不 panic。
        let re = call(&p.ca, "transfer.reinitiate", json!({ "id": id })).await;
        let err = re.expect_err("bridge 记录重发应失败(源已不存在)");
        assert_eq!(err.code, 1, "业务失败码位: {err:?}");
        let rec = task_record(p._home_a.path(), &id);
        assert_eq!(
            rec["state"], "failed",
            "重发失败后原记录回插保留: {rec}"
        );

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ③ 拒开路径(评审焦点 #3:授权中途被撤):伪 App 第二次拒开 → 任务
/// failed「源不可用」;第一件已解析的 fd 随失败收尾释放。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_upload_refusal_fails_source_unavailable() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("bridge3@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        let mut src_paths: Vec<PathBuf> = Vec::new();
        let mut items: Vec<Value> = Vec::new();
        for (i, (r, rel)) in [("mock-ref-r1", "拒开 甲.bin"), ("mock-ref-r2", "拒开 乙.bin")]
            .iter()
            .enumerate()
        {
            let path = src.path().join(format!("src-{i}.bin"));
            fill_random_file(&path, 8 * 1024);
            sources.insert(r.to_string(), path.clone());
            src_paths.push(path);
            items.push(json!({ "ref": r, "rel": rel, "size": 8 * 1024, "mtime": 1760000000000i64 }));
        }

        // 脚本:第一件应答,第二件拒开(授权被撤)。
        let (app, log) = MockApp::spawn(
            p._home_a.path(),
            sources,
            vec![AppStep::Answer, AppStep::Refuse("mock 授权已撤销")],
        )
        .await;

        let r = start_bridge_upload(&p.ca, &items, &p.dev_b, &target.path().to_string_lossy())
            .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("failed"), "终态: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(err.contains("源不可用"), "失败文案含「源不可用」: {err}");
        assert!(err.contains("拒开"), "失败文案点名拒开: {err}");

        // 两件各到一次请求(逐个),第一件正常交付、第二件拒开。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "两件恰两请求: {seen:?}");
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 2, "两件都得到应答(交付/拒开): {answered:?}");
            assert!(
                answered.iter().all(|(_, _, ok)| *ok),
                "交付与拒开均被受理(ack ok:true): {answered:?}"
            );
        }
        // fd 生命周期:第一件已解析的 fd 随失败收尾释放。
        wait_fds_released(&src_paths).await;

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ④ 条目数护栏(评审 Important#1):513 项超限 → 立即业务错(可读文案,
/// 先于建卡/校验/解析);无任何 request_source 事件发出、fd 零开(伪 App
/// 未收到任何请求——超限单若漏进驱动,EMFILE 会以「源不可用: 等待
/// App 应答超时」误诊,此断言把该面钉死在入口)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_upload_over_item_cap_rejected_upfront() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("bridge4@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        // 真实源文件一件(ref 映射给首条目):fd 零开的非空判定面——
        // 若有任何解析动作,伪 App 会打开它、引擎会持有其 fd
        let path = src.path().join("src.bin");
        fill_random_file(&path, 8 * 1024);
        let mut sources: HashMap<String, PathBuf> = HashMap::new();
        sources.insert("mock-cap-0".to_string(), path.clone());
        // 513 项 = 上限 512 + 1(rel 形态合法、无重复)
        let items: Vec<Value> = (0..513)
            .map(|i| {
                json!({
                    "ref": format!("mock-cap-{i}"),
                    "rel": format!("超限/f{i:03}.bin"),
                    "size": 8, "mtime": 1760000000000i64,
                })
            })
            .collect();

        // 伪 App 在场(全默认应答脚本):断言全程未收到任何请求
        let (app, log) = MockApp::spawn(p._home_a.path(), sources, vec![]).await;

        let r = call(
            &p.ca,
            "transfer.start",
            json!({
                "kind": "upload", "source": "bridge", "items": items,
                "to": p.dev_b, "dir": target.path().to_string_lossy(),
                "skip": [], "overwrite": [], "cut": false,
            }),
        )
        .await;
        let err = r.expect_err("超限应立即业务错");
        assert_eq!(err.code, 1, "业务失败码位(现有 ERR_BUSINESS 口径): {err:?}");
        // 可读文案:实际条目数 + 上限 + 行动指引
        assert!(
            err.message.contains("桥接条目过多")
                && err.message.contains("513")
                && err.message.contains("512")
                && err.message.contains("请分批发送"),
            "错误消息须含条目数/上限/分批指引: {}",
            err.message
        );

        // 无 request_source 事件、fd 零开:静默窗后再断言(事件广播即时,
        // 窗内未到即永不到;驱动未起步,无任何 fd 动作)
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            log.seen.lock().unwrap().is_empty(),
            "超限单不得发出任何 fs.request_source(入口即拒)"
        );
        assert!(
            log.answered.lock().unwrap().is_empty(),
            "伪 App 全程未应答任何请求"
        );
        assert_eq!(
            fds_at_sources(&[path]),
            0,
            "超限单不得打开/持有任何源 fd"
        );

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}
