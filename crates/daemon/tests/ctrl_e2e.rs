//! daemon 控制通道端到端:RPC `list`/`stat`/`delete`/`move`/`copy` 直达
//! CtrlPool,覆盖远端往返、本机短路、离线快错、对端硬杀有界失败与重启
//! 复通、双向并发、远端错误,以及 ctrl v2 操作面(删除/移动/同机复制/
//! 跨设备拒绝)与登出池拆卸回归。
//!
//! 覆盖两档(沿 daemon_e2e 样板):
//! - 进程内双 Daemon(Pair):list/stat 远端往返(条目/顺序/过滤/可再入)、
//!   本机短路(不经网络)、双向并发打洞、远端 NOT_FOUND 透传;
//! - 真实 `mtdssd` 子进程当 B:硬杀后池内陈旧连接有界失败(≤30s)→
//!   presence 翻离线 → code 3 → 重启复通。
//!
//! 全部测试限定 worker_threads = 2:并行负载下(全 workspace 并跑)默认
//! workers = cores 会过度订阅机器,饿死 ctrl 通道的 15s 请求预算造成
//! list_remote_roundtrip 偶发超时;本文件测试均为 IO 型,2 worker 足够。

use mtdss_core::identity::DeviceIdentity;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 常规调用超时:远端 list/stat 含打洞(send_request 15s 上界 + 信号往返),
/// 给足余量防悬挂;超时即测试失败(暴露卡点)。
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// 同文件用例并发闸(测试级削峰):cargo 默认按 CPU 数并行跑用例,每用例
/// 自带 2-worker reactor + 进程内双 daemon + 独立信令服务器,6 CPU 机器上
/// 6 并发已过订;叠加硬杀用例 10 万文件垫目录的 CPU/磁盘突刺,其余用例的
/// 打洞/请求墙钟预算被饿穿(list_remote_roundtrip 偶发 15s 超时的根因,
/// 全量跑法下基线可复现)。限并发到 3(约半 CPU 预算),单用例预算回到
/// 余量内;套件总时长增加有限(用例多为 IO 等待)。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(3)));

/// 持有至用例体结束的并发许可(用例体开头取,async 块结束即还)。
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

