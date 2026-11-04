//! V3-1 边角收口 e2e(T14;跨端主链编排在 login_v3p1.rs,本文件只补三边角):
//! - login.cancel 作废本地续接槽的**后效**:cancel 后同会话号的 status 与
//!   submit_code 均即刻 RPC 拒(-32602 参数面,限时 call 证不悬挂),不落
//!   会话(R6:cancel=本地作废+服务端 TTL 自灭;T8 test 9 只钉了 status
//!   一路,此处补 submit_code 路并加 cancel 前正常投影对照);
//! - 服务端限流透传面:echo 门关的生产形态服务器上,login.request_code 超
//!   per-email 1/min 限速(429+Retry-After)对 daemon 客户端呈现为**业务
//!   错 RpcError(code 1)+ 中文文案**(「请求验证码失败」打头、含服务端
//!   「稍后再试」),非裸 HTTP 调试串、非悬挂;
//! - 扫码过期重生成:qr_begin 铸的 token 过期后(TTL 120s 为常量不可注入,
//!   走服务端测试钩子 `_expire_all_qr_tokens_for_test` 置过期,server 套件
//!   qr.rs 同款先例),PC 轮询回 expired 终态,重发 qr_begin 得**全新
//!   token** 且轮询恢复 pending——PC 流自愈,槽被新会话覆盖不残留。

use mtdss_core::identity::DeviceIdentity;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 常规调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂
/// (「cancel 后即刻拒不悬挂」的断言载体)。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// 代理豁免(一次性):e2e 全程打回环,而 reqwest 按 http_proxy 环境变量
/// 把请求改道本地代理进程——代理不可达/规则不直连回环时 502 误伤测试。
/// reqwest 在 client 构建时读 NO_PROXY;daemon 的 REST 客户端按请求现建,
/// 本测试进程启动即免疫(login_v3p1 同款先例)。
fn bypass_proxy_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
    });
}

/// 临时 home + 显式 recv_dir 的启动选项(同 login_v3p1,不碰真实 mtdss_home)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        // 桌面形态:无收件箱发布面(v2-P5 T5b,哨兵回退默认收件目录)
        inbox_publish: false,
        photo_serve: false,
    }
}

/// 限时 IPC 调用;超时视为测试失败(暴露悬挂),RpcError 原样上抛供断言。
async fn call(client: &IpcClient, method: &str, params: Value) -> Result<Value, RpcError> {
    match tokio::time::timeout(CALL_TIMEOUT, client.call(method, params)).await {
        Ok(r) => r,
        Err(_) => panic!("call {method} 超时({CALL_TIMEOUT:?})"),
    }
}

/// dev-mode 验证码回显的完整登录(预置老终端直入;login_v3p1 同款)。
async fn ipc_login(client: &IpcClient, server: &str, email: &str, name: &str) -> Value {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("result 必含 code 字符串");
    let r = call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
    .expect("login.verify 应成功(预置老终端直入)");
    assert_eq!(r["status"], "ok", "预置老终端 verify 必须直入: {r}");
    r
}

