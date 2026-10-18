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
    dev_b: String,
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
        let ra = ipc_login(&ca, &server, email, "A", None).await;
        let rb = ipc_login(&cb, &server, email, "B", None).await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let _dev_a = ra["device_id"].as_str().unwrap().to_string();
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
            dev_b,
        }
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
        let (addr, _state) = spawn_test_server().await;
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
        let (addr, _state) = spawn_test_server().await;
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
        let (addr, _state) = spawn_test_server().await;
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
        let (addr, ca_path, _dir) = signaling::spawn_test_tls_server().await;
        let server = format!("https://{addr}");
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
        signaling::tls::generate_dev_tls(other.path()).unwrap();
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
        let (addr, _state) =
            signaling::spawn_test_server_with(signaling::ws::Keepalive::default(), vec![pa.to_string(), pb.to_string()])
                .await;
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