/// IPC 完整登录:login.request_code(dev 回显取码)→ login.verify。
async fn ipc_login(client: &IpcClient, server: &str, email: &str, name: &str) -> Value {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
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

/// dev-mode 验证码直接回带,REST 直注注册(与 daemon_e2e 同款)。daemon 的
/// login.verify 对同一 pubkey 幂等 upsert(device_id=sha256(pubkey))。
async fn register_rest(
    addr: std::net::SocketAddr,
    email: &str,
    pubkey_hex: &str,
    name: &str,
) -> String /*device_id*/ {
    let client = reqwest::Client::new();
    let c: Value = client
        .post(format!("http://{addr}/auth/request-code"))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v: Value = client
        .post(format!("http://{addr}/auth/verify"))
        .json(&json!({
            "email": email,
            "code": c["code"].as_str().unwrap(),
            "pubkey": pubkey_hex,
            "name": name,
            "platform": "test"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["device_id"].as_str().unwrap().into()
}

/// 轮询 status.online 直到等于 want(在线循环异步起连,不能同步断言)。
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

/// 轮询 devices 快照直到 peer 的 online 等于 want(presence 增量维护缓存)。
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
            "devices 快照未在 {secs}s 内看到 {dev} online={want}: {devs}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 列表结果的 name 序(测试断言的统一投影)。
fn names_of(v: &Value) -> Vec<&str> {
    v["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().expect("entry name"))
        .collect()
}

// ---------- 进程内双 daemon 测试台 ----------

/// 进程内双 daemon 测试台:独立信令服务器 + 两侧独立 home/IPC 客户端,
/// 双方已登录且 A 的 devices 快照已见到 B 在线(与 daemon_e2e::Pair 同构)。
struct Pair {
    /// home 保活到测试结束(TempDir 在此 drop 即删除)。
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    ca: IpcClient,
    cb: IpcClient,
    dev_a: String,
    dev_b: String,
    /// dev 信令服务器地址(`http://{addr}`;登出→重登用例需要重放 login.*)。
    server: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        let (addr, state) = spawn_test_server().await;
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        let ra = ipc_login(&ca, &server, email, "A").await;
        let rb = ipc_login(&cb, &server, email, "B").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let dev_a = ra["device_id"].as_str().unwrap().to_string();
        let dev_b = rb["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&ca, &dev_b, true, 30).await;
        Self {
            _home_a: home_a,
            _home_b: home_b,
            _state: state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_a,
            dev_b,
            server,
        }
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

// ---------- 真实 mtdssd 子进程(硬杀/恢复面) ----------

/// 真实子进程的 stdout/stderr 捕获:失败文案直接内嵌,CI 挂起时日志指认卡点。
struct ChildPipes {
    out_buf: Arc<tokio::sync::Mutex<Vec<u8>>>,
    err_buf: Arc<tokio::sync::Mutex<Vec<u8>>>,
    drains: Vec<tokio::task::JoinHandle<()>>,
}

impl ChildPipes {
    async fn snapshot(&self) -> String {
        let out = String::from_utf8_lossy(&self.out_buf.lock().await).into_owned();
        let err = String::from_utf8_lossy(&self.err_buf.lock().await).into_owned();
        format!("--- stdout ---\n{out}\n--- stderr ---\n{err}")
    }
}

/// 起一个真实 `mtdssd` 子进程(独立 MTDSS_HOME)。stdout 与 stderr 由后台
/// 任务**并发持续抽干**到 EOF(子进程退出)——stderr 只建管道不抽干时,
/// 日志写满管道缓冲会让子进程 write 阻塞、任何后续 `wait` 永不返回
/// (双抽干在 spawn 时建立,kill 与正常退出两条收尾路径都因此安全)。
/// kill_on_drop 保证测试失败也不残留进程。
fn spawn_mtdssd_child(home: &Path, recv_dir: &Path) -> (tokio::process::Child, ChildPipes) {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mtdssd"));
    cmd.env("MTDSS_HOME", home)
        .env("RUST_LOG", "info")
        .arg("--recv-dir")
        .arg(recv_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().expect("spawn mtdssd child");
    let mut pipes = ChildPipes {
        out_buf: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        err_buf: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        drains: Vec::new(),
    };
    let stdout = child.stdout.take().expect("child stdout piped");
    let stderr = child.stderr.take().expect("child stderr piped");
    pipes
        .drains
        .push(tokio::spawn(drain_into(stdout, pipes.out_buf.clone())));
    pipes
        .drains
        .push(tokio::spawn(drain_into(stderr, pipes.err_buf.clone())));
    (child, pipes)
}

/// 子进程收尾统一入口(kill 与正常退出两条路径共用):管道抽干已在
/// spawn 时并发建立(见 [`spawn_mtdssd_child`]),此后 `wait` 不再会被
/// 满缓冲卡死。`kill=true` 硬杀后等待;`kill=false` 自然退出等待。
async fn reap_child(mut child: tokio::process::Child, kill: bool) -> std::process::ExitStatus {
    if kill {
        child.start_kill().expect("kill child");
    }
    child.wait().await.expect("reap child")
}

/// 单条管道的持续抽干:读到 EOF(子进程退出)为止。
async fn drain_into(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    buf: std::sync::Arc<tokio::sync::Mutex<Vec<u8>>>,
) {
    use tokio::io::AsyncReadExt as _;
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.lock().await.extend_from_slice(&chunk[..n]),
        }
    }
}

/// 轮询连接直到子进程 IPC 就绪(单次连接限时 2s,防 interprocess 忙等自旋)。
async fn wait_client(home: &Path, secs: u64) -> IpcClient {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(Ok(c)) =
            tokio::time::timeout(Duration::from_secs(2), IpcClient::connect(home)).await
        {
            return c;
        }
        assert!(Instant::now() < deadline, "子进程 IPC {secs}s 内未就绪");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 起子进程 + 等 IPC 就绪 + IPC 登录,返回 (child, pipes, client, device_id)。
async fn spawn_daemon_child(
    home: &Path,
    recv_dir: &Path,
    server: &str,
    email: &str,
    name: &str,
) -> (tokio::process::Child, ChildPipes, IpcClient, String) {
    let (child, pipes) = spawn_mtdssd_child(home, recv_dir);
    let client = wait_client(home, 20).await;
    let r = ipc_login(&client, server, email, name).await;
    let dev = r["device_id"].as_str().expect("device_id").to_string();
    (child, pipes, client, dev)
}

// ---------- 远端往返 ----------

/// B 侧 tempdir 混合条目 → A 经 ctrl 通道 list 根卷与该目录、stat 文件;
/// 条目/顺序/过滤/`path` 可再入全对。`x.mtdss-part` 是「保留名精确匹配」
/// 的负样本:非精确 `.mtdss-part`,必须照常列出。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_remote_roundtrip() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("ls@x.y").await;

        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("b1.txt"), b"hello").unwrap();
        std::fs::create_dir_all(src.path().join("子目录")).unwrap();
        std::fs::write(src.path().join("子目录").join("c.txt"), b"c").unwrap();
        std::fs::create_dir_all(src.path().join(".mtdss")).unwrap();
        std::fs::write(src.path().join(".mtdss").join("secret.bin"), b"x").unwrap();
        std::fs::write(src.path().join("x.mtdss-part"), b"junk").unwrap();

        // 根卷列表:Windows 至少含本机系统盘 C:;Unix 恰一条 /
        let root = call(&p.ca, "list", json!({ "device": p.dev_b, "path": "" }))
            .await
            .expect("list 根卷应成功");
        let entries = root["entries"].as_array().expect("entries array").clone();
        assert!(!entries.is_empty(), "根卷列表非空: {root}");
        #[cfg(windows)]
        {
            let c = entries
                .iter()
                .find(|e| e["name"].as_str() == Some("C:"))
                .cloned()
                .expect("根卷含 C:");
            assert_eq!(c["path"].as_str(), Some("C:\\"), "根条目 path 可再入: {c}");
        }
        #[cfg(unix)]
        {
            assert_eq!(entries.len(), 1, "Unix 根卷恰一条: {entries:?}");
            assert_eq!(entries[0]["name"].as_str(), Some("/"));
            assert_eq!(entries[0]["path"].as_str(), Some("/"));
        }

        // tempdir 列表:目录优先(子目录)、组内字节升序(b1.txt < x.mtdss-part)、
        // .mtdss 整目录滤除、x.mtdss-part 非保留名照常列出
        let ls = call(
            &p.ca,
            "list",
            json!({ "device": p.dev_b, "path": src.path().to_string_lossy() }),
        )
        .await
        .expect("list tempdir 应成功");
        assert_eq!(ls["truncated"].as_bool(), Some(false), "未截断: {ls}");
        assert_eq!(
            names_of(&ls),
            vec!["子目录", "b1.txt", "x.mtdss-part"],
            "条目/顺序/过滤: {ls}"
        );
        let entries = ls["entries"].as_array().unwrap();
        assert_eq!(entries[0]["is_dir"].as_bool(), Some(true), "子目录 is_dir");
        assert_eq!(entries[1]["is_dir"].as_bool(), Some(false), "b1.txt is_dir");

        // path 可再入:子目录条目的 path 再 list 得 c.txt
        let sub_path = entries[0]["path"].as_str().unwrap().to_string();
        let sub = call(&p.ca, "list", json!({ "device": p.dev_b, "path": sub_path }))
            .await
            .expect("再入子目录应成功");
        assert_eq!(names_of(&sub), vec!["c.txt"], "子目录内容: {sub}");

        // stat b1.txt(用列表条目给出的 path 再入):size = 内容长度
        let b1_path = entries[1]["path"].as_str().unwrap().to_string();
        let st = call(&p.ca, "stat", json!({ "device": p.dev_b, "path": b1_path }))
            .await
            .expect("stat b1.txt 应成功");
        assert_eq!(st["entry"]["name"].as_str(), Some("b1.txt"), "stat: {st}");
        assert_eq!(st["entry"]["size"].as_u64(), Some(5));
        assert_eq!(st["entry"]["is_dir"].as_bool(), Some(false));

        p.shutdown().await;
    });
    run.await.expect("roundtrip e2e exceeded 120s");
}

// ---------- 本机短路 ----------

/// list/stat device=本机 → core fs 直调,不经网络(B 下线亦成)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_self_shortcircuit() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("self@x.y").await;
        // B 先下线:短路路径不得依赖任何网络侧
        p.daemon_b.shutdown().await.unwrap();

        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("solo.txt"), b"only").unwrap();

        let ls = call(
            &p.ca,
            "list",
            json!({ "device": p.dev_a, "path": src.path().to_string_lossy() }),
        )
        .await
        .expect("本机 list 应短路成功");
        assert_eq!(names_of(&ls), vec!["solo.txt"], "本机 list: {ls}");

        let st = call(
            &p.ca,
            "stat",
            json!({ "device": p.dev_a, "path": src.path().join("solo.txt").to_string_lossy() }),
        )
        .await
        .expect("本机 stat 应短路成功");
        assert_eq!(st["entry"]["size"].as_u64(), Some(4), "本机 stat: {st}");

        let _ = p.daemon_a.shutdown().await;
    });
    run.await.expect("self e2e exceeded 120s");
}

