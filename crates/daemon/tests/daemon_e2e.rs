//! daemon 传输端到端:RPC `send`/`set_recv_dir`、常驻接收、事件面与跨进程断链。
//!
//! 覆盖三档:
//! - 进程内双 Daemon(独立 home → 独立命名管道)覆盖常规收发、send_log 事件、
//!   接收目录迁移与持久化、并发互斥、拒绝报告、同名冲突改名、TLS/STUN 全链路;
//! - 真实 `mtdssd` 子进程(独立 MTDSS_HOME)覆盖硬杀与跨进程续传——进程内
//!   drop 必发隐式 CONNECTION_CLOSE,模拟不了静默死亡(与 cli_e2e 同理);
//!   `MTDSS_TEST_*` 调优旋钮经子进程环境注入(daemon 启动时 apply_test_env
//!   一次性读取),替代旧 CLI 的按进程注入面。
//!
//! cli_e2e → daemon_e2e 移植对照表见任务报告(task-7-report.md)。

use mtdss_core::identity::DeviceIdentity;
use mtdss_core::transport::intake::{part_path, record_path};
use mtdss_daemon::{settings::Settings, Daemon, DaemonOpts};
use mtdss_ipc::{EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::{spawn_test_server, AppState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// send 是长调用(传输全程在 RPC 内),单独给更长上界。
const SEND_TIMEOUT: Duration = Duration::from_secs(120);

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
/// 返回 verify 结果(含 device_id)。`ca` 为自签 TLS 场景的 CA 路径。
async fn ipc_login(
    client: &IpcClient,
    server: &str,
    email: &str,
    name: &str,
    ca: Option<&Path>,
) -> Value {
    let mut params = json!({ "server": server, "email": email });
    if let Some(ca) = ca {
        params["ca"] = json!(ca.to_string_lossy());
    }
    let code = call(client, "login.request_code", params)
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

/// dev-mode 验证码直接回带,REST 直注注册(自 cli_e2e 移入;flow::register
/// 是交互式 stdin 流程)。daemon 的 login.verify 对同一 pubkey 幂等 upsert
/// (device_id=sha256(pubkey)),两侧收敛同一 device_id。
async fn register_rest(
    addr: std::net::SocketAddr,
    email: &str,
    pubkey_hex: &str,
    name: &str,
) -> (String /*jwt*/, String /*refresh*/, String /*device_id*/) {
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
    (
        v["jwt"].as_str().unwrap().into(),
        v["refresh"].as_str().unwrap().into(),
        v["device_id"].as_str().unwrap().into(),
    )
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

/// 轮询 devices 快照直到 peer 的 online 等于 want(在线循环 Presence 增量
/// 维护缓存;接收循环注册/断连都会经 presence 广播反映到快照)。
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

/// 轮询事件流直至谓词命中;超时panic(点名在等什么,便于排障)。
async fn wait_event(
    evs: &mut broadcast::Receiver<EventFrame>,
    secs: u64,
    what: &str,
    pred: impl Fn(&EventFrame) -> bool,
) -> EventFrame {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, evs.recv()).await {
            Ok(Ok(f)) => {
                if pred(&f) {
                    return f;
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                // 慢消费者丢帧不致命:send_log 量小,继续等后续帧
                eprintln!("事件通道滞后 {n} 帧,继续等待");
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
            Err(_) => panic!("等待事件超时({secs}s): {what}"),
        }
    }
}

/// 轮询收件区(.mtdss)直到传输已实际开动(首个条目出现)。3ms 轮距——
/// .part 是接收开写的最早信号,此刻动手硬杀必然落在传输中途
/// (与 cli_e2e 同款:等终名文件再杀,loopback 上窗口内就推完了)。
async fn wait_transfer_started(intake: &Path, budget: Duration) {
    let gone = Instant::now() + budget;
    loop {
        let started = std::fs::read_dir(intake)
            .map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.path().is_file()))
            .unwrap_or(false);
        if started {
            return;
        }
        assert!(
            Instant::now() < gone,
            "transfer never started (no intake entry) in {}",
            intake.display()
        );
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
}

/// 断言收件区只剩完成记录(恰 n 个 .json,无 .part 残留)。
fn assert_intake_clean(recv_dir: &Path, n: usize) {
    let intake = recv_dir.join(".mtdss");
    assert!(intake.is_dir(), "intake area must exist under {}", recv_dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&intake)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names.len(),
        n,
        "one finished record per file expected: {names:?}"
    );
    assert!(
        names.iter().all(|x| x.ends_with(".json")),
        "intake must hold only finished records (no .part): {names:?}"
    );
}

/// 随机填充并落盘,返回内容副本(收发两端字节一致性断言用)。
fn fill_random_file(path: &Path, size: usize) -> Vec<u8> {
    use rand::RngCore as _;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(path, &data).unwrap();
    data
}

/// 进程内双 daemon 测试台:独立信令服务器 + 两侧独立 home/IPC 客户端,
/// 双方已登录且 A 的 devices 快照已见到 B 在线(B 的常驻接收已注册)。
struct Pair {
    home_a: tempfile::TempDir,
    home_b: tempfile::TempDir,
    _state: Arc<AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    ca: IpcClient,
    cb: IpcClient,
    dev_a: String,
    dev_b: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        let (addr, state) = spawn_test_server().await;
        // 认证收口后仅预置账号可领码:测试台预置登录邮箱(幂等,下同)
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        let ra = ipc_login(&ca, &server, email, "A", None).await;
        let rb = ipc_login(&cb, &server, email, "B", None).await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let dev_a = ra["device_id"].as_str().unwrap().to_string();
        let dev_b = rb["device_id"].as_str().unwrap().to_string();
        wait_peer_online(&ca, &dev_b, true, 30).await;
        Self {
            home_a,
            home_b,
            _state: state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_a,
            dev_b,
        }
    }

    /// A 的默认收件目录(DaemonOpts.recv_dir = {home}/inbox)。
    fn recv_dir_a(&self) -> PathBuf {
        self.home_a.path().join("inbox")
    }

    /// B 的默认收件目录(DaemonOpts.recv_dir = {home}/inbox)。
    fn recv_dir_b(&self) -> PathBuf {
        self.home_b.path().join("inbox")
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

// ---------- 真实 mtdssd 子进程(硬杀/跨进程续传面) ----------

/// 真实子进程的 stdout/stderr 捕获:失败文案直接内嵌,CI 挂起时日志指认卡点。
struct ChildPipes {
    out_buf: Arc<tokio::sync::Mutex<Vec<u8>>>,
    err_buf: Arc<tokio::sync::Mutex<Vec<u8>>>,
    drains: Vec<tokio::task::JoinHandle<()>>,
}

impl ChildPipes {
    async fn snapshot(&self) -> (String, String) {
        (
            String::from_utf8_lossy(&self.out_buf.lock().await).into_owned(),
            String::from_utf8_lossy(&self.err_buf.lock().await).into_owned(),
        )
    }
}

/// 起一个真实 `mtdssd` 子进程,注入 `MTDSS_TEST_*` 调优 env(daemon 启动时
/// apply_test_env 一次性读取)。管道由后台任务持续抽干(info 日志写满管道
/// 会假死);kill_on_drop 保证测试失败也不残留进程。
fn spawn_mtdssd_child(
    home: &Path,
    recv_dir: &Path,
    env: &[(&str, &str)],
) -> (tokio::process::Child, ChildPipes) {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mtdssd"));
    cmd.env("MTDSS_HOME", home)
        .env("RUST_LOG", "info")
        .arg("--recv-dir")
        .arg(recv_dir)
        // stdin 显式置空:守护进程不读 stdin;置 null 让任何未来引入的
        // stdin 读取立即 EOF 显形,而非阻塞在继承的管道上。
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
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

/// 单条管道的持续抽干:读到 EOF(子进程退出)为止,分片取锁写共享缓冲
/// (不长期持锁,诊断看门狗可随时快照)。
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

/// 轮询连接直到子进程 IPC 就绪(进程起监听需数百毫秒)。单次连接限时 2s:
/// interprocess 对 ERROR_PIPE_BUSY 会无界自旋等待(与 daemon 内 try_connect
/// 同款防御),不给上界会把「就绪等待」变成永久悬挂。
async fn wait_client(home: &Path, secs: u64) -> IpcClient {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(Ok(c)) = tokio::time::timeout(Duration::from_secs(2), IpcClient::connect(home)).await
        {
            return c;
        }
        assert!(Instant::now() < deadline, "子进程 IPC {secs}s 内未就绪");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 起子进程 + 等 IPC 就绪 + IPC 登录,返回 (child, pipes, client, device_id)。
/// client 为 Arc:IpcClient 不可克隆,而 send 是长调用,测试要把调用放进
/// spawn 的 'static future 与本地轮询并存。
async fn spawn_daemon_child(
    home: &Path,
    recv_dir: &Path,
    server: &str,
    email: &str,
    name: &str,
    env: &[(&str, &str)],
) -> (tokio::process::Child, ChildPipes, Arc<IpcClient>, String) {
    let (child, pipes) = spawn_mtdssd_child(home, recv_dir, env);
    let client = Arc::new(wait_client(home, 20).await);
    let r = ipc_login(&client, server, email, name, None).await;
    let dev = r["device_id"].as_str().expect("device_id").to_string();
    (child, pipes, client, dev)
}

/// 诊断看门狗:每 15s 把子进程 stdout(tracing 默认写 stdout)尾部打到
/// 测试输出(定位挂死阶段用;随句柄 drop 停止)。
fn spawn_diag_loop(name: &'static str, buf: Arc<tokio::sync::Mutex<Vec<u8>>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut n = 0u32;
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            n += 1;
            let all = buf.lock().await;
            let text = String::from_utf8_lossy(&all);
            let tail: String = text.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
            eprintln!("--- diag[{name}] t+{}s stdout 尾部 ---\n{tail}", n * 15);
        }
    })
}

// ---------- 混合文件/目录落盘 ----------

/// 混合输入(目录树 + 单文件)经 RPC send 落进 B 的 recv_dir,逐 rel 字节
/// 一致;收件区只剩完成记录。(对号 cli_e2e:cli_send_recv_end_to_end +
/// cli_send_directory_and_files_multi_path 的多路径/目录还原语义。)
#[tokio::test(flavor = "multi_thread")]
async fn send_files_and_dirs_land_in_recv_dir() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "mp@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();

        // 预生成身份并 REST 注册(自 cli_e2e 移入的 register_rest 辅助),
        // 随后 daemon IPC 登录幂等 upsert 同一设备——两侧 device_id 必须收敛,
        // 交叉验证 REST 设备模型与 daemon 登录链路一致。
        let id_a = DeviceIdentity::generate();
        id_a.save_seed(&home_a.path().join("identity.seed")).unwrap();
        let id_b = DeviceIdentity::generate();
        id_b.save_seed(&home_b.path().join("identity.seed")).unwrap();
        let (_, _, dev_a) =
            register_rest(addr, "mp@x.y", &hex::encode(id_a.public_key().as_bytes()), "A").await;
        let (_, _, dev_b) =
            register_rest(addr, "mp@x.y", &hex::encode(id_b.public_key().as_bytes()), "B").await;
        assert_ne!(dev_a, dev_b);

        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        let ra = ipc_login(&ca, &server, "mp@x.y", "A", None).await;
        let rb = ipc_login(&cb, &server, "mp@x.y", "B", None).await;
        assert_eq!(
            ra["device_id"].as_str().unwrap(),
            dev_a,
            "IPC 登录必须收敛 REST 注册的同一设备"
        );
        assert_eq!(rb["device_id"].as_str().unwrap(), dev_b);
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        // 输入:目录(顶层 a.txt + 子目录 sub/b.bin)+ 单文件(unicode 名)
        let src = tempfile::tempdir().unwrap();
        let dir = src.path().join("目录");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let data_a = fill_random_file(&dir.join("a.txt"), 4096);
        let data_b = fill_random_file(&dir.join("sub").join("b.bin"), 128 * 1024);
        let single = src.path().join("多路径 单文件.bin");
        let data_c = fill_random_file(&single, 8192);

        let r = call_bounded(
            &ca,
            "send",
            json!({
                "files": [dir.to_string_lossy(), single.to_string_lossy()],
                "to": dev_b,
            }),
            SEND_TIMEOUT,
        )
        .await
        .expect("send 应成功");
        assert_eq!(r, json!({}), "成功返回空对象");

        // 按 manifest rel 落盘:目录内容还原 dir/...,单文件平铺
        let recv = home_b.path().join("inbox");
        for (got, data) in [
            (recv.join("目录").join("a.txt"), &data_a),
            (recv.join("目录").join("sub").join("b.bin"), &data_b),
            (recv.join("多路径 单文件.bin"), &data_c),
        ] {
            assert_eq!(
                std::fs::read(&got).unwrap(),
                *data,
                "content mismatch at {}",
                got.display()
            );
        }
        assert_intake_clean(&recv, 3);

        daemon_b.shutdown().await.unwrap();
        daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("multi-path e2e exceeded 120s");
}

// ---------- 双向背靠背(presence 槽回归锚点) ----------

/// A→B send 完成后**立即** B→A send:第二轮必须成功落盘。旧代码下发送流
/// 自建信令连接(服务器同设备 presence「后连者胜」),连接关闭时把发送方
/// 从 presence 整体摘除——接收循环的连接还在却收不到来件,对端的
/// ConnectRequest 被服务器丢弃,重试耗尽后失败。本测试是单连接收敛
/// (SignalHub)的回归锚点。
#[tokio::test(flavor = "multi_thread")]
async fn send_both_directions_back_to_back() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("bidir@x.y").await;

        let src = tempfile::tempdir().unwrap();
        let fa = src.path().join("双向 甲.bin");
        let da = fill_random_file(&fa, 64 * 1024);
        let fb = src.path().join("双向 乙.bin");
        let db = fill_random_file(&fb, 64 * 1024);

        // 第一轮 A→B
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [fa.to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("A→B send 应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(p.recv_dir_b().join("双向 甲.bin")).unwrap(),
            da,
            "A→B 内容必须一致"
        );

        // 立即第二轮 B→A:发送流连接刚关闭,旧代码此刻 A 已被摘出 presence
        let r = call_bounded(
            &p.cb,
            "send",
            json!({ "files": [fb.to_string_lossy()], "to": p.dev_a }),
            SEND_TIMEOUT,
        )
        .await
        .expect("B→A send 应成功(发送流不得影响本机收件面)");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(p.recv_dir_a().join("双向 乙.bin")).unwrap(),
            db,
            "B→A 内容必须一致"
        );

        p.shutdown().await;
    });
    run.await.expect("bidirectional e2e exceeded 150s");
}

