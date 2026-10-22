//! 守护进程运行时状态:home、会话、设置与生效接收目录。
//!
//! 读路径(hello/status/devices)无锁化透过接口;写路径(Task 4 登录链路)
//! 经内部互斥锁翻转,锁内不做长阻塞操作。在线循环(Task 6)另经 watch 通知
//! 感知登录翻转、维护目录快照与在线标志,status/devices 直读。传输面
//! (Task 7):send RPC 经发送互斥拒绝并发第二传,recv_dir 可更新并持久化,
//! 编排选项由启动时 apply_test_env 一次性注入。

use anyhow::Result;
use mtdss_ipc::EventFrame;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::ctrl::CtrlPool;
use crate::flow::{RecvOpts, SendOpts};
use crate::hub::SignalHub;
use crate::jobs::Jobs;
use crate::session::Session;
use crate::settings::Settings;

/// 事件方法名:登录态翻转成功后广播(logout 幂等成功也发,UI 据此回引导态)。
const EV_LOGIN: &str = "event.login";

/// devices 单飞备忘的新鲜窗口(M4a Task 5):窗口内并发 devices RPC 直接
/// 复用上一次现拉结果——同屏并发扇出折叠为一次 REST;过窗即重拉。取 1s:
/// 目录变更可见性延迟与 UI 并发节奏同级,远小于现拉 5s 超时,单次过窗成
/// 本可忽略。
pub(crate) const DEVICES_MEMO_TTL: std::time::Duration = std::time::Duration::from_secs(1);

/// devices 单飞备忘槽:(落账时刻, 原始目录列表);None = 尚无可用 memo。
/// 列表存 fetch_devices 的原始返回(非投影快照),复用路径以当下在线态
/// 重投影。
type DevicesMemoSlot = Option<(std::time::Instant, Vec<Value>)>;

/// 待登录暂存:`login.request_code` 成功后记录 server/email/ca,
/// `login.verify` 按 email 取回(客户端无需重传 server 与 CA 路径)。
#[derive(Debug, Clone)]
pub(crate) struct PendingLogin {
    pub(crate) server: String,
    pub(crate) email: String,
    pub(crate) ca: Option<PathBuf>,
}

/// 更新服务句柄的跨平台别名(Task 11):Windows = 真服务(Arc<UpdateService>);
/// 其余平台 = 单元占位(M4 更新仅 Windows——update.rs 整体 `#![cfg(windows)]`
/// 门住,update.* RPC 在 api 层回 UNSUPPORTED_PLATFORM,占位永不被调用)。
#[cfg(windows)]
pub(crate) type UpdateSvc = std::sync::Arc<crate::update::UpdateService>;
#[cfg(not(windows))]
pub(crate) type UpdateSvc = ();

