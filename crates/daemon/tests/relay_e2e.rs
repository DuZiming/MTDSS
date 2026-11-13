//! 中继枢纽 e2e(v2-P1c Task 4):真信令服务器 + 真中继数据面 + 进程内双
//! daemon,钉住枢纽面对 HelloOk 资格的缓存与 RelaySession 推送的路由;
//! 分配/订阅经 Daemon 的 doc(hidden) 直驱面直驱 hub,不进 IPC 契约。
//! T5(兜底编排)的传输面用例在本文件追加。
//!
//! 全部用例限定 worker_threads = 2(jobs_e2e 口径:全 workspace 并跑下
//! 避免过度订阅饿死打洞/信令预算;用例均为 IO 型,2 worker 足够)。

use ed25519_dalek::VerifyingKey;
use mtdss_core::identity::DeviceIdentity;
use mtdss_core::signaling::client::AllocOutcome;
use mtdss_core::transport::endpoint as ep;
use mtdss_core::transport::punch::PunchConfig;
use mtdss_daemon::fallback::{FallbackErr, FallbackRole, RelayPurpose};
use mtdss_daemon::hub::RelayEvt;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::IpcClient;
use serde_json::{json, Value};
use signaling::AppState;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 同文件用例并发闸(jobs_e2e 同款):全 workspace 并跑下限并发保打洞/
/// 信令预算;本文件用例均为重装配(双 daemon + 真中继),闸 2。
static CASE_GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

/// 持有至用例体结束的并发许可(用例体开头取,async 块结束即还)。
async fn case_gate() -> tokio::sync::OwnedSemaphorePermit {
    CASE_GATE
        .clone()
        .acquire_owned()
        .await
        .expect("semaphore never closed")
}

/// 临时 home + 显式 recv_dir 的启动选项(不碰真实 mtdss_home)。
fn opts(home: &std::path::Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        // 桌面形态:无收件箱发布面(v2-P5 T5b,哨兵回退默认收件目录)
        inbox_publish: false,
            photo_serve: false,
            ctrl_tuning: None,
    }
}

/// 限时 IPC 调用;超时视为测试失败(暴露悬挂)。
async fn call(client: &IpcClient, method: &str, params: Value) -> Value {
    tokio::time::timeout(Duration::from_secs(15), client.call(method, params))
        .await
        .expect("call 超时")
        .expect("call 应成功")
}

/// IPC 完整登录:login.request_code(dev 回显取码)→ login.verify。
async fn ipc_login(client: &IpcClient, server: &str, email: &str, name: &str) -> Value {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email })).await;
    let code = code["code"].as_str().expect("result 必含 code 字符串");
    call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
}

/// V3-1 审批状态机:任何上线发生前 REST 预置一台设备行(预生成身份落 seed,
/// daemon IPC 登录幂等收敛同一 device_id),登录恒走老终端直入。
async fn preregister_identity(addr: SocketAddr, home: &Path, email: &str, name: &str) {
    let id = DeviceIdentity::generate();
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

/// 轮询 status.online 直到等于 want。在线循环先握手后上线:mark_online
/// 晚于 HelloOk 握手与代际安装,online=true 即本代资格已捕获(relay_e2e
/// 的 entitlement 断言依赖这一时序)。
async fn wait_online(client: &IpcClient, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = call(client, "status", json!({})).await;
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
        let devs = call(client, "devices", json!({})).await;
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

/// 限时 IPC 调用,上界参数化(transfer.wait 长轮询给足传输全程,常规调用
/// 沿 [`call`] 的 15s)。
async fn call_for(client: &IpcClient, method: &str, params: Value, t: Duration) -> Value {
    tokio::time::timeout(t, client.call(method, params))
        .await
        .unwrap_or_else(|_| panic!("call {method} 超时({t:?})"))
        .expect("call 应成功")
}

/// 在随机端口起中继数据面,以**真实端口**作通告地址:daemon 从 wire 下发的
/// relay_addr 直连中继数据面,"127.0.0.1:0" 字面通告会拨到端口 0(T4 旧注
/// 释只钉路由解析,T5 数据面用例起改真实端口)。先探一个空闲端口再以它
/// bind + 通告(极小 TOCTOU 窗,测试环境可接受)。返回测试可达的回环地址
/// (bind 0.0.0.0,测试走 127.0.0.1)。
async fn spawn_relay_on(state: &Arc<AppState>) -> SocketAddr {
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    signaling::relay::spawn_relay(state.clone(), port, format!("127.0.0.1:{port}")).unwrap();
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// 进程内双 daemon 测试台(沿 jobs_e2e::Pair 样板,加真中继数据面与资格
/// 开通):独立信令服务器 + 两侧独立 home/IPC 客户端,双方已登录、已上线
/// (资格已随 HelloOk 入代)且互见在线。home 的 TempDir 必须随 Pair 存活
/// ——drop 即删目录,daemon 的 identity.seed 会随之消失。
struct Pair {
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<AppState>,
    daemon_a: Daemon,
    daemon_b: Daemon,
    /// 两侧 IPC 客户端(保持连接在场;T5 传输面用例经 jobs RPC 驱动/断言)。
    ca: IpcClient,
    cb: IpcClient,
    dev_a: String,
    dev_b: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        Self::new_inner(email, false).await
    }

    /// T5 数据面变体:双侧编排选项注入 fault_force_relay(经 jobs 面跑真中继
    /// 传输的用例专用——同机回环直连必通,不注错测不到中继路径)。
    async fn new_forced(email: &str) -> Self {
        Self::new_inner(email, true).await
    }

    async fn new_inner(email: &str, force_relay: bool) -> Self {
        // 进程内 daemon 日志捕获(ctrl_e2e 同款:进程级一次性,with_test_
        // writer 仅失败用例落盘,绿跑零噪音)——回切编舞多路时序(warmer
        // 重打/armed 记账/轮登记),失败时凭此直接读时序,不盲猜。
        static LOG_INIT: std::sync::Once = std::sync::Once::new();
        LOG_INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
                .with_test_writer()
                .try_init();
        });
        let (addr, state) = signaling::spawn_test_server().await;
        // 认证收口后仅预置账号可领码;user_add 预建 relay_config 行
        //(enabled=0 + 默认限速 1024 + 默认配额 10 GiB),再显式开通
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        signaling::admin::relay_set_enabled(&state.db.lock().unwrap(), email, true).unwrap();
        // 分配门禁第一步要求数据面已装配(未起即按未开通拒 4);通告地址须
        // 真实端口(daemon 直连 wire 下发的 relay_addr)
        let _relay_addr = spawn_relay_on(&state).await;
        let server = format!("http://{addr}");
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        // V3-1 审批状态机:任何上线前 REST 预置两侧设备行(老终端直入)
        preregister_identity(addr, home_a.path(), email, "A").await;
        preregister_identity(addr, home_b.path(), email, "B").await;
        let daemon_a = if force_relay {
            Daemon::start_with_force_relay(home_a.path(), opts(home_a.path()))
                .await
                .unwrap()
        } else {
            Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap()
        };
        let daemon_b = if force_relay {
            Daemon::start_with_force_relay(home_b.path(), opts(home_b.path()))
                .await
                .unwrap()
        } else {
            Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap()
        };
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        let ra = ipc_login(&ca, &server, email, "A").await;
        let rb = ipc_login(&cb, &server, email, "B").await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let dev_a = ra["device_id"].as_str().unwrap().to_string();
        let dev_b = rb["device_id"].as_str().unwrap().to_string();
        assert_ne!(dev_a, dev_b, "双侧必须是不同设备");
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
        }
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

// ---------- Task 4:枢纽中继面 ----------

/// HelloOk 资格缓存:在线即有本代缓存,值为 admin 默认资格(开通 + 默认
/// 限速 1024 kbps + 默认配额 10 GiB,当期零用量),双 daemon 各自缓存;
/// 离线 None 面由 T5 消费,不在 e2e 面。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entitlement_cached_from_hello() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlyent@x.y").await;
        for (who, d) in [("A", &p.daemon_a), ("B", &p.daemon_b)] {
            let ent = d
                .relay_entitlement()
                .unwrap_or_else(|| panic!("{who} 在线必须有本代资格缓存"));
            assert!(ent.enabled, "{who} 资格须已开通");
            assert_eq!(ent.rate_kbps, 1024, "{who} 限速须为 admin 默认");
            assert_eq!(
                ent.quota_remaining,
                10 * 1024 * 1024 * 1024,
                "{who} 配额余量须为默认 10 GiB(当期零用量)"
            );
        }
        p.shutdown().await;
    });
    run.await.expect("entitlement e2e exceeded 120s");
}