// ---------- 常驻接收「取件晚于来件」窗口(hub 请求通道回归) ----------

/// B 登录前先订 B 事件流,一见本机 online=true(枢纽代际已安装、信令已
/// 上线)就**立刻**从 A 发送:ConnectRequest 大概率落在 B 常驻接收取到
/// 请求通道接收端之前(RECV_ROUND_PACE 级的轮询窗口)。修复后代际安装即
/// 建通道,取件之前的来件在缓冲排队,首轮即达;修复前该窗口随 CI 时序
/// 偶发把 ConnectRequest 落进零订阅者广播里静默丢弃,仅发送侧重试兜底
/// (nat-matrix 上已两次显形)。本测试钉死消费面语义并看守同代际取件后
/// 的继续消费(第二轮背靠背 send 验证轮循环回等、再取件)。
#[tokio::test(flavor = "multi_thread")]
async fn fresh_receiver_catches_immediate_request() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "fresh@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();

        // A 先行上线:发送面(枢纽订阅 + relay)就绪后才有「立刻发送」可言
        let _ra = ipc_login(&ca, &server, "fresh@x.y", "A", None).await;
        wait_online(&ca, true, 30).await;

        // B 登录前先订 B 事件流:online 事件只在翻转沿发一次,迟订必漏
        let mut evs_b = cb.events();
        let rb = ipc_login(&cb, &server, "fresh@x.y", "B", None).await;
        let dev_b = rb["device_id"].as_str().unwrap().to_string();

        // 一见 B online=true 立刻从 A 发送,不给常驻接收留「先取件后来件」
        // 的余地——窗口越窄,越接近 CI 矩阵里 B 唤醒跟不上的形态
        wait_event(&mut evs_b, 30, "B event.online=true", |f| {
            f.method == "event.online" && f.params["online"].as_bool() == Some(true)
        })
        .await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("即刻 来件.bin");
        let data = fill_random_file(&f, 16 * 1024);
        let r = call_bounded(
            &ca,
            "send",
            json!({ "files": [f.to_string_lossy()], "to": dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("上线瞬间发起的 send 应成功(请求通道保帧)");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(home_b.path().join("inbox").join("即刻 来件.bin")).unwrap(),
            data,
            "B 取件晚于来件时,请求通道必须保住该请求"
        );

        // 第二轮背靠背:轮循环干净返回后必须回等并在同一接收端上取到下一件
        let f2 = src.path().join("即刻 二.bin");
        let data2 = fill_random_file(&f2, 16 * 1024);
        let r = call_bounded(
            &ca,
            "send",
            json!({ "files": [f2.to_string_lossy()], "to": dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("第二轮 send 应成功(消费循环回等生命周期)");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(home_b.path().join("inbox").join("即刻 二.bin")).unwrap(),
            data2,
            "第二轮内容必须一致"
        );
        assert_intake_clean(&home_b.path().join("inbox"), 2);

        daemon_b.shutdown().await.unwrap();
        daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("fresh-receiver e2e exceeded 150s");
}

// ---------- send_log 事件面 ----------

/// send 期间至少广播一条清单行与一条完成行的 event.send_log
/// (订阅先于触发,事件在订阅缓冲里顺序排干)。
#[tokio::test(flavor = "multi_thread")]
async fn send_log_events_observed() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("log@x.y").await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("事件 甲.bin");
        fill_random_file(&f, 64 * 1024);

        // 先订阅再触发,避免漏帧
        let mut evs = p.ca.events();
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f.to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("send 应成功");
        assert_eq!(r, json!({}));

        // 传输期间广播的 send_log 帧已在订阅缓冲;带短限排干直到两类齐备
        let mut got_manifest = false;
        let mut got_done = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !got_manifest || !got_done {
            match tokio::time::timeout_at(deadline, evs.recv()).await {
                Ok(Ok(f)) if f.method == "event.send_log" => {
                    let line = f.params["line"].as_str().unwrap_or_default().to_string();
                    if line.contains("清单") {
                        got_manifest = true;
                    }
                    if line.contains("sent") {
                        got_done = true;
                    }
                }
                Ok(Ok(_)) => {}           // 其他事件:忽略
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    eprintln!("事件通道滞后 {n} 帧,继续等待");
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
                Err(_) => panic!(
                    "send_log 事件不全: 清单={got_manifest} 完成={got_done}"
                ),
            }
        }

        p.shutdown().await;
    });
    run.await.expect("send_log e2e exceeded 120s");
}