// ---------- 离线快错 ----------

/// B 仅 REST 注册(无 daemon,快照 online=false)→ A list B → code 3,
/// 且调用 < 2s(不打洞不悬等)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_peer_offline_is_fast_error() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        // B 先 REST 注册(无 daemon):A 连上后的目录快照里 B online=false
        let id_b = DeviceIdentity::generate();
        let dev_b = register_rest(addr, "off@x.y", &hex::encode(id_b.public_key().as_bytes()), "B")
            .await;
        ipc_login(&ca, &server, "off@x.y", "A").await;
        wait_online(&ca, true, 30).await;

        let t0 = Instant::now();
        let err = call(&ca, "list", json!({ "device": dev_b, "path": "" }))
            .await
            .expect_err("离线设备必须失败");
        let elapsed = t0.elapsed();
        assert_eq!(err.code, 3, "离线是 code 3: {err:?}");
        assert!(elapsed < Duration::from_secs(2), "离线快错 <2s,实际 {elapsed:?}: {err:?}");

        daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("offline e2e exceeded 120s");
}

/// 离线 code 3 的正式契约:B 仅 REST 注册(无 daemon)→ A list B →
/// code 3 且 message 为「设备离线: {device}」(CtrlErr::PeerOffline 钉死
/// 的映射,RPC 面不因 Task 5 收口漂移)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_rpc_offline_code3() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        // B 先 REST 注册(无 daemon):快照里 B online=false → 离线预检命中
        let id_b = DeviceIdentity::generate();
        let dev_b = register_rest(addr, "off3@x.y", &hex::encode(id_b.public_key().as_bytes()), "B")
            .await;
        ipc_login(&ca, &server, "off3@x.y", "A").await;
        wait_online(&ca, true, 30).await;

        let err = call(&ca, "list", json!({ "device": dev_b, "path": "" }))
            .await
            .expect_err("离线设备必须失败");
        assert_eq!(err.code, 3, "离线是 code 3: {err:?}");
        assert!(
            err.message.contains(&format!("设备离线: {dev_b}")),
            "message 必须是钉死的离线文案: {err:?}"
        );

        daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("offline-code3 e2e exceeded 120s");
}

