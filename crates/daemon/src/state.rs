//! 守护进程运行时状态:home、会话、设置与生效接收目录。
//!
//! 读路径(hello/status/devices)无锁化透过接口;写路径(Task 4 登录链路)
//! 经内部互斥锁翻转,锁内不做长阻塞操作。在线循环(Task 6)另经 watch 通知
//! 感知登录翻转、维护目录快照与在线标志,status/devices 直读。

use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::session::Session;
use crate::settings::Settings;

/// 待登录暂存:`login.request_code` 成功后记录 server/email/ca,
/// `login.verify` 按 email 取回(客户端无需重传 server 与 CA 路径)。
#[derive(Debug, Clone)]
pub(crate) struct PendingLogin {
    pub(crate) server: String,
    pub(crate) email: String,
    pub(crate) ca: Option<PathBuf>,
}

/// 守护进程共享状态:IPC 分派层经 `Arc<DaemonState>` 访问。
pub(crate) struct DaemonState {
    /// 设备数据目录(MTDSS_HOME 或 `$HOME/.mtdss`)。
    home: PathBuf,
    /// 已登记会话;None = 未登录。登录/登出写路径经互斥锁翻转。
    session: Mutex<Option<Session>>,
    /// settings.json 内容(缺失即默认);Task 7(recv_dir 覆盖)/Task 9
    /// (autostart)经写路径启用。
    #[allow(dead_code)]
    settings: Settings,
    /// 生效接收目录;DaemonOpts 显式传入,settings/默认的收口解析在入口完成。
    recv_dir: PathBuf,
    /// 待登录暂存(单槽:同一时刻只跟踪一次进行中的登录;重复 request_code
    /// 覆盖旧记录)。
    pending_login: Mutex<Option<PendingLogin>>,
    /// 登录互斥:login.verify 全程(身份生成/加载 → REST → 会话落盘翻转)
    /// 与 logout 串行化。并发 verify 在无 seed 的 home 上各自生成身份会让
    /// 磁盘 identity.seed 与 session.json 分歧(后写者胜),持锁全程闭合。
    /// 用 tokio Mutex:verify 体内有 await,std Mutex 的 guard 不能跨 await。
    login_lock: tokio::sync::Mutex<()>,
    /// 设备目录快照缓存(在线循环维护:连接建立全量 + Presence 增量;
    /// devices RPC 直读,None = 尚无快照)。
    devices: Mutex<Option<Vec<Value>>>,
    /// 信令在线标志(status 读;在线循环翻转)。
    online: AtomicBool,
    /// 登录态变更通知:登录/登出翻转时 send,在线循环据此上线/回等。
    /// watch 无丢唤醒问题(send 标记全部接收端,迟到的 changed() 立即返回)。
    login_tx: tokio::sync::watch::Sender<bool>,
    login_rx: tokio::sync::watch::Receiver<bool>,
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
        let logged_in = session.is_some();
        let (login_tx, login_rx) = tokio::sync::watch::channel(logged_in);
        Ok(Self {
            home: home.to_path_buf(),
            session: Mutex::new(session),
            settings,
            recv_dir,
            pending_login: Mutex::new(None),
            login_lock: tokio::sync::Mutex::new(()),
            devices: Mutex::new(None),
            online: AtomicBool::new(false),
            login_tx,
            login_rx,
        })
    }

    /// 是否已登录(session.json 存在且解析成功)。
    pub(crate) fn logged_in(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }

    /// 已登记会话副本;None = 未登录。
    pub(crate) fn session(&self) -> Option<Session> {
        self.session.lock().unwrap().clone()
    }

    /// 生效接收目录。
    pub(crate) fn recv_dir(&self) -> PathBuf {
        self.recv_dir.clone()
    }

    /// 设备数据目录副本。
    pub(crate) fn home(&self) -> PathBuf {
        self.home.clone()
    }

    /// 记录待登录(request_code 成功后调用;同槽覆盖)。
    pub(crate) fn set_pending_login(&self, p: PendingLogin) {
        *self.pending_login.lock().unwrap() = Some(p);
    }

    /// 按 email 窥视待登录记录(不移除):verify 失败(如验证码输错)后
    /// 记录保留,同一次 request_code 的记录内可重试正确验证码。
    pub(crate) fn pending_login_for(&self, email: &str) -> Option<PendingLogin> {
        self.pending_login
            .lock()
            .unwrap()
            .as_ref()
            .filter(|p| p.email == email)
            .cloned()
    }

    /// 按 email 取走待登录记录(仅成功登录后调用)。
    pub(crate) fn take_pending_login_for(&self, email: &str) -> Option<PendingLogin> {
        let mut slot = self.pending_login.lock().unwrap();
        if slot.as_ref().map(|p| p.email == email).unwrap_or(false) {
            slot.take()
        } else {
            None
        }
    }

    /// 登录成功:session.json 落盘 + 内存会话置位(hello/status 随之翻转)。
    /// 落盘失败(磁盘满/权限)则内存不翻转,状态与磁盘保持一致。
    pub(crate) fn login(&self, s: Session) -> Result<()> {
        s.save(&self.home)?;
        *self.session.lock().unwrap() = Some(s);
        // 唤醒在线循环上线。send 恒通知(重复值亦然),循环以 logged_in()
        // 为准,伪唤醒无害。
        let _ = self.login_tx.send(true);
        Ok(())
    }

    /// 登出:删 session.json(不存在视为幂等成功)+ 内存会话清空 + 唤醒
    /// 在线循环回等登录态。
    pub(crate) fn logout(&self) -> Result<()> {
        match std::fs::remove_file(self.home.join("session.json")) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        *self.session.lock().unwrap() = None;
        let _ = self.login_tx.send(false);
        Ok(())
    }

    /// 令牌轮换专用(Task 6 在线循环):session.json 落盘 + 内存替换,不动
    /// 登录通知(登录态未变)。全程持会话槽锁与 logout 串行:登出后到达的
    /// 轮换结果不得复活会话(磁盘与内存一致地保持登出)。
    pub(crate) fn update_session(&self, s: Session) -> Result<()> {
        let mut guard = self.session.lock().unwrap();
        if guard.is_none() {
            tracing::info!("令牌轮换结果被丢弃:已登出");
            return Ok(());
        }
        s.save(&self.home)?;
        *guard = Some(s);
        Ok(())
    }

    /// 取登录互斥锁;guard 持有期间,其他 login.verify / login.logout 排队。
    /// verify 必须从身份生成到会话落盘翻转全程持锁,logout 同锁,登录态的
    /// 磁盘与内存翻转因此原子。
    pub(crate) async fn lock_login(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.login_lock.lock().await
    }

    /// 登录态变更通知的接收端(在线循环持有)。
    pub(crate) fn login_watch(&self) -> tokio::sync::watch::Receiver<bool> {
        self.login_rx.clone()
    }

    /// 本机 device_id(未登录为 None)。
    pub(crate) fn self_device_id(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.device_id.clone())
    }

    /// 当前信令在线标志(status 读)。
    pub(crate) fn online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    /// 置在线并返回是否发生 false→true 转换(真上线,调用方据此只发一次
    /// event.online true)。
    pub(crate) fn mark_online(&self) -> bool {
        !self.online.swap(true, Ordering::SeqCst)
    }

    /// 置离线并返回是否发生 true→false 转换(真下线;断连退避的重复收尾
    /// 不再重复发 event.online false)。
    pub(crate) fn mark_offline(&self) -> bool {
        self.online.swap(false, Ordering::SeqCst)
    }

    /// 当前目录快照副本;None = 尚无快照。
    pub(crate) fn devices(&self) -> Option<Vec<Value>> {
        self.devices.lock().unwrap().clone()
    }

    /// 整体替换目录快照(在线循环:连接建立/未知设备的全量刷新时)。
    pub(crate) fn set_devices(&self, list: Vec<Value>) {
        *self.devices.lock().unwrap() = Some(list);
    }

    /// Presence 增量:已知设备就地翻转 online 标志并返回 true;未知设备
    /// (无快照或不在目录)返回 false,调用方走全量刷新补齐。
    pub(crate) fn set_device_online(&self, device_id: &str, online: bool) -> bool {
        let mut guard = self.devices.lock().unwrap();
        let Some(list) = guard.as_mut() else {
            return false;
        };
        match list
            .iter_mut()
            .find(|d| d["device_id"].as_str() == Some(device_id))
        {
            Some(d) => {
                d["online"] = Value::from(online);
                true
            }
            None => false,
        }
    }

    /// 翻转缓存里本机条目的 online 标志(断连保条目置 false,devices RPC
    /// 因此在离线期间仍自洽);无快照或会话已清即无操作。
    pub(crate) fn set_self_online(&self, online: bool) {
        let Some(self_id) = self.self_device_id() else {
            return;
        };
        let mut guard = self.devices.lock().unwrap();
        let Some(list) = guard.as_mut() else {
            return;
        };
        if let Some(d) = list
            .iter_mut()
            .find(|d| d["device_id"].as_str() == Some(self_id.as_str()))
        {
            d["online"] = Value::from(online);
        }
    }
}