// ---------- 接收目录迁移与持久化 ----------

/// set_recv_dir:改目录 → 新传输落新目录(常驻接收下一轮生效);settings.json
/// 持久化;按入口同口径(main.rs:settings.recv_dir > {home}/inbox)重启后
/// 仍落新目录。
#[tokio::test(flavor = "multi_thread")]
async fn set_recv_dir_persists() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("rd@x.y").await;
        let inbox = p.recv_dir_b();

        // 第一传:落默认 inbox
        let src = tempfile::tempdir().unwrap();
        let f1 = src.path().join("迁移 一.txt");
        let d1 = fill_random_file(&f1, 8 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f1.to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("第一传应成功");
        assert_eq!(r, json!({}));
        assert_eq!(std::fs::read(inbox.join("迁移 一.txt")).unwrap(), d1);

        // 改目录:RPC 空对象 + 状态面即时反映 + settings.json 持久化
        let newdir = p.home_b.path().join("新收件目录");
        let r = call(
            &p.cb,
            "set_recv_dir",
            json!({ "dir": newdir.to_string_lossy() }),
        )
        .await
        .expect("set_recv_dir 应成功");
        assert_eq!(r, json!({}));
        let st = call(&p.cb, "status", json!({})).await.unwrap();
        assert_eq!(
            st["recv_dir"].as_str().unwrap(),
            newdir.to_string_lossy(),
            "status 必须即时反映新目录"
        );
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(p.home_b.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved["recv_dir"].as_str().unwrap(),
            newdir.to_string_lossy(),
            "settings.json 必须持久化新目录"
        );

        // 新传输落新目录(旧 inbox 不再收件)
        let f2 = src.path().join("迁移 二.txt");
        let d2 = fill_random_file(&f2, 16 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f2.to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("第二传应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(newdir.join("迁移 二.txt")).unwrap(),
            d2,
            "新传输必须落新目录"
        );
        assert!(
            !inbox.join("迁移 二.txt").exists(),
            "旧目录不得再收新件"
        );

        // 重启 B:客户端先弃(Windows 上句柄在,旧管道名不释放),按入口
        // 同口径解析 settings → daemon 仍用新目录
        drop(p.cb);
        p.daemon_b.shutdown().await.unwrap();
        let settings = Settings::load(p.home_b.path()).unwrap();
        let resolved = settings
            .recv_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| p.home_b.path().join("inbox"));
        assert_eq!(resolved, newdir, "重启解析必须收敛到新目录");
        let daemon_b2 = Daemon::start(
            p.home_b.path(),
            DaemonOpts {
                recv_dir: resolved,
                tray: false,
            },
        )
        .await
        .unwrap();
        let cb2 = IpcClient::connect(p.home_b.path()).await.unwrap();
        wait_online(&cb2, true, 30).await;
        // B 的常驻接收重新注册;A 的快照随后可见,重试机制兜住毫秒级空窗
        wait_peer_online(&p.ca, &p.dev_b, true, 30).await;

        let f3 = src.path().join("迁移 三.txt");
        let d3 = fill_random_file(&f3, 32 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f3.to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("重启后第三传应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(newdir.join("迁移 三.txt")).unwrap(),
            d3,
            "重启后传输仍须落新目录(settings.json)"
        );

        let _ = daemon_b2.shutdown().await;
        p.daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("recv_dir e2e exceeded 150s");
}

// ---------- 更新偏好(update.prefs.*) ----------

/// update.prefs get/set(D4b 设置面,M4b D8/D4):
/// - 老格式 settings.json(无新字段)前向兼容:daemon 启动加载即 D8 默认
///   (auto_check=true / auto_install=false);
/// - set 单字段(auto_check 缺省)不动另一字段,get 回读一致;
/// - set auto_check=false 连带强制 auto_install=false(D4 父子耦合,收口
///   在 daemon 一处),内存与 settings.json 一致持久化;
/// - 同帧「关检查 + 开安装」自相矛盾 → -32602,不产生半程态。
#[tokio::test]
async fn update_prefs_roundtrip_and_coupling() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let home = tempfile::tempdir().unwrap();
        // 老格式 settings.json:仅既有字段,无 update_*
        std::fs::write(
            home.path().join("settings.json"),
            serde_json::to_string(&json!({ "autostart": false })).unwrap(),
        )
        .unwrap();
        let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
        let client = IpcClient::connect(home.path()).await.unwrap();

        // 老文件加载 → D8 默认
        let r = call(&client, "update.prefs.get", json!({})).await.unwrap();
        assert_eq!(
            r,
            json!({ "auto_check": true, "auto_install": false }),
            "老 settings.json 必须前向兼容出 D8 默认(检查开/安装关): {r}"
        );

        // 单字段 set:auto_check 缺省不动
        let r = call(&client, "update.prefs.set", json!({ "auto_install": true }))
            .await
            .unwrap();
        assert_eq!(r, json!({}));
        let r = call(&client, "update.prefs.get", json!({})).await.unwrap();
        assert_eq!(r, json!({ "auto_check": true, "auto_install": true }));

        // 关检查:连带强制 auto_install=false,且持久化
        call(&client, "update.prefs.set", json!({ "auto_check": false }))
            .await
            .unwrap();
        let r = call(&client, "update.prefs.get", json!({})).await.unwrap();
        assert_eq!(r, json!({ "auto_check": false, "auto_install": false }));
        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["update_auto_check"], json!(false), "检查开关须持久化");
        assert_eq!(
            saved["update_auto_install"], json!(false),
            "关检查必须连带持久化 auto_install=false(D4 耦合)"
        );

        // 跨帧矛盾(Task 11 收口 D4 残余):检查已关时单开安装同样 -32602
        let err = call(&client, "update.prefs.set", json!({ "auto_install": true }))
            .await
            .unwrap_err();
        assert_eq!(err.code, -32602, "跨帧矛盾组合必须拒绝: {err:?}");
        let r = call(&client, "update.prefs.get", json!({})).await.unwrap();
        assert_eq!(
            r,
            json!({ "auto_check": false, "auto_install": false }),
            "被拒不落半程态: {r}"
        );

        // 同帧自相矛盾:-32602,状态不得被半程改写
        let err = call(
            &client,
            "update.prefs.set",
            json!({ "auto_check": false, "auto_install": true }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, -32602, "矛盾组合必须报参数非法: {err:?}");
        let r = call(&client, "update.prefs.get", json!({})).await.unwrap();
        assert_eq!(r, json!({ "auto_check": false, "auto_install": false }));

        daemon.shutdown().await.unwrap();
    });
    run.await.expect("update.prefs e2e exceeded 60s");
}

// ---------- 拒绝面报告 ----------

/// 对端 recv_dir 指向不可写位置 → 发送侧 code=1,message 非空且含
/// 已完成/部分传输/未完成三栏与原始输入路径(结构性断言,不 brittle)。
#[tokio::test(flavor = "multi_thread")]
async fn send_rejected_surfaces_report() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rj@x.y").await;

        // B 的收件目录指到「文件路径之下」:create_dir_all 必败,接收落盘
        // 无法开写 → 发送侧重试耗尽出失败报告。set_recv_dir 只持久化,
        // 不落盘目录,因此 RPC 本身应成功。
        let blocked = p.home_b.path().join("blocked");
        std::fs::write(&blocked, b"not a dir").unwrap();
        let bad_recv = blocked.join("sub");
        call(&p.cb, "set_recv_dir", json!({ "dir": bad_recv.to_string_lossy() }))
            .await
            .expect("set_recv_dir 只持久化,应成功");

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("拒绝 甲.bin");
        fill_random_file(&f, 64 * 1024);

        let err = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f.to_string_lossy()], "to": p.dev_b }),
            Duration::from_secs(90),
        )
        .await
        .expect_err("不可写收件目录必须失败");
        assert_eq!(err.code, 1, "失败必须是业务错误码 1");
        assert!(!err.message.trim().is_empty(), "message 必须携带报告全文");

        let report = &err.message;
        let i_done = report.find("已完成").expect("报告含已完成栏");
        let i_part = report.find("部分传输").expect("报告含部分传输栏");
        let i_never = report.find("未完成").expect("报告含未完成栏");
        assert!(
            i_done < i_part && i_part < i_never,
            "分栏必须有序: {report}"
        );
        assert!(
            report.contains(&f.display().to_string()),
            "报告必须点名原始输入路径 {}: {report}",
            f.display()
        );

        p.shutdown().await;
    });
    run.await.expect("reject e2e exceeded 120s");
}

