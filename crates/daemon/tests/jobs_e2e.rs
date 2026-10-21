//! 传输作业端到端(M3c Task 6/7/8):进程内双/三 Daemon + IpcClient,覆盖
//! `transfer.start/list/pause/resume/cancel/wait` RPC、`event.transfer`
//! 事件面(状态翻转即发 + 进度节流)、暂停/取消生命周期、cut 删源与
//! A→B 编排(Task 8 Trio:X 操作端 + A 源 + B 目标)。
//!
//! 取消语义钉死:cancel = 置 watch + abort 驱动任务(连 run_send_with 的
//! 重试循环一并杀,不复活)——cancel_aborts_retry_loop 在杀掉对端后取消,
//! 若 abort 未生效,驱动会困在「10s 信令超时 → Network → 退避重试」循环,
//! 秒级终态与事件静默两个断言都会翻车。
//!
//! 全部用例限定 worker_threads = 2(沿 ctrl_e2e 口径:全 workspace 并跑
//! 下避免过度订阅饿死打洞/信令预算;用例均为 IO 型,2 worker 足够)。

use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use signaling::spawn_test_server;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 常规调用超时:transfer.* 除 wait 外都是快操作(打洞在驱动任务内异步)。
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// transfer.wait 长轮询:传输全程(含打洞/重试)在驱动内,给足上界防悬挂。
const WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// 同文件用例并发闸(Task 4 先例,ctrl_e2e 同款):cargo 默认按 CPU 数并跑
/// 用例,每用例自带 2-worker reactor + 进程内多 daemon + 独立信令服务器;
/// Task 7 起用例面扩到 11 个(下载/本地复制/devices 现拉),Task 8 增三
/// daemon 编排例后更重,全 workspace 并跑下打洞/信令墙钟预算易被饿穿。
/// 限并发到 3(约半 CPU 预算),单用例预算回到余量内。
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
    .expect("login.verify 应成功")
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

/// 轮询 devices 快照直到 peer 的 online 等于 want。
async fn wait_peer_online(client: &IpcClient, dev: &str, want: bool, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let devs = call(client, "devices", json!({})).await.unwrap();
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

/// 轮询事件流直至谓词命中;超时 panic。
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
                eprintln!("事件通道滞后 {n} 帧,继续等待");
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
            Err(_) => panic!("等待事件超时({secs}s): {what}"),
        }
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

/// 进程内双 daemon 测试台(沿 daemon_e2e::Pair 样板):独立信令服务器 +
/// 两侧独立 home/IPC 客户端,双方已登录且 A 已见 B 在线。home 的 TempDir
/// 必须随 Pair 存活——drop 即删目录,daemon 的 identity.seed 会随之消失。
struct Pair {
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
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
            dev_b,
        }
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
    }
}

/// 进程内三 daemon 测试台(Task 8 A→B 编排):X(操作端)/A(源)/B(目标)
/// 同账号同信令服务器,Pair 样板的加一号位。三方互见在线是编排前置——
/// X 的 ctrl 面(list/stat/orchestrate)按目录快照做离线预检,快照缺对端
/// 即快错。
struct Trio {
    _home_x: tempfile::TempDir,
    _home_a: tempfile::TempDir,
    _home_b: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon_x: Daemon,
    daemon_a: Daemon,
    daemon_b: Daemon,
    cx: IpcClient,
    /// A/B 侧 IPC 客户端(保持连接在场;当前用例只经 X 面,句柄留作后续
    /// 用例与排障,前缀下划线消 dead_code)。
    _ca: IpcClient,
    _cb: IpcClient,
    dev_x: String,
    dev_a: String,
    dev_b: String,
}

impl Trio {
    async fn new(email: &str) -> Self {
        let (addr, state) = spawn_test_server().await;
        let server = format!("http://{addr}");
        let home_x = tempfile::tempdir().unwrap();
        let home_a = tempfile::tempdir().unwrap();
        let home_b = tempfile::tempdir().unwrap();
        let daemon_x = Daemon::start(home_x.path(), opts(home_x.path())).await.unwrap();
        let daemon_a = Daemon::start(home_a.path(), opts(home_a.path())).await.unwrap();
        let daemon_b = Daemon::start(home_b.path(), opts(home_b.path())).await.unwrap();
        let cx = IpcClient::connect(home_x.path()).await.unwrap();
        let ca = IpcClient::connect(home_a.path()).await.unwrap();
        let cb = IpcClient::connect(home_b.path()).await.unwrap();
        let rx = ipc_login(&cx, &server, email, "X").await;
        let ra = ipc_login(&ca, &server, email, "A").await;
        let rb = ipc_login(&cb, &server, email, "B").await;
        wait_online(&cx, true, 30).await;
        wait_online(&ca, true, 30).await;
        wait_online(&cb, true, 30).await;
        let dev_x = rx["device_id"].as_str().unwrap().to_string();
        let dev_a = ra["device_id"].as_str().unwrap().to_string();
        let dev_b = rb["device_id"].as_str().unwrap().to_string();
        assert_ne!(dev_x, dev_a, "三方必须是不同设备");
        assert_ne!(dev_x, dev_b);
        assert_ne!(dev_a, dev_b);
        // 三方互见(编排前置,见 struct 注)
        wait_peer_online(&cx, &dev_a, true, 30).await;
        wait_peer_online(&cx, &dev_b, true, 30).await;
        wait_peer_online(&ca, &dev_x, true, 30).await;
        wait_peer_online(&ca, &dev_b, true, 30).await;
        wait_peer_online(&cb, &dev_x, true, 30).await;
        wait_peer_online(&cb, &dev_a, true, 30).await;
        Self {
            _home_x: home_x,
            _home_a: home_a,
            _home_b: home_b,
            _state: state,
            daemon_x,
            daemon_a,
            daemon_b,
            cx,
            _ca: ca,
            _cb: cb,
            dev_x,
            dev_a,
            dev_b,
        }
    }

    async fn shutdown(self) {
        let _ = self.daemon_b.shutdown().await;
        let _ = self.daemon_a.shutdown().await;
        let _ = self.daemon_x.shutdown().await;
    }
}

/// 递归收集目录下的相对文件路径集合(A→B 的「X home 无数据落地」目录面
/// 断言数据源;分隔符统一 '/',仅用于集合相等比较)。
fn collect_files(root: &Path) -> std::collections::BTreeSet<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut std::collections::BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let rel = format!("{prefix}{}", e.file_name().to_string_lossy());
            if e.path().is_dir() {
                walk(&e.path(), &format!("{rel}/"), out);
            } else {
                out.insert(rel);
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(root, "", &mut out);
    out
}

