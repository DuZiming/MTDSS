//! 守护进程运行时状态:home、会话、设置与生效接收目录。
//!
//! 本任务状态在启动时加载、运行期只读;Task 4 引入登录写路径后再按需
//! 加内部锁(计划中的「内部锁」届时落地)。

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::session::Session;
use crate::settings::Settings;

/// 守护进程共享状态:IPC 分派层经 `Arc<DaemonState>` 只读访问。
pub(crate) struct DaemonState {
    /// 设备数据目录(MTDSS_HOME 或 `$HOME/.mtdss`)。
    home: PathBuf,
    /// 已登记会话;None = 未登录(home 下无 session.json)。
    session: Option<Session>,
    /// settings.json 内容(缺失即默认);本任务仅随状态留存,Task 7(recv_dir
    /// 覆盖)/Task 9(autostart)经写路径启用。
    #[allow(dead_code)]
    settings: Settings,
    /// 生效接收目录;DaemonOpts 显式传入,settings/默认的收口解析在入口完成。
    recv_dir: PathBuf,
}

impl DaemonState {
    /// 从 home 加载 session.json 与 settings.json:两者缺失即取默认
    /// (全新 home = 未登录 + 默认设置),存在但损坏则硬错误。
    pub(crate) fn load(home: &Path, recv_dir: PathBuf) -> Result<Self> {
        let session = if home.join("session.json").exists() {
            Some(Session::load(home)?)
        } else {
            None
        };
        let settings = Settings::load(home)?;
        Ok(Self {
            home: home.to_path_buf(),
            session,
            settings,
            recv_dir,
        })
    }

    /// 是否已登录(session.json 存在且解析成功)。
    pub(crate) fn logged_in(&self) -> bool {
        self.session.is_some()
    }

    /// 已登记会话副本;None = 未登录。
    pub(crate) fn session(&self) -> Option<Session> {
        self.session.clone()
    }

    /// 生效接收目录。
    pub(crate) fn recv_dir(&self) -> PathBuf {
        self.recv_dir.clone()
    }

    /// 设备数据目录副本。
    #[allow(dead_code)] // Task 4(登录)/Task 5(flow)启用
    pub(crate) fn home(&self) -> PathBuf {
        self.home.clone()
    }
}