// ---------- 并发互斥 ----------

/// 并发第二个 send 立即得 code=2「传输进行中」,不打断在途第一传。
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_send_is_code_2() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("busy@x.y").await;

        // 大文件拉长第一传的窗口(清单事件即第一传已入编排、传输未完的锚点)
        let src = tempfile::tempdir().unwrap();
        let f1 = src.path().join("并发 甲.bin");
        fill_random_file(&f1, 16 * 1024 * 1024);
        let f2 = src.path().join("并发 乙.bin");
        fill_random_file(&f2, 8 * 1024);

        // 先订阅;第一传在**另一连接**上发起(send 是长调用,会占住本连接的
        // 读半工),清单行到手即第一传在传输中途,此刻并发第二传必撞互斥
        let mut evs = p.ca.events();
        let cs = IpcClient::connect(p.home_a.path()).await.unwrap();
        let to_b = p.dev_b.clone();
        let send_task = tokio::spawn(async move {
            call_bounded(
                &cs,
                "send",
                json!({ "files": [f1.to_string_lossy()], "to": to_b }),
                SEND_TIMEOUT,
            )
            .await
        });

        wait_event(&mut evs, 30, "第一传的清单行", |f| {
            f.method == "event.send_log"
                && f.params["line"].as_str().map(|l| l.contains("清单")).unwrap_or(false)
        })
        .await;

        let cb2 = IpcClient::connect(p.home_a.path()).await.unwrap();
        let err = call(
            &cb2,
            "send",
            json!({ "files": [f2.to_string_lossy()], "to": p.dev_b }),
        )
        .await
        .expect_err("并发第二传必须被拒");
        assert_eq!(err.code, 2, "并发第二传必须是 code 2: {err:?}");
        assert!(
            err.message.contains("传输进行中"),
            "占用文案须点名传输进行中: {err:?}"
        );

        // 第一传照常完成(互斥只拒新,不打断旧)
        let r = send_task.await.expect("send task panicked");
        assert_eq!(r.expect("第一传应成功"), json!({}));
        p.shutdown().await;
    });
    run.await.expect("concurrent e2e exceeded 150s");
}

// ---------- 对端 daemon 硬杀(真实子进程) ----------

/// 真实 mtdssd 子进程当对端,传输中途硬杀 → 发送侧 RPC 在
/// max_reconnects×backoff + 60s 内返回 code=1(报告非空)。
/// MTDSS_TEST_IDLE/BACKOFF/MAX_RECONNECTS 经 daemon A 的启动环境注入。
#[tokio::test(flavor = "multi_thread")]
async fn peer_daemon_killed_send_fails_bounded() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "kd@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox-b");

        // A 注入小重试预算 + 短 idle(静默死亡快速显形);B 只需短 idle
        let env_a: &[(&str, &str)] = &[
            ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
            ("MTDSS_TEST_MAX_RECONNECTS", "2"),
            ("MTDSS_TEST_BACKOFF_MS", "300"),
        ];
        let env_b: &[(&str, &str)] = &[("MTDSS_TEST_IDLE_TIMEOUT_MS", "800")];
        let (_child_a, _pipes_a, ca, dev_a) =
            spawn_daemon_child(home_a.path(), &home_a.path().join("inbox"), &server, "kd@x.y", "A", env_a).await;
        let (mut child_b, pipes_b, cb, dev_b) =
            spawn_daemon_child(home_b.path(), &recv_b, &server, "kd@x.y", "B", env_b).await;

        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;
        assert_ne!(dev_a, dev_b);

        // 3 文件尺寸错开:首个 .part 出现即杀,硬杀必然落在传输中途
        let src = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, size) in [
            ("硬杀 甲.bin", 4 * 1024usize),
            ("硬杀 乙.bin", 512 * 1024),
            ("硬杀 丙.bin", 1024 * 1024),
        ] {
            let p = src.path().join(name);
            let d = fill_random_file(&p, size);
            files.push((p, d));
        }
        let file_names: Vec<String> = files.iter().map(|(p, _)| p.to_string_lossy().into_owned()).collect();

        let ca_task = ca.clone();
        let send_task = tokio::spawn(async move {
            call_bounded(
                &ca_task,
                "send",
                json!({ "files": file_names, "to": dev_b }),
                SEND_TIMEOUT,
            )
            .await
        });
        wait_transfer_started(&recv_b.join(".mtdss"), Duration::from_secs(30)).await;
        let t0 = Instant::now();
        child_b.start_kill().expect("kill peer daemon");
        let status = child_b.wait().await.expect("reap peer daemon");
        assert!(!status.success(), "被杀对端不得干净退出");

        let err = send_task
            .await
            .expect("send task panicked")
            .expect_err("对端死亡后发送必须失败");
        let elapsed = t0.elapsed();
        let bound = Duration::from_millis(2 * 300) + Duration::from_secs(60);
        assert!(
            elapsed < bound,
            "发送须在 {bound:?} 内有界失败,实际 {elapsed:?}"
        );
        assert_eq!(err.code, 1, "对端死亡是业务失败: {err:?}");
        assert!(!err.message.trim().is_empty(), "报告非空: {err:?}");
        let diag = pipes_b.snapshot().await;
        eprintln!("--- 被杀对端 stderr(诊断)---\n{}", diag.1);

        let _ = ca.call("hello", json!({})).await; // A 仍存活(诊断性)
    });
    run.await.expect("peer-kill e2e exceeded 150s");
}

// ---------- 静默断链跨进程续传(移植) ----------

/// 硬杀接收 daemon → 同 home/recv 重启 → 发送端重连并跨进程续传到完成
/// (对号 cli_e2e:silent_receiver_death_sender_reconnects_and_resumes)。
#[tokio::test(flavor = "multi_thread")]
async fn receiver_daemon_killed_sender_resumes() {
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "rs@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox");

        let env_a: &[(&str, &str)] = &[
            ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
            ("MTDSS_TEST_MAX_RECONNECTS", "8"),
            ("MTDSS_TEST_BACKOFF_MS", "300"),
        ];
        let env_b: &[(&str, &str)] = &[("MTDSS_TEST_IDLE_TIMEOUT_MS", "800")];
        let (_child_a, pipes_a, ca, _dev_a) =
            spawn_daemon_child(home_a.path(), &home_a.path().join("inbox"), &server, "rs@x.y", "A", env_a).await;
        let (mut child_b1, pipes_b1, cb, dev_b) =
            spawn_daemon_child(home_b.path(), &recv_b, &server, "rs@x.y", "B", env_b).await;

        // 诊断看门狗:挂死时能看到两侧 daemon 的最近日志(tracing 走 stdout)
        let _diag_a = spawn_diag_loop("A", pipes_a.out_buf.clone());
        let _diag_b1 = spawn_diag_loop("B#1", pipes_b1.out_buf.clone());

        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let src = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, size) in [
            ("续传 甲.bin", 4 * 1024usize),
            ("续传 乙.bin", 256 * 1024),
            ("续传 丙.bin", 1024 * 1024),
        ] {
            let p = src.path().join(name);
            let d = fill_random_file(&p, size);
            files.push((p, d));
        }
        let file_names: Vec<String> = files.iter().map(|(p, _)| p.to_string_lossy().into_owned()).collect();

        let ca_task = ca.clone();
        let to_b = dev_b.clone();
        let send_task = tokio::spawn(async move {
            call_bounded(
                &ca_task,
                "send",
                json!({ "files": file_names, "to": to_b }),
                SEND_TIMEOUT,
            )
            .await
        });
        wait_transfer_started(&recv_b.join(".mtdss"), Duration::from_secs(30)).await;

        // 硬杀接收 #1:留下 .part/部分位图给 #2 对账(Resume 路径)
        child_b1.start_kill().expect("kill recv#1");
        let st = child_b1.wait().await.expect("reap recv#1");
        assert!(!st.success(), "被杀接收不得干净退出");

        // 客户端先弃再重启:Windows 上句柄在,旧管道名不释放
        drop(cb);
        tokio::time::sleep(Duration::from_secs(2)).await; // 发送端显形断连进入退避
        let (mut child_b2, pipes_b2, _cb2, dev_b2) =
            spawn_daemon_child(home_b.path(), &recv_b, &server, "rs@x.y", "B", env_b).await;
        assert_eq!(dev_b2, dev_b, "同 home 同身份,重启收敛同一设备");
        let _diag_b2 = spawn_diag_loop("B#2", pipes_b2.out_buf.clone());

        // 发送端重连 + 续传收敛(重试预算覆盖 B 重启的陈旧锁确认窗 ~2s)
        let r = send_task.await.expect("send task panicked");
        assert_eq!(
            r.expect("重连续传必须成功"),
            json!({}),
            "跨进程续传后 send 返回空对象"
        );

        // 字节事实:3 文件逐一与源一致;收件区只剩完成记录
        for (p, d) in &files {
            let got = recv_b.join(p.file_name().unwrap());
            assert_eq!(
                std::fs::read(&got).unwrap(),
                *d,
                "content mismatch at {}",
                got.display()
            );
        }
        assert_intake_clean(&recv_b, 3);
        let diag = pipes_b2.snapshot().await;
        eprintln!("--- 重启接收 stderr(诊断)---\n{}", diag.1);
        child_b2.start_kill().ok();
    });
    run.await.expect("resume e2e exceeded 180s");
}