/// 参数校验契约:list 的 device 与 path 都必填(path 允许空串=根卷);
/// 缺任一键 → -32602。参数检查先于 CtrlPool 与登录态,未登录 daemon 即可断言。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_params_validation() {
    let _gate = case_gate().await;
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let ca = IpcClient::connect(home.path()).await.unwrap();

    // 缺 path → -32602(空串合法,键缺失不合法)
    let err = call(&ca, "list", json!({ "device": "whatever" }))
        .await
        .expect_err("缺 path 必须被拒");
    assert_eq!(err.code, -32602, "缺 path 是 -32602: {err:?}");

    // 缺 device → -32602
    let err = call(&ca, "list", json!({ "path": "" }))
        .await
        .expect_err("缺 device 必须被拒");
    assert_eq!(err.code, -32602, "缺 device 是 -32602: {err:?}");

    daemon.shutdown().await.unwrap();
}

// ---------- 对端硬杀:有界失败 → 离线 → 恢复 ----------

/// 真实 mtdssd 子进程当 B:list 成功(建池)→ 硬杀 → 快照尚在线期间 list
/// 有界失败(code 1,≤ 30s:陈旧池连接 15s 请求上界 + 驱逐重试整程打洞)
/// → presence 翻离线 → code 3 → 重启 B(同 home 同身份)→ list 复通。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_daemon_killed_list_fails_bounded_and_recovers() {
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let _gate = case_gate().await;
        let (addr, _state) = spawn_test_server().await;
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox");

        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let (child_b1, pipes_b1, cb, dev_b) =
            spawn_daemon_child(home_b.path(), &recv_b, &server, "kill@x.y", "B").await;
        ipc_login(&ca, &server, "kill@x.y", "A").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("victim.txt"), b"killme").unwrap();
        let b_path = src.path().to_string_lossy().into_owned();

        // 大目录:让 B 的 list 处理耗数百毫秒级,kill 才能确定性地落在请求
        // 中途(健康连接上的往返只有 ~10ms,任何 kill 时序都追不上;10 万
        // 条的枚举下限数百毫秒,150ms 的杀点两侧余量都足)。
        let big = tempfile::tempdir().unwrap();
        for i in 0..100_000u32 {
            std::fs::write(big.path().join(format!("f{i:06}.txt")), b"x").unwrap();
        }
        let big_path = big.path().to_string_lossy().into_owned();

        // 首次 list 成功:打通连接并入池
        let ls = call(&ca, "list", json!({ "device": dev_b, "path": b_path }))
            .await
            .expect("首次 list 应成功");
        assert_eq!(names_of(&ls), vec!["victim.txt"], "首次 list: {ls}");

        // 硬杀 B(真实进程死亡,进程内 drop 模拟不了)。list 先行发起——
        // 快照尚在线,离线预检已过、请求已在 B 的处理中途——杀时落在请求
        // 中途。(本地进程死亡 → FIN → presence 翻转是毫秒级,先杀后发会
        // 先撞离线预检;健康往返 ~10ms,后杀追不上——故用大目录垫长窗口。)
        let ca2 = IpcClient::connect(home_a.path()).await.unwrap();
        let dev_for_task = dev_b.clone();
        let list_task = tokio::spawn(async move {
            call(&ca2, "list", json!({ "device": dev_for_task, "path": big_path })).await
        });
        tokio::time::sleep(Duration::from_millis(150)).await; // 预检已过、请求在途
        let t0 = Instant::now();
        let status = reap_child(child_b1, true).await;
        assert!(!status.success(), "被杀对端不得干净退出");

        // 快照尚在线期间发起的 list:有界失败 code 1 ≤ 30s
        let err = list_task
            .await
            .expect("list task panicked")
            .expect_err("对端死亡后 list 必须失败");
        let elapsed = t0.elapsed();
        assert_eq!(err.code, 1, "连接级失败是 code 1: {err:?}");
        assert!(elapsed <= Duration::from_secs(30), "有界失败 ≤30s,实际 {elapsed:?}: {err:?}");

        // 等 presence 翻离线 → 离线预检快错 code 3
        wait_peer_online(&ca, &dev_b, false, 60).await;
        let err = call(&ca, "list", json!({ "device": dev_b, "path": "" }))
            .await
            .expect_err("presence 离线后必须失败");
        assert_eq!(err.code, 3, "离线预检是 code 3: {err:?}");

        // 重启 B(同 home 同身份收敛同一设备)→ 等在线 → list 复通
        drop(cb);
        let (child_b2, pipes_b2, _cb2, dev_b2) =
            spawn_daemon_child(home_b.path(), &recv_b, &server, "kill@x.y", "B").await;
        assert_eq!(dev_b2, dev_b, "同 home 同身份,重启收敛同一设备");
        wait_peer_online(&ca, &dev_b, true, 60).await;
        let ls2 = call(&ca, "list", json!({ "device": dev_b, "path": b_path }))
            .await
            .expect("重启后 list 应复通");
        assert_eq!(names_of(&ls2), vec!["victim.txt"], "重启后 list 复通: {ls2}");

        let diag1 = pipes_b1.snapshot().await;
        let diag2 = pipes_b2.snapshot().await;
        eprintln!("--- 被杀 B stdout 尾部 ---\n{}", tail(&diag1));
        eprintln!("--- 重启 B stdout 尾部 ---\n{}", tail(&diag2));

        let _ = reap_child(child_b2, true).await;
        daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("kill-recover e2e exceeded 240s");
}