/// 分配 + 推送路由:B 先订中继事件(轮首订阅语义:先订后分配),A 直驱
/// 分配 → A 得 Allocated(限速=账号默认,tag 16 字节,wire 地址可解析);
/// B 的订阅收到 Push(peer=A 的 device_id、sid 与 A 侧一致、tag 16 字节、
/// relay_addr 已解析为回环 SocketAddr)。tag/addr 畸形丢弃路径在 hub 单测
/// 钉住(e2e 造不出畸形服务器帧)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allocation_push_routed() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlyalc@x.y").await;
        // 先订阅再分配(分配推送不得早于订阅窗口)
        let mut rx = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");

        let out = p
            .daemon_a
            .relay_allocate(&p.dev_b, RelayPurpose::Transport.as_str())
            .await
            .expect("分配调用应成功返回(Denied 不是 Err)");
        let sid = match out {
            AllocOutcome::Allocated { sid, tag, relay_addr, expires_at: _, rate_kbps } => {
                assert_eq!(rate_kbps, 1024, "会话限速须为账号配置默认");
                assert_eq!(tag.len(), 16, "请求方槽 tag 须 16 字节");
                assert!(
                    relay_addr.parse::<SocketAddr>().is_ok(),
                    "wire relay_addr 须可解析: {relay_addr}"
                );
                sid
            }
            AllocOutcome::Denied { code, message } => {
                panic!("开通账号的分配不得被拒: {code} {message}")
            }
        };

        let evt = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("分配推送未在 10s 内到达")
            .expect("本代事件通道存活");
        match evt {
            RelayEvt::Push { peer, sid: push_sid, tag, relay_addr, expires_at: _, purpose } => {
                assert_eq!(peer, p.dev_a, "分配帧 peer_device 须为请求方设备号");
                assert_eq!(push_sid, sid, "双帧须同会话号");
                assert_eq!(tag.len(), 16, "对端槽 tag 须 16 字节");
                assert!(
                    relay_addr.ip().is_loopback(),
                    "relay_addr 须为回环 SocketAddr: {relay_addr}"
                );
                assert_eq!(purpose, "transport", "分配推送须回显请求平面(v2-P2 票①)");
            }
            RelayEvt::Dead { sid: dead_sid, reason } => {
                panic!("分配通告不得路由成死帧: {dead_sid} {reason}")
            }
        }

        p.shutdown().await;
    });
    run.await.expect("allocation routing e2e exceeded 120s");
}

// ---------- Task 5:connect-with-fallback 编排 + 传输面 ----------

/// 直连候选黑洞:TEST-NET-1 丢弃端口,拨号无响应按预算快败(brief 钉死的
/// 「直连穷尽」注错形态)。
fn black_hole() -> SocketAddr {
    "192.0.2.1:9".parse().unwrap()
}

/// 直驱用例两侧用 daemon 的真身份(QUIC 证书与 pinning 按登录身份对齐):
/// 直接读 home 的 identity.seed。
fn identity_of(home: &Path) -> DeviceIdentity {
    DeviceIdentity::from_seed_file(&home.join("identity.seed")).unwrap()
}

/// 直驱侧的直连双角色端点(白名单=对端):force 用例不拨、黑洞用例走预算
/// 快败——仅作 connect_with_fallback 的 direct 参数。
fn direct_endpoint(me: &DeviceIdentity, peer_id: &str, peer_pk: &VerifyingKey) -> quinn::Endpoint {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let allowed: HashMap<String, VerifyingKey> =
        [(peer_id.to_string(), *peer_pk)].into_iter().collect();
    ep::make_dual_endpoint_tuned(sock, me, &allowed, ep::TransportTuning::default()).unwrap()
}

/// 直驱 punch 配置:预算参数化(中继互拨沿 T3 校准:真中继丢首轮拨号,
/// 竞态下经 parked/held 配对,3s 预算对 ms 级完成路径有百倍余量)。
fn test_punch(budget: Duration) -> PunchConfig {
    PunchConfig {
        budget,
        ..Default::default()
    }
}

/// 双向流回显:发起端 open_bi 写 payload 并半关,响应端 accept_bi 读全量
/// 后原样回写,发起端读回校验(沿 core relay_sock 测试的回显房风)。
async fn echo_round(init: &quinn::Connection, resp: &quinn::Connection, payload: &[u8]) {
    let (mut tx, mut rx) = init.open_bi().await.unwrap();
    tx.write_all(payload).await.unwrap();
    tx.finish().unwrap();
    let (mut rtx, mut rrx) = resp.accept_bi().await.unwrap();
    let got = rrx.read_to_end(usize::MAX).await.unwrap();
    assert_eq!(got, payload);
    rtx.write_all(&got).await.unwrap();
    rtx.finish().unwrap();
    let echoed = rx.read_to_end(usize::MAX).await.unwrap();
    assert_eq!(echoed, payload);
}

/// 随机填充并落盘,返回内容副本(落盘一致性断言用)。
fn fill_random_file(path: &Path, size: usize) -> Vec<u8> {
    use rand::RngCore as _;
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    std::fs::write(path, &data).unwrap();
    data
}

/// transfer.start upload 形态速构(沿 jobs_e2e 先例);返回 RPC 结果
/// (call 已保证成功)。
async fn start_upload(client: &IpcClient, files: &[PathBuf], to: &str, dir: &str) -> Value {
    call(
        client,
        "transfer.start",
        json!({
            "kind": "upload",
            "files": files.iter().map(|f| f.to_string_lossy()).collect::<Vec<_>>(),
            "to": to,
            "dir": dir,
        }),
    )
    .await
}

/// 邮箱 → 账号 id(服务器 DB 直查,relay_usage 记账断言的键)。
fn account_id(state: &Arc<AppState>, email: &str) -> String {
    state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| r.get(0))
        .unwrap()
}

