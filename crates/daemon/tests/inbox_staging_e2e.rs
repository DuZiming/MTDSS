//! 收件私有暂存 + 完成发布端到端(P4a Task 1,替代 v2-P3 的
//! inbox_bridge_e2e):host 双 daemon + 伪发布 App(真实 IpcClient 订阅
//! `fs.publish` + FdPassClient 回 `0x05 publish_result`)→ A `transfer.start
//! {kind:upload, files, to, dir:"inbox"}` → B(收件侧)经 InboxDestIo
//! (FsDestIo(`{home}/inbox-staging`)薄包装)落**私有暂存**,轮 Ok(网络面
//! 收妥)后发布协调器驱动 publishing→(发布确认)→done / failed
//! 「保存失败」。fdpass 依赖 Unix 域套接字,整文件 cfg(unix)(CI 在 WSL
//! 跑;Windows 编译单元整体缺席,桌面回归面不受影响)。
//!
//! 拓扑注:同 v2-P3——实机收件箱是 android 端 download(dir="inbox");
//! host 以发送端 upload 直指哨兵驱动同一接收面(ConnectRequest
//! target_dir="inbox" → B 的 resident loop 哨兵路由 → InboxDestIo)。
//! download 侧的哨兵放行是 android cfg 分支(桌面拒绝面由契约 golden
//! `transfer.start.download-inbox` 钉死)——**download 驱动的「发布已
//! 落定」合成门与 10s 宽限作用域(spec §6.1)因此不在 host 可跑面**,
//! 由 jobs.rs 单测(download_wait_terminal 的注入缝:紧宽限常量 + 发布
//! 相位直驱)钉死,本文件不重复。
//!
//! 断言面(brief 五支,④ 见上注):
//! - **① 全链**:收→finalize→publishing(卡态可观察)→发布 ok→暂存件
//!   删除、记录留 done+published、任务 done;
//! - **② 发布超时**(注入 1500ms,不真等 30s):伪 App 不应答→任务
//!   failed「保存失败」、暂存保留(记录 done 无 published);
//! - **③ staged-未发布误 Skip 守卫**(§6.2,评审焦点 1):② 之后重发→
//!   final_state 记录门(done+published)未过→不 Skip→走网络(接收字节
//!   >0)→再次 fs.publish→发布→done;
//! - **⑤ 多文件并发发布、单文件失败**=「保存失败(1 项)」其余文件照常
//!   发布保留(镜像 P2 部分缺失语义,§6.1);
//! - **⑥ 0x04 路由存续 + Skip 锚**:已发布记录(done+published)重发 →
//!   记录门过 → 真 fdpass `0x04` 在场查询答「在场」→ Skip 维持(无新
//!   fs.publish、无网络字节)——0x02 退役后查询通道完好、记录= Skip 锚
//!   (§6.2 双层)的回归钉。

#![cfg(unix)]

use mtdss_core::transport::intake::{record_key, INTAKE_DIR};
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::fdpass::{FdPassClient, KIND_ACK, KIND_DEST_QUERY, KIND_PUBLISH_RESULT};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot};