/// REST 直注注册(预置受信终端行;测试台在任何上线发生前先落行)。
async fn preregister_identity(
    addr: std::net::SocketAddr,
    home: &Path,
    email: &str,
    name: &str,
) -> String {
    bypass_proxy_once();
    let id = DeviceIdentity::generate();
    id.save_seed(&home.join("identity.seed")).unwrap();
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
    v["device_id"].as_str().unwrap().to_string()
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

/// 受信终端(A)上线:预置行 + daemon + 完整登录 + 在线
/// (审批面用例的 A 端基座;login_v3p1 同款)。
async fn trusted_online(
    addr: std::net::SocketAddr,
    email: &str,
    name: &str,
) -> (tempfile::TempDir, Daemon, IpcClient, PathBuf) {
    bypass_proxy_once();
    let home = tempfile::tempdir().unwrap();
    preregister_identity(addr, home.path(), email, name).await;
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();
    let server = format!("http://{addr}");
    ipc_login(&client, &server, email, name).await;
    wait_online(&client, true, 30).await;
    let sess_path = home.path().join("session.json");
    (home, daemon, client, sess_path)
}

/// 新终端 verify 撞审批面:返回 session_id(A 端在线时;login_v3p1 同款)。
async fn verify_expect_approval(client: &IpcClient, server: &str, email: &str, name: &str) -> String {
    let code = call(client, "login.request_code", json!({ "server": server, "email": email }))
        .await
        .expect("login.request_code 应成功");
    let code = code["code"].as_str().expect("code 回显").to_string();
    let r = call(
        client,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
    .expect("verify 请求应成功(审批面不是 RPC 错)");
    assert_eq!(r["status"], "approval", "新终端(受信端在线)必须走审批: {r}");
    r["session_id"].as_str().expect("session_id 在场").to_string()
}

/// echo 门关的生产形态测试服务器(echo_code=false + DevEcho 发信恒成功):
/// request-code 走真实投递分支并启限速(spec §3.10;Ruling R5——限速护的
/// 是发信管道,dev 回显门开时不计费不限速,故 spawn_test_server 恒 echo 开
/// 测不到 429)。本文件限流透传用例专用(邮件测试先例 mail_throttle::st
/// 同构,此处起活端口供 daemon REST 打)。
async fn spawn_echo_off_server() -> (std::net::SocketAddr, Arc<signaling::AppState>) {
    bypass_proxy_once();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db");
    let db = signaling::db::open(db_path.to_str().unwrap()).unwrap();
    let state = Arc::new(signaling::AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: Arc::new(signaling::PresenceRegistry::new()),
        keepalive: signaling::ws::Keepalive::default(),
        stun_addrs: Vec::new(),
        relay: std::sync::OnceLock::new(),
        mailer: Box::new(signaling::mail::DevEcho),
        echo_code: false,
        throttle: signaling::throttle::Throttle::new(),
    });
    let app = signaling::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // TempDir 必须活到服务器任务结束(Windows 上提前 drop 会把仍在用的 DB
    // 文件标记删除挂起;spawn_test_server_with 同注)。
    tokio::spawn(async move {
        let _keep = dir;
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

// ---------- login.cancel 作废本地续接槽:status/submit_code 双路即刻拒 ----------

/// cancel 后效(R6/T8 test 9 补口):cancel 前轮询是正常投影(approval),
/// cancel 后同会话号 status 与 submit_code 双路均 -32602 参数面即刻拒
/// (限时 call 上界内返回 = 不悬挂),不落会话。submit_code 路此前无
/// e2e 覆盖——CLI `q` 放弃/UI 取消后残留的输码口是真实可达面。
#[tokio::test(flavor = "multi_thread")]
async fn cancel_voids_pending_then_status_and_submit_code_rejected() {
    let (addr, _state) = spawn_test_server().await;
    signaling::admin::user_add(&_state.db.lock().unwrap(), "cancel2@x.y").unwrap();
    let server = format!("http://{addr}");
    let (home_a, daemon_a, ca, sess_a) = trusted_online(addr, "cancel2@x.y", "终端A").await;

    let home_b = tempfile::tempdir().unwrap();
    let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
    let cb = IpcClient::connect(home_b.path()).await.unwrap();
    let session_id = verify_expect_approval(&cb, &server, "cancel2@x.y", "终端B").await;

    // cancel 前对照:同会话号轮询是正常投影(approval)——证明 cancel 后的
    // -32602 是槽被作废所致,非该会话号固有的参数面
    let st = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect("cancel 前轮询是正常投影");
    assert_eq!(st["status"], "approval", "未裁决前轮询仍 approval: {st}");

    let r = call(&cb, "login.cancel", json!({})).await.unwrap();
    assert_eq!(r, json!({}), "cancel 返回空对象");

    // 双路即刻拒(-32602 参数面):限时 call 上界内返回,不悬挂
    let err = call(&cb, "login.status", json!({ "session_id": session_id }))
        .await
        .expect_err("cancel 后轮询必须被拒");
    assert_eq!(err.code, -32602, "续接槽已清,参数面拒绝: {err:?}");
    let err = call(
        &cb,
        "login.submit_code",
        json!({ "session_id": session_id, "code": "123456" }),
    )
    .await
    .expect_err("cancel 后输码必须被拒");
    assert_eq!(err.code, -32602, "输码同槽同拒: {err:?}");

    // cancel 不落会话(B 端);A 端在线不受影响(本地作废,非服务端裁决)
    assert!(!home_b.path().join("session.json").exists(), "cancel 不落会话");
    let s = call(&ca, "status", json!({})).await.unwrap();
    assert_eq!(s["online"], true, "本地 cancel 不得波及受信端在线: {s}");

    drop(cb);
    daemon_b.shutdown().await.unwrap();
    drop(ca);
    daemon_a.shutdown().await.unwrap();
    drop(sess_a);
    drop(home_a);
    drop(home_b);
}

// ---------- 服务端限流透传面(429+Retry-After → 中文业务错) ----------

/// request-code 限速的生产形态透传:echo 关服务器上 per-email 1/min 第 2 次
/// 撞 429(服务端 api.rs ReqCodeErr::Throttled,带 Retry-After),daemon
/// login.request_code 呈现为**业务错(code 1)+ 中文文案**——「请求验证码
/// 失败」打头、含服务端「稍后再试」,不是裸 HTTP 调试串、不是悬挂。钉死
/// 透传形状:UI/CLI 的可读报错依赖此形状(R3 之外的限流面首条 daemon 侧 e2e)。
#[tokio::test(flavor = "multi_thread")]
async fn request_code_throttle_surfaces_as_friendly_rpc_error() {
    let (addr, state) = spawn_echo_off_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "thr@x.y").unwrap();
    let server = format!("http://{addr}");

    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let c = IpcClient::connect(home.path()).await.unwrap();

    // 第 1 次:真实投递分支(DevEcho 发信恒成功+落库),响应无 code 回显
    // (spec §3.6「有则填」的 None 臂)
    let r = call(&c, "login.request_code", json!({ "server": server, "email": "thr@x.y" }))
        .await
        .expect("首发必须成功");
    assert!(r.get("code").is_none(), "echo 关响应无回显码: {r}");

    // 第 2 次(1/min 内):服务端 429 → daemon 业务错,中文文案透传
    let err = call(&c, "login.request_code", json!({ "server": server, "email": "thr@x.y" }))
        .await
        .expect_err("1/min 内二发必须撞 429 业务错");
    assert_eq!(err.code, 1, "限速是业务错(code 1): {err:?}");
    assert!(
        err.message.contains("请求验证码失败"),
        "daemon 中文打头文案(非裸 HTTP blob): {}", err.message
    );
    assert!(
        err.message.contains("稍后再试"),
        "服务端「稍后再试」文案透传在场: {}", err.message
    );

    drop(c);
    daemon.shutdown().await.unwrap();
    drop(home);
}

// ---------- 扫码过期重生成(PC 流自愈) ----------

/// qr token 过期(120s 常量不可注入 → 服务端测试钩子置过期,qr.rs 先例)
/// 后:PC 轮询回 expired 终态(正常投影非 RPC 错)→ 重发 qr_begin 得全新
/// token(≠ 旧 token;daemon 单槽被新会话覆盖,不残留)→ 新 token 轮询
/// pending。PC 扫码流对过期自愈,无需重启 daemon。
#[tokio::test(flavor = "multi_thread")]
async fn qr_expiry_then_rebegin_yields_new_token() {
    let (addr, state) = spawn_test_server().await;
    signaling::admin::user_add(&state.db.lock().unwrap(), "qrre@x.y").unwrap();
    let server = format!("http://{addr}");

    let home_pc = tempfile::tempdir().unwrap();
    let daemon_pc = Daemon::start(home_pc.path(), opts(home_pc.path())).await.unwrap();
    let cpc = IpcClient::connect(home_pc.path()).await.unwrap();

    let r1 = call(&cpc, "login.qr_begin", json!({ "server": server, "name": "PC端" }))
        .await
        .expect("首轮 qr_begin 应成功");
    let token1 = r1["token"].as_str().expect("token1 在场").to_string();
    let st = call(&cpc, "login.qr_status", json!({ "token": token1 })).await.unwrap();
    assert_eq!(st["status"], "pending", "首发 token 未认领恒 pending: {st}");

    // 注入过期(全部 qr_tokens expires_at=0;轮询惰性落库 expired 终态)
    signaling::auth::_expire_all_qr_tokens_for_test(&state);
    let st = call(&cpc, "login.qr_status", json!({ "token": token1 })).await.unwrap();
    assert_eq!(st["status"], "expired", "过期轮询回终态投影(非 RPC 错): {st}");

    // 重发 qr_begin:全新 token(≠ 旧 token;槽覆盖不残留)
    let r2 = call(&cpc, "login.qr_begin", json!({ "server": server, "name": "PC端" }))
        .await
        .expect("过期后重发 qr_begin 应成功(PC 流自愈)");
    let token2 = r2["token"].as_str().expect("token2 在场").to_string();
    assert_ne!(token2, token1, "重发必须铸新 token: {r2}");
    assert_eq!(
        r2["qr_url"].as_str().unwrap(),
        format!("https://www.mtdss.top/qr?t={token2}"),
        "二维码内容 = 固定前缀 + 新 token: {r2}"
    );
    let st = call(&cpc, "login.qr_status", json!({ "token": token2 })).await.unwrap();
    assert_eq!(st["status"], "pending", "新 token 轮询恢复 pending: {st}");

    drop(cpc);
    daemon_pc.shutdown().await.unwrap();
    drop(home_pc);
}
