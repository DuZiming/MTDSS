//! mtdss-daemon:常驻守护进程。
//!
//! `Daemon::start` 在 home 上加载 session/settings,起 mtdss-ipc 服务端
//! (监督循环护航:serve 异常返回即 1s 重拉,见 supervise_serve;hello/
//! status/devices[现拉+缓存回退,Task 7]/login.*/send/set_recv_dir/
//! autostart.*/list/stat/delete/move/copy/transfer.*[upload/download/
//! local_copy/remote_copy,Task 6/7/8])、在线循环(未登录自挂起,登录翻转即上线,维持
//! 信令长连与目录快照广播)、常驻接收循环(登录后拉起,永久等来件;来件
//! 轮按 ConnectRequest 的 job_id 绑定下载作业或落匿名 incoming 卡,进度/
//! 暂停/取消经 RoundHandle 直达,Task 7)、更新后台循环(M4b Task 11/12,
//! 仅 Windows:启动即首查 + 24h 定时,auto_check 门 + auto_install 档 +
//! 通知档,pending_install 的作业终态/定时重试;apply 至 Installing 序列
//! ——标记/事件/等客户端归零/detached spawn/退出请求——并于开机消费更新
//! 标记:等版本「已更新到 X」通知、未完成告警清理、restart-ui 主窗唤回)、
//! ctrl 响应端(控制通道:远端
//! 浏览/操作/编排请求的打洞响应侧,M3b/M3c)与 fdpass 通道(unix,
//! v2-P3 T4:`{home}/fds.sock` 上 SCM_RIGHTS 的 fd 桥服务端,0x01
//! source_fd 交付 jobs 的 SourceResolver——bridge 形态上传的逐文件
//! fs.request_source 应答面;P4a 起查询通道二合一:0x04 在场查询与 0x05
//! 发布应答交付 DestQueries(收件私有暂存的 final_state 门/发布协调器),
//! v2-P3 的 0x02/0x03 随收件 fd 桥退役;监督循环 1s 重拉自愈);Windows 且未 --no-tray 时
//! 另起专属托盘线程(Task 9,登录态换图标 + 右键菜单:唤起/聚焦主窗
//! (uilaunch)、登录/登出、开机自启与优雅退出,退出请求经 quit_watch
//! 交回调用方与 Ctrl-C 同台 select)。`mtdssd` 二进制(src/main.rs)解析
//! 旗标、初始化日志,起 Daemon 后等待收尾信号。测试注入面:启动时以
//! `|k| std::env::var(k).ok()` 跑 apply_test_env,两侧编排选项收口进
//! state(nat-matrix 等经 mtdssd 进程环境传参)。优雅收尾(fix-wave
//! Defect D):两条用户主动退出路径(托盘「退出」/Ctrl-C)都汇入
//! `Daemon::shutdown`,入口先广播 `event.quit`(已连 UI 随之关窗,不再
//! 留孤儿窗对着死 IPC 端点);崩溃/被杀不经过该入口,不发此帧,UI 保持
//! 既有离线行为。

mod account;
mod api;
mod ctrl;
// 收件私有暂存目的地(P4a):InboxDestIo(FsDestIo 薄包装)+ 发布协调器
// 的暂存面(发布集/打标/清扫/清除)。v2-P3 的 bridge_dest(fd 桥)已退役。
mod inbox_dest;
// fallback 公开仅为其编排类型与死因映射(relay_e2e/flow_unit 经 doc(hidden)
// 直驱面消费;编排本体 pub(crate),不外泄)。
pub mod fallback;
pub mod flow;
// hub 公开仅为其 RelayEvt 类型(relay_e2e 经 doc(hidden) 直驱面消费;
// SignalHub 本体 pub(crate),不外泄)。
pub mod hub;
mod jobs;
// 系统通知(notify.rs):android/ios 剔除(P5 iOS 门禁收敛,设计 §4.1
// 桌面专属模块 cfg 门;notify-rust 走 DBus 桌面总线,mobile 无总线;flow
// 的 event.notify 帧照发)
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod notify;
pub mod ops;
// 授权根守卫(v2-P5 T2,spec §3 授权目录集的引擎侧正身):ctrl 浏览/
// 操作与 jobs 发送源的路径收口面;桌面 None 直通,行为不变。
mod roots;
pub mod session;
pub mod settings;
mod state;
mod taskstore;
#[cfg(windows)]
pub mod update;
// 托盘与 ui 唤起:桌面专属模块(设计 §4.1 cfg 门;P5 iOS 门禁收敛,
// mobile 目标整体编译剔除),android/ios 目标整体编译剔除;模块内
// Windows 专属件另受 cfg(windows) 门,Linux 桌面行为不变。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod tray;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod uilaunch;