/// 诊断日志尾部(挂死排障看最近在做什么)。
fn tail(text: &str) -> String {
    text.chars()
        .rev()
        .take(600)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// ---------- 双向并发 ----------

/// A/B 同时互相 list 各自 tempdir,双双成功(双向打洞并发会话互不串扰,
/// session id 过滤交叉信令)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_bidirectional_concurrent() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("bidir-list@x.y").await;

        let src_a = tempfile::tempdir().unwrap();
        std::fs::write(src_a.path().join("甲.txt"), b"a").unwrap();
        let src_b = tempfile::tempdir().unwrap();
        std::fs::write(src_b.path().join("乙.txt"), b"b").unwrap();
        let path_a = src_a.path().to_string_lossy().into_owned();
        let path_b = src_b.path().to_string_lossy().into_owned();

        let (ra, rb) = tokio::join!(
            call(&p.ca, "list", json!({ "device": p.dev_b, "path": path_b })),
            call(&p.cb, "list", json!({ "device": p.dev_a, "path": path_a })),
        );
        let ra = ra.expect("A→B list 应成功");
        assert_eq!(names_of(&ra), vec!["乙.txt"], "A 看到的 B 目录: {ra}");
        let rb = rb.expect("B→A list 应成功");
        assert_eq!(names_of(&rb), vec!["甲.txt"], "B 看到的 A 目录: {rb}");

        p.shutdown().await;
    });
    run.await.expect("bidir e2e exceeded 120s");
}