// ---------- 重启中断标记(v2-P2 Task 4) ----------

/// 传输中硬杀发起端 daemon → 同 home 重启 → transfer.list 含该任务
/// state=interrupted、进度为最后已知值(spec §4.2 重启恢复;load 把非终
/// 态标 interrupted,注册表不自动复活)。进度确定性:3 小文件 + 1 个
/// 128MiB 大文件,轮询到发起端 transfer.list files_done≥2(FileDone→
/// 持久层是同步回调)才杀,大文件保证杀点必然落在传输中途、作业未终态。
#[tokio::test(flavor = "multi_thread")]
async fn restart_marks_interrupted() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "ri@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox-b");

        let (mut child_a, pipes_a, ca, dev_a) = spawn_daemon_child(
            home_a.path(),
            &home_a.path().join("inbox"),
            &server,
            "ri@x.y",
            "A",
            &[],
        )
        .await;
        let (_child_b, _pipes_b, cb, dev_b) = spawn_daemon_child(
            home_b.path(),
            &recv_b,
            &server,
            "ri@x.y",
            "B",
            &[],
        )
        .await;
        let _diag_a = spawn_diag_loop("A", pipes_a.out_buf.clone());

        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        // 3 小文件 + 1 大文件(内容零填充即可:本测不断言字节一致,只断言
        // 记录面;128MiB 让「末文件传完」远慢于观察-杀窗)
        let src = tempfile::tempdir().unwrap();
        let mut files: Vec<String> = Vec::new();
        for name in ["中断 甲.bin", "中断 乙.bin", "中断 丙.bin"] {
            let p = src.path().join(name);
            fill_random_file(&p, 4 * 1024);
            files.push(p.to_string_lossy().into_owned());
        }
        let big = src.path().join("中断 丁.bin");
        std::fs::write(&big, vec![0u8; 128 * 1024 * 1024]).unwrap();
        files.push(big.to_string_lossy().into_owned());

        let r = call(
            &ca,
            "transfer.start",
            json!({
                "kind": "upload",
                "files": files,
                "to": dev_b,
                "dir": recv_b.to_string_lossy(),
            }),
        )
        .await
        .expect("transfer.start 应成功");
        let job_id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 轮询发起端 transfer.list 直到 files_done ≥ 2(FileDone→store 是
        // 同步回调,此刻持久记录必已带 ≥2 的最后已知进度);大文件在途,
        // 作业不可能在此窗内终态
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let lst = call(&ca, "transfer.list", json!({})).await.unwrap();
            let fd = lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(job_id.as_str()))
                .and_then(|j| j["files_done"].as_u64())
                .unwrap_or(0);
            if fd >= 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "进度未在预算内到 ≥2(最后: {lst})"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // 硬杀发起端(记录已持久,非终态)
        child_a.start_kill().expect("kill initiator daemon");
        let st = child_a.wait().await.expect("reap initiator daemon");
        assert!(!st.success(), "被杀发起端不得干净退出");

        // 同 home 重启:客户端先弃再重启(Windows 句柄占着旧管道名)
        drop(ca);
        let (_child_a2, pipes_a2, ca2, dev_a2) = spawn_daemon_child(
            home_a.path(),
            &home_a.path().join("inbox"),
            &server,
            "ri@x.y",
            "A",
            &[],
        )
        .await;
        let _diag_a2 = spawn_diag_loop("A#2", pipes_a2.out_buf.clone());
        assert_eq!(dev_a2, dev_a, "同 home 同身份,重启收敛同一设备");

        // 新 daemon 的 transfer.list:该任务以持久记录身份在场,state=
        // interrupted(load 的重启标记),进度为最后已知值,不自动复活
        let lst = call(&ca2, "transfer.list", json!({})).await.unwrap();
        let row = lst["jobs"]
            .as_array()
            .expect("jobs 数组")
            .iter()
            .find(|j| j["id"].as_str() == Some(job_id.as_str()))
            .expect("重启后任务记录必须在场(持久层即历史面)")
            .clone();
        assert_eq!(
            row["state"].as_str(),
            Some("interrupted"),
            "非终态重启 → interrupted(不自动复活): {row}"
        );
        assert_eq!(row["kind"].as_str(), Some("upload"));
        assert_eq!(
            row["name"].as_str(),
            Some("中断 甲.bin"),
            "显示名 = 首项末段名: {row}"
        );
        let fd = row["files_done"].as_u64().expect("进度字段在场");
        assert!((2..=3).contains(&fd), "进度为最后已知值(≥2 文件已收): {row}");
        assert!(
            row["files_total"].as_u64() == Some(4),
            "预扫总数随记录保留: {row}"
        );
    });
    run.await.expect("restart_marks_interrupted exceeded 150s");
}

// ---------- 同名冲突唯一改名(移植) ----------

/// 同 rel 的第二传内容不同 → intake 对账重收 → 冲突唯一改名,
/// 两份内容并存(对号 core same_name_retransfer 语义,经 daemon 全链路)。
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_conflict_renames_uniquely() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("dup@x.y").await;
        let recv = p.recv_dir_b();

        // 两个源分居不同子目录(同路径会互相覆盖,内容就不再不同)
        let src = tempfile::tempdir().unwrap();
        let s1 = src.path().join("s1");
        let s2 = src.path().join("s2");
        std::fs::create_dir_all(&s1).unwrap();
        std::fs::create_dir_all(&s2).unwrap();
        let c1 = fill_random_file(&s1.join("dup.txt"), 1000);
        let c2 = fill_random_file(&s2.join("dup.txt"), 2000);

        // 第一传:dup.txt 原名落盘
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [s1.join("dup.txt").to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("第一传应成功");
        assert_eq!(r, json!({}));
        assert_eq!(std::fs::read(recv.join("dup.txt")).unwrap(), c1);

        // 第二传:同 rel 不同内容 → 冲突改名,原名保持第一份
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [s2.join("dup.txt").to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("第二传应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(recv.join("dup (1).txt")).unwrap(),
            c2,
            "第二份必须落在唯一改名副本"
        );
        assert_eq!(
            std::fs::read(recv.join("dup.txt")).unwrap(),
            c1,
            "原名文件必须保持第一份内容"
        );
        // intake 记录按 rel 键控:同 rel 两传收敛同一条记录(内容为后写者),
        // 不按条数断言,只验无 .part 残留
        let intake = recv.join(".mtdss");
        let names: Vec<String> = std::fs::read_dir(&intake)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.is_empty() && names.iter().all(|x| x.ends_with(".json")),
            "intake must hold only finished records (no .part): {names:?}"
        );

        p.shutdown().await;
    });
    run.await.expect("duplicate e2e exceeded 120s");
}

// ---------- TLS 全链路(移植) ----------

/// 自签 TLS 信令服务器上,daemon 以 ca 参数登录并发收全链路
/// (对号 cli_e2e:cli_send_recv_end_to_end_over_tls;session.ca 经 IPC 登录
/// 注入,REST 与 WS 均走该信任锚)。
#[tokio::test(flavor = "multi_thread")]
async fn send_recv_over_tls() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let (addr, ca_path, dir) = signaling::spawn_test_tls_server().await;
        let server = format!("https://{addr}");
        // 认证收口后仅预置账号可领码:AppState 已随服务器任务移走,借返回的
        // TempDir 定位 DB 另开连接预置(db::open 幂等,WAL 下双连接安全)
        let conn =
            signaling::db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
        signaling::admin::user_add(&conn, "tls@x.y").unwrap();
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        ipc_login(&ca, &server, "tls@x.y", "A", Some(&ca_path)).await;
        ipc_login(&cb, &server, "tls@x.y", "B", Some(&ca_path)).await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let st = call(&ca, "status", json!({})).await.unwrap();
        let dev_b = {
            // B 的 device_id 从 hello 取(登录即有)
            let h = call(&cb, "hello", json!({})).await.unwrap();
            h["device"]["device_id"].as_str().unwrap().to_string()
        };
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("tls 传输.bin");
        let data = fill_random_file(&f, 16 * 1024);
        let r = call_bounded(
            &ca,
            "send",
            json!({ "files": [f.to_string_lossy()], "to": dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("TLS 全链路 send 应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(home_b.path().join("inbox").join("tls 传输.bin")).unwrap(),
            data
        );

        daemon_b.shutdown().await.unwrap();
        daemon_a.shutdown().await.unwrap();
        drop(st);
    });
    run.await.expect("tls e2e exceeded 120s");
}