/// 守护进程共享状态:IPC 分派层经 `Arc<DaemonState>` 访问。
pub(crate) struct DaemonState {
    /// 设备数据目录(MTDSS_HOME 或 `$HOME/.mtdss`)。
    home: PathBuf,
    /// 已登记会话;None = 未登录。登录/登出写路径经互斥锁翻转。
    session: Mutex<Option<Session>>,
    /// settings.json 内容(读写经互斥锁;set_recv_dir 更新 recv_dir 字段
    /// 并持久化,set_autostart 同理接 autostart 字段,set_update_check/
    /// set_update_install 接 update_* 字段)。
    settings: Mutex<Settings>,
    /// 生效接收目录(RPC set_recv_dir 可更新;常驻接收每轮现读,不捕获一次)。
    recv_dir: Mutex<PathBuf>,
    /// 发送互斥:send RPC try_lock,占用即拒(并发第二传 code 2),不排队。
    send_lock: tokio::sync::Mutex<()>,
    /// daemon 启动时由 apply_test_env 一次性注入的发送编排选项
    /// (MTDSS_TEST_* 面收口;send RPC 每次取用)。
    send_opts: SendOpts,
    /// 同上的接收编排选项(常驻接收循环每轮取用)。
    recv_opts: RecvOpts,
    /// 事件广播发送端(与 Daemon/ipc serve 共享同一 channel;send RPC 的
    /// event.send_log 经它广播)。
    events: tokio::sync::broadcast::Sender<EventFrame>,
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
    /// devices 单飞备忘(1s TTL,DEVICES_MEMO_TTL):并发 devices RPC 在
    /// 锁上排队,队首持锁现拉并落 memo,后来者醒来见新鲜 memo 即复用
    /// (一次 REST 服务整屏并发)。存原始目录列表而非投影快照:复用路径
    /// 以当下在线态重投影(本机条目 online 恒取本地连接态)。失败/超时不
    /// 落 memo(无负缓存,下一调用重试现拉)。tokio Mutex:持锁跨 await,
    /// 现拉撞外层 5s 超时被取消时 guard 随之释放,排队者不死锁。Arc:经
    /// 访问器克隆句柄出借,guard 不借 &self。
    devices_memo: Arc<tokio::sync::Mutex<DevicesMemoSlot>>,
    /// 信令在线标志(status 读;在线循环翻转)。
    online: AtomicBool,
    /// 登录态变更通知:登录/登出翻转时 send,在线循环据此上线/回等。
    /// watch 无丢唤醒问题(send 标记全部接收端,迟到的 changed() 立即返回)。
    login_tx: tokio::sync::watch::Sender<bool>,
    login_rx: tokio::sync::watch::Receiver<bool>,
    /// 信令枢纽:全进程唯一信令连接的代际与公钥缓存(在线循环独占建连,
    /// 发送流/常驻接收经它复用 relay/订阅/STUN 通告)。
    hub: SignalHub,
    /// 控制通道连接池(全进程唯一,构造期建):RPC list/stat/delete/
    /// move/copy 的远端面经它打洞;登出成功路径 reset 清池拆卸(端点随
    /// 条目 drop,连接一并关闭——登出后旧连接/旧打洞会话不得复用)。
    /// 池不反持 state(无 Arc 环),生命周期随 state。
    ctrl: Arc<CtrlPool>,
    /// 传输作业注册表(M3c Task 6):transfer.* RPC 与驱动任务的控制面。
    /// 作业不经 send 互斥(并发合法);驱动任务自持 state 的 Arc,注册表
    /// 只持句柄,无环。
    jobs: Jobs,
    /// 更新服务(M4b Task 11):update.* RPC 的执行体与后台检查循环的状态。
    /// Windows = 真服务;非 Windows 单元占位(见 [`UpdateSvc`])。配置由
    /// Daemon::start 装配(生产 env 推导 / 测试注入),此处只持句柄。
    /// 非 Windows 占位无读取点(消费面全在 Windows),按平台保留 allow。
    #[cfg_attr(not(windows), allow(dead_code))]
    update: UpdateSvc,
}

impl DaemonState {
    /// 从 home 加载 session.json 与 settings.json:两者缺失即取默认
    /// (全新 home = 未登录 + 默认设置),存在但损坏则硬错误。编排选项、
    /// 事件广播发送端与更新服务由 Daemon::start 构造后传入。
    pub(crate) fn load(
        home: &Path,
        recv_dir: PathBuf,
        send_opts: SendOpts,
        recv_opts: RecvOpts,
        events: tokio::sync::broadcast::Sender<EventFrame>,
        update: UpdateSvc,
    ) -> Result<Self> {
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
            settings: Mutex::new(settings),
            recv_dir: Mutex::new(recv_dir),
            send_lock: tokio::sync::Mutex::new(()),
            send_opts,
            recv_opts,
            events,
            pending_login: Mutex::new(None),
            login_lock: tokio::sync::Mutex::new(()),
            devices: Mutex::new(None),
            devices_memo: Arc::new(tokio::sync::Mutex::new(None)),
            online: AtomicBool::new(false),
            login_tx,
            login_rx,
            hub: SignalHub::new(),
            ctrl: Arc::new(CtrlPool::new()),
            jobs: Jobs::new(),
            update,
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
        self.recv_dir.lock().unwrap().clone()
    }