// ---------- 远端错误透传 ----------

/// stat 远端不存在的路径 → code 1,message 含 `远端: NOT_FOUND`
/// (对端 FsError 经 CtrlError 透传,RPC 层加「远端: 」前缀)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stat_remote_missing_is_peer_not_found() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("nf@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let missing = src.path().join("不存在.txt").to_string_lossy().into_owned();

        let err = call(&p.ca, "stat", json!({ "device": p.dev_b, "path": missing }))
            .await
            .expect_err("远端不存在的路径必须失败");
        assert_eq!(err.code, 1, "远端 FS 错误是 code 1: {err:?}");
        assert!(
            err.message.contains("远端: NOT_FOUND"),
            "message 须含「远端: NOT_FOUND」: {err:?}"
        );

        p.shutdown().await;
    });
    run.await.expect("not-found e2e exceeded 120s");
}

// ---------- ctrl v2 操作面(delete / move / copy)----------

/// 回收面用例的临时目录:Unix 建在 $HOME 挂载点内(freedesktop home trash
/// 直接适用;/tmp 多为独立 tmpfs,回收区可用性不可控,与 core fs 测试同
/// 口径);Windows 任意处(IFileOperation 全卷可用)。
#[cfg(unix)]
fn recycle_tempdir() -> tempfile::TempDir {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            if let Ok(d) = tempfile::Builder::new()
                .prefix("mtdss-ctrl-e2e-recycle")
                .tempdir_in(home)
            {
                return d;
            }
        }
    }
    tempfile::tempdir().unwrap()
}

#[cfg(windows)]
fn recycle_tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A 经 RPC delete B 的一批路径:文件与目录整树进回收站(recycled=true)、
/// B 盘上消失;部分不存在逐条进 failed 表(NOT_FOUND),批不半途弃废。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_remote_roundtrip() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("del@x.y").await;
        let src = recycle_tempdir();
        std::fs::write(src.path().join("gone.txt"), b"bye").unwrap();
        std::fs::create_dir_all(src.path().join("gone_dir")).unwrap();
        std::fs::write(src.path().join("gone_dir").join("inner.txt"), b"i").unwrap();
        let gone_txt = src.path().join("gone.txt").to_string_lossy().into_owned();
        let gone_dir = src.path().join("gone_dir").to_string_lossy().into_owned();
        let missing = src.path().join("从未存在.txt").to_string_lossy().into_owned();

        let r = call(
            &p.ca,
            "delete",
            json!({
                "device": p.dev_b,
                "paths": [gone_txt, gone_dir, missing],
                "force": false,
            }),
        )
        .await;
        // 设施级回收不可用(异常环境)只可能整批报错:Unix 侧放行跳过,
        // Windows IFileOperation 恒可用必须成功(与 core fs 测试同口径)。
        let r = match r {
            Ok(r) => r,
            #[cfg(unix)]
            Err(e) if e.message.contains("回收站不可用") => {
                eprintln!("skip recycle assertion: freedesktop trash unavailable: {e:?}");
                p.shutdown().await;
                return;
            }
            Err(e) => panic!("delete 应成功: {e:?}"),
        };
        assert_eq!(r["deleted"].as_u64(), Some(2), "文件+目录两条成功: {r}");
        assert_eq!(r["recycled"].as_bool(), Some(true), "回收优先: {r}");
        let failed = r["failed"].as_array().expect("failed array");
        assert_eq!(failed.len(), 1, "缺失一条进失败表: {r}");
        assert_eq!(failed[0]["code"].as_str(), Some("NOT_FOUND"));
        assert_eq!(failed[0]["path"].as_str(), Some(missing.as_str()));

        // B 盘上确已消失
        assert!(!src.path().join("gone.txt").exists());
        assert!(!src.path().join("gone_dir").exists());

        p.shutdown().await;
    });
    run.await.expect("delete e2e exceeded 120s");
}