/// 错误 CA:daemon 登录链路必须拒绝不受信根(对号 cli_e2e:wrong_ca_is_rejected)。
#[tokio::test(flavor = "multi_thread")]
async fn wrong_ca_rejected_at_login() {
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let (addr, _ca, _dir) = signaling::spawn_test_tls_server().await;
        // 另一套独立 CA:不受信
        let other = tempfile::tempdir().unwrap();
        signaling::tls::generate_dev_tls(other.path(), &[]).unwrap();
        let home = tempfile::tempdir().unwrap();
        let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
        let client = IpcClient::connect(home.path()).await.unwrap();
        let err = call(
            &client,
            "login.request_code",
            json!({
                "server": format!("https://{addr}"),
                "email": "x@y.z",
                "ca": other.path().join("ca.crt").to_string_lossy(),
            }),
        )
        .await
        .expect_err("不受信 CA 必须让登录失败");
        assert_eq!(err.code, 1, "登录失败是业务错误码: {err:?}");
        daemon.shutdown().await.unwrap();
    });
    run.await.expect("wrong-ca e2e exceeded 60s");
}

// ---------- 回环 STUN 打洞全链路(移植) ----------

/// 信令服务器通告本地回环 STUN,双 daemon 真实候选收集 → 互拨 → 跨块传输
/// (对号 cli_e2e:cli_send_recv_with_stun_punch;3 MiB + 尾块确保跨块)。
#[tokio::test(flavor = "multi_thread")]
async fn send_recv_with_stun_punch() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        // 真实 STUN 双端口(回环)
        let sa = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let sb = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let (pa, pb) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = stun::run_listener(sa, 100, 100).await;
        });
        tokio::spawn(async move {
            let _ = stun::run_listener(sb, 100, 100).await;
        });
        let (addr, state) =
            signaling::spawn_test_server_with(signaling::ws::Keepalive::default(), vec![pa.to_string(), pb.to_string()])
                .await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "punch@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        ipc_login(&ca, &server, "punch@x.y", "A", None).await;
        ipc_login(&cb, &server, "punch@x.y", "B", None).await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let dev_b = {
            let h = call(&cb, "hello", json!({})).await.unwrap();
            h["device"]["device_id"].as_str().unwrap().to_string()
        };
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("打洞 成功.bin");
        // 3 MiB + 尾块(CHUNK_SIZE = 1 MiB):4 块,真正跨块经打洞路径传输
        let data = fill_random_file(&f, 3 * 1024 * 1024 + 777);
        let r = call_bounded(
            &ca,
            "send",
            json!({ "files": [f.to_string_lossy()], "to": dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("STUN 打洞 send 应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(home_b.path().join("inbox").join("打洞 成功.bin")).unwrap(),
            data
        );

        daemon_b.shutdown().await.unwrap();
        daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("stun e2e exceeded 120s");
}

// ---------- 定向接收(M3c Task 5) ----------

/// send RPC 带 `dir`(B 侧 tempdir)→ 文件落**该**目录、默认收件目录无;
/// 收件区记录随 out_dir 走(在目标目录的 .mtdss 下)。
#[tokio::test(flavor = "multi_thread")]
async fn send_with_target_dir_lands_there() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("tdir@x.y").await;
        let target = tempfile::tempdir().unwrap();

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("定向 甲.bin");
        let d = fill_random_file(&f, 16 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({
                "files": [f.to_string_lossy()],
                "to": p.dev_b,
                "dir": target.path().to_string_lossy(),
            }),
            SEND_TIMEOUT,
        )
        .await
        .expect("定向 send 应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(target.path().join("定向 甲.bin")).unwrap(),
            d,
            "文件必须落信令指定的目标目录"
        );
        assert!(
            !p.recv_dir_b().join("定向 甲.bin").exists(),
            "默认收件目录不得收到该文件"
        );
        assert_intake_clean(target.path(), 1);

        p.shutdown().await;
    });
    run.await.expect("target-dir e2e exceeded 120s");
}

/// `dir="C:foo"`(盘符相对形态,Task 1 起 fs normalize 拒收)→ B 侧 warn
/// 回退默认收件目录,文件落默认目录,send 照常成功。B 用真实 mtdssd 子
/// 进程:warn 在子进程 stdout(tracing fmt 缺省写 stdout),快照可 grep。
#[tokio::test(flavor = "multi_thread")]
async fn send_target_invalid_falls_back() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "tinv@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox-b");
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let (_child_b, pipes_b, cb, dev_b) = spawn_daemon_child(
            home_b.path(),
            &recv_b,
            &server,
            "tinv@x.y",
            "B",
            &[],
        )
        .await;
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        ipc_login(&ca, &server, "tinv@x.y", "A", None).await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("回退 甲.bin");
        let d = fill_random_file(&f, 16 * 1024);
        let r = call_bounded(
            &ca,
            "send",
            json!({
                "files": [f.to_string_lossy()],
                "to": dev_b,
                "dir": "C:foo",
            }),
            SEND_TIMEOUT,
        )
        .await
        .expect("非法 dir 由接收方回退,send 仍应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(recv_b.join("回退 甲.bin")).unwrap(),
            d,
            "非法 target_dir 必须回退默认收件目录"
        );
        // B 日志含回退 warn(tracing fmt 缺省写 stdout,同 spawn_diag_loop 口径)
        let diag = pipes_b.snapshot().await;
        assert!(
            diag.0.contains("回退默认收件目录"),
            "B 日志必须含定向目录回退 warn: {}",
            diag.0
        );
        let _ = daemon_a.shutdown().await;
    });
    run.await.expect("target-invalid e2e exceeded 150s");
}

/// 不带 `dir` 的 send(缺省)→ 默认收件目录,行为与 M2 逐字节一致
/// (既有用例天然覆盖该路径,此处补一条显式回归锚点)。
#[tokio::test(flavor = "multi_thread")]
async fn send_without_dir_keeps_legacy_semantics() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("tleg@x.y").await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("legacy 甲.bin");
        let d = fill_random_file(&f, 8 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f.to_string_lossy()], "to": p.dev_b }),
            SEND_TIMEOUT,
        )
        .await
        .expect("无 dir 的 send 应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(p.recv_dir_b().join("legacy 甲.bin")).unwrap(),
            d,
            "缺省 dir 必须保持 legacy 默认收件目录语义"
        );

        p.shutdown().await;
    });
    run.await.expect("legacy-send e2e exceeded 120s");
}

/// 接收完成通知:B 侧 IPC 事件流收到 `event.notify {"title","body"}`,
/// body 含落盘目录与数量(系统通知 notify_one 与事件共用同一文案)。
#[tokio::test(flavor = "multi_thread")]
async fn receive_completion_emits_notify_event() {
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("tev@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let dir_str = target.path().to_string_lossy().into_owned();

        // 先订阅 B 事件流再触发:通知事件在接收轮完成时广播,不能漏订
        let mut evs = p.cb.events();
        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("通知 甲.bin");
        let d = fill_random_file(&f, 16 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({
                "files": [f.to_string_lossy()],
                "to": p.dev_b,
                "dir": dir_str,
            }),
            SEND_TIMEOUT,
        )
        .await
        .expect("定向 send 应成功");
        assert_eq!(r, json!({}));
        assert_eq!(
            std::fs::read(target.path().join("通知 甲.bin")).unwrap(),
            d
        );

        let ev = wait_event(&mut evs, 30, "event.notify", |f| {
            f.method == "event.notify"
        })
        .await;
        assert_eq!(
            ev.params["title"].as_str(),
            Some("MTDSS"),
            "通知标题恒为 MTDSS: {:?}",
            ev.params
        );
        let body = ev.params["body"].as_str().unwrap_or_default().to_string();
        assert!(body.contains(&dir_str), "body 必须含落盘目录: {body}");
        assert!(body.contains("写入 1 个文件"), "body 必须含数量: {body}");

        p.shutdown().await;
    });
    run.await.expect("notify e2e exceeded 120s");
}

// ---------- upload 重新发起(v2-P2 Task 6) ----------

