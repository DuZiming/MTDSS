//! mtdss-daemon:常驻守护进程。
//!
//! `Daemon::start` 在 home 上加载 session/settings,起 mtdss-ipc 服务端
//! (hello/status/devices/login.*/send/set_recv_dir)、在线循环(未登录自
//! 挂起,登录翻转即上线,维持信令长连与目录快照广播)与常驻接收循环
//! (登录后拉起,永久等来件);`mtdssd` 二进制(src/main.rs)解析旗标、
//! 初始化日志,起 Daemon 后等待 Ctrl-C 收尾。测试注入面:启动时以
//! `|k| std::env::var(k).ok()` 跑 apply_test_env,两侧编排选项收口进 state
//! (nat-matrix 等经 mtdssd 进程环境传参)。

mod account;
mod api;
pub mod flow;
mod online;
pub mod session;
pub mod settings;
mod state;

use anyhow::{bail, Context, Result};
use mtdss_ipc::{socket_path, Dispatcher, EventFrame, IpcClient};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::state::DaemonState;

/// 守护进程事件广播容量(慢消费者由 ipc 层按连接断开,不拖垮他人)。
const EVENT_CAP: usize = 256;
/// start 返回前等待监听就绪的上限。
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// 监听就绪轮询间隔。
const READY_POLL: Duration = Duration::from_millis(25);
/// 单次连接尝试上限:interprocess 对 ERROR_PIPE_BUSY 会无界自旋等待,
/// 显式限时防悬挂(命中即按「连不上」处理)。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// 排他启动锁文件名(home 下)。
const LOCK_FILE: &str = "daemon.lock";
/// 锁已存在时确认「陈旧锁」的观察窗:窗内探到活 daemon 即让路,
/// 持续探不到(启动方崩溃残留)才清除重试。须大于典型启动耗时。
const STALE_CONFIRM_WINDOW: Duration = Duration::from_secs(2);
/// 常驻接收任务关停的协作宽限:先发 watch 关停信号给退出窗,超时再 abort
/// 兜底(轮循环在每个 await 点都能感知关停,正常远快于窗)。
const RECV_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// 限时连接探测:成功返回 true;失败或超时一律按连不上处理。
async fn try_connect(home: &Path) -> bool {
    tokio::time::timeout(CONNECT_TIMEOUT, IpcClient::connect(home))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// 窗口期内轮询探活:任一次连上即 true,窗口耗尽仍连不上则 false。
/// 逐次 try_connect 之间让出调度(睡眠),同线程运行时下其他任务
/// (如正在启动的服务端)得以推进。
async fn wait_until_connectable(home: &Path, budget: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if try_connect(home).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// 锁文件落一行 pid + 启动时间,便于残留锁排障(失败仅忽略)。
fn write_lock_pid_line(mut file: &std::fs::File) {
    use std::io::Write as _;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "pid={} started_unix={}", std::process::id(), secs);
}

/// 跨平台排他启动锁:`File::create_new`(O_EXCL)保证同 home 恰好一个
/// 持有者,File 由 Daemon 持有至 shutdown(届时删文件)。
/// 锁已存在时分流:观察窗内探到活 daemon → 报「已在运行」;持续探不到
/// (启动方崩溃残留的陈旧锁)→ 清除后重试一次,再被抢即报「已在运行」。
/// 单发探活会把「正在启动中」误读为「陈旧」,故陈旧判定须经观察窗确认。
async fn acquire_start_lock(home: &Path) -> Result<std::fs::File> {
    let path = home.join(LOCK_FILE);
    std::fs::create_dir_all(home).with_context(|| format!("create dir {}", home.display()))?;
    match std::fs::File::create_new(&path) {
        Ok(file) => {
            write_lock_pid_line(&file);
            Ok(file)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if wait_until_connectable(home, STALE_CONFIRM_WINDOW).await {
                bail!("已有守护进程在监听 {}", socket_path(home));
            }
            // 陈旧锁:清除后重试一次(已消失亦无妨);再被并发者抢走则让路
            let _ = std::fs::remove_file(&path);
            match std::fs::File::create_new(&path) {
                Ok(file) => {
                    write_lock_pid_line(&file);
                    Ok(file)
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    bail!("已有守护进程在监听 {}", socket_path(home));
                }
                Err(err) => {
                    Err(err).with_context(|| format!("create {}", path.display()))
                }
            }
        }
        Err(err) => Err(err).with_context(|| format!("create {}", path.display())),
    }
}

/// 守护进程启动选项。
pub struct DaemonOpts {
    /// 接收目录。
    pub recv_dir: PathBuf,
    /// 是否启用系统托盘(本任务未实现,占位;Task 9 接线)。
    pub tray: bool,
}

impl Default for DaemonOpts {
    fn default() -> Self {
        Self {
            recv_dir: mtdss_ipc::mtdss_home().join("inbox"),
            tray: false,
        }
    }
}

/// 守护进程句柄:事件广播与 IPC 服务端任务。
pub struct Daemon {
    /// 推送事件通道(向所有连接广播)。
    pub events: broadcast::Sender<EventFrame>,
    /// IPC 服务端任务句柄(shutdown 时中止并等待收尾)。
    serve_task: JoinHandle<()>,
    /// 在线循环任务句柄(shutdown 时中止;未登录自挂起,登录翻转即上线)。
    online_task: JoinHandle<()>,
    /// 常驻接收任务句柄(shutdown 先发协作信号,宽限后 abort 兜底)。
    recv_task: JoinHandle<()>,
    /// 常驻接收的协作关停信号(shutdown 置 true,轮循环在每个 await 点感知)。
    recv_shutdown: watch::Sender<bool>,
    /// 设备数据目录(shutdown 时据其删除启动锁)。
    home: PathBuf,
    /// 排他启动锁句柄:持有即占用;shutdown 时随 self 释放并删锁文件。
    _start_lock: std::fs::File,
}

impl Daemon {
    /// 在 `home` 上启动守护进程:先取排他启动锁(并发/重复启动恰好一胜,
    /// 活实例在场即报错),再加载 session/settings(缺失即默认),起 IPC
    /// 服务端、在线循环与常驻接收,并等待监听就绪后才返回。在线循环与
    /// 常驻接收未登录自挂起,已登录(session.json 在场,如重启场景)即
    /// 直接上线/开轮。
    pub async fn start(home: &Path, opts: DaemonOpts) -> Result<Daemon> {
        // 排他启动锁:探针+spawn 之间的竞速窗口由此闭合(锁在 serve 之前)。
        let start_lock = acquire_start_lock(home).await?;
        // 测试注入面:MTDSS_TEST_* 旋钮在本进程内一次性读定,两侧编排选项
        // 收口进 state(旧 CLI 的按进程注入由此取代,nat-matrix 把 env 传给
        // mtdssd 子进程即可)。
        let mut send_opts = flow::SendOpts::default();
        let mut recv_opts = flow::RecvOpts::default();
        flow::apply_test_env(&mut send_opts, &mut recv_opts, |k| std::env::var(k).ok());
        let (events, _) = broadcast::channel(EVENT_CAP);
        let state = Arc::new(DaemonState::load(
            home,
            opts.recv_dir,
            send_opts,
            recv_opts,
            events.clone(),
        )?);
        let dispatcher: Arc<dyn Dispatcher> = Arc::new(api::ApiDispatcher::new(state.clone()));
        // 在线循环:自等登录(未登录挂起在 state 的 watch 通知上),登录翻转
        // 即上线并常驻;维持目录快照与 event.online/event.devices 广播。
        let online_task = tokio::spawn(online::run(state.clone(), events.clone()));
        // 常驻接收:登录翻转即开轮,永久等来件;协作关停信号 + abort 兜底。
        let (recv_shutdown, recv_shutdown_rx) = watch::channel(false);
        let recv_task = tokio::spawn(flow::resident_recv_loop(
            state.clone(),
            recv_shutdown_rx,
        ));
        let serve_home = home.to_path_buf();
        let serve_events = events.clone();
        let serve_task = tokio::spawn(async move {
            if let Err(err) = mtdss_ipc::serve(&serve_home, dispatcher, serve_events).await {
                tracing::error!(%err, "IPC 服务端退出");
            }
        });
        // 轮询连接确认监听就绪,保证 start 返回后立即可连(时序确定)。
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if tokio::time::Instant::now() >= deadline {
                // 启动失败路径:三个任务一并中止,不留孤儿循环
                online_task.abort();
                recv_task.abort();
                serve_task.abort();
                bail!("守护进程监听未就绪(超时 {})", socket_path(home));
            }
            if try_connect(home).await {
                break;
            }
            tokio::time::sleep(READY_POLL).await;
        }
        Ok(Daemon {
            events,
            serve_task,
            online_task,
            recv_task,
            recv_shutdown,
            home: home.to_path_buf(),
            _start_lock: start_lock,
        })
    }

    /// 停止常驻接收、在线循环与 IPC 服务端并等待任务收尾(监听随之释放,
    /// connect 立即失败),最后删除启动锁。等待收尾后删锁,避免后来者把活
    /// 启动误判为陈旧锁。常驻接收先停:不再有落盘/信令动作;先发协作信号
    /// 给宽限窗(轮循环在 await 点感知即自行收尾),超时 abort 兜底,关闭
    /// 不挂任务收尾。
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.recv_shutdown.send(true);
        let mut recv_task = self.recv_task;
        // 协作宽限:轮循环在每个 await 点感知关停信号即自行收尾;超时才
        // abort 兜底(不留泄漏任务,也不把收尾挂在任务卡住的分支上)。
        // 注意 JoinHandle 不得在完成后二次 poll(panic),完成态直接取用。
        match tokio::time::timeout(RECV_SHUTDOWN_GRACE, &mut recv_task).await {
            Ok(join) => {
                if let Err(err) = join {
                    if !err.is_cancelled() {
                        tracing::warn!(%err, "常驻接收任务收尾异常");
                    }
                }
            }
            Err(_) => {
                recv_task.abort();
                if let Err(err) = recv_task.await {
                    // 取消是 abort 的预期路径;panic 等异常收尾必须留痕。
                    if !err.is_cancelled() {
                        tracing::warn!(%err, "常驻接收任务收尾异常");
                    }
                }
            }
        }
        self.online_task.abort();
        if let Err(err) = self.online_task.await {
            // 取消是 abort 的预期路径;panic 等异常收尾必须留痕,不容吞掉。
            if !err.is_cancelled() {
                tracing::warn!(%err, "在线循环任务收尾异常");
            }
        }
        self.serve_task.abort();
        // 等待中止落地:确保监听句柄已 drop,后续连接必失败。
        // 取消是 abort 的预期路径;panic 等异常收尾必须留痕,不容吞掉。
        if let Err(err) = self.serve_task.await {
            if !err.is_cancelled() {
                tracing::warn!(%err, "IPC 服务端任务收尾异常");
            }
        }
        if let Err(err) = std::fs::remove_file(self.home.join(LOCK_FILE)) {
            tracing::warn!(%err, "删除启动锁失败");
        }
        Ok(())
    }
}