/// transfer.start upload 形态速构;返回 (job_id, start 结果)。
async fn start_upload(
    client: &IpcClient,
    files: &[PathBuf],
    to: &str,
    dir: &str,
    extra: Value,
) -> Value {
    let mut params = json!({
        "kind": "upload",
        "files": files.iter().map(|f| f.to_string_lossy()).collect::<Vec<_>>(),
        "to": to,
        "dir": dir,
    });
    if let (Some(obj), Some(ext)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in ext {
            obj.insert(k.clone(), v.clone());
        }
    }
    call(client, "transfer.start", params).await.expect("transfer.start 应成功")
}

/// transfer.start download 形态速构(IPC 契约面:device/paths/dir/skip/
/// overwrite/cut)。
async fn start_download(
    client: &IpcClient,
    device: &str,
    paths: &[PathBuf],
    dir: &Path,
    extra: Value,
) -> Value {
    let mut params = json!({
        "kind": "download",
        "device": device,
        "paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "dir": dir.to_string_lossy(),
        "skip": [],
        "overwrite": [],
        "cut": false,
    });
    if let (Some(obj), Some(ext)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in ext {
            obj.insert(k.clone(), v.clone());
        }
    }
    call(client, "transfer.start", params).await.expect("transfer.start 应成功")
}

/// transfer.start remote_copy 形态速构(IPC 契约面 verbatim:from/paths/
/// to/dir/skip/overwrite/cut)。
async fn start_remote_copy(
    client: &IpcClient,
    from: &str,
    paths: &[PathBuf],
    to: &str,
    dir: &Path,
    extra: Value,
) -> Value {
    let mut params = json!({
        "kind": "remote_copy",
        "from": from,
        "paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "to": to,
        "dir": dir.to_string_lossy(),
        "skip": [],
        "overwrite": [],
        "cut": false,
    });
    if let (Some(obj), Some(ext)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in ext {
            obj.insert(k.clone(), v.clone());
        }
    }
    call(client, "transfer.start", params).await.expect("transfer.start 应成功")
}

// ---------- upload 全链路 ----------

/// A 起 upload 作业至 B 定向目录 → transfer.wait 终态 done,B 盘逐 rel 校验
/// 内容;wait 报告的 files/bytes 计数与预扫一致。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_job_completes() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("job1@x.y").await;
        let target = tempfile::tempdir().unwrap();

        let src = tempfile::tempdir().unwrap();
        let dir = src.path().join("作业 目录");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let data_a = fill_random_file(&dir.join("a.bin"), 16 * 1024);
        let data_b = fill_random_file(&dir.join("sub").join("b.bin"), 32 * 1024);
        let single = src.path().join("单件.bin");
        let data_c = fill_random_file(&single, 8 * 1024);

        let r = start_upload(
            &p.ca,
            &[dir, single],
            &p.dev_b,
            &target.path().to_string_lossy(),
            json!({}),
        )
        .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        assert!(!id.is_empty(), "job_id 非空: {r}");

        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        assert_eq!(fin["files_done"].as_u64(), Some(3), "完成计数: {fin}");
        assert_eq!(fin["files_total"].as_u64(), Some(3), "预扫总数: {fin}");
        assert_eq!(fin["bytes_total"].as_u64(), Some(56 * 1024), "字节总数: {fin}");

        // B 盘逐 rel 校验(定向目录落盘,rel 口径与 send 相同)
        assert_eq!(
            std::fs::read(target.path().join("作业 目录").join("a.bin")).unwrap(),
            data_a
        );
        assert_eq!(
            std::fs::read(target.path().join("作业 目录").join("sub").join("b.bin")).unwrap(),
            data_b
        );
        assert_eq!(std::fs::read(target.path().join("单件.bin")).unwrap(), data_c);

        p.shutdown().await;
    });
    run.await.expect("upload_job_completes exceeded 120s");
}

/// B 预置同名旧文件,A 以 overwrite 策略上传 → 内容翻新、无 " (1)" 改名
/// (策略随 ConnectRequest 下发,接收端 prepare_overwrite 清障)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_conflict_policy_applied() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("jobcf@x.y").await;
        let target = tempfile::tempdir().unwrap();
        std::fs::write(target.path().join("dup.txt"), b"stale-content").unwrap();

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("dup.txt");
        let data = fill_random_file(&f, 24 * 1024);

        let r = start_upload(
            &p.ca,
            &[f],
            &p.dev_b,
            &target.path().to_string_lossy(),
            json!({ "overwrite": ["dup.txt"] }),
        )
        .await;
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        assert_eq!(
            std::fs::read(target.path().join("dup.txt")).unwrap(),
            data,
            "同名旧文件必须被新内容顶替"
        );
        assert!(
            !target.path().join("dup (1).txt").exists(),
            "overwrite 命中不得触发冲突改名"
        );

        p.shutdown().await;
    });
    run.await.expect("upload_conflict_policy e2e exceeded 120s");
}

/// 大文件 pause → 进度停走(两次采样 bytes_done 相等、state=paused)→
/// resume → 终态 done 且内容一致。pause 在 start 后立即下发:暂停门在
/// 发送波前/块流开启前挂起,无论停在传输的哪一段,门语义都收敛。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_resume_job() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("jobpz@x.y").await;
        let target = tempfile::tempdir().unwrap();

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("暂停 大文件.bin");
        let data = fill_random_file(&f, 16 * 1024 * 1024);

        let r = start_upload(
            &p.ca,
            std::slice::from_ref(&f),
            &p.dev_b,
            &target.path().to_string_lossy(),
            json!({}),
        )
        .await;
        let id = r["job_id"].as_str().unwrap().to_string();

        // 立即暂停:门生效于驱动与发送引擎的每个波前/块流开启点
        assert_eq!(
            call(&p.ca, "transfer.pause", json!({ "id": id })).await.unwrap(),
            json!({})
        );

        // 进度停走:两次 list 快照(间隔 > 节流窗)state=paused 且 bytes 相等
        let snap = |lst: &Value| -> (String, u64) {
            let job = lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(id.as_str()))
                .expect("list 含作业");
            (
                job["state"].as_str().unwrap_or_default().to_string(),
                job["bytes_done"].as_u64().unwrap_or(0),
            )
        };
        let l1 = call(&p.ca, "transfer.list", json!({})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let l2 = call(&p.ca, "transfer.list", json!({})).await.unwrap();
        let (s1, b1) = snap(&l1);
        let (s2, b2) = snap(&l2);
        assert_eq!(s1, "paused", "暂停后状态: {l1}");
        assert_eq!(s2, "paused", "暂停持续: {l2}");
        assert_eq!(b1, b2, "暂停期间进度必须停走: {b1} vs {b2}");

        // 恢复 → 终态 done,内容一致
        assert_eq!(
            call(&p.ca, "transfer.resume", json!({ "id": id }))
                .await
                .unwrap(),
            json!({})
        );
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("resume 后应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        assert_eq!(
            std::fs::read(target.path().join("暂停 大文件.bin")).unwrap(),
            data,
            "恢复后传输必须完整落盘"
        );

        p.shutdown().await;
    });
    run.await.expect("pause_resume e2e exceeded 150s");
}

