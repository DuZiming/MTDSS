//! 在线循环端到端:登录 → 自动上线 → 设备目录快照与事件 → 断连退避恢复。
//!
//! 覆盖三块:presence 驱动的快照联动(`event.devices` / `devices` RPC)、
//! 死 JWT 的 refresh 自愈(轮换令牌写回 session.json)、服务器重启后的同
//! 端口重连。退避纯函数(`backoff_for`)的封顶锚点在 online.rs 库内单测。
//!
//! 固定端口服务器为测试自建:axum::serve 的连接任务是独立 spawn、句柄不可
//! 及(abort 只停 accept,在线连接不灭,模拟不了"服务器死亡"),这里自管
//! accept 循环,kill 时强断全部连接任务——套接字随之关闭,与进程死亡等效。
//! 同端口重启以 SO_REUSEADDR 绕过 TIME_WAIT 残留(Windows 上旧连接占端口)。

use mtdss_core::signaling::client::SignalingClient;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::{spawn_test_server, AppState};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂。
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// 临时 home + 显式 recv_dir 的启动选项(不碰真实 mtdss_home)。
fn opts(home: &std::path::Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
    }
}

/// 限时 IPC 调用;超时视为测试失败(暴露悬挂),RpcError 原样上抛供断言。
async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    match tokio::time::timeout(CALL_TIMEOUT, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("call {method} 超时({CALL_TIMEOUT:?})"),
    }
}

/// 等过 devices 单飞备忘窗口(M4a Task 5,1s TTL):presence 驱动的快照
/// 联动只改缓存快照、不落 memo——快照变更后立即调 devices RPC 会撞 memo
/// 命中拿到变更前数据。断言「变更即时可见」前先让窗口过期(1s + 余量)。
async fn memo_window_pass() {
    tokio::time::sleep(Duration::from_millis(1100)).await;
}

/// 完整登录走一遍:login.request_code(dev 回显取码)→ login.verify。
async fn login(client: &IpcClient, server: &str, email: &str, name: &str) -> Result<Value, RpcError> {
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
}

/// dev-mode 验证码直接回带,REST 直注注册同账号第二台设备(flow::register
/// 是交互式 stdin 流程;与 cli_e2e 的 register_rest 同款)。
async fn register_rest(
    addr: SocketAddr,
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

/// 轮询事件流,直到出现含 dev(online=want) 的 event.devices 全量快照。
async fn wait_device_event(evs: &mut broadcast::Receiver<EventFrame>, dev: &str, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let frame = match tokio::time::timeout_at(tokio::time::Instant::from(deadline), evs.recv()).await {
            Ok(Ok(f)) => f,
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                // 慢消费者丢帧不致命:后续快照是全量,继续等
                eprintln!("事件通道滞后 {n} 帧,继续等待");
                continue;
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
            Err(_) => panic!("等待 event.devices({dev} online={want}) 超时({secs}s)"),
        };
        assert_eq!(frame.method, "event.devices", "非预期事件: {:?}", frame);
        for d in frame.params["devices"].as_array().cloned().unwrap_or_default() {
            if d["device_id"].as_str() == Some(dev) && d["online"].as_bool() == Some(want) {
                return;
            }
        }
    }
}

/// 轮询事件流,直到出现 event.online {online:want}。
async fn wait_online_event(evs: &mut broadcast::Receiver<EventFrame>, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let frame = match tokio::time::timeout_at(tokio::time::Instant::from(deadline), evs.recv()).await {
            Ok(Ok(f)) => f,
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                eprintln!("事件通道滞后 {n} 帧,继续等待");
                continue;
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
            Err(_) => panic!("等待 event.online {{online:{want}}} 超时({secs}s)"),
        };
        if frame.method == "event.online" && frame.params["online"].as_bool() == Some(want) {
            return;
        }
    }
}

/// 断言快照中该设备的名字与 online,并核对恰好四字段投影(pubkey 不出 IPC 面)。
fn assert_device(devs: &Value, device_id: &str, name: &str, online: bool) {
    let list = devs["devices"].as_array().expect("devices 数组");
    let entry = list
        .iter()
        .find(|d| d["device_id"].as_str() == Some(device_id))
        .unwrap_or_else(|| panic!("快照缺设备 {device_id}: {devs}"));
    assert_eq!(
        entry["online"].as_bool(),
        Some(online),
        "设备 {device_id} online 不符: {devs}"
    );
    assert_eq!(entry["name"].as_str(), Some(name), "设备 {device_id} 名字不符: {devs}");
    let mut keys: Vec<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["device_id", "name", "online", "platform"],
        "设备条目必须恰好四字段: {entry}"
    );
}

// ---------- 登录上线 + presence 快照联动 ----------