/// 账号当期 relay_usage 已落库字节(未刷库/无行为 0;台账另有 1 MiB/5s
/// 自动刷库阈值,断言前以 wait_until 收口)。
fn relay_usage_bytes(state: &Arc<AppState>, acc: &str) -> u64 {
    let period = signaling::admin::current_period();
    state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COALESCE(bytes_forwarded, 0) FROM relay_usage WHERE account_id=?1 AND period=?2",
            [acc, period.as_str()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

/// 轮询等待同步条件成立(服务器内存/DB 面的同步点;超时 panic)。
async fn wait_until(secs: u64, mut cond: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "condition not met within {secs}s: {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Review Focus #1(推送先于等待订阅的时序竞态):发起方 force_relay 即刻
/// 分配,响应方黑洞直连打洞 1s 超时后才进兜底等待——分配推送(~ms 量级)
/// 早已落入其轮首订阅的缓冲,等待段必须排干缓冲命中(漏缓冲的实现会干等
/// 5s 超窗后 RelayFailed)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_arrives_before_punch_end() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlypsh@x.y").await;
        let me_a = identity_of(p._home_a.path());
        let me_b = identity_of(p._home_b.path());
        let (pk_a, pk_b) = (me_a.public_key(), me_b.public_key());
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        let ep_b = direct_endpoint(&me_b, &p.dev_a, &pk_a);
        // 双侧轮首订阅先于一切(join! 并发起步:发起方分配 ~ms 即推送,
        // 响应方黑洞打洞 1s——推送必然先进缓冲)
        let mut rx_a = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_b = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let hole = [black_hole()];
        let (oa, ob) = tokio::join!(
            // 发起方:force_relay 即刻分配(预算 5s 覆盖响应方 1s 打洞 + 配对)
            p.daemon_a.connect_with_fallback(
                &mut rx_a, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                test_punch(Duration::from_secs(5)), FallbackRole::Initiator,
                RelayPurpose::Transport, true,
            ),
            // 响应方:黑洞直连 1s 快败后进等待,缓冲推送须命中
            p.daemon_b.connect_with_fallback(
                &mut rx_b, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                test_punch(Duration::from_secs(1)), FallbackRole::Responder,
                RelayPurpose::Transport, false,
            ),
        );
        let path_a = oa
            .expect("发起方 force_relay 兜底应成功")
            .relay
            .expect("发起方须走中继");
        let path_b = ob
            .expect("响应方缓冲推送命中后兜底应成功")
            .relay
            .expect("响应方须走中继");
        assert_eq!(path_a.sid, path_b.sid, "两侧须为同一中继会话");
        p.shutdown().await;
    });
    run.await.expect("push-before-wait e2e exceeded 120s");
}

/// 兜底互拨成对:Initiator(分配)与 Responder(推送)得同一会话,连接
/// 双向回显互通;rate 两侧一致(发起方取分配值,响应方取自身资格——同
/// 账号同配)。punch 配置沿生产形态(test_punch = Default tuning 1452 +
/// 3s 小预算,沿 T3 情报房风):连接成功即证 relay_dial 内部的 MTU 覆写
/// (整份 PunchConfig 覆写 1400,修复轮)不破互拨;线上单包尺寸断言
/// (≤ 1416)由 core relay_sock::mtu_capped(Default 接线)承担——本文件
/// 走真中继数据面(spawn_relay),无逐包观测点(修复轮裁定,二选一取此)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallback_connect_over_relay() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlycwo@x.y").await;
        let me_a = identity_of(p._home_a.path());
        let me_b = identity_of(p._home_b.path());
        let (pk_a, pk_b) = (me_a.public_key(), me_b.public_key());
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        let ep_b = direct_endpoint(&me_b, &p.dev_a, &pk_a);
        let mut rx_a = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_b = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let hole = [black_hole()];
        let (oa, ob) = tokio::join!(
            p.daemon_a.connect_with_fallback(
                &mut rx_a, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                test_punch(Duration::from_secs(3)), FallbackRole::Initiator,
                RelayPurpose::Transport, true,
            ),
            p.daemon_b.connect_with_fallback(
                &mut rx_b, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                test_punch(Duration::from_secs(3)), FallbackRole::Responder,
                RelayPurpose::Transport, true,
            ),
        );
        let oa = oa.expect("Initiator 兜底应成功");
        let ob = ob.expect("Responder 兜底应成功");
        let path_a = oa.relay.expect("发起方须走中继");
        let path_b = ob.relay.expect("响应方须走中继");
        assert_eq!(path_a.sid, path_b.sid, "两侧须为同一中继会话");
        assert_eq!(path_a.rate_kbps, 1024, "发起方 rate 取分配值(admin 默认)");
        assert_eq!(path_b.rate_kbps, 1024, "响应方 rate 取自身资格(同账号同配)");
        // 双向回显:互拨必相配于同一连接的两端
        echo_round(&oa.conn, &ob.conn, &[0xA5u8; 32 * 1024]).await;
        echo_round(&ob.conn, &oa.conn, &[0x5Au8; 32 * 1024]).await;
        p.shutdown().await;
    });
    run.await.expect("fallback connect e2e exceeded 120s");
}

/// Review Focus #2(同对端并发兜底串线):A→B 与 B→A 同时兜底(各自
/// Initiator + 对向 Responder,四臂并发,恰好打满账号 2 会话并发上限),
/// 两条连接各自成对回显、互不串线(推送按 peer 过滤、死亡按 sid 路由、
/// 仅发起方分配 + per-peer 串行锁)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bidirectional_concurrent_fallbacks() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlybidr@x.y").await;
        let me_a = identity_of(p._home_a.path());
        let me_b = identity_of(p._home_b.path());
        let (pk_a, pk_b) = (me_a.public_key(), me_b.public_key());
        // 同侧两臂共用一个直连端点(force 下不拨,仅作 direct 参数);事件
        // 流按臂各取一份订阅(发起方臂不消费,签名要求;broadcast 接收端
        // 不可克隆,各自订阅即各自游标)
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        let ep_b = direct_endpoint(&me_b, &p.dev_a, &pk_a);
        let mut rx_a = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_a_init = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_b = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let mut rx_b_init = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let cfg = test_punch(Duration::from_secs(3));
        let hole = [black_hole()];
        let (a_init, b_resp, b_init, a_resp) = tokio::join!(
            p.daemon_a.connect_with_fallback(
                &mut rx_a_init, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                cfg, FallbackRole::Initiator, RelayPurpose::Transport, true,
            ),
            p.daemon_b.connect_with_fallback(
                &mut rx_b, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                cfg, FallbackRole::Responder, RelayPurpose::Transport, true,
            ),
            p.daemon_b.connect_with_fallback(
                &mut rx_b_init, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                cfg, FallbackRole::Initiator, RelayPurpose::Transport, true,
            ),
            p.daemon_a.connect_with_fallback(
                &mut rx_a, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                cfg, FallbackRole::Responder, RelayPurpose::Transport, true,
            ),
        );
        let a_init = a_init.expect("A→B 发起兜底应成功");
        let b_resp = b_resp.expect("B 响应 A 的兜底应成功");
        let b_init = b_init.expect("B→A 发起兜底应成功");
        let a_resp = a_resp.expect("A 响应 B 的兜底应成功");
        // 两条会话互异,各自两端同 sid
        let s_ab = a_init.relay.as_ref().expect("A→B 须走中继").sid;
        let s_ba = b_init.relay.as_ref().expect("B→A 须走中继").sid;
        assert_eq!(
            s_ab,
            b_resp.relay.as_ref().expect("B 响应须走中继").sid,
            "A→B 两端同会话"
        );
        assert_eq!(
            s_ba,
            a_resp.relay.as_ref().expect("A 响应须走中继").sid,
            "B→A 两端同会话"
        );
        assert_ne!(s_ab, s_ba, "双向兜底必须是两个独立会话(不串线)");
        // 互不串线:两条连接各自回显(载荷标记不同,串线即错帧/悬挂)
        echo_round(&a_init.conn, &b_resp.conn, &[0x11u8; 16 * 1024]).await;
        echo_round(&b_init.conn, &a_resp.conn, &[0x22u8; 16 * 1024]).await;
        p.shutdown().await;
    });
    run.await.expect("bidirectional fallback e2e exceeded 120s");
}