/// 取消语义:杀 B(驱动必入「信令超时→Network→退避重试」循环)→ 起作业
/// 即 cancel → 秒级终态 cancelled(abort 杀驱动任务与重试循环);事件流
/// 在 cancelled 之后再无该作业事件;list 仍报 cancelled。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_aborts_retry_loop() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("jobcx@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("取消 甲.bin");
        fill_random_file(&f, 64 * 1024);

        // 先订事件流(状态翻转与取消后的静默断言都依赖不漏订)
        let mut evs = p.ca.events();

        // 杀 B:对端不可达,驱动会进入重试循环(若 cancel 未能 abort 它)
        drop(p.cb); // Windows 上客户端句柄先弃,管道名随 daemon 关闭释放
        p.daemon_b.shutdown().await.unwrap();
        let dev_b = p.dev_b.clone();

        let t0 = Instant::now();
        let r = start_upload(&p.ca, &[f], &dev_b, &target.path().to_string_lossy(), json!({}))
            .await;
        let id = r["job_id"].as_str().unwrap().to_string();
        assert_eq!(
            call(&p.ca, "transfer.cancel", json!({ "id": id }))
                .await
                .unwrap(),
            json!({})
        );
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), Duration::from_secs(10))
            .await
            .expect("取消后 wait 必须立即返回");
        assert_eq!(fin["state"].as_str(), Some("cancelled"), "终态: {fin}");
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "取消必须秒退(abort 杀重试循环;若未生效,首轮 10s 信令超时都过不去): {:?}",
            t0.elapsed()
        );

        // 事件流:cancelled 事件到达后,静默窗内不得再有该作业事件
        wait_event(&mut evs, 10, "cancelled 事件", |f| {
            f.method == "event.transfer"
                && f.params["id"].as_str() == Some(id.as_str())
                && f.params["state"].as_str() == Some("cancelled")
        })
        .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match tokio::time::timeout_at(deadline, evs.recv()).await {
                Err(_) => break, // 静默窗耗尽:无后续事件(重试已被 abort 杀)
                Ok(Ok(f)) => assert!(
                    !(f.method == "event.transfer"
                        && f.params["id"].as_str() == Some(id.as_str())),
                    "取消后不得再有该作业事件: {f:?}"
                ),
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
            }
        }

        // list 快照仍是 cancelled(终态粘滞)
        let lst = call(&p.ca, "transfer.list", json!({})).await.unwrap();
        let job = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("list 含已取消作业");
        assert_eq!(job["state"].as_str(), Some("cancelled"));

        p.daemon_a.shutdown().await.unwrap();
    });
    run.await.expect("cancel e2e exceeded 120s");
}

/// cut=true 的 upload:驱动成功后本地 delete_paths(force=false) 删源
/// (尽力回收)→ 源消失、目标在。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cut_upload_deletes_sources() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("jobcut@x.y").await;
        let target = tempfile::tempdir().unwrap();

        let src = tempfile::tempdir().unwrap();
        let f = src.path().join("剪切 甲.bin");
        let data = fill_random_file(&f, 32 * 1024);

        let r = start_upload(
            &p.ca,
            std::slice::from_ref(&f),
            &p.dev_b,
            &target.path().to_string_lossy(),
            json!({ "cut": true }),
        )
        .await;
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        assert_eq!(
            std::fs::read(target.path().join("剪切 甲.bin")).unwrap(),
            data,
            "目标侧内容一致"
        );
        assert!(!f.exists(), "cut 成功后源必须被删(回收优先)");

        p.shutdown().await;
    });
    run.await.expect("cut e2e exceeded 120s");
}

/// transfer.list 快照字段齐(id/kind/state/files_*/bytes_*/speed/src/dst/
/// error);事件面含状态翻转(queued→running→done)与节流进度(首条即发,
/// 之后 ≥400ms 间隔)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_list_and_events() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("jobls@x.y").await;
        let target = tempfile::tempdir().unwrap();
        let dir_str = target.path().to_string_lossy().into_owned();

        let src = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, size) in [
            ("清点 甲.bin", 4 * 1024usize),
            ("清点 乙.bin", 64 * 1024),
            ("清点 丙.bin", 256 * 1024),
        ] {
            let f = src.path().join(name);
            fill_random_file(&f, size);
            files.push(f);
        }
        let total: u64 = 4 * 1024 + 64 * 1024 + 256 * 1024;

        // 先订阅再触发(状态翻转与进度帧不得漏订)
        let mut evs = p.ca.events();
        let r = start_upload(&p.ca, &files, &p.dev_b, &dir_str, json!({})).await;
        let id = r["job_id"].as_str().unwrap().to_string();

        // 收帧至 done 事件(带终态帧到达上界;记录到达时刻供节流断言)
        let mut frames: Vec<(String, Instant)> = Vec::new(); // (state, arrived)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            match tokio::time::timeout_at(deadline, evs.recv()).await {
                Ok(Ok(f)) if f.method == "event.transfer"
                    && f.params["id"].as_str() == Some(id.as_str()) =>
                {
                    let state = f.params["state"].as_str().unwrap_or_default().to_string();
                    let done = state == "done";
                    frames.push((state, Instant::now()));
                    if done {
                        break;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    eprintln!("事件通道滞后 {n} 帧,继续等待");
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
                Err(_) => panic!("done 事件未到(30s): {frames:?}"),
            }
        }

        // 状态翻转序列:连续去重后恰为 queued → running → done
        let mut dedup: Vec<&str> = Vec::new();
        for (s, _) in &frames {
            if dedup.last().copied() != Some(s.as_str()) {
                dedup.push(s.as_str());
            }
        }
        assert_eq!(dedup, vec!["queued", "running", "done"], "翻转序列: {frames:?}");

        // 节流进度:首条进度即发(≥1 条 running 态帧);多条时到达间隔 ≥380ms
        // (400ms 节流 - 少量调度容差;事件按发送序到达,到达间隔不可能压缩)
        let progress: Vec<Instant> = frames
            .iter()
            .filter(|(s, _)| s == "running")
            .map(|(_, t)| *t)
            .collect();
        assert!(
            !progress.is_empty(),
            "至少一条 running 态进度帧(首条即发): {frames:?}"
        );
        for w in progress.windows(2) {
            let gap = w[1].duration_since(w[0]);
            assert!(
                gap >= Duration::from_millis(380),
                "进度帧间隔必须 ≥400ms(节流): {gap:?}"
            );
        }
        // 末帧必为终态 done
        assert_eq!(frames.last().unwrap().0, "done");

        // 等驱动收尾稳定后查 list(终态字段齐)
        let lst = call(&p.ca, "transfer.list", json!({})).await.unwrap();
        let job = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("list 含作业");
        assert_eq!(job["kind"].as_str(), Some("upload"));
        assert_eq!(job["state"].as_str(), Some("done"));
        assert_eq!(job["files_done"].as_u64(), Some(3));
        assert_eq!(job["files_total"].as_u64(), Some(3));
        assert_eq!(job["bytes_done"].as_u64(), Some(total));
        assert_eq!(job["bytes_total"].as_u64(), Some(total));
        assert!(job["speed_bps"].is_u64(), "speed_bps 恒为数: {job}");
        assert!(!job["src"].as_str().unwrap_or_default().is_empty(), "src 非空");
        let dst = job["dst"].as_str().unwrap_or_default();
        assert!(dst.contains(&dir_str) && dst.contains(&p.dev_b), "dst 含对端与目录: {dst}");
        assert!(job["error"].is_null(), "成功作业无 error: {job}");

        p.shutdown().await;
    });
    run.await.expect("list+events e2e exceeded 120s");
}