use anyhow::{bail, Context, Result};
use mtdss_ipc::{socket_path, Dispatcher, EventFrame, IpcClient};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::state::DaemonState;

/// android 端 `set_recv_dir` 的业务错误文案(v2-P3 T6:收件箱固定):
/// api.rs 的 android 分支与契约 golden 的 android 变体帧
/// (set_recv_dir.resp.android.json)共用同一常量——桌面 host 上无
/// android daemon 也能逐字节生成变体。`#[doc(hidden)]` 测试直驱面。
#[doc(hidden)]
pub const ANDROID_SET_RECV_DIR_ERR: &str = "安卓端收件箱固定";

/// 更新配置的跨平台别名(Task 11):Windows = 真配置;其余 = 单元占位
/// (M4 更新仅 Windows——update.rs 整体 `#![cfg(windows)]` 门住,update.*
/// RPC 在 api 层回 UNSUPPORTED_PLATFORM,占位不被调用)。
#[cfg(windows)]
type UpdateCfg = crate::update::UpdateConfig;
#[cfg(not(windows))]
type UpdateCfg = ();

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
/// ipc serve 监督循环的重启间隔(异常返回后稍歇再拉,持续失败留诊断刷屏)。
const SERVE_RETRY: Duration = Duration::from_secs(1);
/// 优雅退出事件方法名(fix-wave Defect D):**只**在 [`Daemon::shutdown`]
/// 入口广播——托盘「退出」与 Ctrl-C 是仅有的两个到达该入口的用户主动收尾
/// 路径;崩溃/被杀/panic 不经过它,不发此帧。UI 据此区分「守护进程主动
/// 收摊(随之关窗)」与「异常失联(保持 M3b 离线态)」。
const EV_QUIT: &str = "event.quit";
/// 优雅退出帧的冲刷让渡:`events.send` 只进进程内广播缓冲,推上各连接的
/// 套接字还须 serve 写半工被调度一轮;紧随其后的任务中止不等这个轮转,
/// 给一个有界窗口(100ms,远小于任何 UI 观感)保证已连客户端真的收到帧。
const QUIT_FLUSH_GRACE: Duration = Duration::from_millis(100);

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