/// force=true 永久删除:recycled=false,盘上消失(回收站不经过)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_force_permanent() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("delf@x.y").await;
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("永别.txt"), b"x").unwrap();
        let victim = src.path().join("永别.txt").to_string_lossy().into_owned();

        let r = call(
            &p.ca,
            "delete",
            json!({ "device": p.dev_b, "paths": [victim], "force": true }),
        )
        .await
        .expect("force delete 应成功");
        assert_eq!(r["deleted"].as_u64(), Some(1), "恰删一条: {r}");
        assert_eq!(
            r["recycled"].as_bool(),
            Some(false),
            "force 永久删不走回收: {r}"
        );
        assert!(r["failed"].as_array().unwrap().is_empty(), "无失败: {r}");
        assert!(!src.path().join("永别.txt").exists(), "盘上消失");

        p.shutdown().await;
    });
    run.await.expect("delete-force e2e exceeded 120s");
}

/// A 经 RPC move 改名 B 上的一条文件与一个目录整树(同卷 rename 直通):
/// moved 计数、failed 空、源不在、目标内容保真。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_remote_same_volume() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("mv@x.y").await;
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("旧名.txt"), b"mvme").unwrap();
        std::fs::create_dir_all(src.path().join("树")).unwrap();
        std::fs::write(src.path().join("树").join("叶.txt"), b"leaf").unwrap();

        let r = call(
            &p.ca,
            "move",
            json!({
                "device": p.dev_b,
                "pairs": [
                    {
                        "from": src.path().join("旧名.txt").to_string_lossy(),
                        "to": src.path().join("新名.txt").to_string_lossy(),
                    },
                    {
                        "from": src.path().join("树").to_string_lossy(),
                        "to": src.path().join("树改名").to_string_lossy(),
                    },
                ],
            }),
        )
        .await
        .expect("move 应成功");
        assert_eq!(r["moved"].as_u64(), Some(2), "两对全成: {r}");
        assert!(r["failed"].as_array().unwrap().is_empty(), "无失败: {r}");

        // B 盘上:源不在,目标在且内容保真(整树随改名整体迁移)
        assert!(!src.path().join("旧名.txt").exists());
        assert_eq!(
            std::fs::read(src.path().join("新名.txt")).unwrap(),
            b"mvme"
        );
        assert!(!src.path().join("树").exists());
        assert_eq!(
            std::fs::read(src.path().join("树改名").join("叶.txt")).unwrap(),
            b"leaf"
        );

        p.shutdown().await;
    });
    run.await.expect("move e2e exceeded 120s");
}