// ---------- download 全链路(Task 7) ----------

/// A 的 RPC download{device:B, paths, dir:A tempdir} → A 侧 wait done、
/// A 盘逐 rel 校验内容、B 侧源仍在(cut=false 不动源);作业卡 kind=
/// download。全链 = orchestrate(B 起同 id upload 驱动)→ ConnectRequest
/// 带 job_id 回来 → resident loop bind_incoming 路由。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_job_roundtrip() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("dlrt@x.y").await;
        // B 侧源(远端绝对路径),A 侧落盘目录
        let src_b = tempfile::tempdir().unwrap();
        let sub = src_b.path().join("下载 目录");
        std::fs::create_dir_all(&sub).unwrap();
        let data_a = fill_random_file(&sub.join("甲.bin"), 64 * 1024);
        let data_b = fill_random_file(&src_b.path().join("乙.txt"), 8 * 1024);
        let dst = tempfile::tempdir().unwrap();

        let r = start_download(
            &p.ca,
            &p.dev_b,
            &[sub.clone(), src_b.path().join("乙.txt")],
            dst.path(),
            json!({}),
        )
        .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        assert!(!id.is_empty());

        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        // A 盘逐 rel 校验(rel 口径与 send 相同:顶层名 = 输入名)
        assert_eq!(
            std::fs::read(dst.path().join("下载 目录").join("甲.bin")).unwrap(),
            data_a
        );
        assert_eq!(std::fs::read(dst.path().join("乙.txt")).unwrap(), data_b);
        // B 侧源仍在(非剪切)
        assert!(sub.join("甲.bin").exists(), "下载不动源(cut=false)");
        assert!(src_b.path().join("乙.txt").exists());

        // 作业卡 kind=download
        let lst = call(&p.ca, "transfer.list", json!({})).await.unwrap();
        let job = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("list 含下载作业");
        assert_eq!(job["kind"].as_str(), Some("download"));

        p.shutdown().await;
    });
    run.await.expect("download_roundtrip e2e exceeded 150s");
}

/// download 作业的进度事件来自收端 RecvProgress 路由:bytes_done 序列单调
/// 不减且终值达全量(4MB 多块多流,进度帧充足)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_progress_routes_to_job() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("dlpg@x.y").await;
        let src_b = tempfile::tempdir().unwrap();
        let f = src_b.path().join("进度 大文件.bin");
        fill_random_file(&f, 4 * 1024 * 1024);
        let dst = tempfile::tempdir().unwrap();

        // 先订阅再触发(进度帧不得漏订)
        let mut evs = p.ca.events();
        let r = start_download(&p.ca, &p.dev_b, &[f], dst.path(), json!({})).await;
        let id = r["job_id"].as_str().unwrap().to_string();

        // 收帧至 done(记录 bytes_done 序列)
        let mut seq: Vec<u64> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            match tokio::time::timeout_at(deadline, evs.recv()).await {
                Ok(Ok(fr))
                    if fr.method == "event.transfer"
                        && fr.params["id"].as_str() == Some(id.as_str()) =>
                {
                    seq.push(fr.params["bytes_done"].as_u64().unwrap_or(0));
                    if fr.params["state"].as_str() == Some("done") {
                        break;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    eprintln!("事件通道滞后 {n} 帧,继续等待");
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
                Err(_) => panic!("done 事件未到(60s): {seq:?}"),
            }
        }
        assert!(!seq.is_empty(), "至少一条事件帧");
        for w in seq.windows(2) {
            assert!(w[1] >= w[0], "bytes_done 必须单调不减: {seq:?}");
        }
        assert_eq!(
            *seq.last().unwrap(),
            4 * 1024 * 1024,
            "终值应达全量: {seq:?}"
        );

        p.shutdown().await;
    });
    run.await.expect("download_progress e2e exceeded 150s");
}