/// v2-P2 票①(双面并发组合):同一对端的 ctrl 与传输两路兜底同时进行
/// (A 双发起 + B 双响应,四臂并发)——两条分配推送先后落进 B 的订阅缓冲,
/// 响应方必须按 purpose 双判别各取各面。错绑形态(修复前):ctrl 响应拿
/// 到传输会话的 tag/addr——两面按身份白名单仍能 QUIC 配对,但测试级配对
/// 错位,回显即错帧/悬挂,测试显式失败。断言:两面各自成对同 sid、两会
/// 话互异(标签各归各)、两条连接各自双向回显。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dual_plane_concurrent_fallback() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlydual@x.y").await;
        let me_a = identity_of(p._home_a.path());
        let me_b = identity_of(p._home_b.path());
        let (pk_a, pk_b) = (me_a.public_key(), me_b.public_key());
        // 同侧两臂共用一个直连端点(force 下不拨,仅作 direct 参数);事件
        // 流按臂各取一份订阅(broadcast 接收端不可克隆,各自游标)
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        let ep_b = direct_endpoint(&me_b, &p.dev_a, &pk_a);
        let mut rx_a_ctrl = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_a_tp = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_b_ctrl = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let mut rx_b_tp = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let cfg = test_punch(Duration::from_secs(3));
        let hole = [black_hole()];
        let (a_ctrl, b_ctrl, a_tp, b_tp) = tokio::join!(
            p.daemon_a.connect_with_fallback(
                &mut rx_a_ctrl, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                cfg, FallbackRole::Initiator, RelayPurpose::Ctrl, true,
            ),
            p.daemon_b.connect_with_fallback(
                &mut rx_b_ctrl, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                cfg, FallbackRole::Responder, RelayPurpose::Ctrl, true,
            ),
            p.daemon_a.connect_with_fallback(
                &mut rx_a_tp, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                cfg, FallbackRole::Initiator, RelayPurpose::Transport, true,
            ),
            p.daemon_b.connect_with_fallback(
                &mut rx_b_tp, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                cfg, FallbackRole::Responder, RelayPurpose::Transport, true,
            ),
        );
        let a_ctrl = a_ctrl.expect("ctrl 面发起兜底应成功");
        let b_ctrl = b_ctrl.expect("ctrl 面响应兜底应成功");
        let a_tp = a_tp.expect("传输面发起兜底应成功");
        let b_tp = b_tp.expect("传输面响应兜底应成功");
        // 两面各自成对同 sid,两会话互异(不错绑)
        let s_ctrl = a_ctrl.relay.as_ref().expect("ctrl 面须走中继").sid;
        let s_tp = a_tp.relay.as_ref().expect("传输面须走中继").sid;
        assert_eq!(
            s_ctrl,
            b_ctrl.relay.as_ref().expect("ctrl 响应须走中继").sid,
            "ctrl 面两端同会话"
        );
        assert_eq!(
            s_tp,
            b_tp.relay.as_ref().expect("传输面响应须走中继").sid,
            "传输面两端同会话"
        );
        assert_ne!(s_ctrl, s_tp, "双面并发必须是两个独立会话(标签各归各)");
        // 互不错绑:两条连接各自回显(载荷标记不同,错绑即错帧/悬挂)
        echo_round(&a_ctrl.conn, &b_ctrl.conn, &[0x33u8; 16 * 1024]).await;
        echo_round(&a_tp.conn, &b_tp.conn, &[0x44u8; 16 * 1024]).await;
        p.shutdown().await;
    });
    run.await.expect("dual-plane fallback e2e exceeded 120s");
}

/// 全链兜底传输:双侧 fault_force_relay,经 jobs 注册表发起 upload → 真中继
/// 数据面落盘齐;两作业(上传卡 + 匿名来件卡)snapshot.relay=true;服务器
/// 当期 relay_usage 记账 ≥ 文件字节且 ≤ 1.25×(裁定 H:规格容差 +10~25%;
/// 载荷 1 MiB 摊薄:双会话固定开销实测 ctrl ~25KB + 传输 ~23KB,128KiB
/// 载荷下合计 1.37× 失真——修复轮自 1.5× 收紧,失败按 F1 同源排查,不是
/// 放宽容差的理由)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_via_relay_full() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlyfull@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("中继 全链.bin");
        // 1 MiB:摊薄双会话固定开销(沿 download_via_relay_full 同一裁定,
        // 128KiB 载荷下固定开销失真)
        let size = 1024 * 1024;
        let data = fill_random_file(&file, size);

        let r = start_upload(&p.ca, &[file], &p.dev_b, &target.path().to_string_lossy()).await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        let fin =
            call_for(&p.ca, "transfer.wait", json!({ "id": id }), Duration::from_secs(60)).await;
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        // 落盘齐(定向目录,rel = 源文件名)
        let landed = target.path().join("中继 全链.bin");
        assert_eq!(std::fs::read(&landed).unwrap(), data, "落盘字节须一致");

        // 两作业的 relay 标记:上传卡(发起方 on_relay 恰一次)+ 匿名来件卡
        // (响应方轮内 mark_relay)
        assert_eq!(p.daemon_a.job_relay(&id), Some(true), "上传卡须带 relay 标记");
        // B 侧来件卡终态 done(轮收口在 A 的 wait 之后片刻,轮询收口)
        let id_b = {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let lst = call(&p.cb, "transfer.list", json!({})).await;
                let hit = lst["jobs"].as_array().unwrap().iter().find(|j| {
                    j["kind"].as_str() == Some("incoming") && j["state"].as_str() == Some("done")
                });
                if let Some(j) = hit {
                    break j["id"].as_str().unwrap().to_string();
                }
                assert!(Instant::now() < deadline, "B 侧来件卡未在 15s 内到 done: {lst}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        assert_eq!(p.daemon_b.job_relay(&id_b), Some(true), "来件卡须带 relay 标记");

        // 服务器当期记账(5s 刷库周期,wait_until 收口):≥ 文件字节(每个
        // 数据字节都过中继)且 ≤ 1.25×(裁定 H:规格容差 +10~25%,双会话
        // 固定开销 ctrl ~25KB + 传输 ~23KB 由 1 MiB 摊薄;修复轮自 1.5× 收紧)
        let acc = account_id(&p._state, "rlyfull@x.y");
        wait_until(
            20,
            || relay_usage_bytes(&p._state, &acc) >= size as u64,
            "relay_usage 落库",
        )
        .await;
        let used = relay_usage_bytes(&p._state, &acc);
        assert!(used >= size as u64, "记账须 ≥ 文件字节: {used}");
        assert!(used <= size as u64 * 5 / 4, "记账须 ≤ 1.25× 文件字节: {used}");

        p.shutdown().await;
    });
    run.await.expect("full relay transfer e2e exceeded 120s");
}

/// 裁定 C/G:配额杀会话 = 传输终局。传输在途后注入 1KB 余量(沿 server
/// 测试先例:直改台账字段)→ 下一笔记账超限杀会话、双端收
/// Dead{reason=quota} → 发送侧传输段按 relay_dead_err 收束为 SendError::
/// Relay 终局:job failed、error 含「中继配额已耗尽」、尝试轮数有界(终局
/// 错误不重试,恰 1 轮)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quota_kill_terminal() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlyqk@x.y").await;
        let acc = account_id(&p._state, "rlyqk@x.y");
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("配额.bin");
        // 1 MiB:会话限速 1024kbps(满桶 128KB 突发后 ~128KB/s)下传输需
        // 数秒,注入窗口充分;1KB 余量注入后下一笔记账即杀
        fill_random_file(&file, 1024 * 1024);

        let r = start_upload(&p.ca, &[file], &p.dev_b, &target.path().to_string_lossy()).await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 等传输已在途(上传卡 bytes_done > 0:兜底已建立、文件数据在流上,
        // 注杀必落在传输段而非互拨段)
        {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let lst = call(&p.ca, "transfer.list", json!({})).await;
                let done = lst["jobs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|j| j["id"].as_str() == Some(id.as_str()))
                    .and_then(|j| j["bytes_done"].as_u64())
                    .unwrap_or(0);
                if done > 0 {
                    break;
                }
                assert!(Instant::now() < deadline, "传输未在 30s 内起步: {lst}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        // 台账已 seed(首个被转发数据报后按库值惰性预置)→ 注入 1KB 余量
        wait_until(
            5,
            || p._state.relay.get().unwrap().quota.lock().unwrap().known(&acc),
            "台账 seed",
        )
        .await;
        p._state
            .relay
            .get()
            .unwrap()
            .quota
            .lock()
            .unwrap()
            .accounts
            .get_mut(&acc)
            .unwrap()
            .remaining = 1024;

        // 终局:failed + 配额文案 + 有界轮数(终局错误不空耗重试轮次)
        let fin =
            call_for(&p.ca, "transfer.wait", json!({ "id": id }), Duration::from_secs(30)).await;
        assert_eq!(fin["state"].as_str(), Some("failed"), "终态: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(err.contains("中继配额已耗尽"), "error 须含配额终局文案: {err}");
        assert!(err.contains("共尝试 1 轮"), "终局错误不空耗重试轮次: {err}");
        p.shutdown().await;
    });
    run.await.expect("quota kill e2e exceeded 120s");
}

/// 裁定 G:admin disable 后 force_relay 分配拒 4 = 终局「中继未开通」
/// (门禁实时查库,不被 HelloOk 缓存的旧资格绑架——disable 落在登录之后)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusal_disabled_terminal() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlydis@x.y").await;
        signaling::admin::relay_set_enabled(&p._state.db.lock().unwrap(), "rlydis@x.y", false)
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("停用.bin");
        fill_random_file(&file, 4 * 1024);
        let r = start_upload(&p.ca, &[file], &p.dev_b, &target.path().to_string_lossy()).await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        let fin =
            call_for(&p.ca, "transfer.wait", json!({ "id": id }), Duration::from_secs(30)).await;
        assert_eq!(fin["state"].as_str(), Some("failed"), "终态: {fin}");
        let err = fin["error"].as_str().unwrap_or_default();
        assert!(err.contains("中继未开通"), "error 须含未开通终局文案: {err}");
        assert!(err.contains("共尝试 1 轮"), "终局错误不空耗重试轮次: {err}");
        p.shutdown().await;
    });
    run.await.expect("refusal disabled e2e exceeded 120s");
}