    /// 更新接收目录:先以更新副本落盘,成功后才翻转内存(settings 与
    /// recv_dir 一起换)——落盘失败即整体 Err,内存两处均不动,真正消除
    /// 「内存已改、磁盘仍旧值」的半程态(旧实现先换内存后落盘,注释却声称
    /// 一致,实为相反;写序硬化即修此)。常驻接收每轮现读目录,下一来件
    /// 自然用新目录(单连接收敛后无重启通知机制)。
    pub(crate) fn set_recv_dir(&self, dir: PathBuf) -> Result<()> {
        {
            let mut guard = self.settings.lock().unwrap();
            let updated = {
                let mut u = guard.clone();
                u.recv_dir = Some(dir.to_string_lossy().into_owned());
                u
            };
            // 落盘失败:Err 直接出栈,guard 所指内存未翻转
            updated.save(&self.home)?;
            *guard = updated;
        }
        let mut guard = self.recv_dir.lock().unwrap();
        *guard = dir;
        Ok(())
    }

    /// 当前开机自启设置(settings 为权威;Windows 上 set 成功后注册表同步)。
    pub(crate) fn autostart(&self) -> bool {
        self.settings.lock().unwrap().autostart
    }

    /// 设置开机自启:先落注册表(Windows;失败即整体回退,设置不动),
    /// 再更新 settings 并持久化。RPC `autostart.set` 与托盘「开机自启」
    /// 勾选共用此路径,语义完全一致。
    pub(crate) fn set_autostart(&self, enabled: bool) -> Result<()> {
        crate::api::registry_set_autostart(enabled)?;
        {
            let mut settings = self.settings.lock().unwrap();
            settings.autostart = enabled;
            settings.save(&self.home)?;
        }
        Ok(())
    }

    /// 当前更新自动检查开关(D8 默认开;settings 为权威)。
    pub(crate) fn update_auto_check(&self) -> bool {
        self.settings.lock().unwrap().update_auto_check
    }

    /// 当前更新自动安装开关(D8 默认关;仅在 auto_check 开启时有意义)。
    pub(crate) fn update_auto_install(&self) -> bool {
        self.settings.lock().unwrap().update_auto_install
    }

    /// 设置更新自动检查:仅持久化(set_autostart 的注册表面在此不涉及)。
    /// D4 父子耦合收口在 daemon 一处:关闭检查连带强制 auto_install=false
    /// ——内存与磁盘一次落定,不留「检查已关、安装仍开」的半程态。
    pub(crate) fn set_update_check(&self, enabled: bool) -> Result<()> {
        let mut settings = self.settings.lock().unwrap();
        settings.update_auto_check = enabled;
        if !enabled {
            settings.update_auto_install = false;
        }
        settings.save(&self.home)?;
        Ok(())
    }

    /// 设置更新自动安装:仅持久化。耦合方向单向(关检查 ⇒ 关安装),开启
    /// 安装不反向拉起检查——UI 只在检查开启时暴露该开关,RPC 面的矛盾组合
    /// 由 api 层先拒(update.prefs.set)。
    pub(crate) fn set_update_install(&self, enabled: bool) -> Result<()> {
        let mut settings = self.settings.lock().unwrap();
        settings.update_auto_install = enabled;
        settings.save(&self.home)?;
        Ok(())
    }

    /// 尝试获取发送互斥;占用返回 None(send RPC 据此立即回 code 2,不排队)。
    pub(crate) fn try_send_permit(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.send_lock.try_lock().ok()
    }

    /// 发送编排选项(启动时注入,Copy 取用)。
    pub(crate) fn send_opts(&self) -> SendOpts {
        self.send_opts
    }

    /// 接收编排选项(启动时注入,常驻接收每轮取用)。
    pub(crate) fn recv_opts(&self) -> RecvOpts {
        self.recv_opts
    }

    /// 事件广播发送端副本(send RPC 的 event.send_log 经它广播)。
    pub(crate) fn events(&self) -> tokio::sync::broadcast::Sender<EventFrame> {
        self.events.clone()
    }

    /// 信令枢纽(发送流/常驻接收经它复用在线循环的唯一连接代际)。
    pub(crate) fn hub(&self) -> &SignalHub {
        &self.hub
    }