/// A pause → 接收轮门挂起 → B 发端被 QUIC 流控背压(经 B 侧注册表断言
/// 发端进度停走;与 ctrl job_query 同一数据源,RPC 面无 job_query,测试
/// 走 B 的 IPC 读同一注册表)→ resume → 完成。
///
/// 负载几何(M3c CI 教训,与引擎级 paused_receiver_backpressures 同构):
/// 必须 **150 个单块文件**——块流数要超过对端 MAX_STREAMS(100) 预算,
/// 发端才会在预授予流控窗口耗尽处冻结。单文件大负载(块流数 <100)可以
/// 整批装进预授予窗口,暂停期间照样推完,「停走」只在推完后才成立。
/// 采样用「等启动 → 收敛检测」而非固定睡眠:冻结值须 < 总量(确在中途)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_pause_pauses_sender() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("dlpz@x.y").await;
        let src_b = tempfile::tempdir().unwrap();
        const N: usize = 150;
        const PER: usize = 64 * 1024;
        let mut files = Vec::new();
        for i in 0..N {
            let f = src_b.path().join(format!("暂停下载{i:03}.bin"));
            fill_random_file(&f, PER);
            files.push(f);
        }
        let total = (N * PER) as u64;
        let dst = tempfile::tempdir().unwrap();

        let r = start_download(&p.ca, &p.dev_b, &files, dst.path(), json!({})).await;
        let id = r["job_id"].as_str().unwrap().to_string();

        // 立即暂停:门生效于接收主循环的每个流间点(发送端背压停滞)
        assert_eq!(
            call(&p.ca, "transfer.pause", json!({ "id": id })).await.unwrap(),
            json!({})
        );

        // A 侧作业可见状态 = paused(排除「暂停前已完成」竞态——若已完成,
        // pause 会因终态报错而失败)
        let snap_a = |lst: &Value| -> (String, u64) {
            let job = lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(id.as_str()))
                .expect("A list 含下载作业");
            (
                job["state"].as_str().unwrap_or_default().to_string(),
                job["bytes_done"].as_u64().unwrap_or(0),
            )
        };
        let (s1, _) = snap_a(&call(&p.ca, "transfer.list", json!({})).await.unwrap());
        assert_eq!(s1, "paused", "暂停后状态: {s1}");

        // B 发端进度停走:先等回显作业在 B 侧注册(orchestrate 已接受),
        // 再等发端启动(bytes > 0),最后收敛检测(700ms 间隔两次采样相等,
        // 30s 上界)——冻结值必须 < 总量,证明冻结发生在中途而非推完。
        let snap_b = |lst: &Value| -> Option<u64> {
            lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(id.as_str()))
                .map(|j| j["bytes_done"].as_u64().unwrap_or(0))
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let b_start = loop {
            let lst = call(&p.cb, "transfer.list", json!({})).await.unwrap();
            if let Some(b) = snap_b(&lst).filter(|b| *b > 0) {
                break b;
            }
            assert!(
                Instant::now() < deadline,
                "B 侧回显作业未在 30s 内出现或未启动(orchestrate 未接受?)"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last = b_start;
        let frozen = loop {
            tokio::time::sleep(Duration::from_millis(700)).await;
            let b = snap_b(&call(&p.cb, "transfer.list", json!({})).await.unwrap()).unwrap();
            if b == last {
                break b;
            }
            last = b;
            assert!(
                Instant::now() < deadline,
                "暂停期间 B 发端进度未收敛(未停走): 末值 {last}/{total}"
            );
        };
        assert!(
            frozen < total,
            "冻结值必须 < 总量(中途冻结而非推完): {frozen}/{total}"
        );

        // 恢复 → 终态 done,内容一致
        assert_eq!(
            call(&p.ca, "transfer.resume", json!({ "id": id }))
                .await
                .unwrap(),
            json!({})
        );
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("resume 后应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        for i in 0..N {
            let name = format!("暂停下载{i:03}.bin");
            assert_eq!(
                std::fs::read(dst.path().join(&name)).unwrap(),
                std::fs::read(src_b.path().join(&name)).unwrap(),
                "恢复后 {name} 内容一致"
            );
        }

        p.shutdown().await;
    });
    run.await.expect("download_pause e2e exceeded 180s");
}

/// local_copy 作业:本机复制落盘 + 暂存清理(目标无 .mtdss 残留);cut=true
/// 走移动快路(源移走)。纯本地操作,未登录单 daemon 直测。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_copy_job() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(60), async {
        let home = tempfile::tempdir().unwrap();
        let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
        let c = IpcClient::connect(home.path()).await.unwrap();

        let src = tempfile::tempdir().unwrap();
        let dir = src.path().join("复制 目录");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let data_a = fill_random_file(&dir.join("a.bin"), 32 * 1024);
        let data_b = fill_random_file(&dir.join("sub").join("b.bin"), 16 * 1024);
        let single = src.path().join("单件.txt");
        let data_c = fill_random_file(&single, 4 * 1024);

        // 复制(cut=false):整树落盘 + 暂存清理 + 源保留
        let target = tempfile::tempdir().unwrap();
        let r = call(
            &c,
            "transfer.start",
            json!({
                "kind": "local_copy",
                "paths": [dir.to_string_lossy(), single.to_string_lossy()],
                "dir": target.path().to_string_lossy(),
                "cut": false,
            }),
        )
        .await
        .expect("local_copy start 应成功");
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&c, "transfer.wait", json!({ "id": id }), Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(fin["state"].as_str(), Some("done"), "复制终态: {fin}");
        assert_eq!(
            std::fs::read(target.path().join("复制 目录").join("a.bin")).unwrap(),
            data_a
        );
        assert_eq!(
            std::fs::read(target.path().join("复制 目录").join("sub").join("b.bin")).unwrap(),
            data_b
        );
        assert_eq!(std::fs::read(target.path().join("单件.txt")).unwrap(), data_c);
        // 暂存清理:copy_paths 的暂存键(loc-*.part)必须被消费——.mtdss 是
        // 常驻 intake 区(隐藏保留目录,与传输引擎同一间房),允许在,但
        // 不得残留半截暂存键
        let intake = target.path().join(".mtdss");
        let staged_left = std::fs::read_dir(&intake)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("loc-"))
            })
            .unwrap_or(false);
        assert!(!staged_left, "暂存键不得残留: {intake:?}");
        assert!(dir.join("a.bin").exists(), "非剪切源保留");

        // 移动(cut=true):源移走,内容一致
        let target2 = tempfile::tempdir().unwrap();
        let r2 = call(
            &c,
            "transfer.start",
            json!({
                "kind": "local_copy",
                "paths": [dir.to_string_lossy()],
                "dir": target2.path().to_string_lossy(),
                "cut": true,
            }),
        )
        .await
        .expect("local_copy cut start 应成功");
        let id2 = r2["job_id"].as_str().unwrap().to_string();
        let fin2 = call_bounded(&c, "transfer.wait", json!({ "id": id2 }), Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(fin2["state"].as_str(), Some("done"), "移动终态: {fin2}");
        assert!(!dir.exists(), "cut 后源目录必须移走");
        assert_eq!(
            std::fs::read(target2.path().join("复制 目录").join("a.bin")).unwrap(),
            data_a
        );
        assert_eq!(
            std::fs::read(target2.path().join("复制 目录").join("sub").join("b.bin")).unwrap(),
            data_b
        );

        // 作业卡 kind=local_copy
        let lst = call(&c, "transfer.list", json!({})).await.unwrap();
        let job = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("list 含 local_copy 作业");
        assert_eq!(job["kind"].as_str(), Some("local_copy"));

        daemon.shutdown().await.unwrap();
    });
    run.await.expect("local_copy e2e exceeded 60s");
}

