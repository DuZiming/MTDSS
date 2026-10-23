//! 中继枢纽 e2e(v2-P1c Task 4):真信令服务器 + 真中继数据面 + 进程内双
//! daemon,钉住枢纽面对 HelloOk 资格的缓存与 RelaySession 推送的路由;
//! 分配/订阅经 Daemon 的 doc(hidden) 直驱面直驱 hub,不进 IPC 契约。
//! T5(兜底编排)的传输面用例在本文件追加。
//!
//! 全部用例限定 worker_threads = 2(jobs_e2e 口径:全 workspace 并跑下
//! 避免过度订阅饿死打洞/信令预算;用例均为 IO 型,2 worker 足够)。

use mtdss_core::signaling::client::AllocOutcome;
use mtdss_daemon::hub::RelayEvt;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::IpcClient;
use serde_json::{json, Value};
use signaling::AppState;
use std::net::SocketAddr;
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

/// 在随机端口起中继数据面,返回测试可达的回环地址(bind 0.0.0.0,测试走
/// 127.0.0.1;server/tests/relay.rs 的 spawn_relay_on 先例照搬)。注意
/// wire 上下发的 relay_addr 是 spawn 传入的字面 pub_addr("127.0.0.1:0",
/// 解析为端口 0 回环地址)——本文件路由测试只钉解析成功;T5 数据面用例
/// 若需真连中继,须以真实端口作 pub_addr 起。
async fn spawn_relay_on(state: &Arc<AppState>) -> SocketAddr {
    signaling::relay::spawn_relay(state.clone(), 0, "127.0.0.1:0".into()).unwrap();
    let relay = state.relay.get().unwrap();
    let port = relay.socket.local_addr().unwrap().port();
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
    /// 两侧 IPC 客户端(保持连接在场;当前用例只直驱 hub 面,句柄留作
    /// T5 传输面用例与排障,前缀下划线消 dead_code)。
    _ca: IpcClient,
    _cb: IpcClient,
    dev_a: String,
    dev_b: String,
}

impl Pair {
    async fn new(email: &str) -> Self {
        let (addr, state) = signaling::spawn_test_server().await;
        // 认证收口后仅预置账号可领码;user_add 预建 relay_config 行
        //(enabled=0 + 默认限速 1024 + 默认配额 10 GiB),再显式开通
        signaling::admin::user_add(&state.db.lock().unwrap(), email).unwrap();
        signaling::admin::relay_set_enabled(&state.db.lock().unwrap(), email, true).unwrap();
        // 分配门禁第一步要求数据面已装配(未起即按未开通拒 4)
        let _relay_addr = spawn_relay_on(&state).await;
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
        assert_ne!(dev_a, dev_b, "双侧必须是不同设备");
        wait_peer_online(&ca, &dev_b, true, 30).await;
        Self {
            _home_a: home_a,
            _home_b: home_b,
            _state: state,
            daemon_a,
            daemon_b,
            _ca: ca,
            _cb: cb,
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
            .relay_allocate(&p.dev_b)
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
            RelayEvt::Push { peer, sid: push_sid, tag, relay_addr, expires_at: _ } => {
                assert_eq!(peer, p.dev_a, "分配帧 peer_device 须为请求方设备号");
                assert_eq!(push_sid, sid, "双帧须同会话号");
                assert_eq!(tag.len(), 16, "对端槽 tag 须 16 字节");
                assert!(
                    relay_addr.ip().is_loopback(),
                    "relay_addr 须为回环 SocketAddr: {relay_addr}"
                );
            }
            RelayEvt::Dead { sid: dead_sid, reason } => {
                panic!("分配通告不得路由成死帧: {dead_sid} {reason}")
            }
        }

        p.shutdown().await;
    });
    run.await.expect("allocation routing e2e exceeded 120s");
}