/// 传 5 → 硬杀对端 → 预算耗尽落 failed → 删 2 个已交付源文件 →
/// transfer.reinitiate:翻新换新 id、部分预检只带在的 8 项(清单行计数)、
/// 已交付项对账 SKIP 不重推、8 项全部交付、混合后果终态 failed 且 error
/// 含「2 项源已不存在」、对端 `.mtdss` 恰 8 条记录(被删 2 项的 done 记录
/// 清前在场、清后消失——清理可证)。
///
/// 几何钉死:5 个小文件(4KiB)+ 5 个 32MiB 大文件(合计 160MiB,作业在
/// 杀点前不可能终态)。杀门 = 对端收件区 5 条小文件 done 记录(磁盘事实
/// ——发送侧 files_done 要到轮末 checkpoint 消化才刷新,不可作杀门);
/// 杀点落在 5 项完成之后、大文件完成之前(32MiB @debug 回环需数秒,轮询
/// 到杀的窗口是毫秒级)。删的是两个已完成项——done 记录必在,清理才可
/// 证;其余断言对任意杀点(5 项完成后、作业终态前)均成立。
#[tokio::test(flavor = "multi_thread")]
async fn reinitiate_upload_resume_and_missing() {
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "rini@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox-b");

        // A 注入小重试预算(杀对端后快速耗尽落 failed)+ 短 idle(静默死亡
        // 快速显形);B 只需短 idle
        let env_a: &[(&str, &str)] = &[
            ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
            ("MTDSS_TEST_MAX_RECONNECTS", "2"),
            ("MTDSS_TEST_BACKOFF_MS", "300"),
        ];
        let env_b: &[(&str, &str)] = &[("MTDSS_TEST_IDLE_TIMEOUT_MS", "800")];
        let (_child_a, pipes_a, ca, _dev_a) = spawn_daemon_child(
            home_a.path(),
            &home_a.path().join("inbox"),
            &server,
            "rini@x.y",
            "A",
            env_a,
        )
        .await;
        let (mut child_b1, _pipes_b1, cb, dev_b) = spawn_daemon_child(
            home_b.path(),
            &recv_b,
            &server,
            "rini@x.y",
            "B",
            env_b,
        )
        .await;

        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        // 5 小文件 + 5 个 32MiB 大文件(内容零填充:大文件只断言落盘一致,
        // 对比时以仍在场的源文件为基准,不复制内容入内存)
        let src = tempfile::tempdir().unwrap();
        let mut files: Vec<PathBuf> = Vec::new();
        for name in ["重发 甲", "重发 乙", "重发 丙", "重发 丁", "重发 戊"] {
            let p = src.path().join(format!("{name}.bin"));
            fill_random_file(&p, 4 * 1024);
            files.push(p);
        }
        let big = vec![0u8; 32 * 1024 * 1024];
        for name in ["重发 己", "重发 庚", "重发 辛", "重发 壬", "重发 癸"] {
            let p = src.path().join(format!("{name}.bin"));
            std::fs::write(&p, &big).unwrap();
            files.push(p);
        }
        let names: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        let r = call(
            &ca,
            "transfer.start",
            json!({
                "kind": "upload",
                "files": names,
                "to": dev_b,
                "dir": recv_b.to_string_lossy(),
            }),
        )
        .await
        .expect("transfer.start 应成功");
        let job_id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 传 5:轮询对端收件区,5 个小文件的完成记录齐(读记录 done 位)→
        // 硬杀对端。发送侧 files_done 轮末 checkpoint 才刷新,收端磁盘事实
        // 才是杀门的真源;大文件 32MiB/个在毫秒级杀窗内无一可能完成。
        let small_rels: Vec<String> = files[..5]
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        let record_done = |rel: &str| -> bool {
            std::fs::read_to_string(record_path(&recv_b, rel))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v["done"].as_bool())
                .unwrap_or(false)
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if small_rels.iter().all(|r| record_done(r)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "5 个小文件未在预算内完成(收件区无齐 done 记录)"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        child_b1.start_kill().expect("kill peer daemon");
        let st = child_b1.wait().await.expect("reap peer daemon");
        assert!(!st.success(), "被杀对端不得干净退出");

        // 预算耗尽失败:对端死亡后 A 的重试按注入预算耗尽,作业落 failed
        // (idle 0.8s + 2×300ms 退避 + 10s 信令超时,45s 预算足够)
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let lst = call(&ca, "transfer.list", json!({})).await.unwrap();
            let st = lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(job_id.as_str()))
                .expect("作业在场")["state"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if st == "failed" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "对端死后作业未在预算内落 failed(最后: {st})"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 删 2 个源文件(取两个已完成项:对端 done 记录必在,清理可证)。
        // 清前断言:两条记录都在。
        let (del1, del2) = (files[0].clone(), files[1].clone());
        let rel1 = del1.file_name().unwrap().to_string_lossy().into_owned();
        let rel2 = del2.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            record_path(&recv_b, &rel1).exists(),
            "清前 done 记录须在: {rel1}"
        );
        assert!(
            record_path(&recv_b, &rel2).exists(),
            "清前 done 记录须在: {rel2}"
        );
        std::fs::remove_file(&del1).unwrap();
        std::fs::remove_file(&del2).unwrap();

        // 重启 B(同 home/recv;客户端先弃,Windows 上句柄在旧管道名不释放)
        drop(cb);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (_child_b2, _pipes_b2, _cb2, dev_b2) = spawn_daemon_child(
            home_b.path(),
            &recv_b,
            &server,
            "rini@x.y",
            "B",
            env_b,
        )
        .await;
        assert_eq!(dev_b2, dev_b, "同 home 同身份,重启收敛同一设备");
        wait_peer_online(&ca, &dev_b, true, 30).await;

        // reinitiate:翻新换新 id,RPC 回 {}。列表面:旧记录已自持久层摘除
        // (重启后只剩新卡),但终态 live 行不驱逐(M4a 口径,注册表无驱逐)
        // ——同会话内旧 failed 行 + 新行并存。
        let r = call(&ca, "transfer.reinitiate", json!({ "id": job_id }))
            .await
            .expect("transfer.reinitiate 应成功");
        assert_eq!(r, json!({}));
        let lst = call(&ca, "transfer.list", json!({})).await.unwrap();
        let rows = lst["jobs"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "旧 failed live 行 + 新翻新行并存: {lst}");
        let old_row = rows
            .iter()
            .find(|j| j["id"].as_str() == Some(job_id.as_str()))
            .expect("旧 id live 行仍在(终态无驱逐)");
        assert_eq!(old_row["state"].as_str(), Some("failed"));
        let new_row = rows
            .iter()
            .find(|j| j["id"].as_str() != Some(job_id.as_str()))
            .expect("新行必须在场");
        assert_ne!(
            new_row["id"].as_str().unwrap(),
            job_id.as_str(),
            "翻新必须换新 id: {lst}"
        );
        assert_eq!(new_row["kind"].as_str(), Some("upload"));
        assert_eq!(
            new_row["files_total"].as_u64(),
            Some(8),
            "部分预检:仅 8 项在场(2 项已消失): {lst}"
        );
        let new_id = new_row["id"].as_str().unwrap().to_string();

        // 混合后果:驱动 Ok 且 2 项源已不存在 → 终态 failed、error 点名、
        // 8 项全部交付(已交付 3 项 SKIP + 其余 5 项续传/重传)
        let fin = call_bounded(&ca, "transfer.wait", json!({ "id": new_id }), Duration::from_secs(180))
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("failed"), "混合后果必须落 failed: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("2 项源已不存在"),
            "error 必须点名缺失数: {fin}"
        );
        assert_eq!(fin["files_done"].as_u64(), Some(8), "8 项全部交付: {fin}");

        // 计数面(块流只补缺块):重发轮清单行恰 8 个文件(部分预检把已消失
        // 的 2 项筛出驱动面);完成行报告 8 files。首轮的清单行是 10 个文件。
        let diag = pipes_a.snapshot().await;
        assert!(
            diag.0.contains("清单: 10 个文件"),
            "首轮清单行必须 10 个文件: {}",
            diag.0
        );
        assert!(
            diag.0.contains("清单: 8 个文件"),
            "重发轮清单行必须 8 个文件(部分预检): {}",
            diag.0
        );
        assert!(
            diag.0.contains("(8 files)"),
            "重发轮完成行必须报 8 files: {}",
            diag.0
        );

        // 磁盘事实:8 项交付件与源一致;被清 2 项的记录与 .part 消失,
        // .mtdss 恰 8 条完成记录
        for p in &files[2..] {
            let got = recv_b.join(p.file_name().unwrap());
            assert_eq!(
                std::fs::read(&got).expect("交付件在"),
                std::fs::read(p).expect("源件在"),
                "content mismatch: {}",
                got.display()
            );
        }
        assert!(!record_path(&recv_b, &rel1).exists(), "记录须被清: {rel1}");
        assert!(!record_path(&recv_b, &rel2).exists(), "记录须被清: {rel2}");
        assert!(!part_path(&recv_b, &rel1).exists(), ".part 须被清: {rel1}");
        assert!(!part_path(&recv_b, &rel2).exists(), ".part 须被清: {rel2}");
        assert_intake_clean(&recv_b, 8);
    });
    run.await.expect("reinitiate e2e exceeded 240s");
}

// ---------- transfer.clear(v2-P2 Task 7,spec §5.2) ----------