/// devices RPC 现拉:REST 命中 → stale=false 且目录新增可见(M4a Task 5
/// 起单飞 memo 以 1s TTL 为界,用例内先过窗再断言带新);现拉失败
/// (服务器目录面被拆,REST /devices → 500)→ 回退缓存快照 stale=true。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_rpc_now_pulls() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = Pair::new("devpull@x.y").await;
        let dev_a = {
            let st = call(&p.ca, "status", json!({})).await.unwrap();
            st["device"]["device_id"].as_str().unwrap().to_string()
        };

        // ① 现拉命中:服务器目录插一行探针设备(A 账号)→ RPC stale=false
        //    且含探针(缓存路径不可能有它——只有现拉能带出)。M4a Task 5
        //    起 Pair::new 的 wait_peer_online 轮询已填单飞 memo,目录变更
        //    可见性以 1s TTL 为界,先过窗再断言现拉带新。
        let account_id: String = {
            let conn = p._state.db.lock().unwrap();
            conn.query_row(
                "SELECT account_id FROM devices WHERE device_id=?1",
                [dev_a.as_str()],
                |r| r.get(0),
            )
            .unwrap()
        };
        {
            let conn = p._state.db.lock().unwrap();
            conn.execute(
                "INSERT INTO devices(account_id,device_id,pubkey,name,platform,created_at) \
                 VALUES(?1,'probe-dev-1','00','现拉探针','test',0)",
                [account_id.as_str()],
            )
            .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let devs = call(&p.ca, "devices", json!({})).await.unwrap();
        assert_eq!(
            devs["stale"].as_bool(),
            Some(false),
            "现拉成功必须 stale=false: {devs}"
        );
        assert!(
            devs["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["device_id"].as_str() == Some("probe-dev-1")),
            "现拉必须带出目录新增: {devs}"
        );

        // ② 现拉失败:拆掉 devices 表(REST /devices → 500)→ 回退缓存
        //    stale=true,缓存条目仍在。同样先过 ① 现拉填下的 memo 窗,
        //    否则命中 memo 会返回 stale=false。
        p._state
            .db
            .lock()
            .unwrap()
            .execute("DROP TABLE devices", [])
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let devs = call(&p.ca, "devices", json!({})).await.unwrap();
        assert_eq!(
            devs["stale"].as_bool(),
            Some(true),
            "现拉失败必须 stale=true: {devs}"
        );
        assert!(
            devs["devices"]
                .as_array()
                .map(|l| !l.is_empty())
                .unwrap_or(false),
            "回退必须是缓存快照(非空): {devs}"
        );
        assert!(
            devs["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["device_id"].as_str() == Some(p.dev_b.as_str())),
            "缓存须含对端: {devs}"
        );

        p.shutdown().await;
    });
    run.await.expect("devices_rpc e2e exceeded 120s");
}

/// Task 5 计数测试服:沿 signaling::spawn_test_server_with 形态自建 in-proc
/// AppState + axum,/devices 路由外挂原子命中计数(from_fn 中间层拦 path,
/// 不改 signaling 原路由)。返回 (地址, 服务器状态, 持 DB 目录——须随用例
/// 存活,tempfile 约定同 spawn_test_server_with)。
async fn spawn_counting_server(
    hits: Arc<AtomicUsize>,
) -> (std::net::SocketAddr, Arc<signaling::AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = signaling::db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
    let state = Arc::new(signaling::AppState {
        db: std::sync::Mutex::new(db),
        jwt_secret: rand::random(),
        presence: signaling::PresenceRegistry::new(),
        keepalive: signaling::ws::Keepalive::default(),
        stun_addrs: Vec::new(),
    });
    let app = signaling::build_router(state.clone()).layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let hits = hits.clone();
            async move {
                if req.uri().path() == "/devices" {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                next.run(req).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state, dir)
}

/// Task 5 devices 单飞测试台:计数测试服 + 单 daemon(登录且上线)。
/// 上线循环的目录全量拉先于 mark_online(hub.rs),wait_online 返回即该
/// 拉已落定;在线期本机 presence 属已知设备、只翻快照不触发 REST——短暂
/// settle 后基线干净,用例窗口内的 /devices 命中只可能来自 devices RPC。
struct CountingRig {
    _home: tempfile::TempDir,
    _dir: tempfile::TempDir,
    _state: Arc<signaling::AppState>,
    daemon: Daemon,
    hits: Arc<AtomicUsize>,
}

impl CountingRig {
    async fn new(email: &str) -> Self {
        let hits = Arc::new(AtomicUsize::new(0));
        let (addr, state, dir) = spawn_counting_server(hits.clone()).await;
        let server = format!("http://{addr}");
        let home = tempfile::tempdir().unwrap();
        let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
        let ca = IpcClient::connect(home.path()).await.unwrap();
        ipc_login(&ca, &server, email, "A").await;
        wait_online(&ca, true, 30).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        Self {
            _home: home,
            _dir: dir,
            _state: state,
            daemon,
            hits,
        }
    }

    /// n 条独立 IPC 连接并发 devices RPC(daemon 侧逐连接读半工,同一连接
    /// 内的请求串行分派,多连接才有真并发)。每调用带 30s timeout 包裹:
    /// 悬挂即测试失败,而非干等。
    async fn concurrent_devices(&self, n: usize) -> Vec<Value> {
        let mut handles = Vec::new();
        for _ in 0..n {
            let c = IpcClient::connect(self._home.path()).await.unwrap();
            handles.push(tokio::spawn(async move {
                call_bounded(&c, "devices", json!({}), Duration::from_secs(30)).await
            }));
        }
        let mut out = Vec::new();
        for h in handles {
            out.push(h.await.unwrap().expect("devices RPC 应成功"));
        }
        out
    }

    async fn shutdown(self) {
        let _ = self.daemon.shutdown().await;
    }
}

/// Task 5 devices 单飞:并发 5 个 devices RPC(独立 IPC 连接)必须共享同
/// 一次 REST 现拉——/devices 命中数基线增量 == 1,且 5 个响应均 stale=false、
/// 数据完整(目录含本机条目)。无 memo 时 5 路各自现拉,增量 == 5,本测
/// 试红。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_single_flight_shares_one_pull() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = CountingRig::new("devsf@x.y").await;
        let before = p.hits.load(Ordering::SeqCst);

        let replies = p.concurrent_devices(5).await;
        for r in &replies {
            assert_eq!(
                r["stale"].as_bool(),
                Some(false),
                "单飞命中的响应必须全部 stale=false: {r}"
            );
            assert!(
                !r["devices"].as_array().unwrap().is_empty(),
                "现拉数据必须完整(目录含本机条目): {r}"
            );
        }
        assert_eq!(
            p.hits.load(Ordering::SeqCst) - before,
            1,
            "并发 5 个 devices RPC 必须共享恰好一次 REST 现拉"
        );

        p.shutdown().await;
    });
    run.await.expect("devices single_flight e2e exceeded 120s");
}

