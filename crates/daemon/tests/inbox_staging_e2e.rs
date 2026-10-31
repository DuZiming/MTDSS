//! 收件私有暂存 + 完成发布端到端(P4a Task 1/2,替代 v2-P3 的
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
//! download 侧的哨兵放行是 android cfg 分支(桌面首发拒绝面由契约 golden
//! `transfer.start.download-inbox` 钉死);**T2 起 reinitiate 的收件箱
//! download 臂沿记录原值放行**(重放持久记录信任原判——首发已验),下载
//! 驱动的发布门/10s 宽限合成(spec §6.1)由此进 host 可跑面(T1 拓扑注的
//! 空窗补上;纯合成注入面仍由 jobs.rs 单测钉死)。收件 download 记录的
//! host 构形:杀 B 后直写 tasks.json 再重启(首发不可为,重放可以)。
//!
//! 断言面(T1 brief 五支 + T2 三支,④ 见上注):
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
//!   (§6.2 双层)的回归钉;
//! - **T2-① 跨进程续传**(headline,首次可 e2e):传中杀 B daemon → 同
//!   home 重启(收件 download 记录直写构形)→ reinitiate → **位图续传跳
//!   块**(字节账断言:B 已收 − A 实发 == 分区前已收字节,恰等于磁盘位图
//!   的已收块和)→ publishing → done(spec §1 真断点续传);
//! - **T2-② republish-only**:暂存 done + 任一任务态(failed「保存失败」
//!   /cancelled)→ reinitiate → **零网络**(A 无回显卡、B 卡零字节、发布
//!   名零改名)→ 发布 → done(§6.3 免登录/免对端);
//! - **T2-③ 混合分区**:2 件 staged done + 1 件缺失 → reinitiate → 2 件
//!   republish + 1 件网络;**staged 件字节零变化**(hold 窗断言 mtime+内
//!   容)且 republish 相先于网络相落定(hold 窗 A 无回显卡)——分区保证
//!   staged 件永不进接收集,Full 截断与 "(1)" 改名孤儿无从产生(评审验
//!   收 #1/#2)。

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
    /// Hold 握住的请求 (request_id, rel, size)(SetMode 时按新模式补答)。
    held: Arc<Mutex<Vec<(u64, String, u64)>>>,
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
                        let held: Vec<(u64, String, u64)> =
                            std::mem::take(&mut *self.held.lock().unwrap());
                        for (id, rel, size) in held {
                            self.dispatch(id, &rel, size).await;
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
        // 发布素材真实性:暂存件必须在场且 size 吻合(§3.2 协议前提)。
        match std::fs::metadata(&path) {
            Ok(m) => assert_eq!(m.len(), size, "fs.publish 的 size 必须与暂存件一致: {path:?}"),
            Err(e) => panic!("fs.publish 的暂存件必须在场: {path:?}: {e}"),
        }
        self.dispatch(id, &rel, size).await;
    }

    /// 单条发布的处置:script(按已应答数索引)优先,否则默认模式。rel/size
    /// 逐条显式携带(answer 记 published 集合用——不落共享"最近一条"状态:
    /// Hold 放行批量补答时各条各归各,避免后到事件盖前条的发布事实)。
    async fn dispatch(&mut self, id: u64, rel: &str, size: u64) {
        let step = self
            .script
            .get(self.log.answered.lock().unwrap().len())
            .cloned()
            .unwrap_or_else(|| self.mode.lock().unwrap().clone());
        match step {
            PublishMode::Answer => self.answer(id, true, None, rel, size).await,
            PublishMode::Fail(reason) => self.answer(id, false, Some(reason), rel, size).await,
            PublishMode::Ignore => {} // 不应答:仅 seen 记录
            PublishMode::Hold => self.held.lock().unwrap().push((id, rel.to_string(), size)),
        }
    }

    /// 经查询通道连接回 0x05(字母序 payload: error?/ok/request_id),应答
    /// 必须被受理(ack ok:true——等待方在册)。
    async fn answer(&mut self, id: u64, ok: bool, error: Option<&str>, rel: &str, size: u64) {
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
        if ok {
            self.log.published.lock().unwrap().insert(rel.to_string(), size);
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

// ---------- T2:重发分区与 republish(spec §6.3) ----------

/// 暂存记录位图已收块数(hex 位图 → 置位数;跳块断言的账面原料)。
fn bitmap_popcount(rec: &Value) -> u64 {
    rec["bitmap"]
        .as_str()
        .map(|hex| {
            hex.chars()
                .map(|c| u8::from_str_radix(&c.to_string(), 16).unwrap().count_ones() as u64)
                .sum()
        })
        .unwrap_or(0)
}

/// 按 id 在 transfer.list(live+store 合并面)找一张卡/行。
async fn find_card_by_id(client: &IpcClient, id: &str) -> Option<Value> {
    let lst = call(client, "transfer.list", json!({})).await.ok()?;
    lst["jobs"]
        .as_array()?
        .iter()
        .find(|j| j["id"].as_str() == Some(id))
        .cloned()
}

/// 轮询 transfer.list 直到 id 在场(deadline 到即 panic)。
async fn wait_card_id(client: &IpcClient, id: &str, what: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(j) = find_card_by_id(client, id).await {
            return j;
        }
        assert!(Instant::now() < deadline, "卡片未在预算内出现({what}): {id}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 轮询 id 卡直到谓词命中(deadline 90s)。
async fn wait_card<F: Fn(&Value) -> bool>(
    client: &IpcClient,
    id: &str,
    what: &str,
    pred: F,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(j) = find_card_by_id(client, id).await {
            if pred(&j) {
                return j;
            }
        }
        assert!(Instant::now() < deadline, "卡片 {id} 未在预算内满足 {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 文件指纹(内容 + mtime;staged 件字节零变化断言的原料)。
fn fingerprint(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("读 {path:?}: {e}"));
    let mtime = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {path:?}: {e}"))
        .modified()
        .unwrap();
    (bytes, mtime)
}

/// staging 区(不含 .mtdss 记账目录)的全部文件——孤儿/残留断言原料。
fn staging_finals(home_b: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&staging_dir(home_b), &mut out);
    let records = staging_dir(home_b).join(".mtdss");
    out.retain(|p| !p.starts_with(&records));
    out
}

/// 往 B 的 tasks.json 追加记录(B 停机窗独占写;文件缺席自空表起)。
fn append_tasks_json(home_b: &Path, recs: &[Value]) {
    let path = home_b.join("tasks.json");
    let mut all: Vec<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    all.extend(recs.iter().cloned());
    std::fs::write(&path, serde_json::to_string_pretty(&all).unwrap()).unwrap();
}

/// B 的收件 download 记录直写构形(T2 host 拓扑:首发不可为 inbox
/// download,重放可以——杀 B 后往 tasks.json 追加记录再同 home 重启)。
/// state 写 "running" 让 load 的重启标记翻成 interrupted,等价真实
/// 「传中被杀」。调用方负责停 B、重等在线。
fn inbox_download_rec(
    id: &str,
    from_dev: &str,
    to_dev: &str,
    rels: &[&str],
    src: &[PathBuf],
    state: &str,
) -> Value {
    json!({
        "id": id,
        "kind": "download",
        "name": rels[0],
        "from_dev": from_dev,
        "to_dev": to_dev,
        "src_paths": src.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "target_dir": "inbox",
        "policy_skip": [],
        "policy_overwrite": [],
        "cut": false,
        "rels": rels,
        "state": state,
    })
}

/// B 杀停 + 直写记录 + 同 home 重启 + 重连(② 的整装三步)。
async fn bounce_b_with_recs(
    daemon_b: Daemon,
    home_b: &Path,
    recs: Vec<Value>,
) -> (Daemon, IpcClient) {
    daemon_b.shutdown().await.expect("B 停机(暂存面在盘)");
    append_tasks_json(home_b, &recs);
    let daemon = Daemon::start(home_b, opts(home_b)).await.expect("B 同 home 重启");
    let client = IpcClient::connect(home_b).await.expect("重连 B IPC");
    (daemon, client)
}

/// reinitiate 的翻新 id 发现:RPC 响应面 `{}` 由契约 golden 钉死(不回
/// 新 id),调用前后 transfer.list 的差集即新卡(旧记录摘除、新卡在场,
/// 差集恰一)。
async fn reinitiate_and_find_new_id(client: &IpcClient, old_id: &str) -> String {
    let before: Vec<String> = call(client, "transfer.list", json!({}))
        .await
        .unwrap()["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|j| j["id"].as_str().map(|s| s.to_string()))
        .collect();
    let r = call(client, "transfer.reinitiate", json!({ "id": old_id }))
        .await
        .expect("transfer.reinitiate 应成功");
    assert_eq!(r, json!({}), "响应面由 golden 钉死");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let lst = call(client, "transfer.list", json!({})).await.unwrap();
        let fresh: Vec<&str> = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|j| j["id"].as_str())
            .filter(|id| !before.iter().any(|b| b == id))
            .collect();
        if fresh.len() == 1 {
            return fresh[0].to_string();
        }
        assert!(
            Instant::now() < deadline,
            "翻新新卡未在预算内出现(差集 {:?}): {lst}",
            fresh
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// T2-① 跨进程续传(headline,首次可 e2e):64MiB 单件传中杀 B daemon →
/// 同 home 重启(收件 download 记录直写构形,interrupted)→ reinitiate →
/// **位图续传跳块**——字节账断言:暂停 A 回显卡取静息点,B 已收字节 − A
/// 实发字节 == 分区前磁盘位图的已收块和(64MiB 全 1MiB 整块,恰为块数×
/// 1MiB;Full 重收下该差恒 0)→ 放行收尾 → publishing → done。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_reinitiate_cross_process_resume_publishes() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(300), async {
        let p = Pair::new("inbox6@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel = "跨进程 续传.bin";
        let path = src.path().join(rel);
        // 64 块 × 1MiB:首个落盘记录(FLUSH_CHUNKS=16 块批或 2s 间隔,先到
        // 者——并发负载下可能 <16 块)出现后仍有 ≥48MiB 在途,杀点/续传
        // 窗对 10ms 轮询都足够宽。
        fill_random_file(&path, 64 * 1024 * 1024);
        let ra = call(&p.ca, "status", json!({})).await.unwrap();
        let dev_a = ra["device"]["device_id"].as_str().unwrap().to_string();

        // 首传:A upload 直指收件箱(B 匿名接收落私有暂存)。
        let id1 = start_inbox_upload(&p.ca, std::slice::from_ref(&path), &p.dev_b).await;
        // 首个落盘记录出现(≥1 置位 = 已过落盘点)且未完成即到杀点;上限
        // 48 块保证重启后仍有 ≥16 块缺块(续传窗)。
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let rec = staging_record(p.home_b.path(), rel);
            if let Some(v) = &rec {
                let n = bitmap_popcount(v);
                if v["done"].as_bool() != Some(true) && (1..=48).contains(&n) {
                    break;
                }
                assert!(
                    v["done"].as_bool() != Some(true) || n > 48,
                    "杀窗未赶上(记录已 done): {v}"
                );
            }
            assert!(Instant::now() < deadline, "首传记录未在预算内出现: {rec:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // 冻结杀点:暂停 A 的首传(波/块流门),让在途块自然落盘(B 侧
        // 继续收已开流)。续传起点(磁盘位图)在杀停**之后**读取——进程
        // 已死,记录不再前进;停机前读会漏掉停机窗内的最后一次批量落盘
        // (在途块落盘后的 flush),把起点读小(实测踩中:+16 块)。
        call(&p.ca, "transfer.pause", json!({ "id": id1 })).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await; // 在途块排干窗

        // 杀 B(优雅停即真杀的等价面:轮协作中止,暂存记录/.part 原样在盘)
        // + 弃旧客户端;取消 A 首传(取消哨对死 B 的通知 best-effort 失败,
        // 无碍)——避免 B 重启后旧驱动重连污染续传断言。
        let Pair {
            _home_a,
            home_b,
            _state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_b,
        } = p;
        drop(cb);
        daemon_b.shutdown().await.expect("杀 B(传中)");
        call(&ca, "transfer.cancel", json!({ "id": id1 })).await.unwrap();
        // 续传起点:死进程的磁盘位图即重启后 reconcile 的 Resume 基数。
        let stabilized = staging_record(home_b.path(), rel).expect("杀停后记录在场");
        assert_ne!(
            stabilized["done"].as_bool(),
            Some(true),
            "杀点必须在传输中途(记录未完成): {stabilized}"
        );
        let received_before = bitmap_popcount(&stabilized) * 1024 * 1024;
        assert!(
            (1..=64).contains(&(received_before / 1024 / 1024)),
            "续传起点在窗内(≥1 块已收即有跳块账面,≥16 缺块保续传窗): {received_before} 字节"
        );

        // 收件 download 记录直写(state "running" → load 翻 interrupted)
        append_tasks_json(
            home_b.path(),
            &[inbox_download_rec(
                "devB:9001",
                &dev_a,
                &dev_b,
                &[rel],
                std::slice::from_ref(&path),
                "running",
            )],
        );
        let daemon_b2 = Daemon::start(home_b.path(), opts(home_b.path()))
            .await
            .expect("B 同 home 重启");
        let cb2 = IpcClient::connect(home_b.path()).await.expect("重连 B IPC");
        wait_online(&cb2, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;
        // 重启后记录以 interrupted 在场(重启标记的行面)
        let row = wait_card_id(&cb2, "devB:9001", "interrupted 记录行").await;
        assert_eq!(row["state"].as_str(), Some("interrupted"), "重启标记: {row}");

        let (app, log, _cmd) =
            MockPublisher::spawn(home_b.path(), vec![], PublishMode::Answer).await;
        let id2 = reinitiate_and_find_new_id(&cb2, "devB:9001").await;

        // ---- 跳块字节账(静息点核对)----
        // A 回显卡一出现即暂停(零字节起步,消除「首块即完」竞面),放行
        // 后观察到首个新块(B 卡字节越过续传起点)再暂停 → 双侧静息 →
        // B 已收 − A 实发 == 分区前已收(续传引擎把已收块计入 done 基数,
        // 发送端只推缺块——差值恰为跳过的字节)。
        wait_card_id(&ca, &id2, "A 回显卡").await;
        call(&ca, "transfer.pause", json!({ "id": id2 })).await.unwrap();
        call(&ca, "transfer.resume", json!({ "id": id2 })).await.unwrap();
        wait_card(&cb2, &id2, "首个新块落地", |j| {
            j["bytes_done"].as_u64().unwrap_or(0) > received_before
        })
        .await;
        call(&ca, "transfer.pause", json!({ "id": id2 })).await.unwrap();
        let mut last = (0u64, 0u64);
        let (a_card, b_card) = loop {
            let a = find_card_by_id(&ca, &id2).await.expect("A 卡在场");
            let b = find_card_by_id(&cb2, &id2).await.expect("B 卡在场");
            let now = (
                a["bytes_done"].as_u64().unwrap_or(0),
                b["bytes_done"].as_u64().unwrap_or(0),
            );
            let settled = now == last
                && a["state"].as_str() == Some("paused")
                && b["state"].as_str() != Some("done");
            last = now;
            if settled {
                break (a, b);
            }
            assert!(
                b["state"].as_str() != Some("done"),
                "静息点前已完成(暂停未冻结传输): {b}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(
            b_card["bytes_done"].as_u64().unwrap() - a_card["bytes_done"].as_u64().unwrap(),
            received_before,
            "跳块字节账:B 已收({:?}) − A 实发({:?}) == 分区前已收({received_before})",
            b_card["bytes_done"],
            a_card["bytes_done"]
        );
        call(&ca, "transfer.resume", json!({ "id": id2 })).await.unwrap();

        // ---- 收尾:续传完成 → publishing → done(T1 拓扑注的空窗补上:
        // 下载驱动的发布门合成首次在 e2e 走通)----
        let fin_a = call_bounded(&ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("A 回显卡应到终态");
        assert_eq!(fin_a["state"].as_str(), Some("done"), "A 侧终态: {fin_a}");
        assert_eq!(fin_a["files_done"].as_u64(), Some(1), "A 侧完成计数: {fin_a}");
        let card = wait_card(&cb2, &id2, "publishing 保存中", |j| {
            j["state"].as_str() == Some("publishing")
        })
        .await;
        assert_eq!(card["state"].as_str(), Some("publishing"), "保存中态: {card}");
        wait_card(&cb2, &id2, "done 终态", |j| j["state"].as_str() == Some("done")).await;

        // 发布面:恰一次 fs.publish,发布名 = rel(无 "(1)" 改名——分区下
        // staged 续传件不重收);记录 done+published;暂存终件删除。
        {
            let seen = log.seen.lock().unwrap().clone();
            assert_eq!(seen.len(), 1, "恰一次发布: {seen:?}");
            assert_eq!(seen[0].1, rel, "发布名(无冲突改名): {seen:?}");
        }
        let rec = wait_record(home_b.path(), rel, "done+published", |v| {
            v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
        })
        .await;
        assert!(
            !staged_final(home_b.path(), rec["final_rel"].as_str().unwrap()).exists(),
            "发布后暂存终件删除"
        );
        assert!(staging_finals(home_b.path()).is_empty(), "暂存区无残留终件");

        app.abort();
        let _ = daemon_b2.shutdown().await;
        let _ = daemon_a.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// T2-② republish-only(spec §6.3):两件暂存 done(② 形态:发布超时保
/// 留)+ 两条收件记录(failed「保存失败」/cancelled——**任一任务态皆可**,
/// cancelled 是 P4a 放行面)→ reinitiate → **零网络**(A 无回显卡 = 编排
/// 未发;B 卡零字节;发布名零 "(1)" 改名 = staged 件未重收)→ 发布 →
/// done;翻新记录携带 rels(后续重发可再分区)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_republish_only_republishes_without_network() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(240), async {
        let p = Pair::new("inbox7@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel_f = "重发 失败态.bin";
        let rel_c = "重发 取消态.bin";
        let f_f = src.path().join(rel_f);
        fill_random_file(&f_f, 16 * 1024);
        let f_c = src.path().join(rel_c);
        fill_random_file(&f_c, 24 * 1024);

        // 首传:发布不应答 → 两件 staged done 保留(② 形态)。
        let (app1, _log1, _cmd1) =
            MockPublisher::spawn(p.home_b.path(), vec![], PublishMode::Ignore).await;
        let files = vec![f_f.clone(), f_c.clone()];
        let id1 = start_inbox_upload(&p.ca, &files, &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id1 }), WAIT_TIMEOUT)
            .await
            .expect("首传网络面应收妥");
        assert_eq!(fin["state"].as_str(), Some("done"), "首传网络面: {fin}");
        for rel in [rel_f, rel_c] {
            wait_record(p.home_b.path(), rel, "staged done", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() != Some(true)
            })
            .await;
        }
        app1.abort();

        let ra = call(&p.ca, "status", json!({})).await.unwrap();
        let dev_a = ra["device"]["device_id"].as_str().unwrap().to_string();
        let a_cards_before = call(&p.ca, "transfer.list", json!({})).await.unwrap();

        // 杀 B → 直写两条收件记录(failed / cancelled)→ 重启。
        let Pair {
            _home_a,
            home_b,
            _state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_b,
        } = p;
        drop(cb);
        let (daemon_b2, cb2) = bounce_b_with_recs(
            daemon_b,
            home_b.path(),
            vec![
                inbox_download_rec(
                    "devB:9001",
                    &dev_a,
                    &dev_b,
                    &[rel_f],
                    std::slice::from_ref(&f_f),
                    "failed",
                ),
                inbox_download_rec(
                    "devB:9002",
                    &dev_a,
                    &dev_b,
                    &[rel_c],
                    std::slice::from_ref(&f_c),
                    "cancelled",
                ),
            ],
        )
        .await;
        wait_online(&cb2, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        let (app, log, _cmd) =
            MockPublisher::spawn(home_b.path(), vec![], PublishMode::Answer).await;
        for (old_id, rel) in [("devB:9001", rel_f), ("devB:9002", rel_c)] {
            let new_id = reinitiate_and_find_new_id(&cb2, old_id).await;
            assert_ne!(new_id, old_id, "翻新换新 id");
            let card = wait_card(&cb2, &new_id, "republish done", |j| {
                j["state"].as_str() == Some("done")
            })
            .await;
            assert_eq!(card["bytes_done"].as_u64(), Some(0), "零网络字节: {card}");
            assert!(card["error"].is_null(), "成功无终因: {card}");
            // 记录面:旧摘新在,新记录终态。
            assert!(find_card_by_id(&cb2, old_id).await.is_none(), "旧记录摘除");
            let new_row = find_card_by_id(&cb2, &new_id).await.expect("新记录在场");
            assert_eq!(new_row["state"].as_str(), Some("done"), "新记录终态: {new_row}");
            // 发布面:记录 done+published;暂存终件删除(发布消费,非截断)。
            let rec = wait_record(home_b.path(), rel, "done+published", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
            })
            .await;
            assert!(
                !staged_final(home_b.path(), rec["final_rel"].as_str().unwrap()).exists(),
                "发布后暂存终件删除"
            );
        }
        // 零网络的行证据:恰两次发布、发布名零改名(staged 件未重收——若
        // 误走网络相,A 必然出现回显卡、发布名必带 "(1)")。
        {
            let seen = log.seen.lock().unwrap().clone();
            let rels: Vec<&str> = seen.iter().map(|(_, r, _, _)| r.as_str()).collect();
            assert_eq!(rels, vec![rel_f, rel_c], "恰两次发布、零改名: {seen:?}");
        }
        let a_cards_after = call(&ca, "transfer.list", json!({})).await.unwrap();
        assert_eq!(
            a_cards_after["jobs"].as_array().map(Vec::len),
            a_cards_before["jobs"].as_array().map(Vec::len),
            "A 侧零新卡(编排未发,免对端): {a_cards_after}"
        );
        assert!(staging_finals(home_b.path()).is_empty(), "暂存区无残留终件");

        app.abort();
        let _ = daemon_b2.shutdown().await;
        let _ = daemon_a.shutdown().await;
    });
    run.await.expect("用例超时");
}

/// T2-③ 混合分区(评审验收 #1/#2):2 件 staged done + 1 件缺失 →
/// reinitiate → **2 件 republish + 1 件网络**——hold 窗(发布请求握住不
/// 应答)断言:staged 件 mtime+内容零变化(分区保证其永不进接收集,Full
/// 截断无从触及)且 **A 无回显卡**(republish 相先于网络相落定,网络相
/// 若抢先,staged 件必被 Full 重收→"(1)" 改名孤儿);放行后 staged 件发
/// 布删除、缺失件网络补齐,Skip 双证(A 侧 3 件计完成、B 侧仅缺件字节)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_mixed_partition_republishes_staged_and_recovers_missing() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(300), async {
        let p = Pair::new("inbox8@x.y").await;
        let src = tempfile::tempdir().unwrap();
        let rel_a = "混合 甲.bin";
        let rel_b = "混合 乙.bin";
        let rel_c = "混合 丙.bin";
        let sizes = [(rel_a, 16 * 1024usize), (rel_b, 24 * 1024), (rel_c, 32 * 1024)];
        let mut files: Vec<PathBuf> = Vec::new();
        for (rel, size) in &sizes {
            let f = src.path().join(rel);
            fill_random_file(&f, *size);
            files.push(f);
        }

        // 首传:发布不应答 → 三件 staged done 保留(② 形态)。
        let (app1, _log1, _cmd1) =
            MockPublisher::spawn(p.home_b.path(), vec![], PublishMode::Ignore).await;
        let id1 = start_inbox_upload(&p.ca, &files, &p.dev_b).await;
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id1 }), WAIT_TIMEOUT)
            .await
            .expect("首传网络面应收妥");
        assert_eq!(fin["state"].as_str(), Some("done"), "首传网络面: {fin}");
        for (rel, _) in &sizes {
            wait_record(p.home_b.path(), rel, "staged done", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() != Some(true)
            })
            .await;
        }
        app1.abort();

        // staged 件指纹基线(评审验收 #1:重发全程 mtime+内容零变化)。
        let fin_a_rel = staging_record(p.home_b.path(), rel_a).unwrap()["final_rel"]
            .as_str()
            .unwrap()
            .to_string();
        let fin_b_rel = staging_record(p.home_b.path(), rel_b).unwrap()["final_rel"]
            .as_str()
            .unwrap()
            .to_string();
        let fp_a = fingerprint(&staged_final(p.home_b.path(), &fin_a_rel));
        let fp_b = fingerprint(&staged_final(p.home_b.path(), &fin_b_rel));

        let ra = call(&p.ca, "status", json!({})).await.unwrap();
        let dev_a = ra["device"]["device_id"].as_str().unwrap().to_string();

        // 杀 B → 删丙的暂存记录+终件(构「缺失」件:分区应判 missing 走
        // 网络)→ 直写混合记录 → 重启。
        let Pair {
            _home_a,
            home_b,
            _state,
            daemon_a,
            daemon_b,
            ca,
            cb,
            dev_b,
        } = p;
        drop(cb);
        daemon_b.shutdown().await.expect("杀 B");
        let rec_c_path = staging_dir(home_b.path())
            .join(INTAKE_DIR)
            .join(format!("{}.json", record_key(rel_c)));
        std::fs::remove_file(&rec_c_path).expect("删丙记录");
        std::fs::remove_file(staged_final(home_b.path(), rel_c)).expect("删丙终件");
        append_tasks_json(
            home_b.path(),
            &[inbox_download_rec(
                "devB:9001",
                &dev_a,
                &dev_b,
                &[rel_a, rel_b, rel_c],
                &files,
                "failed",
            )],
        );
        let daemon_b2 = Daemon::start(home_b.path(), opts(home_b.path()))
            .await
            .expect("B 同 home 重启");
        let cb2 = IpcClient::connect(home_b.path()).await.expect("重连 B IPC");
        wait_online(&cb2, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;

        // Hold:甲乙的 republish 请求到达即握住(发布未落定 → 网络相不
        // 起),hold 窗内完成两组钉死断言。
        let (app, log, cmd) =
            MockPublisher::spawn(home_b.path(), vec![], PublishMode::Hold).await;
        let id2 = reinitiate_and_find_new_id(&cb2, "devB:9001").await;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let n = log.seen.lock().unwrap().len();
            if n == 2 {
                break;
            }
            assert!(Instant::now() < deadline, "staged 两件的发布请求未到达: {n}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // 断言组 1(验收 #1):staged 件字节零变化——mtime+内容与基线全等
        // (网络 Full 若已触及其 .part/终件,mtime 必然位移、内容截断)。
        assert_eq!(
            fingerprint(&staged_final(home_b.path(), &fin_a_rel)),
            fp_a,
            "甲 mtime+内容零变化"
        );
        assert_eq!(
            fingerprint(&staged_final(home_b.path(), &fin_b_rel)),
            fp_b,
            "乙 mtime+内容零变化"
        );
        // 断言组 2(验收 #2 的根):republish 相先于网络相——A 无回显卡
        // (编排未发);B 卡 publishing(保存中)。
        let a_cards = call(&ca, "transfer.list", json!({})).await.unwrap();
        assert!(
            !a_cards["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|j| j["id"].as_str() == Some(id2.as_str())),
            "hold 窗内 A 不得有回显卡(网络相未起): {a_cards}"
        );
        let card = wait_card(&cb2, &id2, "publishing 保存中", |j| {
            j["state"].as_str() == Some("publishing")
        })
        .await;
        assert_eq!(card["state"].as_str(), Some("publishing"), "保存中态: {card}");
        // 放行:甲乙发布落定 → 网络相起(丙补齐,甲乙经 0x04 在场 Skip)。
        set_mode(&cmd, PublishMode::Answer).await;

        // A 侧:3 件全计完成(Skip 件经 FileDone 计数,⑥ 口径)。先等回显
        // 卡在场再 wait(wait 对缺席 id 立即报「作业不存在」,与编排到达有
        // 竞窗)。
        wait_card_id(&ca, &id2, "A 回显卡(网络相起)").await;
        let fin_a = call_bounded(&ca, "transfer.wait", json!({ "id": id2 }), WAIT_TIMEOUT)
            .await
            .expect("A 回显卡应到终态");
        assert_eq!(fin_a["state"].as_str(), Some("done"), "A 侧终态: {fin_a}");
        assert_eq!(fin_a["files_done"].as_u64(), Some(3), "Skip 件计完成: {fin_a}");
        // B 侧:仅丙的接收字节(甲乙 Skip 零字节)→ publishing → done。
        let card = wait_card(&cb2, &id2, "B done", |j| j["state"].as_str() == Some("done")).await;
        assert_eq!(
            card["bytes_done"].as_u64(),
            Some(32 * 1024_u64),
            "B 侧仅缺件字节(甲乙 Skip 零字节): {card}"
        );
        assert_eq!(card["files_done"].as_u64(), Some(1), "B 侧仅缺件落盘: {card}");

        // 发布面:恰三次(甲乙 republish + 丙网络后),发布名全等原件
        // (零 "(1)" 改名 = 分区路径不产生孤儿,验收 #2)。
        {
            let seen = log.seen.lock().unwrap().clone();
            let mut rels: Vec<&str> = seen.iter().map(|(_, r, _, _)| r.as_str()).collect();
            rels.sort_unstable();
            let mut expect = vec![rel_a, rel_b, rel_c];
            expect.sort_unstable();
            assert_eq!(
                rels, expect,
                "恰三次发布、发布名全等原件: {seen:?}"
            );
            assert!(rels.iter().all(|r| !r.contains("(1)")), "零冲突改名: {seen:?}");
        }
        // 记录面:三件 done+published;暂存区无残留终件(孤儿断言:全件
        // 经发布消费,无 "(1)" 无主件)。
        for (rel, _) in &sizes {
            wait_record(home_b.path(), rel, "done+published", |v| {
                v["done"].as_bool() == Some(true) && v["published"].as_bool() == Some(true)
            })
            .await;
        }
        assert!(
            staging_finals(home_b.path()).is_empty(),
            "暂存区无残留终件(无孤儿): {:?}",
            staging_finals(home_b.path())
        );

        app.abort();
        let _ = daemon_b2.shutdown().await;
        let _ = daemon_a.shutdown().await;
    });
    run.await.expect("用例超时");
}