    /// 控制通道连接池(全局唯一实例;ApiDispatcher 与登出拆卸共用)。
    pub(crate) fn ctrl(&self) -> Arc<CtrlPool> {
        self.ctrl.clone()
    }

    /// 传输作业注册表(transfer.* RPC 面;驱动任务经它 spawn/取消)。
    pub(crate) fn jobs(&self) -> &Jobs {
        &self.jobs
    }

    /// 更新服务句柄(Task 11;仅 Windows 有真实现——后台循环经它 run,
    /// api 层 update.* 臂经它 check/state/apply;非 Windows 无调用方)。
    #[cfg(windows)]
    pub(crate) fn update(&self) -> std::sync::Arc<crate::update::UpdateService> {
        self.update.clone()
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
        // 换账号即弃备忘(M4a 终审 M-3):旧账号目录 memo 复用有 ≤1s 跨账号
        // 陈旧窗口,登录翻转处一并清掉(try_lock 不阻塞,在途现拉例外无害)。
        if let Ok(mut memo) = self.devices_memo.try_lock() {
            *memo = None;
        }
        // 唤醒在线循环上线。send 恒通知(重复值亦然),循环以 logged_in()
        // 为准,伪唤醒无害。
        let _ = self.login_tx.send(true);
        // 翻转成功后广播 event.login(UI 据此全量刷新;失败路径不发)。
        let _ = self.events.send(EventFrame {
            method: EV_LOGIN.into(),
            params: json!({ "logged_in": true }),
        });
        Ok(())
    }

    /// 登出:删 session.json(不存在视为幂等成功)+ 内存会话清空 + ctrl
    /// 连接池拆卸(reset 清池,端点随条目 drop、连接一并关闭——登出后
    /// 旧连接不得复用)+ 唤醒在线循环回等登录态。
    pub(crate) fn logout(&self) -> Result<()> {
        match std::fs::remove_file(self.home.join("session.json")) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        *self.session.lock().unwrap() = None;
        // 登出同样弃备忘(M4a 终审 M-3):会话已清,旧 memo 不得跨账号复用。
        if let Ok(mut memo) = self.devices_memo.try_lock() {
            *memo = None;
        }
        // 池拆卸在会话清空之后、登录通知之前:后续 RPC 的远端面请求会先撞
        // 未登录短路失败,即使有在途请求也只拿到「未登录」,绝不复用旧连接。
        self.ctrl.reset();
        let _ = self.login_tx.send(false);
        // 翻转成功后广播 event.login false(UI 回引导态;幂等登出同样发)。
        let _ = self.events.send(EventFrame {
            method: EV_LOGIN.into(),
            params: json!({ "logged_in": false }),
        });
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

    /// 非终态传输作业计数 watch(Task 13,托盘 tooltip「N 个传输中」数据
    /// 源):建队 +1 / 终态 -1,jobs 注册表内 CountWatch 广播。唯一消费者
    /// 是 Windows 托盘线程——非 Windows 编译无消费点,按平台门住。
    #[cfg(windows)]
    pub(crate) fn jobs_count_rx(&self) -> tokio::sync::watch::Receiver<usize> {
        self.jobs.count_rx()
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
    /// event.online true)。单连接收敛后在线循环的连接即全进程唯一信令
    /// 连接,接收面订阅枢纽,无需重启通知。
    pub(crate) fn mark_online(&self) -> bool {
        !self.online.swap(true, Ordering::SeqCst)
    }

    /// 置离线并返回是否发生 true→false 转换(真下线;断连退避的重复收尾
    /// 不再重复发 event.online false)。
    pub(crate) fn mark_offline(&self) -> bool {
        self.online.swap(false, Ordering::SeqCst)
    }

    /// devices 单飞备忘句柄(api devices RPC 经它锁内查新/现拉/落 memo;
    /// 克隆 Arc 出借,guard 不借 &self)。
    pub(crate) fn devices_memo(&self) -> Arc<tokio::sync::Mutex<DevicesMemoSlot>> {
        self.devices_memo.clone()
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