/// Task 5 现拉失败面(服务器目录面 DROP TABLE → /devices 500):并发 N 个
/// 全部沿缓存回 stale=true,timeout 包裹无一悬挂——memo 持锁现拉的取消安
/// 全性回归:失败不落 memo,排队者逐个重试后各自回退,绝不死锁。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_pull_failure_all_stale_no_deadlock() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(120), async {
        let p = CountingRig::new("devsfail@x.y").await;
        p._state
            .db
            .lock()
            .unwrap()
            .execute("DROP TABLE devices", [])
            .unwrap();

        let replies = p.concurrent_devices(5).await;
        for r in &replies {
            assert_eq!(
                r["stale"].as_bool(),
                Some(true),
                "现拉失败必须回退缓存 stale=true: {r}"
            );
        }

        p.shutdown().await;
    });
    run.await.expect("devices pull_failure e2e exceeded 120s");
}

/// 评审 Critical#1 回归:源端注册表已有既有作业(一笔 legacy send 即产生
/// incoming 卡 id "1")后,操作端再发起 download——裸数字 id 会与源端键
/// 空间撞号被「任务 id 已存在」回绝;命名空间 id(`<操作端>:<序号>`)必须
/// 畅通,且源端既有卡不被回显条目覆盖。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_orchestration_survives_source_prior_jobs() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(150), async {
        let p = Pair::new("dlcol@x.y").await;

        // 第一笔:legacy send A→B,B 的注册表自此持有 incoming 卡(id "1")
        let src_a = tempfile::tempdir().unwrap();
        let f1 = src_a.path().join("先来 后到.txt");
        let data1 = fill_random_file(&f1, 32 * 1024);
        let r = call_bounded(
            &p.ca,
            "send",
            json!({ "files": [f1.to_string_lossy()], "to": p.dev_b }),
            Duration::from_secs(60),
        )
        .await
        .expect("legacy send 应成功");
        assert_eq!(r, json!({}));
        let on_b = p._home_b.path().join("inbox").join("先来 后到.txt");
        assert!(on_b.exists(), "legacy send 必须已落 B 盘");

        // 第二笔:操作端 A 发起 download(id 带本机命名空间),源端 B 既有
        // 卡 "1" 不得构成撞号回绝
        let dst = tempfile::tempdir().unwrap();
        let r = start_download(&p.ca, &p.dev_b, std::slice::from_ref(&on_b), dst.path(), json!({})).await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        assert!(
            id.contains(':'),
            "download id 必须带操作端命名空间: {id}"
        );
        let fin = call_bounded(&p.ca, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");
        assert_eq!(
            std::fs::read(dst.path().join("先来 后到.txt")).unwrap(),
            data1,
            "下载内容必须一致(源端 inbox 副本)"
        );

        // 源端既有 incoming 卡完好(未被回显条目覆盖/干扰)
        let lst = call(&p.cb, "transfer.list", json!({})).await.unwrap();
        let incoming = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some("1"))
            .expect("源端既有 incoming 卡必须仍在");
        assert_eq!(incoming["state"].as_str(), Some("done"));
        assert_eq!(incoming["kind"].as_str(), Some("incoming"));

        p.shutdown().await;
    });
    run.await.expect("download_prior_jobs e2e exceeded 150s");
}

// ---------- A→B 编排(Task 8:Trio X/A/B) ----------

/// A→B 编排全链:X 起 remote_copy(A 源 → B 定向目录)→ wait done → B 盘
/// 逐 rel 校验、A 源仍在(cut=false)、作业卡 kind=remote_copy 且 id 带操作
/// 端命名空间;X 自身 home 目录面不变(数据不经操作端落地的钉死断言;
/// 基线取在 ctrl 预热之后——TOFU 锚 peers.json 属编排自身的落盘,不算
/// 传输数据)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a2b_transfer_lands_on_b() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let t = Trio::new("a2b@x.y").await;

        // A 侧源:目录(嵌套) + 单件
        let src_a = tempfile::tempdir().unwrap();
        let dir_a = src_a.path().join("直传 目录");
        std::fs::create_dir_all(dir_a.join("sub")).unwrap();
        let data_a = fill_random_file(&dir_a.join("a.bin"), 64 * 1024);
        let data_b = fill_random_file(&dir_a.join("sub").join("b.bin"), 16 * 1024);
        let single = src_a.path().join("单件.txt");
        let data_c = fill_random_file(&single, 8 * 1024);

        // B 侧目标目录
        let dst_b = tempfile::tempdir().unwrap();

        // ctrl 预热(编排前置的 stat/list 会先于传输在 X home 落 TOFU 锚
        // peers.json;基线取在预热之后,目录面断言才只对传输数据敏感)
        call(&t.cx, "stat", json!({ "device": t.dev_a, "path": single.to_string_lossy() }))
            .await
            .expect("X stat A 预热应成功");
        call(&t.cx, "list", json!({ "device": t.dev_b, "path": dst_b.path().to_string_lossy() }))
            .await
            .expect("X list B 预热应成功");
        let before = collect_files(t._home_x.path());

        let r = start_remote_copy(
            &t.cx,
            &t.dev_a,
            &[dir_a.clone(), single.clone()],
            &t.dev_b,
            dst_b.path(),
            json!({}),
        )
        .await;
        let id = r["job_id"].as_str().expect("job_id 字符串").to_string();
        assert!(
            id.starts_with(&format!("{}:", t.dev_x)),
            "remote_copy id 必须带操作端命名空间(`<本机>:<序号>`,Task 7 口径): {id}"
        );

        let fin = call_bounded(&t.cx, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("transfer.wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        // B 盘逐 rel 校验(rel 口径与 send 相同:顶层名 = 输入名)
        assert_eq!(
            std::fs::read(dst_b.path().join("直传 目录").join("a.bin")).unwrap(),
            data_a
        );
        assert_eq!(
            std::fs::read(dst_b.path().join("直传 目录").join("sub").join("b.bin")).unwrap(),
            data_b
        );
        assert_eq!(std::fs::read(dst_b.path().join("单件.txt")).unwrap(), data_c);

        // A 源仍在(cut=false 不动源)
        assert!(dir_a.join("a.bin").exists(), "非剪切源保留");
        assert!(single.exists(), "非剪切源保留");

        // X home 目录面不变(无数据落地)
        let after = collect_files(t._home_x.path());
        assert_eq!(
            before, after,
            "X home 不得有数据落地: 新增 {:?} / 消失 {:?}",
            after.difference(&before),
            before.difference(&after)
        );

        // 作业卡 kind=remote_copy
        let lst = call(&t.cx, "transfer.list", json!({})).await.unwrap();
        let job = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("list 含 remote_copy 作业");
        assert_eq!(job["kind"].as_str(), Some("remote_copy"));

        t.shutdown().await;
    });
    run.await.expect("a2b_transfer e2e exceeded 180s");
}