/// 常规调用超时(transfer.* 除 wait 外都是快操作)。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// transfer.wait 长轮询上界。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// 发布超时注入(生产 30s,不真等):`MTDSS_TEST_PUBLISH_TIMEOUT_MS`(沿
/// MTDSS_TEST_* 家族口径,daemon 构造期一次读定)。全部用例共享测试进程,
/// 首例起跑前经 OnceLock 一次性设置。
const INJECTED_PUBLISH_TIMEOUT_MS: u64 = 1500;
/// 发送端重试退避注入(`MTDSS_TEST_BACKOFF_MS`,flow 既有旋钮):重试轮
/// 收敛提速。0x04 查询超时同注(③ 的对账路径若误发查询,失败方向快速
/// 显形)。
const INJECTED_BACKOFF_MS: u64 = 300;
static ENV_INJECT: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn inject_env() {
    ENV_INJECT.get_or_init(|| {
        std::env::set_var(
            "MTDSS_TEST_PUBLISH_TIMEOUT_MS",
            INJECTED_PUBLISH_TIMEOUT_MS.to_string(),
        );
        std::env::set_var(
            "MTDSS_TEST_QUERY_DEST_TIMEOUT_MS",
            INJECTED_PUBLISH_TIMEOUT_MS.to_string(),
        );
        std::env::set_var(
            "MTDSS_TEST_BACKOFF_MS",
            INJECTED_BACKOFF_MS.to_string(),
        );
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

/// 进程内双 daemon 测试台:A = 发送端(源文件 + upload 作业),B = 收件
/// 侧(私有暂存 `{home}/inbox-staging`)。双方已登录且 A 已见 B 在线。
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

// ---------- 伪发布 App(镜像 Kotlin 发布器的职责面,T3 前的 host 桩) ----------

/// 伪 App 对单条 fs.publish 的处置模式(前 N 条可被 script 覆盖,见
/// MockPublisher::spawn)。
#[derive(Clone)]
enum PublishMode {
    /// 应答 ok(顺带断言暂存件在场且 size 吻合——发布请求携带的素材必须
    /// 真实在位)。
    Answer,
    /// 收下请求但不应答(超时路径)。
    Ignore,
    /// 应答 ok:false(显式失败,镜像 MediaStore 拷贝失败/磁盘满)。
    Fail(&'static str),
    /// 握住不应答:等测试观察到 publishing 卡态后经 SetMode(Answer) 放行
    /// (① 的「保存中」可观察面)。
    Hold,
}

/// 伪 App 的共享日志(断言数据源)。
#[derive(Default)]
struct AppLog {
    /// 到达的全部 fs.publish (request_id, rel, staging_path, size)。
    seen: Mutex<Vec<(u64, String, PathBuf, u64)>>,
    /// 已应答的 (request_id, ok)——Answer/Fail 都算已应答。
    answered: Mutex<Vec<(u64, bool)>>,
    /// 发布成功集合 rel → size(0x04 在场应答的事实面)。
    published: Mutex<std::collections::HashMap<String, u64>>,
    /// 到达的 fs.query_dest (request_id, rel)。
    queries: Mutex<Vec<(u64, String)>>,
}

/// 伪 App 的运行时指令(测试体 → 应答循环)。
enum AppCmd {
    /// 切换默认处置模式并放行全部 Held(按新模式补答),回执同步。
    SetMode { mode: PublishMode, reply: oneshot::Sender<()> },
}

/// 伪发布 App:订阅 fs.publish → 按模式经查询通道连接回 `0x05
/// publish_result`(§6.5:0x04/0x05 共用查询通道 + 统一 id 计数器);应答
/// 被 daemon ack ok:true 受理。事件流关闭即退。
struct MockPublisher {
    ipc: IpcClient,
    fdpass: FdPassClient,
    log: Arc<AppLog>,
    /// 默认处置模式(SetMode 可换)。
    mode: Arc<Mutex<PublishMode>>,
    /// 前 N 条发布的脚本化处置(按已应答数索引;⑤ 单文件失败用)。
    script: Vec<PublishMode>,
    /// Hold 握住的请求(SetMode 时按新模式补答)。
    held: Arc<Mutex<Vec<(u64, String)>>>,
    /// 最近一条 fs.publish 的 rel/size(answer 记 published 集合用;事件
    /// 循环串行,无并发竞面)。
    last_rel: String,
    pending_size: u64,
}

impl MockPublisher {
    async fn spawn(
        home: &Path,
        script: Vec<PublishMode>,
        default: PublishMode,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<AppLog>,
        mpsc::Sender<AppCmd>,
    ) {
        let ipc = IpcClient::connect(home).await.expect("伪 App 连 IPC");
        let fdpass = connect_fdpass_retry(home).await;
        let log = Arc::new(AppLog::default());
        let app = Self {
            ipc,
            fdpass,
            log: log.clone(),
            mode: Arc::new(Mutex::new(default)),
            script,
            held: Arc::new(Mutex::new(Vec::new())),
            last_rel: String::new(),
            pending_size: 0,
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
                    Some(AppCmd::SetMode { mode, reply }) => {
                        *self.mode.lock().unwrap() = mode;
                        // 放行 Held:按(新)默认模式补答——测试只用 Hold→Answer,
                        // 其余组合按同口径收口(Ignore = 继续不应答)。先取
                        // 走 Held 清单再逐条答(dispatch 需 &mut self,不与
                        // MutexGuard 借用共存)。
                        let held: Vec<(u64, String)> =
                            std::mem::take(&mut *self.held.lock().unwrap());
                        for (id, rel) in held {
                            self.dispatch(id, &rel).await;
                        }
                        let _ = reply.send(());
                    }
                    None => return,
                },
            }
        }
    }

    async fn handle_event(&mut self, ev: mtdss_ipc::EventFrame) {
        if ev.method == "fs.query_dest" {
            return self.handle_query(&ev).await;
        }
        if ev.method != "fs.publish" {
            return;
        }
        let id = ev.params["request_id"].as_u64().expect("request_id u64");
        let rel = ev.params["rel"].as_str().expect("rel 字符串").to_string();
        let path = PathBuf::from(ev.params["staging_path"].as_str().expect("staging_path 字符串"));
        let size = ev.params["size"].as_u64().expect("size u64");
        self.log.seen.lock().unwrap().push((id, rel.clone(), path.clone(), size));
        self.last_rel = rel.clone();
        self.pending_size = size;
        // 发布素材真实性:暂存件必须在场且 size 吻合(§3.2 协议前提)。
        match std::fs::metadata(&path) {
            Ok(m) => assert_eq!(m.len(), size, "fs.publish 的 size 必须与暂存件一致: {path:?}"),
            Err(e) => panic!("fs.publish 的暂存件必须在场: {path:?}: {e}"),
        }
        self.dispatch(id, &rel).await;
    }

    /// 单条发布的处置:script(按已应答数索引)优先,否则默认模式。
    async fn dispatch(&mut self, id: u64, rel: &str) {
        let step = self
            .script
            .get(self.log.answered.lock().unwrap().len())
            .cloned()
            .unwrap_or_else(|| self.mode.lock().unwrap().clone());
        match step {
            PublishMode::Answer => self.answer(id, true, None).await,
            PublishMode::Fail(reason) => self.answer(id, false, Some(reason)).await,
            PublishMode::Ignore => {} // 不应答:仅 seen 记录
            PublishMode::Hold => self.held.lock().unwrap().push((id, rel.to_string())),
        }
    }

    /// 经查询通道连接回 0x05(字母序 payload: error?/ok/request_id),应答
    /// 必须被受理(ack ok:true——等待方在册)。
    async fn answer(&mut self, id: u64, ok: bool, error: Option<&str>) {
        let ok_rel = if ok { Some((self.last_rel.clone(), self.pending_size)) } else { None };
        let mut payload = json!({ "ok": ok, "request_id": id });
        if let Some(e) = error {
            payload["error"] = Value::from(e);
        }
        let ack = self
            .fdpass
            .send_msg(KIND_PUBLISH_RESULT, payload, None)
            .await
            .expect("0x05 发送应成功");
        assert_eq!(ack.kind, KIND_ACK, "0x05 应答应为 ack: {ack:?}");
        assert_eq!(
            ack.payload["ok"].as_bool(),
            Some(true),
            "0x05 应答必须被受理(等待方在册): {ack:?}"
        );
        self.log.answered.lock().unwrap().push((id, ok));
        if let Some((rel, size)) = ok_rel {
            self.log.published.lock().unwrap().insert(rel, size);
        }
    }

    /// fs.query_dest(0x04 在场查询)处置:按已发布集合应答(在场且 size
    /// 吻合 → exists;否则不在场——镜像 MediaStore 行查询语义);经同一
    /// 查询通道连接回 `0x04 dest_query`。
    async fn handle_query(&mut self, ev: &mtdss_ipc::EventFrame) {
        let id = ev.params["request_id"].as_u64().expect("request_id u64");
        let rel = ev.params["rel"].as_str().expect("rel 字符串").to_string();
        let size = ev.params["size"].as_u64().expect("size u64");
        self.log.queries.lock().unwrap().push((id, rel.clone()));
        let published = self.log.published.lock().unwrap().get(&rel).copied();
        let exists = published == Some(size);
        let actual = published.unwrap_or(0);
        let ack = self
            .fdpass
            .send_msg(
                KIND_DEST_QUERY,
                json!({ "exists": exists, "request_id": id, "size": actual }),
                None,
            )
            .await
            .expect("0x04 发送应成功");
        assert_eq!(ack.kind, KIND_ACK, "0x04 应答应为 ack: {ack:?}");
        assert_eq!(
            ack.payload["ok"].as_bool(),
            Some(true),
            "0x04 应答必须被受理(等待方在册): {ack:?}"
        );
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

/// B 侧收件箱暂存区(`{home}/inbox-staging`)。
fn staging_dir(home_b: &Path) -> PathBuf {
    home_b.join("inbox-staging")
}

/// B 侧暂存记录(`{home}/inbox-staging/.mtdss/<sha256(rel)>.json`)→ 解析
/// JSON;缺席 → None。
fn staging_record(home_b: &Path, rel: &str) -> Option<Value> {
    let p = staging_dir(home_b)
        .join(INTAKE_DIR)
        .join(format!("{}.json", record_key(rel)));
    let raw = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 记录 final_rel 对应的暂存终件路径(平台分隔符形态)。
fn staged_final(home_b: &Path, final_rel: &str) -> PathBuf {
    staging_dir(home_b).join(final_rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// 轮询直至谓词命中(记录面观察;deadline 到即 panic)。
async fn wait_record<F: Fn(&Value) -> bool>(home_b: &Path, rel: &str, what: &str, pred: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(v) = staging_record(home_b, rel) {
            if pred(&v) {
                return v;
            }
        }
        assert!(
            Instant::now() < deadline,
            "记录面未在预算内满足 {what}: {:?}",
            staging_record(home_b, rel)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// B 侧匿名 incoming 卡快照(transfer.list 里 kind=incoming 的行;多卡
/// 时按列表尾向找**最新**命中卡——同文件多轮重发会留多张终态卡)。
async fn find_incoming<F: Fn(&Value) -> bool>(cb: &IpcClient, pred: F) -> Option<Value> {
    let lst = call(cb, "transfer.list", json!({})).await.unwrap();
    lst["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|j| j["kind"].as_str() == Some("incoming") && pred(j))
        .cloned()
}

/// 轮询 B 侧匿名 incoming 卡直到谓词命中(deadline 90s)。
async fn wait_incoming<F: Fn(&Value) -> bool>(cb: &IpcClient, what: &str, pred: F) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(j) = find_incoming(cb, &pred).await {
            return j;
        }
        assert!(
            Instant::now() < deadline,
            "B 侧 incoming 卡未在预算内满足 {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 切换伪 App 处置模式(同步回执:切换 + Held 放行完成)。
async fn set_mode(cmd: &mpsc::Sender<AppCmd>, mode: PublishMode) {
    let (tx, rx) = oneshot::channel();
    cmd.send(AppCmd::SetMode { mode, reply: tx })
        .await
        .unwrap();
    rx.await.unwrap();
}

// ---------- 用例 ----------

/// ① 全链:两件(含嵌套 rel)收→finalize→publishing(Hold 握住,卡态可
/// 观察)→放行应答 ok→暂存件删除、记录留 done+published、任务 done;
/// fs.publish 恰每文件一次(rel=final_rel);默认收件目录与旧桥布局不被
/// 触碰(防串台)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_staging_full_chain_publishes_and_cleans() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("inbox1@x.y").await;
        let src = tempfile::tempdir().unwrap();

        // 件面(顶层文件 → rel = 文件名;顶层目录 → rel = 目录名/内部
        // 相对路径——collect_items 口径,嵌套 rel 由目录输入产生)。
        let f1 = src.path().join("暂存 收件 甲.bin");
        let data1 = fill_random_file(&f1, 16 * 1024);
        let nested = src.path().join("嵌套");
        std::fs::create_dir_all(&nested).unwrap();
        let f2 = nested.join("乙.bin");
        let data2 = fill_random_file(&f2, 48 * 1024);
        let files: Vec<PathBuf> = vec![f1.clone(), nested.clone()];
        let contents: Vec<(String, Vec<u8>)> = vec![
            ("暂存 收件 甲.bin".to_string(), data1),
            ("嵌套/乙.bin".to_string(), data2),
        ];

        // Hold:发布请求到达先握住,让 publishing 卡态可观察(§3.3 保存中)。
        let (app, log, cmd) =
            MockPublisher::spawn(p.home_b.path(), vec![], PublishMode::Hold).await;

        let id = start_inbox_upload(&p.ca, &files, &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "A 侧网络面终态: {fin}");
        assert_eq!(fin["files_done"].as_u64(), Some(2), "完成计数: {fin}");

        // 发布窗:卡态必须先经过 publishing(发布未确认不假完成,§2 发布
        // 时序诚实)。
        let card = wait_incoming(&p.cb, "publishing 态", |j| j["state"].as_str() == Some("publishing"))
            .await;
        assert_eq!(card["state"].as_str(), Some("publishing"), "保存中态: {card}");
        // 放行:Held 补答 ok → 发布确认。
        set_mode(&cmd, PublishMode::Answer).await;

        // 终局:done + 发布面核对。
        let card = wait_incoming(&p.cb, "done 终态", |j| j["state"].as_str() == Some("done")).await;
        assert_eq!(card["files_done"].as_u64(), Some(2), "完成计数: {card}");
        assert!(card["error"].is_null(), "成功无终因: {card}");

        // fs.publish:恰每文件一次,rel = final_rel(无冲突改名时 = rel),
        // 全部应答 ok。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "每文件恰一次 fs.publish: {seen:?}");
            let rels: Vec<&str> = seen.iter().map(|(_, r, _, _)| r.as_str()).collect();
            for (rel, _) in &contents {
                assert!(rels.contains(&rel.as_str()), "发布名=final_rel: {rels:?}");
            }
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 2, "两件全应答: {answered:?}");
            assert!(answered.iter().all(|(_, ok)| *ok), "应答全 ok: {answered:?}");
        }

        // 记录面:done + published 保留(Skip 锚,§6.2);暂存件删除(完成
        // 即清理,§2)。
        for (rel, data) in &contents {
            let rec = wait_record(p.home_b.path(), rel, "done+published 记录", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
            })
            .await;
            assert_eq!(rec["rel"].as_str(), Some(rel.as_str()), "记录 rel: {rec}");
            assert_eq!(rec["final_rel"].as_str(), Some(rel.as_str()), "final_rel: {rec}");
            assert_eq!(rec["size"].as_u64(), Some(data.len() as u64), "size: {rec}");
            let fin_rel = rec["final_rel"].as_str().unwrap();
            assert!(
                !staged_final(p.home_b.path(), fin_rel).exists(),
                "发布成功后暂存终件必须删除: {fin_rel}"
            );
        }

        // 防串台:默认收件目录不被收件箱轮触碰;旧桥布局(inbox-records)
        // 不存在(已退役)。
        assert!(
            !p.home_b.path().join("inbox").join(".mtdss").exists(),
            "收件箱轮不得在默认收件目录建 .mtdss"
        );
        assert!(
            !p.home_b.path().join("inbox-records").exists(),
            "旧桥记录布局必须缺席(0x02 退役)"
        );

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ② 发布超时(注入 1500ms):伪 App 不应答→任务 failed「保存失败(N 项)」、
/// 暂存保留(终件在场 + 记录 done 无 published——重发的 republish 素材,
/// §4);fs.publish 恰一次未被应答。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_publish_timeout_fails_and_retains_staging() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("inbox2@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel = "超时 件.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 16 * 1024);

        let (app, log, _cmd) =
            MockPublisher::spawn(p.home_b.path(), vec![], PublishMode::Ignore).await;

        let id = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("A 侧网络面应正常收妥");
        assert_eq!(fin["state"].as_str(), Some("done"), "网络面终态: {fin}");

        // 发布超时 → failed「保存失败」(§3.3)。
        let card = wait_incoming(&p.cb, "failed 保存失败", |j| {
            j["state"].as_str() == Some("failed")
        })
        .await;
        let err = card["error"].as_str().unwrap_or_default();
        assert!(err.contains("保存失败"), "终因词面: {err}");
        assert!(err.contains("超时"), "超时细节: {err}");

        // fs.publish 恰一次,未被应答。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 1, "恰一次 fs.publish: {seen:?}");
            assert_eq!(seen[0].1, rel, "发布名: {seen:?}");
            assert!(
                log.answered.lock().unwrap().is_empty(),
                "伪 App 全程未应答"
            );
        }

        // 暂存保留(§4 失败保留待重试):记录 done 无 published + 终件在场。
        let rec = wait_record(p.home_b.path(), rel, "done 未发布记录", |v| {
            v["done"].as_bool() == Some(true)
        })
        .await;
        assert_ne!(rec["published"].as_bool(), Some(true), "不得带 published: {rec}");
        let fin_rel = rec["final_rel"].as_str().unwrap();
        let staged = staged_final(p.home_b.path(), fin_rel);
        let got = std::fs::read(&staged).unwrap_or_else(|e| panic!("读暂存终件 {staged:?}: {e}"));
        assert_eq!(&got, &data, "暂存终件逐字节一致(发布失败保留)");

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ③ staged-未发布误 Skip 守卫(§6.2/评审焦点 1):② 形态(记录 done 未
/// published、暂存终件在场)后重发→final_state 记录门未过→**不 Skip**→
/// 走网络(接收字节>0)→再次 fs.publish(发布名携带 unique_target 冲突
/// 改名)→发布→done。修复前病灶:final_state 若 stat 暂存路径,staged 件
/// 在场即假 Skip——任务秒完、文件永不进收件箱。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_staged_unpublished_resend_walks_network_not_skip() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("inbox3@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel = "守卫 件.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 16 * 1024);

        // 首传:发布不应答 → failed 保存失败 + 暂存保留(② 形态)。
        let (app, log, cmd) =
            MockPublisher::spawn(p.home_b.path(), vec![], PublishMode::Ignore).await;
        let id1 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let fin1 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id1 }), WAIT_TIMEOUT)
            .await
            .expect("首传网络面应正常收妥");
        assert_eq!(fin1["state"].as_str(), Some("done"), "首传网络面: {fin1}");
        wait_incoming(&p.cb, "首传 failed 保存失败", |j| {
            j["state"].as_str() == Some("failed")
                && j["error"].as_str().unwrap_or_default().contains("保存失败")
        })
        .await;
        let rec1 = wait_record(p.home_b.path(), rel, "done 未发布记录", |v| {
            v["done"].as_bool() == Some(true) && v["published"].as_bool() != Some(true)
        })
        .await;
        let stale_final = rec1["final_rel"].as_str().unwrap().to_string();
        assert!(
            staged_final(p.home_b.path(), &stale_final).exists(),
            "守卫前提:staged 终件在场({stale_final})"
        );

        // 重发(同源同 rel;Task 1 的网络重发路径——republish 分区是 T2):
        // 伪 App 切应答。
        set_mode(&cmd, PublishMode::Answer).await;
        let id2 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let fin2 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("重发应到终态");
        assert_eq!(fin2["state"].as_str(), Some("done"), "重发网络面: {fin2}");
        let card2 = wait_incoming(&p.cb, "重发 done", |j| {
            j["state"].as_str() == Some("done")
        })
        .await;

        // 不 Skip 的证据 ①:接收字节 > 0(Skip 卡 FileDone 无 Bytes 回调,
        // bytes_done 恒 0;真收 = 整件字节)。
        assert_eq!(
            card2["bytes_done"].as_u64(),
            Some(data.len() as u64),
            "重发必须真收整件(不假跳过): {card2}"
        );

        // 不 Skip 的证据 ②:第二次 fs.publish(误 Skip 无新发布请求)。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "重发必须再次 fs.publish: {seen:?}");
            assert_ne!(
                seen[1].1, seen[0].1,
                "重收的发布名携带 unique_target 冲突改名(旧暂存件保留的后果): {seen:?}"
            );
            let answered = log.answered.lock().unwrap().clone();
            // 首传按 Ignore 不应答(超时路径),重发切 Answer:恰第二次已应答。
            assert_eq!(
                answered.len(),
                1,
                "恰第二次发布已应答(首传超时未应答): {answered:?}"
            );
            assert_eq!(answered[0].0, seen[1].0, "应答对应第二次请求: {answered:?} vs {seen:?}");
            assert!(answered[0].1, "第二次发布 ok: {answered:?}");
        }

        // 终局记录:done+published(final_rel = 第二次发布名);旧暂存件
        // 原样保留(T2 的分区重发消费它)。
        let rec2 = wait_record(p.home_b.path(), rel, "重发后 done+published", |v| {
            v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
        })
        .await;
        let fin_rel2 = rec2["final_rel"].as_str().unwrap();
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(fin_rel2, seen[1].1, "记录 final_rel = 第二次发布名: {rec2}");
        }
        assert!(
            !staged_final(p.home_b.path(), fin_rel2).exists(),
            "新暂存件发布后删除"
        );
        assert!(
            staged_final(p.home_b.path(), &stale_final).exists(),
            "旧暂存件保留(republish 素材,T2 消费)"
        );

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ⑤ 多文件并发发布、单文件失败:两件并发 fs.publish,首条应答 ok:false
/// →任务 failed「保存失败(1 项)」;失败件暂存保留(记录 done 无
/// published),成功件照常发布保留(暂存删除 + 记录 published)——镜像
/// P2 部分缺失语义(§6.1)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_multi_publish_single_failure_fails_one_retains_rest() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("inbox4@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel_a = "并发 甲.bin";
        let rel_b = "并发 乙.bin";
        let fa = src.path().join(rel_a);
        fill_random_file(&fa, 16 * 1024);
        let fb = src.path().join(rel_b);
        fill_random_file(&fb, 24 * 1024);
        let files: Vec<PathBuf> = vec![fa, fb];

        // script:首条发布应答失败(哪个文件先到由协调器并发序决定,断言
        // 动态识别);其后默认应答 ok。
        let (app, log, _cmd) = MockPublisher::spawn(
            p.home_b.path(),
            vec![PublishMode::Fail("模拟发布失败")],
            PublishMode::Answer,
        )
        .await;

        let id = start_inbox_upload(&p.ca, &files, &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("A 侧网络面应正常收妥");
        assert_eq!(fin["state"].as_str(), Some("done"), "网络面终态: {fin}");

        // 终局:failed「保存失败(1 项)」。
        let card = wait_incoming(&p.cb, "failed 保存失败(1 项)", |j| {
            j["state"].as_str() == Some("failed")
        })
        .await;
        let err = card["error"].as_str().unwrap_or_default();
        assert!(err.contains("保存失败"), "终因词面: {err}");
        assert!(err.contains("1 项"), "失败计数: {err}");
        assert!(err.contains("模拟发布失败"), "失败细节透传: {err}");

        // 发布面:两件各恰一次,一 ok 一失败。
        let (fail_rel, ok_rel) = {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 2, "两件并发发布: {seen:?}");
            let answered = log.answered.lock().unwrap().clone();
            assert_eq!(answered.len(), 2, "两件全应答: {answered:?}");
            assert_eq!(
                answered.iter().filter(|(_, ok)| !ok).count(),
                1,
                "恰一件失败: {answered:?}"
            );
            let by_id: std::collections::HashMap<u64, &str> =
                seen.iter().map(|(i, r, _, _)| (*i, r.as_str())).collect();
            let fail_id = answered.iter().find(|(_, ok)| !ok).unwrap().0;
            let ok_id = answered.iter().find(|(_, ok)| *ok).unwrap().0;
            (
                by_id[&fail_id].to_string(),
                by_id[&ok_id].to_string(),
            )
        };

        // 失败件:暂存保留 + 记录 done 无 published。
        {
            let rec = wait_record(p.home_b.path(), &fail_rel, "失败件 done 未发布", |v| {
                v["done"].as_bool() == Some(true)
            })
            .await;
            assert_ne!(rec["published"].as_bool(), Some(true), "失败件无 published: {rec}");
            assert!(
                staged_final(p.home_b.path(), rec["final_rel"].as_str().unwrap()).exists(),
                "失败件暂存保留: {rec}"
            );
        }
        // 成功件:暂存删除 + 记录 done+published(其余文件照常发布保留)。
        {
            let rec = wait_record(p.home_b.path(), &ok_rel, "成功件 done+published", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
            })
            .await;
            assert!(
                !staged_final(p.home_b.path(), rec["final_rel"].as_str().unwrap()).exists(),
                "成功件暂存删除: {rec}"
            );
        }
        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// ⑥ 0x04 路由存续 + Skip 锚(§6.2 双层的正面钉):已发布记录(done+
/// published)重发 → 记录门过 → **真 fdpass `0x04`** 在场查询(App 按已
/// 发布集合答「在场且 size 吻合」)→ Skip 维持:无新 fs.publish、无网络
/// 字节(bytes_done=0)、暂存无新件;任务照常 done。0x02 退役后查询通道
/// 完好、发布记录即后续 Skip 锚。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_published_record_re_send_skips_via_0x04() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("inbox5@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel = "在场保持 件.bin";
        let path = src.path().join(rel);
        let data = fill_random_file(&path, 16 * 1024);

        let (app, log, _cmd) =
            MockPublisher::spawn(p.home_b.path(), vec![], PublishMode::Answer).await;

        // 首传:发布 ok,记录 done+published。
        let id1 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let fin1 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id1 }), WAIT_TIMEOUT)
            .await
            .expect("首传网络面应正常收妥");
        assert_eq!(fin1["state"].as_str(), Some("done"), "首传网络面: {fin1}");
        wait_incoming(&p.cb, "首传 done", |j| j["state"].as_str() == Some("done")).await;
        wait_record(p.home_b.path(), rel, "done+published 记录", |v| {
            v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
        })
        .await;

        // 重发(不删任何东西):对账记录门过 → 0x04 问在场 → App 答在场
        // → Skip 维持。
        let id2 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        let fin2 = call_bounded(&p.ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("重发应到终态");
        assert_eq!(fin2["state"].as_str(), Some("done"), "重发终态: {fin2}");
        // 发送面:Skip 件经 FileDone 帧照常计完成(A 侧 files_done=1)。
        assert_eq!(fin2["files_done"].as_u64(), Some(1), "Skip 计完成: {fin2}");
        // 接收面:Skip 不落盘不发进度(引擎口径)——B 卡零字节零件数。
        let card2 = wait_incoming(&p.cb, "重发 done", |j| j["state"].as_str() == Some("done"))
            .await;
        assert_eq!(card2["bytes_done"].as_u64(), Some(0), "零网络字节: {card2}");
        assert_eq!(card2["files_done"].as_u64(), Some(0), "零落盘件: {card2}");

        // 发布面:无新 fs.publish;查询面:恰一次 0x04 且已应答在场。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 1, "Skip 维持:不得再发 fs.publish: {seen:?}");
            let queries = log.queries.lock().unwrap().clone();
            assert_eq!(queries.len(), 1, "恰一次在场查询: {queries:?}");
            assert_eq!(queries[0].1, rel, "查询 rel(=final_rel): {queries:?}");
        }
        // 暂存无新件;记录原样保留(发布后 App 侧才是正身,暂存件已按
        // §6.2 删除)。
        let rec = staging_record(p.home_b.path(), rel).expect("记录在场");
        let fin_rel = rec["final_rel"].as_str().unwrap();
        assert!(
            !staged_final(p.home_b.path(), fin_rel).exists(),
            "暂存无新件: {fin_rel}"
        );
        let _ = data;

        app.abort();
        p.shutdown().await;
    });
    run.await.expect("用例超时");
}