/// A 经 RPC copy 在 B 内复制(源 tempdir → 目标 tempdir):copied 计数、
/// failed 空、源仍在、目标内容一致(整树)。同 daemon 内的同机复制,
/// 不经网络传输面(对照传输面用例)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copy_remote_same_device() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("cp@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("单文件.txt"), b"copied").unwrap();
        std::fs::create_dir_all(src.path().join("嵌套")).unwrap();
        std::fs::write(src.path().join("嵌套").join("深.txt"), b"deep").unwrap();

        let r = call(
            &p.ca,
            "copy",
            json!({
                "device": p.dev_b,
                "paths": [
                    src.path().join("单文件.txt").to_string_lossy(),
                    src.path().join("嵌套").to_string_lossy(),
                ],
                "dir": dst.path().to_string_lossy(),
            }),
        )
        .await
        .expect("copy 应成功");
        assert_eq!(r["copied"].as_u64(), Some(2), "两条复制成功: {r}");
        assert!(r["failed"].as_array().unwrap().is_empty(), "无失败: {r}");

        // 源仍在(复制非移动),目标内容一致
        assert!(src.path().join("单文件.txt").exists());
        assert!(src.path().join("嵌套").exists());
        assert_eq!(
            std::fs::read(dst.path().join("单文件.txt")).unwrap(),
            b"copied"
        );
        assert_eq!(
            std::fs::read(dst.path().join("嵌套").join("深.txt")).unwrap(),
            b"deep"
        );

        p.shutdown().await;
    });
    run.await.expect("copy e2e exceeded 120s");
}

/// move/copy 目标含跨设备(to_device ≠ device)→ 直接 code 1 拒绝
/// (「跨设备请用复制/传输」:跨设备走传输面,不是 ctrl 同机操作面)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_cross_device_rejected() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let _gate = case_gate().await;
        let p = Pair::new("mvx@x.y").await;
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("原地.txt"), b"stay").unwrap();

        let err = call(
            &p.ca,
            "move",
            json!({
                "device": p.dev_b,
                "to_device": p.dev_a,
                "pairs": [
                    {
                        "from": src.path().join("原地.txt").to_string_lossy(),
                        "to": src.path().join("异地.txt").to_string_lossy(),
                    },
                ],
            }),
        )
        .await
        .expect_err("跨设备 move 必须被拒");
        assert_eq!(err.code, 1, "跨设备是 code 1: {err:?}");
        assert!(
            err.message.contains("跨设备请用复制/传输"),
            "message 必须是钉死的跨设备指引: {err:?}"
        );
        // 源保持原状(拒绝发生在任何 fs 动作之前)
        assert!(src.path().join("原地.txt").exists());

        p.shutdown().await;
    });
    run.await.expect("move-cross e2e exceeded 120s");
}

/// 登出池拆卸回归:A delete 建立一条 ctrl 连接入池 → logout(池 reset)
/// → 重登(同 home 同身份收敛同一设备)→ delete 仍工作。池拆卸后请求
/// 必须从打洞重来且成功——陈旧连接若未拆也不会掩盖问题(逐次驱逐重试
/// 兜底),本用例钉的是「拆卸不破坏后续操作」的功能回归面。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_evicts_ctrl_pool() {
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let _gate = case_gate().await;
        let p = Pair::new("lo@x.y").await;
        let src = tempfile::tempdir().unwrap();

        // 第一轮:delete 打通 ctrl 连接并入池(force=true,免回收设施依赖)
        std::fs::write(src.path().join("轮回.txt"), b"1").unwrap();
        let victim = src.path().join("轮回.txt").to_string_lossy().into_owned();
        let r = call(
            &p.ca,
            "delete",
            json!({ "device": p.dev_b, "paths": [victim], "force": true }),
        )
        .await
        .expect("首轮 delete 应成功");
        assert_eq!(r["deleted"].as_u64(), Some(1), "首轮: {r}");

        // 登出(池拆卸)→ 重登 → 第二轮 delete 仍工作
        call(&p.ca, "login.logout", json!({}))
            .await
            .expect("logout 应成功");
        ipc_login(&p.ca, &p.server, "lo@x.y", "A").await;
        wait_online(&p.ca, true, 30).await;
        wait_peer_online(&p.ca, &p.dev_b, true, 30).await;
        std::fs::write(src.path().join("轮回.txt"), b"2").unwrap();
        let r2 = call(
            &p.ca,
            "delete",
            json!({ "device": p.dev_b, "paths": [victim], "force": true }),
        )
        .await
        .expect("重登后 delete 应仍工作(池已拆,重新打洞)");
        assert_eq!(r2["deleted"].as_u64(), Some(1), "重登后: {r2}");
        assert!(!src.path().join("轮回.txt").exists());

        p.shutdown().await;
    });
    run.await.expect("logout e2e exceeded 180s");
}