#[tokio::test(flavor = "multi_thread")]
async fn login_then_online_and_devices_snapshot() {
    let (addr, state) = spawn_test_server().await;
    // 认证收口后仅预置账号可领码:登录前先预置(幂等,下同)
    signaling::admin::user_add(&state.db.lock().unwrap(), "snap@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    // 登录 → 在线循环自动上线
    let r = login(&client, &server, "snap@x.y", "本机").await.unwrap();
    let me = r["device_id"].as_str().unwrap().to_string();
    wait_online(&client, true, 30).await;

    // 初始快照:仅本机且 online=true
    let devs = call(&client, "devices", json!({})).await.unwrap();
    let list = devs["devices"].as_array().unwrap();
    assert_eq!(list.len(), 1, "登录初期目录仅本机: {devs}");
    assert_device(&devs, &me, "本机", true);

    // 第二台设备:REST 注册同一账号 → 直连信令制造 presence online
    let id_b = mtdss_core::identity::DeviceIdentity::generate();
    let (jwt_b, _ref_b, dev_b) =
        register_rest(addr, "snap@x.y", &hex::encode(id_b.public_key().as_bytes()), "副机").await;

    // 先订阅再触发,避免漏帧;副机注册晚于上次全量 → 在线循环走未知设备
    // 的全量刷新补齐,快照含副机 online=true
    let mut evs = client.events();
    let (_sb, _eb) = SignalingClient::connect(&format!("ws://{addr}/ws"), &jwt_b, &dev_b)
        .await
        .unwrap();
    wait_device_event(&mut evs, &dev_b, true, 30).await;
    memo_window_pass().await;
    let devs = call(&client, "devices", json!({})).await.unwrap();
    assert_device(&devs, &dev_b, "副机", true);
    assert_device(&devs, &me, "本机", true);

    // 副机断开 → 快照转 false;本机保持在线
    drop(_eb);
    drop(_sb);
    wait_device_event(&mut evs, &dev_b, false, 30).await;
    memo_window_pass().await;
    let devs = call(&client, "devices", json!({})).await.unwrap();
    assert_device(&devs, &dev_b, "副机", false);
    assert_device(&devs, &me, "本机", true);

    daemon.shutdown().await.unwrap();
}

// ---------- 死 JWT 的 refresh 自愈 ----------

#[tokio::test(flavor = "multi_thread")]
async fn dead_jwt_refreshes_and_recovers() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "rot@x.y").unwrap();
    let server = format!("http://{addr}");
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    login(&client, &server, "rot@x.y", "r1").await.unwrap();
    wait_online(&client, true, 30).await;
    daemon.shutdown().await.unwrap();
    // 先弃旧 IPC 客户端:Windows 上客户端句柄在,旧命名管道名不释放,
    // 第二个 Daemon::start 的监听会因名字占用而就绪失败。
    drop(client);

    // 手动把 jwt 换成垃圾串,refresh 保持有效(模拟访问令牌失效)
    let garbage = "garbage-jwt-not-a-token";
    let mut sess = mtdss_daemon::session::Session::load(home.path()).unwrap();
    sess.jwt = garbage.into();
    sess.save(home.path()).unwrap();

    // 重启 daemon:在线循环握手收 code=1 → account::refresh 轮换 → 重连上线
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    wait_online(&client, true, 30).await;

    // session.json 的 jwt 已换新(≠垃圾串)
    let after = mtdss_daemon::session::Session::load(home.path()).unwrap();
    assert_ne!(after.jwt, garbage, "轮换后的 jwt 必须已写回 session.json");

    daemon.shutdown().await.unwrap();
}

// ---------- 服务器重启后的同端口重连 ----------

/// 自管 accept 循环的测试服务器:连接任务句柄在手,kill 强断全部在线连接。
/// (axum::serve 的连接任务不可及;且 axum 的 WS 会话在自己的 spawn 任务里,
/// 连接任务 abort 杀不到——真杀靠 KillableStream 的 IO 层开关。)
struct KillableServer {
    addr: SocketAddr,
    accept: tokio::task::JoinHandle<()>,
    conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    kill: Arc<AtomicBool>,
}

impl KillableServer {
    /// 杀服务器:停 accept + 置 IO 层杀开关(在线 WS 的下一次读写即报错,
    /// 服务端随即断连清理)+ 强断连接任务。杀开关在先,确保升级后的 WS
    /// (不受任务 abort 管)也被切断。
    fn kill(&self) {
        self.accept.abort();
        self.kill.store(true, Ordering::SeqCst);
        let mut conns = self.conns.lock().unwrap();
        for h in conns.drain(..) {
            h.abort();
        }
    }
}

impl Drop for KillableServer {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 可外杀的流包装:kill 置位后所有读写立即报 ConnectionAborted。升级后的
/// WS IO 沿同一流走,这是进程内唯一可靠的"服务器死亡"模拟点。
struct KillableStream {
    inner: tokio::net::TcpStream,
    kill: Arc<AtomicBool>,
}

impl KillableStream {
    fn check(&self) -> std::io::Result<()> {
        if self.kill.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "test server killed",
            ));
        }
        Ok(())
    }
}