/// 修复轮裁定:分配拒 6(超并发)= 容量瞬态可重试,非终局——与 4/5(裁定
/// G 只钉「配额耗尽」终局,未开通/停用同理)本质不同:并发槽位由服务端
/// 120s 空闲回收自愈,下一轮重试即可通过,终局失真。钉:占满账号 8 会话
/// 并发上限(V3-5 Stage B 实机场热修 1f0fc5d:2→8——旧值下一对设备的
/// ctrl+传输两用途即占满,同账号其余设备对全数互饿)后下一笔分配兜底 →
/// FallbackErr::RelayFailed(「中继并发会话已达上限」,send_round 归
/// Network 可重试),不得误归 RelayDenied 终局。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusal_cap_limit_retryable() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlycap@x.y").await;
        // 占满并发上限:直驱分配各持一会话(服务端无释放帧,测试窗口内
        // 必然在场;同账号上限 8)
        for i in 1..=8 {
            match p.daemon_a.relay_allocate(&p.dev_b, RelayPurpose::Transport.as_str()).await {
                Ok(AllocOutcome::Allocated { .. }) => {}
                other => panic!("第 {i} 次分配应成功占位: {other:?}"),
            }
        }
        // 第 9 次经兜底编排分配(Initiator + force_relay):拒 6 须映射
        // RelayFailed(可重试);4/5 终局路径由 refusal_disabled_terminal /
        // quota_kill_terminal 另钉,此处不重复
        let me_a = identity_of(p._home_a.path());
        let pk_b = identity_of(p._home_b.path()).public_key();
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        let mut rx_a = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let hole = [black_hole()];
        match p
            .daemon_a
            .connect_with_fallback(
                &mut rx_a,
                &me_a,
                &p.dev_b,
                &pk_b,
                &ep_a,
                &hole,
                test_punch(Duration::from_secs(3)),
                FallbackRole::Initiator,
                RelayPurpose::Transport,
                true,
            )
            .await
        {
            Err(FallbackErr::RelayFailed(m)) => {
                assert_eq!(m, "中继并发会话已达上限", "拒 6 文案钉死: {m}");
            }
            other => panic!("拒 6 须映射可重试 RelayFailed,不得终局: {other:?}"),
        }
        p.shutdown().await;
    });
    run.await.expect("cap-limit refusal e2e exceeded 120s");
}

// ---------- v2-P1c 第二刀:ctrl 两点位兜底 ----------

/// 限时 IPC 调用断言 Err,返回 RpcError 的 (code, message)(ctrl 死链/终局
/// 拒绝等失败面用;成功路径沿 [`call`]/[`call_for`])。
async fn call_err(client: &IpcClient, method: &str, params: Value, t: Duration) -> (i64, String) {
    let err = tokio::time::timeout(t, client.call(method, params))
        .await
        .unwrap_or_else(|_| panic!("call {method} 超时({t:?})"))
        .expect_err("调用必须失败(错误面断言)");
    (err.code, err.message)
}

/// 列表结果的 name 序(条目含文件名断言的统一投影;ctrl_e2e::names_of 同款)。
fn ctrl_names(v: &Value) -> Vec<String> {
    v["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().expect("entry name").to_string())
        .collect()
}

/// ctrl 发起/响应两点位兜底主用例:双侧 fault_force_relay 注错下,A 经 IPC
/// list/stat 浏览 B——打洞面跳过直连走中继(发起方分配/响应方推送配对,
/// QUIC 控制面经真中继数据面往返)。中继判别面只有服务器台账 relay_usage
/// (同机回环直连必通;实现前 ctrl 走回环直连,用量恒 0 → RED)。第二笔
/// list 同结果钉池内中继连接复用(死链驱逐语义不动,命中不重拨)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_browse_via_relay() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlyctl@x.y").await;
        // B 侧 home 造文件(list/stat 的浏览对象)
        let file = p._home_b.path().join("ctrl-中继.txt");
        std::fs::write(&file, b"via relay").unwrap();
        let home_b = p._home_b.path().to_string_lossy().into_owned();
        let params = json!({ "device": p.dev_b, "path": home_b });

        // list:条目含新造文件(打洞限 60s,ctrl_e2e 的打洞往返口径)
        let ls = call_for(&p.ca, "list", params.clone(), Duration::from_secs(60)).await;
        assert!(
            ctrl_names(&ls).iter().any(|n| n == "ctrl-中继.txt"),
            "list 条目须含新造文件: {ls}"
        );
        // 第二笔 list 同结果:池内中继连接复用
        let ls2 = call_for(&p.ca, "list", params.clone(), Duration::from_secs(60)).await;
        assert_eq!(ctrl_names(&ls), ctrl_names(&ls2), "池内复用须同结果: {ls2}");
        // stat 单条查询同通(同一池连接)
        let st = call_for(
            &p.ca,
            "stat",
            json!({ "device": p.dev_b, "path": file.to_string_lossy() }),
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(st["entry"]["name"].as_str(), Some("ctrl-中继.txt"), "stat: {st}");
        assert_eq!(st["entry"]["size"].as_u64(), Some(9), "stat size: {st}");

        // 确经中继:服务器当期台账落库 ≥ 1(5s 刷库,wait_until 收口;直连恒 0)
        let acc = account_id(&p._state, "rlyctl@x.y");
        wait_until(
            20,
            || relay_usage_bytes(&p._state, &acc) >= 1,
            "relay_usage 落库(ctrl 浏览经中继)",
        )
        .await;

        p.shutdown().await;
    });
    run.await.expect("ctrl relay browse e2e exceeded 120s");
}