/// ipc 服务端监督循环:serve 正常永不返回,任何返回(Ok/Err)都是异常——
/// 旧实现 serve 一退 IPC 即永久失联(daemon 活着却连不上,还占着启动锁,
/// 是最坏的半死态)。此处 warn 留痕后 1s 重拉;监听绑定在 serve 内部,
/// 重启即重建。绑定持续失败(名字被占)则每秒重试,刷屏即诊断信息;
/// Daemon::start 的就绪轮询照旧兜门,监督不影响启动判定。
async fn supervise_serve<S, F>(home: PathBuf, make_serve: F)
where
    F: Fn() -> S,
    S: std::future::Future<Output = anyhow::Result<()>>,
{
    loop {
        match make_serve().await {
            Ok(()) => {
                tracing::warn!(home = %home.display(), "ipc serve 异常返回(Ok),{SERVE_RETRY:?} 后重启")
            }
            Err(err) => {
                tracing::warn!(home = %home.display(), %err, "ipc serve 异常退出,{SERVE_RETRY:?} 后重启")
            }
        }
        tokio::time::sleep(SERVE_RETRY).await;
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
    /// 是否启用系统托盘(Task 9:仅 Windows 生效;--no-tray 或非 Windows
    /// 即无托盘;托盘装配失败不拦启动,降级为无托盘运行并留痕)。
    pub tray: bool,
    /// 授权目录集(v2-P5 T2,spec §3「授权目录集」):Some = 引擎的 ctrl
    /// 浏览/操作与发送源读取只认集内路径(集外一律拒);None = 未配置
    /// (桌面默认)= 守卫直通,行为不变。iOS 形态由 ios-bridge 注入:
    /// 收件箱(App 初始授权,恒在)+ 信任目录(T10 书签解析结果经本面
    /// 扩注入)。
    pub authorized_roots: Option<Vec<PathBuf>>,
}

impl Default for DaemonOpts {
    fn default() -> Self {
        Self {
            recv_dir: mtdss_ipc::mtdss_home().join("inbox"),
            tray: false,
            authorized_roots: None,
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
    /// ctrl 响应端任务句柄(shutdown 时中止;未登录自挂起,登录后常驻)。
    ctrl_task: JoinHandle<()>,
    /// 更新后台任务句柄(Task 11;shutdown 时中止;启动即首查 + 24h 定时 +
    /// pending_install 的作业终态/定时重试;仅 Windows)。
    #[cfg(windows)]
    update_task: JoinHandle<()>,
    /// 常驻接收任务句柄(shutdown 先发协作信号,宽限后 abort 兜底)。
    recv_task: JoinHandle<()>,
    /// fdpass 服务任务句柄(v2-P3 T4,unix;shutdown 时中止——监听随
    /// 任务释放,在途连接的 actor 线程随对端断连自然收尾)。
    #[cfg(unix)]
    fdpass_task: JoinHandle<()>,
    /// 常驻接收的协作关停信号(shutdown 置 true,轮循环在每个 await 点感知)。
    recv_shutdown: watch::Sender<bool>,
    /// 托盘线程句柄(shutdown 时 stop:置退出标志 + join,图标同线程移除);
    /// 无托盘(--no-tray / 非 Windows / 装配失败)为 None。mobile 整字段
    /// 剔除(设计 §4.1 cfg 门,P5 iOS 收敛同款;无托盘面)。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    tray: Option<tray::TrayHandle>,
    /// 托盘「退出」请求的接收端(quit_watch 交 main 与 Ctrl-C 同台 select);
    /// 仅托盘在场时 Some。
    quit_rx: Option<watch::Receiver<bool>>,
    /// 设备数据目录(shutdown 时据其删除启动锁)。
    home: PathBuf,
    /// 排他启动锁句柄:持有即占用;shutdown 时随 self 释放并删锁文件。
    _start_lock: std::fs::File,
    /// 共享状态句柄(各任务各持克隆;Daemon 侧留存供中继面 doc(hidden)
    /// 直驱方法取用,字段私有,类型不出 crate)。
    state: Arc<state::DaemonState>,
}

impl Daemon {
    /// 在 `home` 上启动守护进程:先取排他启动锁(并发/重复启动恰好一胜,
    /// 活实例在场即报错),再加载 session/settings(缺失即默认),起 IPC
    /// 服务端、在线循环与常驻接收,并等待监听就绪后才返回。在线循环与
    /// 常驻接收未登录自挂起,已登录(session.json 在场,如重启场景)即
    /// 直接上线/开轮。更新服务(Task 11)以生产配置(env 推导)装配;
    /// 测试注入走 [`Daemon::start_with_update`]。
    pub async fn start(home: &Path, opts: DaemonOpts) -> Result<Daemon> {
        #[cfg(windows)]
        let update_cfg = crate::update::UpdateConfig::production();
        #[cfg(not(windows))]
        let update_cfg = ();
        Self::start_inner(home, opts, update_cfg, false).await
    }

    /// 显式携带更新配置启动(Task 11 测试注入面,零 env 依赖;仅 Windows
    /// ——M4 更新仅 Windows)。生产入口 [`Daemon::start`] 以
    /// `UpdateConfig::production()`(env 推导)走同一装配。
    #[cfg(windows)]
    pub async fn start_with_update(
        home: &Path,
        opts: DaemonOpts,
        update_cfg: crate::update::UpdateConfig,
    ) -> Result<Daemon> {
        Self::start_inner(home, opts, update_cfg, false).await
    }

    /// 中继强制注错启动(v2-P1c T5 测试注入面,零 env 依赖):与
    /// [`Daemon::start`] 同装配,双侧编排选项(send+recv)注入
    /// `fault_force_relay`——经 jobs 面跑真中继传输的 e2e 用例专用(同机
    /// 回环直连必通,不注错测不到中继路径)。
    #[doc(hidden)]
    pub async fn start_with_force_relay(home: &Path, opts: DaemonOpts) -> Result<Daemon> {
        #[cfg(windows)]
        let update_cfg = crate::update::UpdateConfig::production();
        #[cfg(not(windows))]
        let update_cfg = ();
        Self::start_inner(home, opts, update_cfg, true).await
    }

    /// start 的共用装配体(update_cfg:Windows = 注入/生产配置,其余 =
    /// 单元占位,见 [`UpdateCfg`];force_relay:双侧编排选项的中继强制注
    /// 错,生产恒 false)。
    // 参数 update_cfg/force_relay 仅 Windows 侧或测试注入路径消费(下方
    // cfg(windows) 块与 force_relay 覆写);非 Windows 编译单元里 update_cfg
    // 的 cfg 块不存在,绑定永不被读——unix 下静默 unused-variables(否则
    // clippy -D warnings 红,Windows 本机编译不可见;同款口径见下方
    // allow(unused_mut))。
    #[cfg_attr(not(windows), allow(unused_variables))]
    async fn start_inner(
        home: &Path,
        opts: DaemonOpts,
        update_cfg: UpdateCfg,
        force_relay: bool,
    ) -> Result<Daemon> {
        // 排他启动锁:探针+spawn 之间的竞速窗口由此闭合(锁在 serve 之前)。
        let start_lock = acquire_start_lock(home).await?;
        // v2-P3 旧收件桥记录布局清扫(P4a 退役面):`{home}/inbox-records`
        // 只在测试机存在(直发未上架无存量用户)——不做迁移(spec §2 非
        // 目标),装新版自然清;尽力而为,失败不拦启动。
        crate::inbox_dest::sweep_legacy_records(home);
        // 测试注入面:MTDSS_TEST_* 旋钮在本进程内一次性读定,两侧编排选项
        // 收口进 state(旧 CLI 的按进程注入由此取代,nat-matrix 把 env 传给
        // mtdssd 子进程即可)。
        let mut send_opts = flow::SendOpts::default();
        let mut recv_opts = flow::RecvOpts::default();
        flow::apply_test_env(&mut send_opts, &mut recv_opts, |k| std::env::var(k).ok());
        if force_relay {
            send_opts.fault_force_relay = true;
            recv_opts.fault_force_relay = true;
        }
        let (events, _) = broadcast::channel(EVENT_CAP);
        // 退出请求 watch:托盘「退出」与更新 Installing 收尾(Task 12,
        // 安装器已 spawn)共用一条源;main 在任一翻转即走 `Daemon::shutdown`
        // 全套收尾(event.quit 广播、任务收尾、锁删除)。
        let (quit_tx, quit_rx) = watch::channel(false);
        // 更新服务(Task 11/12):配置注入(测试)或生产 env 推导;携带
        // 退出请求发送端(Installing 收尾用);非 Windows 为单元占位,不进
        // update 模块依赖图。
        #[cfg(windows)]
        let boot_current = update_cfg.current.clone();
        #[cfg(windows)]
        let update_svc = std::sync::Arc::new(crate::update::UpdateService::new(
            update_cfg,
            events.clone(),
            quit_tx.clone(),
        ));
        #[cfg(not(windows))]
        let update_svc = ();
        let state = Arc::new(DaemonState::load(
            home,
            opts.recv_dir,
            opts.authorized_roots,
            send_opts,
            recv_opts,
            events.clone(),
            update_svc,
        )?);
        // 连接池随 state 生命周期(state 持有唯一实例;登出拆卸走
        // state.logout 内的 ctrl.reset,API 分派经 state.ctrl() 取共享句柄)。
        let dispatcher: Arc<dyn Dispatcher> =
            Arc::new(api::ApiDispatcher::new(state.clone(), state.ctrl()));
        // 在线循环(信令枢纽):自等登录(未登录挂起在 state 的 watch 通知上),
        // 登录翻转即上线并常驻;独占唯一信令连接,维持目录快照与
        // event.online/event.devices 广播,并把已验签入站信令广播给订阅方。
        let online_task = tokio::spawn(hub::run(state.clone(), events.clone()));
        // ctrl 响应端(控制通道):登录后订枢纽等 CtrlConnect,每条 spawn
        // 会话任务(punch + serve_conn);未登录自挂起(订阅自然等待)。
        let ctrl_task = tokio::spawn(ctrl::responder_loop(state.clone()));
        // 更新后台任务(Task 11,仅 Windows):启动即首查 + 24h 定时
        // (auto_check 门 + auto_install 档 + 通知档),pending_install 的
        // 作业终态/定时重试。未配置(无公钥)静默不查。
        #[cfg(windows)]
        let update_task = {
            let svc = state.update();
            tokio::spawn(svc.run(state.clone()))
        };
        // fdpass 通道(v2-P3 T4 起,unix:fd 桥依赖 Unix 域套接字的
        // SCM_RIGHTS;Windows 编译单元整体缺席,bridge 形态的源请求在
        // resolver 占位面直接「不可用」收口):监听 {home}/fds.sock,
        // 0x01 source_fd 分派给 jobs 的 SourceResolver(上行);0x04
        // dest_query 与 0x05 publish_result 分派给 DestQueries(查询通道
        // 二合一,P4a §6.5;v2-P3 的 0x02/0x03 已随收件 fd 桥退役)。
        // serve_fdpass 的 accept 循环任何返回都属异常(含瞬时
        // EMFILE——fd 压力下恰是 bridge 场景),监督循环 1s 重拉自愈
        // (supervise_serve 同款口径;重拉重建监听,残留 socket 文件由
        // serve_fdpass bind 前自清)。
        #[cfg(unix)]
        let fdpass_task = {
            let state = state.clone();
            let home = home.to_path_buf();
            tokio::spawn(async move {
                let handler = Arc::new(crate::jobs::FdBridgeHandler::new(
                    state.jobs().source_resolver().clone(),
                    state.jobs().dest_queries().clone(),
                ));
                loop {
                    if let Err(err) = mtdss_ipc::fdpass::serve_fdpass(&home, handler.clone()).await {
                        tracing::warn!(%err, home = %home.display(), "fdpass 服务异常退出,1s 后重启");
                    }
                    tokio::time::sleep(SERVE_RETRY).await;
                }
            })
        };
        // 常驻接收:登录翻转即开轮,永久等来件;协作关停信号 + abort 兜底。
        let (recv_shutdown, recv_shutdown_rx) = watch::channel(false);
        let recv_task = tokio::spawn(flow::resident_recv_loop(
            state.clone(),
            recv_shutdown_rx,
        ));
        let serve_home = home.to_path_buf();
        let serve_events = events.clone();
        // serve 包进监督循环(见 supervise_serve 注释):异常返回即重启,
        // 不再一退永失。闭包每次调用克隆共享件,serve 拿自有副本。
        let serve_task = tokio::spawn(supervise_serve(serve_home.clone(), move || {
            let dispatcher = dispatcher.clone();
            let events = serve_events.clone();
            let home = serve_home.clone();
            async move { mtdss_ipc::serve(&home, dispatcher, events).await }
        }));
        // 轮询连接确认监听就绪,保证 start 返回后立即可连(时序确定)。
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if tokio::time::Instant::now() >= deadline {
                // 启动失败路径:全部任务一并中止,不留孤儿循环
                online_task.abort();
                ctrl_task.abort();
                #[cfg(windows)]
                update_task.abort();
                #[cfg(unix)]
                fdpass_task.abort();
                recv_task.abort();
                serve_task.abort();
                bail!("守护进程监听未就绪(超时 {})", socket_path(home));
            }
            if try_connect(home).await {
                break;
            }
            tokio::time::sleep(READY_POLL).await;
        }
        // 更新标记开机消费(Task 12,仅 Windows;非 Windows 从不写标记):
        // IPC 就绪后、托盘/唤起面之前收口——等版本「已更新到 X」通知/未完成
        // 告警 + 清标记,restart-ui 在场则唤回主窗(先清标记再唤窗:唤起
        // 守卫读的就是该标记)。
        #[cfg(windows)]
        crate::update::consume_boot_markers(home, &boot_current);
        // 托盘(Task 9):Windows 且未关时,监听就绪后起专属线程(失败不拦
        // 启动,降级无托盘并留痕);「退出」请求经 watch 交回调用方。非
        // Windows / --no-tray 下 quit_tx 无接收方,quit_rx 置 None。
        // Windows 侧按 opts.tray 条件赋值才需要 mut;非 Windows 编译单元里
        // cfg 块不存在,绑定永不写入——unix 下静默 unused-mut(否则 clippy
        // -D warnings 红,Windows 本机编译不可见)。mobile:tray 模块整体
        // 剔除(设计 §4.1 cfg 门,P5 iOS 收敛同款),绑定不存在,has_tray
        // 恒 false。
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut tray_handle: Option<tray::TrayHandle> = None;
        #[cfg(windows)]
        if opts.tray {
            match tray::spawn_tray(state.clone(), quit_tx) {
                Ok(handle) => tray_handle = Some(handle),
                Err(err) => {
                    tracing::warn!(%err, "托盘启动失败,以无托盘模式继续");
                }
            }
        }
        #[cfg(not(windows))]
        let _ = quit_tx;
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let has_tray = tray_handle.is_some();
        // mobile:无托盘面,恒 false——quit_rx 走下方 None 臂
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let has_tray = false;
        Ok(Daemon {
            events,
            serve_task,
            online_task,
            ctrl_task,
            #[cfg(windows)]
            update_task,
            recv_task,
            #[cfg(unix)]
            fdpass_task,
            recv_shutdown,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            tray: tray_handle,
            quit_rx: if has_tray { Some(quit_rx) } else { None },
            home: home.to_path_buf(),
            _start_lock: start_lock,
            state,
        })
    }

    /// 托盘「退出」请求的接收端:main 与 Ctrl-C 同台 select;无托盘为
    /// None(main 只等 Ctrl-C)。托盘线程点「退出」→ send(true)。
    pub fn quit_watch(&self) -> Option<watch::Receiver<bool>> {
        self.quit_rx.clone()
    }

    /// 本代握手缓存的中继资格(离线 None)。`#[doc(hidden)]` 测试直驱面:
    /// relay_e2e 绕过 IPC 直读枢纽中继面(SignalHub 为 pub(crate) 不可跨
    /// crate 命名),不进 RPC 契约。
    #[doc(hidden)]
    pub fn relay_entitlement(&self) -> Option<mtdss_core::signaling::client::RelayEntitlement> {
        self.state.hub().relay_entitlement()
    }

    /// 订阅中继事件广播(测试直驱面,同 [`Daemon::relay_entitlement`] 注)。
    #[doc(hidden)]
    pub fn relay_events(&self) -> Option<tokio::sync::broadcast::Receiver<hub::RelayEvt>> {
        self.state.hub().relay_events()
    }

    /// 中继分配(测试直驱面,同上;10s 有界,Denied 是 Ok 分支由调用方
    /// 分派,见 hub)。purpose 为兜底平面判别(v2-P2 票①,见 hub)。
    #[doc(hidden)]
    pub async fn relay_allocate(
        &self,
        to: &str,
        purpose: &str,
    ) -> anyhow::Result<mtdss_core::signaling::client::AllocOutcome> {
        self.state.hub().relay_allocate(to, purpose).await
    }

    /// connect_with_fallback 测试直驱面(T5 relay_e2e:真信令 + 真中继数据
    /// 面直驱兜底编排,hub 从本 daemon 枢纽取,relay_rx 由调用方轮首订阅后
    /// 传入;参数原样转发,签名见 fallback 模块)。
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)] // fallback::connect_with_fallback 的 verbatim 转发面
    pub async fn connect_with_fallback(
        &self,
        relay_rx: &mut tokio::sync::broadcast::Receiver<hub::RelayEvt>,
        me: &mtdss_core::identity::DeviceIdentity,
        peer_id: &str,
        peer_pk: &ed25519_dalek::VerifyingKey,
        direct: &quinn::Endpoint,
        peer_candidates: &[std::net::SocketAddr],
        punch: mtdss_core::transport::punch::PunchConfig,
        role: fallback::FallbackRole,
        purpose: fallback::RelayPurpose,
        force_relay: bool,
    ) -> Result<fallback::ConnectOutcome, fallback::FallbackErr> {
        fallback::connect_with_fallback(
            self.state.hub(),
            relay_rx,
            me,
            peer_id,
            peer_pk,
            direct,
            peer_candidates,
            punch,
            role,
            purpose,
            force_relay,
        )
        .await
    }

    /// 作业快照的 relay 标记读取(测试直驱面,T5 e2e 断言 snapshot.relay;
    /// IPC JSON 面的 relay 字段是 Task 7)。作业不存在 → None。
    #[doc(hidden)]
    pub fn job_relay(&self, id: &str) -> Option<bool> {
        self.state
            .jobs()
            .snapshot()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.relay)
    }

    /// 远端收件区残留清理(测试直驱面,v2-P2 Task 5 ctrl_e2e:CtrlPool
    /// 为 pub(crate) 不可跨 crate 命名,而该操作不进 RPC 契约——生产调用
    /// 方是 transfer.clear 与重新发起的 dropped 清理,均在 daemon 内部)。
    /// 参数原样转发 [`CtrlPool::cleanup_outbox`],错误经 CtrlErr::rpc()
    /// 取文案。
    #[doc(hidden)]
    pub async fn cleanup_outbox(
        &self,
        to_device: &str,
        dir: &str,
        rels: &[String],
    ) -> Result<u32, String> {
        self.state
            .ctrl()
            .cleanup_outbox(&self.state, to_device, dir, rels)
            .await
            .map_err(|e| e.rpc().1)
    }

    /// 停止常驻接收、在线循环与 IPC 服务端并等待任务收尾(监听随之释放,
    /// connect 立即失败),最后删除启动锁。等待收尾后删锁,避免后来者把活
    /// 启动误判为陈旧锁。常驻接收先停:不再有落盘/信令动作;先发协作信号
    /// 给宽限窗(轮循环在 await 点感知即自行收尾),超时 abort 兜底,关闭
    /// 不挂任务收尾。托盘最先收(图标即时消失、泵线程不泄漏),再走常规
    /// 任务收尾。
    ///
    /// 入口即广播 `event.quit`(fix-wave Defect D):main 把托盘「退出」与
    /// Ctrl-C 两个用户主动收尾都汇到本入口,先于一切拆台发事件——已连的
    /// UI 收到即随之关窗,不再对着死 IPC 端点当孤儿窗。崩溃/被杀不会进入
    /// 本函数,天然不发此帧(UI 保持既有离线行为)。发后让渡
    /// [`QUIT_FLUSH_GRACE`] 一拍,让 serve 写半工把帧推上各连接再中止任务。
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.events.send(EventFrame {
            method: EV_QUIT.into(),
            params: serde_json::json!({}),
        });
        tokio::time::sleep(QUIT_FLUSH_GRACE).await;
        // mobile 无此字段(设计 §4.1 cfg 门,P5 iOS 收敛同款;无托盘面)
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        drop(self.tray); // TrayHandle::Drop:置退出标志 + join,同线程移除图标
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
        self.ctrl_task.abort();
        if let Err(err) = self.ctrl_task.await {
            // 取消是 abort 的预期路径;panic 等异常收尾必须留痕,不容吞掉。
            if !err.is_cancelled() {
                tracing::warn!(%err, "ctrl 响应端任务收尾异常");
            }
        }
        #[cfg(windows)]
        {
            // 更新后台任务(Task 11):中止语义同上;panic 异常收尾留痕。
            self.update_task.abort();
            if let Err(err) = self.update_task.await {
                if !err.is_cancelled() {
                    tracing::warn!(%err, "更新后台任务收尾异常");
                }
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
        // fdpass 服务(unix;Windows 无此字段):中止语义同上。
        #[cfg(unix)]
        {
            self.fdpass_task.abort();
            if let Err(err) = self.fdpass_task.await {
                if !err.is_cancelled() {
                    tracing::warn!(%err, "fdpass 服务任务收尾异常");
                }
            }
        }
        if let Err(err) = std::fs::remove_file(self.home.join(LOCK_FILE)) {
            tracing::warn!(%err, "删除启动锁失败");
        }
        Ok(())
    }
}

/// 监督循环单测(mock serve,不起真实监听):两败一挂起。
#[cfg(test)]
mod supervise_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 前两次失败第三次挂起:监督循环应各重试一次(共 3 次进入 make_serve),
    /// 且自身永不返回(第三次挂起持续占用)。真实时钟:两次 1s 重试间隔
    /// 合计约 2s,外层 10s 有界收口(超时是预期路径,断言落在计数上)。
    #[tokio::test]
    async fn supervise_restarts_after_failures() {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let sup = supervise_serve(PathBuf::from("mock-home"), move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(anyhow::anyhow!("mock failure #{n}"))
                } else {
                    // serve 的正常形态:不返回
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        });
        // 有界收口:正常即监督循环永不返回,超时是预期路径;断言此时计数
        // 已到 3(失败两次 + 第三次进入挂起态,不是又一轮重试)。
        let outcome = tokio::time::timeout(Duration::from_secs(10), sup).await;
        assert!(outcome.is_err(), "监督循环不得返回(第三次应挂起)");
        assert_eq!(
            count.load(Ordering::SeqCst),
            3,
            "失败两次后第三次应进入 make_serve(且不再退出)"
        );
    }
}