impl tokio::io::AsyncRead for KillableStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.check()?;
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for KillableStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.check()?;
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// 在 `127.0.0.1:port`(port=0 由 OS 分配)上起信令服务器。
/// `db_name` 与 `jwt_secret` 由调用方注入:同端口重启需同 DB + 同密钥
/// (生产语义:持久数据的服务器重启,既有令牌继续有效)。keepalive 取
/// 1s ping:kill 后服务端下一次 ping 写入(≤1s)即撞杀开关断连,测试
/// 不必等默认 20s;silent_limit 放宽避免误杀安静期的既有连接。
async fn spawn_server(
    dir: &std::path::Path,
    db_name: &str,
    jwt_secret: [u8; 32],
    port: u16,
) -> KillableServer {
    let db = signaling::db::open(dir.join(db_name).to_str().unwrap()).expect("open test db");
    let state = Arc::new(AppState {
        db: Mutex::new(db),
        jwt_secret,
        presence: Arc::new(signaling::PresenceRegistry::new()),
        keepalive: signaling::ws::Keepalive {
            ping_interval: Duration::from_secs(1),
            silent_limit: Duration::from_secs(300),
        },
        stun_addrs: Vec::new(),
        relay: std::sync::OnceLock::new(),
    });
    let app = signaling::build_router(state);
    let listener = reuse_listener(port).expect("bind test listener");
    let addr = listener.local_addr().unwrap();
    let kill = Arc::new(AtomicBool::new(false));
    let conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let accept_conns = conns.clone();
    let accept_kill = kill.clone();
    let accept = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            let app = app.clone();
            let k = accept_kill.clone();
            let h = tokio::spawn(async move {
                serve_conn(
                    app,
                    KillableStream {
                        inner: stream,
                        kill: k,
                    },
                )
                .await
            });
            let mut guard = accept_conns.lock().unwrap();
            guard.retain(|h| !h.is_finished()); // 顺手回收已结束句柄,防增长
            guard.push(h);
        }
    });
    KillableServer {
        addr,
        accept,
        conns,
        kill,
    }
}

/// SO_REUSEADDR 监听器:同端口重启时绕过 TIME_WAIT 残留(仅测试内使用;
/// 生产 bind 无此语义,不影响被测代码)。
fn reuse_listener(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    sock.listen(1024)?;
    // from_std 本就返回 io::Result,内层 ? 与 Ok 包装是同型冗余(clippy)
    tokio::net::TcpListener::from_std(sock.into())
}

/// 单连接服务:axum Router 以 hyper 直驱(与 axum::serve 内部同构);
/// Router 的 Service 实现自带 Incoming→Body 映射,WS upgrade 由
/// with_upgrades 支持。IO 为 KillableStream(升级后的 WS 仍经其读写,
/// kill 开关可达);任务句柄归测试所有。
async fn serve_conn(app: axum::Router, stream: KillableStream) {
    use tower::Service as _;
    let service = hyper::service::service_fn(
        move |req: hyper::Request<hyper::body::Incoming>| {
            let mut app = app.clone();
            async move {
                // Router::Service::call 的 Future 直接产出 Result<Response, Infallible>,
                // Infallible 错误侧 unwrap 即可(永不触发)
                Ok::<_, std::convert::Infallible>(app.call(req).await.unwrap())
            }
        },
    );
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_restart_daemon_recovers() {
    // daemon 库内不初始化日志;测试里补一个全局订阅,在线循环的上线/断连
    // 追踪随之可见(多测并发重复初始化由 try_init 容忍)
    let _ = tracing_subscriber::fmt().try_init();
    let dir = tempfile::tempdir().unwrap();
    let secret: [u8; 32] = rand::random();

    // 第一台服务器:固定端口(port=0 首绑由 OS 分配,phase 2 复用同端口)。
    // 两台共用同一 DB 文件与 jwt_secret:生产语义是"持久数据的服务器重启",
    // 既有账号/设备行与已发令牌在重启后继续有效(server1 被 kill 后其
    // Connection 随任务中止释放,重开同一文件安全;下方 offline 轮询窗口
    // 已给它留足落定时间)。
    let server1 = spawn_server(dir.path(), "t.db", secret, 0).await;
    let port = server1.addr.port();
    let server_url = format!("http://{}", server1.addr);

    // 认证收口后仅预置账号可领码:服务器自有 AppState 已移入任务,借 dir 里
    // 的 DB 路径另开连接预置(db::open 幂等,WAL 下双连接安全)
    let conn = signaling::db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
    signaling::admin::user_add(&conn, "re@x.y").unwrap();

    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    login(&client, &server_url, "re@x.y", "r").await.unwrap();
    wait_online(&client, true, 30).await;

    // 先订阅,再杀服务器(强断连接 = 进程死亡的套接字效果)
    let mut evs = client.events();
    drop(server1);

    // 观测 offline 事件 + status 同步翻转(此窗口端口已死,daemon 无法重连)
    wait_online_event(&mut evs, false, 30).await;
    wait_online(&client, false, 10).await;

    // 同端口重启:同 DB + 同 jwt_secret → daemon 退避重连后最终 online=true,
    // 期间 daemon 进程不退(同一个 Daemon 句柄与 IPC 连接走完全程)
    let server2 = spawn_server(dir.path(), "t.db", secret, port).await;
    assert_eq!(server2.addr.port(), port, "同端口重启必须绑定成功");
    wait_online(&client, true, 60).await;
    wait_online_event(&mut evs, true, 60).await;

    daemon.shutdown().await.unwrap();
}