/// 裁定 C + 终局拒绝链(ctrl 面):browse 建池(连接走中继)→ admin
/// disable + 直调巡检(裁定 C 杀活跃会话;60s 周期,测试直调一次等效——
/// server relay 测试先例)→ 下一笔 list 有界失败 ≤30s:死链 15s 请求上界
/// 显形 → 驱逐 → 整程重试 → force 分配实时门禁拒 4 → code 1 +「中继未
/// 开通」(经 fallback::denied_message;实现前直连连接不受 disable 影响、
/// list 仍成功 → RED)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_relay_conn_death_then_denied() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlycd@x.y").await;
        let file = p._home_b.path().join("停用后.txt");
        std::fs::write(&file, b"x").unwrap();
        let home_b = p._home_b.path().to_string_lossy().into_owned();
        let params = json!({ "device": p.dev_b, "path": home_b });

        // browse 成功:连接入池(经中继)
        let ls = call_for(&p.ca, "list", params.clone(), Duration::from_secs(60)).await;
        assert!(
            ctrl_names(&ls).iter().any(|n| n == "停用后.txt"),
            "list 条目须含新造文件: {ls}"
        );

        // admin disable + 巡检直调:活跃中继会话即杀(裁定 C)
        signaling::admin::relay_set_enabled(&p._state.db.lock().unwrap(), "rlycd@x.y", false)
            .unwrap();
        signaling::relay::sweep_refresh(&p._state).unwrap();

        // 下一笔 list 有界失败(≤30s):code 1 + 中继未开通
        let (code, message) = call_err(&p.ca, "list", params, Duration::from_secs(30)).await;
        assert_eq!(code, 1, "终局拒绝经 CtrlErr::Internal 是 code 1: {message}");
        assert!(
            message.contains("中继未开通"),
            "message 须含未开通终局文案: {message}"
        );

        p.shutdown().await;
    });
    run.await.expect("ctrl relay death e2e exceeded 120s");
}

/// 全链兜底下载(v2-P1c 第二刀组合验证,远端拉取旗舰场景):ctrl 面
/// (orchestrate/job_query)走第二刀兜底,数据面由源端 upload 驱动以传输
/// 面 Initiator 发起(第一刀兜底 + 配速)——组合证明「远端拉取全链经中
/// 继」成立。零新实现的验证用例,先跑即绿是预期,红即全链回归。断言链:
/// download 卡 done → A 落盘字节一致 → 双侧卡 relay=true(A 下载卡经常
/// 驻轮 mark_relay;B 回显 upload 卡同 id 回显,经 on_relay 回调)→ 服
/// 务器当期台账 ≥ 文件字节且 ≤ 1.25×。
///
/// 载荷 1 MiB 与容差 1.25× 均为台账裁定(实测驱动,勿放宽容差):中继
/// 记账按整报文含头、每转发 datagram 恰一笔,全链= ctrl + 传输两会话。
/// 128 KiB 载荷实测(会话级拆分):传输会话 154416(文件 131072 + 固定
/// 开销 ~23.3KB:握手/配对/ACK/标签)、ctrl 会话 24937(双向互 TLS 握手
/// + punch 配对 + orchestrate/job_query),合计 1.37× 超 1.25×——固定开
/// 销被小载荷放大,非行为回归(传输会话与上传旗舰同形实测 155168 一致)。
/// 1 MiB 摊薄后固定开销占比降到 ~2%,实测 1154169 = 1.10×;容差保持第
/// 一刀修复轮的 1.25×(裁定 H 本义:文件字节主导,比率不因握手固定开销
/// 放宽)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_via_relay_full() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlydl@x.y").await;
        // B 侧 home 子目录造源文件(Pair 存活期内可读)
        let src = p._home_b.path().join("dl-src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("远拉 全链.bin");
        let size = 1024 * 1024;
        let data = fill_random_file(&file, size);
        let target = tempfile::tempdir().unwrap();

        // A 发起远端拉取(download 形态 verbatim:device/paths/dir/skip/
        // overwrite/cut;paths = B 侧绝对路径)
        let r = call(
            &p.ca,
            "transfer.start",
            json!({
                "kind": "download",
                "device": p.dev_b,
                "paths": [file.to_string_lossy()],
                "dir": target.path().to_string_lossy(),
                "skip": [],
                "overwrite": [],
                "cut": false,
            }),
        )
        .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        let fin =
            call_for(&p.ca, "transfer.wait", json!({ "id": id }), Duration::from_secs(60)).await;
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        // A 落盘齐(定向目录,rel = 源文件名)
        let landed = target.path().join("远拉 全链.bin");
        assert_eq!(std::fs::read(&landed).unwrap(), data, "落盘字节须一致");

        // A 下载卡的 relay 标记(响应侧常驻轮 mark_relay)
        assert_eq!(p.daemon_a.job_relay(&id), Some(true), "下载卡须带 relay 标记");

        // B 侧回显 upload 卡(同 id 回显):轮收口在 A 的 wait 之后片刻,
        // transfer.list 轮询至 done
        {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let lst = call(&p.cb, "transfer.list", json!({})).await;
                let done_hit = lst["jobs"]
                    .as_array()
                    .unwrap_or_else(|| panic!("transfer.list 缺 jobs 数组: {lst}"))
                    .iter()
                    .any(|j| {
                        j["id"].as_str() == Some(id.as_str())
                            && j["kind"].as_str() == Some("upload")
                            && j["state"].as_str() == Some("done")
                    });
                if done_hit {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "B 侧回显 upload 卡未在 15s 内到 done: {lst}"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        assert_eq!(
            p.daemon_b.job_relay(&id),
            Some(true),
            "回显 upload 卡须带 relay 标记"
        );

        // 服务器当期记账(5s 刷库周期,wait_until 收口):≥ 文件字节(每个
        // 数据字节都过中继)且 ≤ 1.25×(台账裁定:1 MiB 摊薄双会话固定
        // 开销后实测 1.10×,容差不放宽,见用例头注释的双会话算术)
        let acc = account_id(&p._state, "rlydl@x.y");
        wait_until(
            20,
            || relay_usage_bytes(&p._state, &acc) >= size as u64,
            "relay_usage 落库",
        )
        .await;
        let used = relay_usage_bytes(&p._state, &acc);
        assert!(used >= size as u64, "记账须 ≥ 文件字节: {used}");
        assert!(
            used <= size as u64 * 5 / 4,
            "记账须 ≤ 1.25× 文件字节: {used}/{size}"
        );

        p.shutdown().await;
    });
    run.await.expect("full relay download e2e exceeded 120s");
}

/// 暖复用绝不触及中继(V3-5 Stage B T5):双侧 force_relay(真中继全链场)
/// 下两笔 upload 均成功——发送端①暖资格判定含中继闸(`!fault_force_relay`),
/// 注错轮 ConnectRequest 恒不带 warm_reuse,传输按操作逐次建立中继会话;
/// 两侧 `transfer_dial_count` 每作业递增(发送端打洞尾段/接收端握手入口,
/// 暖臂零计的反证面)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_transfers_never_warm() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new_forced("rlynw@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();

        let mut prev_a = p.daemon_a.transfer_dial_count();
        let mut prev_b = p.daemon_b.transfer_dial_count();
        for i in 0..2u32 {
            let file = src.path().join(format!("中继 暖{}.bin", i + 1));
            let data = fill_random_file(&file, 128 * 1024);
            let r = start_upload(&p.ca, &[file], &p.dev_b, &target.path().to_string_lossy()).await;
            let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
            let fin =
                call_for(&p.ca, "transfer.wait", json!({ "id": id }), Duration::from_secs(60)).await;
            assert_eq!(fin["state"].as_str(), Some("done"), "第 {} 笔终态: {fin}", i + 1);
            let landed = target.path().join(format!("中继 暖{}.bin", i + 1));
            assert_eq!(
                std::fs::read(&landed).unwrap(),
                data,
                "第 {} 笔落盘字节一致",
                i + 1
            );
            // 每作业两侧计数递增(中继会话按操作建立;暖臂在此恒不进——若
            // 暖臂误触达,传输经暖直连旁路中继,计数不动,本断言即反证)
            let (a, b) = (
                p.daemon_a.transfer_dial_count(),
                p.daemon_b.transfer_dial_count(),
            );
            assert!(
                a > prev_a,
                "A 侧每作业至少一次打洞尾段(中继按操作建立): {a} !> {prev_a}"
            );
            assert!(
                b > prev_b,
                "B 侧每作业至少一次接收端握手: {b} !> {prev_b}"
            );
            prev_a = a;
            prev_b = b;
        }
        p.shutdown().await;
    });
    run.await.expect("relay never warm e2e exceeded 120s");
}