/// 在线回执(取消上传构形,对端全程在线):A→B 传 32MiB → 等 B `.mtdss`
/// 记录落盘(残留就位的确定门:接收开写即落全零位图记录)→
/// transfer.cancel → cancelled 终态(含 cancelled 终态可清)→ 等 B 侧接收
/// 轮死透(降级匿名卡 failed,清理不再与在途写竞跑)→ transfer.clear:
/// 对端 cleanup_outbox 回执 Ok → 本地记录消失(二清报「不存在」可证)+
/// 对端残留净(`.mtdss` 恰 0 条)。
#[tokio::test(flavor = "multi_thread")]
async fn clear_upload_online_acks_then_removes() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("tclron@x.y").await;
        let rel = "清除 甲.bin";
        let src = tempfile::tempdir().unwrap();
        let f = src.path().join(rel);
        std::fs::write(&f, vec![0u8; 32 * 1024 * 1024]).unwrap();

        let r = call(
            &p.ca,
            "transfer.start",
            json!({
                "kind": "upload",
                "files": [f.to_string_lossy()],
                "to": p.dev_b,
                "dir": p.recv_dir_b().to_string_lossy(),
            }),
        )
        .await
        .expect("transfer.start 应成功");
        let job_id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 等 B 侧记录落盘(残留就位),再取消
        let rec_b = record_path(&p.recv_dir_b(), rel);
        let deadline = Instant::now() + Duration::from_secs(60);
        while !rec_b.exists() {
            assert!(
                Instant::now() < deadline,
                "B 侧 .mtdss 记录未在预算内出现: {}",
                rec_b.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        call(&p.ca, "transfer.cancel", json!({ "id": job_id }))
            .await
            .expect("cancel 应成功");
        let fin = call(&p.ca, "transfer.wait", json!({ "id": job_id }))
            .await
            .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("cancelled"), "{fin}");

        // 等 B 侧接收轮死透(匿名卡 failed):轮死亡后不再有数据写入,随后
        // 的清理不与在途写竞跑(否则被清的 .part 可能被垂死写复活)
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let lst = call(&p.cb, "transfer.list", json!({})).await.unwrap();
            let dead = lst["jobs"].as_array().unwrap().iter().any(|j| {
                j["kind"].as_str() == Some("incoming")
                    && j["state"].as_str() == Some("failed")
            });
            if dead {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B 侧接收轮未在预算内收口(匿名卡未 failed): {lst}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // 清除:对端在线回执 → Ok;二清「任务不存在」证记录已自持久层摘除
        let r = call(&p.ca, "transfer.clear", json!({ "id": job_id }))
            .await
            .expect("在线清除应成功");
        assert_eq!(r, json!({}));
        let err = call(&p.ca, "transfer.clear", json!({ "id": job_id }))
            .await
            .expect_err("二清必须失败(记录已摘)");
        assert_eq!(err.code, 1, "存在性失败是 code 1: {err:?}");
        assert!(err.message.contains("不存在"), "二清报存在性: {err:?}");

        // 对端残留净:记录与 .part 双清,`.mtdss` 恰 0 条
        assert_intake_clean(&p.recv_dir_b(), 0);

        p.shutdown().await;
    });
    run.await.expect("clear-online e2e exceeded 150s");
}

/// 对端离线(T6 杀门构形):A→B 传 32MiB → 传输开动即硬杀 B → A 预算耗尽
/// 落 failed → presence 翻离线 → transfer.clear:离线预检 code 3「设备离线」,
/// 任务保留可重试(二清仍 code 3 可证记录在——若记录被摘,二清应报存在性
/// code 1)。
#[tokio::test(flavor = "multi_thread")]
async fn clear_upload_fails_and_keeps_record_when_peer_offline() {
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let (addr, state) = spawn_test_server().await;
        signaling::admin::user_add(&state.db.lock().unwrap(), "tclroff@x.y").unwrap();
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let recv_b = home_b.path().join("inbox-b");

        // A 注入小重试预算(杀对端后快速耗尽落 failed)+ 短 idle;B 只需短 idle
        let env_a: &[(&str, &str)] = &[
            ("MTDSS_TEST_IDLE_TIMEOUT_MS", "800"),
            ("MTDSS_TEST_MAX_RECONNECTS", "2"),
            ("MTDSS_TEST_BACKOFF_MS", "300"),
        ];
        let env_b: &[(&str, &str)] = &[("MTDSS_TEST_IDLE_TIMEOUT_MS", "800")];
        let (_child_a, _pipes_a, ca, _dev_a) = spawn_daemon_child(
            home_a.path(),
            &home_a.path().join("inbox"),
            &server,
            "tclroff@x.y",
            "A",
            env_a,
        )
        .await;
        let (mut child_b, _pipes_b, cb, dev_b) = spawn_daemon_child(
            home_b.path(),
            &recv_b,
            &server,
            "tclroff@x.y",
            "B",
            env_b,
        )
        .await;

        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("离线清除 甲.bin");
        std::fs::write(&f, vec![0u8; 32 * 1024 * 1024]).unwrap();
        let r = call(
            &ca,
            "transfer.start",
            json!({
                "kind": "upload",
                "files": [f.to_string_lossy()],
                "to": dev_b,
                "dir": recv_b.to_string_lossy(),
            }),
        )
        .await
        .expect("transfer.start 应成功");
        let job_id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 传输开动即硬杀对端(32MiB 在毫秒级杀窗内不可能终态)
        wait_transfer_started(&recv_b.join(".mtdss"), Duration::from_secs(60)).await;
        child_b.start_kill().expect("kill peer daemon");
        let st = child_b.wait().await.expect("reap peer daemon");
        assert!(!st.success(), "被杀对端不得干净退出");

        // A 预算耗尽落 failed(idle 0.8s + 2×300ms 退避 + 信令超时上界)
        let fin = call_bounded(
            &ca,
            "transfer.wait",
            json!({ "id": job_id }),
            Duration::from_secs(60),
        )
        .await
        .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("failed"), "{fin}");

        // presence 翻离线 → 清除走离线预检:code 3,任务保留(二清仍 code 3)
        wait_peer_online(&ca, &dev_b, false, 60).await;
        let err = call(&ca, "transfer.clear", json!({ "id": job_id }))
            .await
            .expect_err("对端离线,清除必须失败");
        assert_eq!(err.code, 3, "离线是 code 3: {err:?}");
        assert!(err.message.contains("离线"), "报文点名离线: {err:?}");
        let err2 = call(&ca, "transfer.clear", json!({ "id": job_id }))
            .await
            .expect_err("二清仍须失败");
        assert_eq!(
            err2.code, 3,
            "记录仍在(未摘除),二清仍走离线预检 code 3: {err2:?}"
        );

        drop(cb);
    });
    run.await.expect("clear-offline e2e exceeded 240s");
}

/// download 本地清(Pair 轻量例,spec §5.2 接收行):A 从 B 下载半程 →
/// cancel → cancelled 终态(可清)→ transfer.clear 恒可执行(不依赖对端)
/// → Ok;二清「不存在」证记录已摘。当期 download 记录 rels 为空(待
/// Task 8 job_query 回填),清除走「无可清面直接删记录」退化路径;rels
/// 回填后的本地 `.mtdss` 清理由 jobs 单测
/// `clear_download_cleans_local_intake_and_removes_record` 钉死。
#[tokio::test(flavor = "multi_thread")]
async fn clear_download_removes_record() {
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("tdlclr@x.y").await;
        let src_b = tempfile::tempdir().unwrap();
        let f = src_b.path().join("下载待清.bin");
        std::fs::write(&f, vec![0u8; 32 * 1024 * 1024]).unwrap();
        let dst = tempfile::tempdir().unwrap();

        let r = call(
            &p.ca,
            "transfer.start",
            json!({
                "kind": "download",
                "device": p.dev_b,
                "paths": [f.to_string_lossy()],
                "dir": dst.path().to_string_lossy(),
            }),
        )
        .await
        .expect("transfer.start 应成功");
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 等本机(接收端)intake 记录落盘:传输确实开动,取消落在半程
        let rec_a = record_path(dst.path(), "下载待清.bin");
        let deadline = Instant::now() + Duration::from_secs(60);
        while !rec_a.exists() {
            assert!(
                Instant::now() < deadline,
                "本机 .mtdss 记录未在预算内出现: {}",
                rec_a.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        call(&p.ca, "transfer.cancel", json!({ "id": id }))
            .await
            .expect("cancel 应成功");
        let fin = call(&p.ca, "transfer.wait", json!({ "id": id }))
            .await
            .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("cancelled"), "{fin}");

        // 清除:恒可执行 → Ok;二清「任务不存在」证记录已摘
        let r = call(&p.ca, "transfer.clear", json!({ "id": id }))
            .await
            .expect("download 清除应成功");
        assert_eq!(r, json!({}));
        let err = call(&p.ca, "transfer.clear", json!({ "id": id }))
            .await
            .expect_err("二清必须失败(记录已摘)");
        assert_eq!(err.code, 1, "{err:?}");
        assert!(err.message.contains("不存在"), "{err:?}");

        p.shutdown().await;
    });
    run.await.expect("download-clear e2e exceeded 150s");
}