/// A→B cut 收尾:cut=true 的 remote_copy 在 from 端作业 done 后对 A 端
/// ctrl delete 源路径(回收优先)→ A 源消失、B 内容一致。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a2b_cut_deletes_a_sources() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let t = Trio::new("a2bcut@x.y").await;

        let src_a = tempfile::tempdir().unwrap();
        let f = src_a.path().join("剪切 直传.bin");
        let data = fill_random_file(&f, 32 * 1024);
        let dst_b = tempfile::tempdir().unwrap();

        let r = start_remote_copy(
            &t.cx,
            &t.dev_a,
            std::slice::from_ref(&f),
            &t.dev_b,
            dst_b.path(),
            json!({ "cut": true }),
        )
        .await;
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&t.cx, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        assert_eq!(
            std::fs::read(dst_b.path().join("剪切 直传.bin")).unwrap(),
            data,
            "目标侧内容一致"
        );
        assert!(!f.exists(), "cut 成功后 A 源必须被删(回收优先)");

        t.shutdown().await;
    });
    run.await.expect("a2b cut e2e exceeded 180s");
}

/// 取消传播链(download,Task 7 哨 + Task 8 同款口径):X cancel 一进行中
/// 的 download → 源端回显作业短窗内转 cancelled(源端注册表 = ctrl
/// job_query 的同一数据源),且源端事件流在 cancelled 之后静默(驱动被
/// abort,无后续重试轮事件)。确定性设计:等首字节落地后立即 pause(绑定
/// 轮的门挂起,发送端被 QUIC 流控背压冻结)再 cancel——不依赖传输快慢,
/// 源端作业在 cancel_job 落地前必然存活。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_cancel_propagates() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let p = Pair::new("dlcx@x.y").await;
        let src_b = tempfile::tempdir().unwrap();
        let f = src_b.path().join("传播 取消.bin");
        fill_random_file(&f, 128 * 1024 * 1024);
        let dst = tempfile::tempdir().unwrap();

        // 先订两侧事件流(源端 cancelled 帧与静默窗断言依赖不漏订)
        let mut evs_b = p.cb.events();

        let r = start_download(&p.ca, &p.dev_b, &[f], dst.path(), json!({})).await;
        let id = r["job_id"].as_str().unwrap().to_string();

        // 等首字节落地(绑定轮已活跃,取消有实际传播对象)
        let bytes_of = |lst: &Value| -> Option<u64> {
            lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(id.as_str()))
                .map(|j| j["bytes_done"].as_u64().unwrap_or(0))
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let b = bytes_of(&call(&p.ca, "transfer.list", json!({})).await.unwrap());
            if b.unwrap_or(0) > 0 {
                break;
            }
            assert!(Instant::now() < deadline, "下载未在 60s 内进入传输中");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // 立即暂停:绑定轮门挂起 → 发送端被流控背压冻结(剩余量远大于在途
        // 窗,源端作业在取消传播落地前必然存活)
        assert_eq!(
            call(&p.ca, "transfer.pause", json!({ "id": id })).await.unwrap(),
            json!({})
        );

        // X 侧取消
        assert_eq!(
            call(&p.ca, "transfer.cancel", json!({ "id": id })).await.unwrap(),
            json!({})
        );

        // 源端注册表短窗内转 cancelled(= ctrl job_query 读的同一快照面;
        // 传播链 = 本地哨 → ctrl cancel_job → 源端 registry.cancel)
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let lst = call(&p.cb, "transfer.list", json!({})).await.unwrap();
            let st = lst["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"].as_str() == Some(id.as_str()))
                .map(|j| j["state"].as_str().unwrap_or_default().to_string());
            if st.as_deref() == Some("cancelled") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "源端作业未在 15s 内转 cancelled(取消传播链断裂): {st:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // 源端事件流:cancelled 帧到达后 2s 静默窗内不得再有该作业事件
        // (重试轮已被 abort 止步)
        wait_event(&mut evs_b, 15, "源端 cancelled 事件", |f| {
            f.method == "event.transfer"
                && f.params["id"].as_str() == Some(id.as_str())
                && f.params["state"].as_str() == Some("cancelled")
        })
        .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match tokio::time::timeout_at(deadline, evs_b.recv()).await {
                Err(_) => break, // 静默窗耗尽:无后续事件
                Ok(Ok(f)) => assert!(
                    !(f.method == "event.transfer"
                        && f.params["id"].as_str() == Some(id.as_str())),
                    "取消传播后不得再有该作业事件: {f:?}"
                ),
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("事件通道已关闭"),
            }
        }

        // X 侧卡片终态 cancelled(终态粘滞)
        let lst = call(&p.ca, "transfer.list", json!({})).await.unwrap();
        let job = lst["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"].as_str() == Some(id.as_str()))
            .expect("list 含已取消作业");
        assert_eq!(job["state"].as_str(), Some("cancelled"));

        p.shutdown().await;
    });
    run.await.expect("cancel propagate e2e exceeded 180s");
}

/// A→B 冲突策略流:B 目标目录预置同名旧文件 → X 覆盖策略随编排下发 →
/// from 端 upload 驱动把它带进 ConnectRequest → B 接收端清障原位落盘,
/// 内容翻新、无 " (1)" 改名。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a2b_conflict_policy_flows() {
    let _g = case_gate().await;
    let run = tokio::time::timeout(Duration::from_secs(180), async {
        let t = Trio::new("a2bcf@x.y").await;

        let src_a = tempfile::tempdir().unwrap();
        let f = src_a.path().join("dup.txt");
        let data = fill_random_file(&f, 24 * 1024);

        let dst_b = tempfile::tempdir().unwrap();
        std::fs::write(dst_b.path().join("dup.txt"), b"stale-content").unwrap();

        let r = start_remote_copy(
            &t.cx,
            &t.dev_a,
            &[f],
            &t.dev_b,
            dst_b.path(),
            json!({ "overwrite": ["dup.txt"] }),
        )
        .await;
        let id = r["job_id"].as_str().unwrap().to_string();
        let fin = call_bounded(&t.cx, "transfer.wait", json!({ "id": id }), WAIT_TIMEOUT)
            .await
            .expect("wait 应到终态");
        assert_eq!(fin["state"].as_str(), Some("done"), "终态: {fin}");

        assert_eq!(
            std::fs::read(dst_b.path().join("dup.txt")).unwrap(),
            data,
            "同名旧文件必须被新内容顶替"
        );
        assert!(
            !dst_b.path().join("dup (1).txt").exists(),
            "overwrite 命中不得触发冲突改名"
        );

        t.shutdown().await;
    });
    run.await.expect("a2b conflict e2e exceeded 180s");
}