// ---------- Task 8:并行预备——打洞与中继分配赛跑(spec §4.2) ----------

/// 注册表中本账号的会话数(预备在场/弃单释放的观测面;Pair 级进程内
/// 服务器,account 随用例独立)。
fn registry_count(state: &Arc<AppState>, email: &str) -> usize {
    let acc = account_id(state, email);
    state
        .relay
        .get()
        .expect("relay plane assembled")
        .registry
        .lock()
        .unwrap()
        .count_for_account(&acc)
}

/// 穷尽即切(spec §4.2 主收益面):发起方黑洞直连(预算 8s)+ 预备计时
/// 默认 3s——计时先到并行分配并推送,响应方(黑洞 1s 快败后进 5s 等推窗)
/// 即刻收推、先行配对毕;发起方 8s 穷尽 → 预备结果即切(零等待零再分配)。
/// 决定性证据 = 响应方等推窗(5s,~6s 到点):预备分配的推送 ~3s 落窗内;
/// 若无预备(穷尽后才串行分配)推送 ~8s 落窗外,响应方必超窗败——两侧
/// 成功即证「预备在场、穷尽即切、无串行分配+互拨段」;两端同 sid + 回显
/// 互通 + 会话在场未弃单(已交付即用)一并钉住。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hedged_exhaust_switches_instantly_within_push_window() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlyhedge@x.y").await;
        let me_a = identity_of(p._home_a.path());
        let me_b = identity_of(p._home_b.path());
        let (pk_a, pk_b) = (me_a.public_key(), me_b.public_key());
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        let ep_b = direct_endpoint(&me_b, &p.dev_a, &pk_a);
        let mut rx_a = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let mut rx_b = p.daemon_b.relay_events().expect("B 在线必须有中继事件通道");
        let hole = [black_hole()];
        let (oa, ob) = tokio::join!(
            // 发起方:黑洞 8s(预备计时 3s 先到 → 并行分配+推送;8s 穷尽即切)
            p.daemon_a.connect_with_fallback(
                &mut rx_a, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                test_punch(Duration::from_secs(8)), FallbackRole::Initiator,
                RelayPurpose::Transport, false,
            ),
            // 响应方:黑洞 1s 快败后进等推(窗 5s,~6s 到点)——预备分配
            // ~3s 的推送必落窗内;无预备的串行推送 ~8s 必落窗外
            p.daemon_b.connect_with_fallback(
                &mut rx_b, &me_b, &p.dev_a, &pk_a, &ep_b, &hole,
                test_punch(Duration::from_secs(1)), FallbackRole::Responder,
                RelayPurpose::Transport, false,
            ),
        );
        let oa = oa.expect("发起方穷尽即切必成(预备在场)");
        let ob = ob.expect("响应方等推窗内必收预备分配的推送(无预备则 ~8s 推送落窗外必超窗败)");
        let sa = oa.relay.expect("发起方即切走中继").sid;
        let sb = ob.relay.expect("响应方须走中继").sid;
        assert_eq!(sa, sb, "即切用的是预备会话(两端同 sid)");
        echo_round(&oa.conn, &ob.conn, &[0x33u8; 16 * 1024]).await;
        assert_eq!(
            registry_count(&p._state, "rlyhedge@x.y"),
            1,
            "预备会话在场未弃单(已交付即用)"
        );
        p.shutdown().await;
    });
    run.await.expect("hedged instant-switch e2e exceeded 120s");
}

/// 打洞胜出弃单释放 + 配额回收复用(spec §4.2 弃单纪律):A 直连打洞的
/// 候选恒为黑洞(拨号臂永不成),其 punch 的 **accept 臂**(accept_pinned,
/// 生产代码)等拨入——B 为纯客户端,迟激活、迟拨入(注册表见预备分配在
/// 场后才起 B 的拨入,接受令牌舞步照旧):胜出必然晚于 3s 预备计时,时序
/// 由观测驱动非真睡。胜出 → 直连返场(预备会话弃单,从未承载数据);预
/// 备互拨无人配对,预算内快败 → 预备任务收口显式释放 → 注册表清零;释放
/// 后 8 槽全量可再分配(未释放则第 8 次必拒 6)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hedged_punch_win_releases_for_reuse() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlyrelease@x.y").await;
        let me_a = identity_of(p._home_a.path());
        let me_b = identity_of(p._home_b.path());
        let (pk_a, pk_b) = (me_a.public_key(), me_b.public_key());
        let ep_a = direct_endpoint(&me_a, &p.dev_b, &pk_b);
        // direct_endpoint 绑 0.0.0.0:0——拨入地址取回环 + 实占端口
        // (quinn 拒拨 0.0.0.0 远端)
        let addr_a = SocketAddr::from(([127, 0, 0, 1], ep_a.local_addr().unwrap().port()));
        let mut rx_a = p.daemon_a.relay_events().expect("A 在线必须有中继事件通道");
        let hole = [black_hole()];
        // 预备臂与 A 轮并发(join!):预备分配在场(3s 计时先到的铁证)
        // → 此刻才起 B 的拨入,A 的 accept 臂方有可能结算,胜出必然晚于
        // 预备分配
        let staging = async {
            wait_until(
                15,
                || registry_count(&p._state, "rlyrelease@x.y") >= 1,
                "预备分配在场(预备计时先到的铁证)",
            )
            .await;
            let ep_b = ep::make_endpoint(SocketAddr::from(([127, 0, 0, 1], 0)))
                .expect("client endpoint");
            let dial = tokio::spawn(async move {
                // B 拨入即目的(接受的连接弃置);A 的 accept 臂写接受令牌,
                // 本端 connect_pinned 的确认读随即结算
                let _ = ep::connect_pinned_tuned(
                    &ep_b,
                    addr_a,
                    &pk_a,
                    &me_b,
                    ep::TransportTuning::default(),
                )
                .await;
            });
            drop(dial);
        };
        let (oa, ()) = tokio::join!(
            p.daemon_a.connect_with_fallback(
                &mut rx_a, &me_a, &p.dev_b, &pk_b, &ep_a, &hole,
                test_punch(Duration::from_secs(10)), FallbackRole::Initiator,
                RelayPurpose::Transport, false,
            ),
            staging,
        );
        let oa = oa.expect("直连胜出返场");
        assert!(
            oa.relay.is_none(),
            "直连胜出返场(预备会话弃单,从未承载数据)"
        );
        assert_eq!(
            registry_count(&p._state, "rlyrelease@x.y"),
            1,
            "预备会话滞留在场(互拨无人配对,预算内败后收口)"
        );
        // 弃单显式释放:预备互拨对无人配对的中继端点,预算(10s)内快败
        // → 预备任务收口显式释放(Task 7 RelayRelease 面)
        wait_until(
            30,
            || registry_count(&p._state, "rlyrelease@x.y") == 0,
            "弃单显式释放(注册表清零)",
        )
        .await;
        // 配额回收复用:释放后 8 槽全量可再分配(未释放则第 8 次必拒 6)
        for i in 1..=8 {
            match p
                .daemon_a
                .relay_allocate(&p.dev_b, RelayPurpose::Transport.as_str())
                .await
            {
                Ok(AllocOutcome::Allocated { .. }) => {}
                other => panic!("释放后第 {i} 槽再分配应成功(配额回收复用): {other:?}"),
            }
        }
        p.shutdown().await;
    });
    run.await.expect("hedged release e2e exceeded 120s");
}

// ---------- V3-6 Task 9:探洞豁免 + 回切轮换(spec §4.4) ----------

