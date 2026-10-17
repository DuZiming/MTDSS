//! mtdss-daemon:常驻守护进程。
//!
//! `Daemon::start` 在 home 上加载 session/settings 并起 mtdss-ipc 服务端
//! (hello/status);`mtdssd` 二进制(src/main.rs)解析旗标、初始化日志,
//! 起 Daemon 后等待 Ctrl-C 收尾。后续任务在此挂登录(Task 4)与收发(Task 5+)。

mod api;
pub mod session;
pub mod settings;
mod state;

use anyhow::{bail, Result};
use mtdss_ipc::{socket_path, Dispatcher, EventFrame, IpcClient};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
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

/// 限时连接探测:成功返回 true;失败或超时一律按连不上处理。
async fn try_connect(home: &Path) -> bool {
    tokio::time::timeout(CONNECT_TIMEOUT, IpcClient::connect(home))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
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
}

impl Daemon {
    /// 在 `home` 上启动守护进程:加载 session/settings(缺失即默认),
    /// 起 IPC 服务端并等待监听就绪后才返回。同 home 已有守护进程在监听
    /// 时快速失败(socket 占用)。
    pub async fn start(home: &Path, opts: DaemonOpts) -> Result<Daemon> {
        // 预检:同 home 已有守护进程 → 立即报错,而非与既有实例混跑。
        if try_connect(home).await {
            bail!("已有守护进程在监听 {}", socket_path(home));
        }
        let state = Arc::new(DaemonState::load(home, opts.recv_dir)?);
        let (events, _) = broadcast::channel(EVENT_CAP);
        let dispatcher: Arc<dyn Dispatcher> = Arc::new(api::ApiDispatcher::new(state));
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
        })
    }

    /// 停止 IPC 服务端并等待任务收尾(监听随之释放,connect 立即失败)。
    pub async fn shutdown(self) -> Result<()> {
        self.serve_task.abort();
        // 等待中止落地:确保监听句柄已 drop,后续连接必失败。
        let _ = self.serve_task.await;
        Ok(())
    }
}
