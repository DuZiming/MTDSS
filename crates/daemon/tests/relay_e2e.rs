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
/// 120s 空闲回收自愈,下一轮重试即可通过,终局失真。钉:占满账号 2 会话
/// 并发上限后第三次分配兜底 → FallbackErr::RelayFailed(「中继并发会话
/// 已达上限」,send_round 归 Network 可重试),不得误归 RelayDenied 终局。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refusal_cap_limit_retryable() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("rlycap@x.y").await;
        // 占满并发上限:两次直驱分配各持一会话(服务端无释放帧,测试窗口内
        // 必然在场;同账号上限 2)
        for i in 1..=2 {
            match p.daemon_a.relay_allocate(&p.dev_b, RelayPurpose::Transport.as_str()).await {
                Ok(AllocOutcome::Allocated { .. }) => {}
                other => panic!("第 {i} 次分配应成功占位: {other:?}"),
            }
        }
        // 第三次经兜底编排分配(Initiator + force_relay):拒 6 须映射
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