/// 布尔条件限时轮询,到达即 true / 超时 false(软同步点:回切编舞的分支
/// 分派观察面;硬断言仍走 [`wait_until`] 的 panic 口径)。
async fn wait_flag(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 回切编舞全链(spec §4.4 治瘾机制):发送端(小 pk 侧 = 预热主动侧 =
/// 回切轮换宿主)注入 armed → 杀暖连(首发暖闸 `warm_xfer_conn` 活性门
/// 直读死条目必落打洞尾段)→ 起传输即中继轮(armed 消费跳直连,发送轮
/// 落地即登记)→ warmer 重打直连成 → 回切:drain 中继半连逐条 close +
/// 清 armed → 发送端重试轮落暖直连(池内刚回切的直连,warm_reuse 位;对
/// 端 respond_session 被动入池)→ done + 落盘一致 + 服务器无在剧中继
/// 会话(轮 1 会话随回切 RelayRelease 释放,轮 2 暖直连零分配)。
///
/// armed 建立面(真穷尽)需对端不可达,回环 rig 造不出——由 fallback 单
/// 测钉死,此处 `punch_note_exhaust` 注入判据态(两端注入:发送端过回切
/// 一发闸;收端免首发 10s punch 预算烧窗,直落等推配对)。
///
/// 双分支确定性:首发登记与 warmer 首次重打(杀暖连后 ~1s)的先后二选一
/// 皆收敛——登记先到则首次重打即回切(早分支);反之首次重打扑空(闸短
/// 路,一发标记不耗),补杀暖连驱动第二次重打(晚分支:bytes>0 已证轮在
/// 途即登记在场,且配速窗 ~8s 覆盖 1s 退避窗)。round 2 恒走暖臂(池内回
/// 切直连),收端 armed 残留不碍暖轮(暖轮不经兜底记忆)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_switchback_closes_relay_and_retry_lands_warm() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("rlyrot@x.y").await;
        let pk_a = identity_of(p._home_a.path()).public_key();
        let pk_b = identity_of(p._home_b.path()).public_key();
        // 发送端 = 小 pk 侧(warm_targets 单侧主动:预热主动侧即回切宿主)
        let (snd_dev, rcv_dev, snd_daemon, rcv_daemon, snd_client) =
            if pk_a.as_bytes() < pk_b.as_bytes() {
                (
                    p.dev_a.clone(),
                    p.dev_b.clone(),
                    &p.daemon_a,
                    &p.daemon_b,
                    &p.ca,
                )
            } else {
                (
                    p.dev_b.clone(),
                    p.dev_a.clone(),
                    &p.daemon_b,
                    &p.daemon_a,
                    &p.cb,
                )
            };
        // 注入 armed(判据态;真穷尽面 fallback 单测已钉)
        snd_daemon.punch_note_exhaust(&rcv_dev);
        rcv_daemon.punch_note_exhaust(&snd_dev);
        assert!(snd_daemon.punch_armed(&rcv_dev), "发送端 armed 在册");

        // 等登录预热落地(warmer 事件去抖 750ms + 回环打洞,晚于 Pair
        // 装配返回;只小 pk 侧预热,恰发送端)
        wait_until(
            30,
            || snd_daemon.ctrl_warm_conn(&rcv_dev).is_some(),
            "登录预热直连条目在场",
        )
        .await;

        // 占住传输 try-lock(测试直驱面):首发暖闸的占用臂 → warm_reuse=
        // false,首发必落打洞尾段(armed → 直进中继)——比「杀暖连赌闸读
        // 竞速」确定(warmer 的重打/重算回填可能在闸读前回填暖条目,首发
        // 误落暖臂)。登记确认后即还锁,轮 2 的暖闸照常过。登录暖连不杀,
        // warmer 全程无重打扰动,回切驱动由下文唯一一次杀暖连承担。
        let xfer_lock = snd_daemon
            .ctrl_try_xfer_lock(&rcv_dev)
            .expect("传输 try-lock 空闲");
        let warm_login = snd_daemon
            .ctrl_warm_conn(&rcv_dev)
            .expect("登录预热直连条目在场");

        // 起传输(4 MiB:中继 1024kbps 配速在途窗 ~32s——发起方中继互拨
        // 沿 T3 丢首轮拨号口径满烧一次 punch 预算(~10s)后数据才起,
        // 窗口必须覆盖「bytes>0 → 杀暖连 → 1s 退避 → 回切」全链)
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("回切 全链.bin");
        let data = fill_random_file(&file, 4 * 1024 * 1024);
        let r = start_upload(snd_client, &[file], &rcv_dev, &target.path().to_string_lossy()).await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();

        // 发送轮登记在场(轮落地即注册;上界覆盖信令+分配+发起方中继互拨
        // 的 ~10s punch 预算全程)
        wait_until(
            30,
            || snd_daemon.punch_relay_round_active(&rcv_dev),
            "发送轮中继登记在场",
        )
        .await;
        let relay_conn = snd_daemon
            .punch_registered_relay_conn(&rcv_dev)
            .expect("登记连接克隆在场");
        drop(xfer_lock); // 还锁:轮 2 暖闸照常过

        // 回切同步点(双分支):armed 清 = 回切已发
        if !wait_flag(3, || !snd_daemon.punch_armed(&rcv_dev)).await {
            // 晚分支(warmer 未扰时恒此支):首发登记已确认在场 → 接收轮
            // 登记亦在场 → 杀暖连驱动 warmer 重打(1s 退避 → ensure_warm
            // 回环直连成 → 回切;轮在途配速窗 ~30s,裕量充分)
            wait_until(
                30,
                || rcv_daemon.punch_relay_round_active(&snd_dev),
                "接收轮中继登记在场",
            )
            .await;
            {
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let lst = call(snd_client, "transfer.list", json!({})).await;
                    let done = lst["jobs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|j| j["id"].as_str() == Some(id.as_str()))
                        .and_then(|j| j["bytes_done"].as_u64())
                        .unwrap_or(0);
                    if done > 0 {
                        break;
                    }
                    assert!(Instant::now() < deadline, "传输未在 30s 内起步: {lst}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            warm_login.close(0u32.into(), b"e2e: warm kill (rotation driver)");
            assert!(
                wait_flag(20, || !snd_daemon.punch_armed(&rcv_dev)).await,
                "回切必发(armed 清)"
            );
        }

        // 回切铁证:登记的中继半连被关(应用层码 close;armed 清与 drain
        // close 同调用序,克隆句柄 closed() 即刻就绪)
        let closed = tokio::time::timeout(Duration::from_secs(10), relay_conn.closed()).await;
        assert!(closed.is_ok(), "回切必关中继半连: {closed:?}");
        assert!(!snd_daemon.punch_armed(&rcv_dev), "回切清 armed(直连返场)");
        assert!(
            !snd_daemon.punch_relay_round_active(&rcv_dev),
            "drain 即清登记"
        );
        assert_eq!(
            snd_daemon.ctrl_warm_state(&rcv_dev),
            Some(false),
            "暖池直连条目在场(回切重打的直连)"
        );

        // 重试轮落暖直连:终态 done + 落盘一致;服务器无在剧中继会话
        //(轮 1 会话已随回切显式释放——RelayRelease 双端 Dead 通告、服务
        // 端注册表清账;恒 0 = 轮 2 暖直连零分配)
        let fin =
            call_for(snd_client, "transfer.wait", json!({ "id": id }), Duration::from_secs(90))
                .await;
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        let landed = target.path().join("回切 全链.bin");
        assert_eq!(std::fs::read(&landed).unwrap(), data, "落盘字节须一致");
        assert_eq!(
            registry_count(&p._state, "rlyrot@x.y"),
            0,
            "轮 1 会话随回切释放,重试轮无新中继会话(直连返场)"
        );
        p.shutdown().await;
    });
    run.await.expect("rotation switchback e2e exceeded 150s");
}
