//! 系统托盘(Task 9,Windows):登录态换图标 + 右键菜单 + 优雅退出。
//!
//! 纯函数(`icon_for` / `tray_icon_rgba`)跨平台编译可测;托盘装配与
//! win32 消息泵在 `#[cfg(windows)]` 下,Linux/macOS 构建不进入依赖图。
//!
//! 行为规范(设计 §5.1 / Task 9 简报;Task 13 增 tooltip 计数面):
//! - 图标:登录 = 主色 #3B82F6 实心圆(亮),登出 = 灰 #9CA3AF;
//!   登录翻转经 state 的 login watch 通知,托盘线程轮询即换。
//! - tooltip(Task 13):非终态作业数 > 0 → 「MTDSS — N 个传输中」,
//!   0 → 「MTDSS」;数据源 = jobs 注册表的计数 watch(建队 +1/终态 -1),
//!   托盘线程与 login watch 同一同步轮询样板。
//! - 右键菜单:「打开主窗口」(`uilaunch::open_main_window` 后台拉起 ui,
//!   已在运行时由 ui 侧 tauri-plugin-single-instance 唤起既有主窗)、
//!   托盘图标**双击**同入口(fix-wave:TrayIconEvent 通道;左键单击
//!   不再弹菜单——模态菜单会吞掉双击的第二次按下,右键菜单照旧)、
//!   「登录/登出」(文案随登录态动态翻转;已登录点击即 `state.logout()`,
//!   图标与文案随 login watch 自动翻转;未登录点击拉起主窗引导登录)、
//!   「设置 ▸ 开机自启」(勾选态 = settings.autostart,
//!   点击与 `autostart.set` RPC 走同一 `state.set_autostart` 路径)、
//!   「设置 ▸ 自动更新 / 自动安装」(Task 14,D4 三档联动:种子取
//!   settings 权威值;自动安装仅在自动更新开启时使能;取消自动更新成功
//!   后 daemon 侧连带关安装,安装项去勾+禁用;切换沿开机自启的「成功才
//!   落勾、失败回读」回滚模式,与 `update.prefs.set` RPC 同一
//!   `state.set_update_check/set_update_install` 路径)、
//!   「检查更新…」(Task 14:强制检查,三分支——有新版 → 通知
//!   「发现新版本 X」+ 拉起更新窗;无更新 → 「已是最新版本」;失败 →
//!   「检查更新失败」。检查是 async,托盘线程以专属 current-thread
//!   运行时同步阻塞桥接,期间菜单事件在通道排队,泵恢复后照常消化)、
//!   「退出」(优雅:经 watch 通知 main 选择分支,main 走完整
//!   `daemon.shutdown`,启动锁文件照删;shutdown 入口广播 `event.quit`,
//!   已连 UI 主窗随之收口——fix-wave Defect D,孤儿窗不再对着死端点)。
//!
//! Windows 线程模型(tray-icon 0.20 文档要求:"On Windows ... a win32
//! event loop must be running on the thread ... you have to create the
//! tray icon on the same thread as the event loop"):专属 std::thread 上
//! 先建 TrayIcon 与菜单,再进 `MsgWaitForMultipleObjectsEx` 泵循环——
//! 有消息立即泵干,无消息 ~150ms 超时让出;每个空档轮询退出标志、
//! 登录 watch 与 muda 的全局菜单事件通道,兼顾菜单响应与可关停
//! (无需跨线程 PostThreadMessage)。

/// 图标边长(托盘图标 32×32)。
const ICON_SIZE: u32 = 32;
/// 实心圆半径(像素);圆心取 32×32 网格几何中心 (15.5, 15.5)。
const ICON_RADIUS: f64 = 14.0;
/// 登录态主色 #3B82F6(蓝)。
const COLOR_LOGGED_IN: [u8; 3] = [59, 130, 246];
/// 登出态灰 #9CA3AF。
const COLOR_LOGGED_OUT: [u8; 3] = [156, 163, 175];

/// 登录态 → 是否用彩色图标(true = 登录,亮色;false = 登出,灰色)。
pub fn icon_for(logged_in: bool) -> bool {
    logged_in
}

/// 托盘 tooltip 文案(纯函数,跨平台可测):非终态作业数 > 0 →
/// 「MTDSS — N 个传输中」(Task 13);0 → 静态「MTDSS」。
pub fn tray_tooltip(count: usize) -> String {
    if count > 0 {
        format!("MTDSS — {count} 个传输中")
    } else {
        "MTDSS".to_string()
    }
}

/// 「检查更新…」三分支(Task 14,D4/D13):有新版 / 已是最新 / 失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckOutcome {
    /// 有新版本(携带版本号,入通知文案)。
    Available(String),
    /// 已是最新。
    UpToDate,
    /// 检查失败(网络/验签/未配置;细节只进日志,不进通知文案)。
    Failed,
}

/// check() 成功返回的状态 JSON → 三分支投影(纯函数,跨平台可测):
/// `state == "available"` → Available(latest);其余(idle,及检查入口被
/// 服务幂等挡回的 downloading/pending_install/ready/installing)→
/// UpToDate。Err 分支不经投影——调用方直接落 Failed。
pub fn update_check_outcome(state_json: &serde_json::Value) -> UpdateCheckOutcome {
    if state_json["state"].as_str() == Some("available") {
        UpdateCheckOutcome::Available(
            state_json["latest"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    } else {
        UpdateCheckOutcome::UpToDate
    }
}

/// 三分支通知文案选择器(纯函数):(标题, 正文)。正文为设计钉死的三句;
/// 有新版的标题沿自动检查通知的「MTDSS 更新可用」先例(update.rs)。
pub fn update_check_notify(outcome: &UpdateCheckOutcome) -> (String, String) {
    match outcome {
        UpdateCheckOutcome::Available(v) => {
            ("MTDSS 更新可用".to_string(), format!("发现新版本 {v}"))
        }
        UpdateCheckOutcome::UpToDate => ("MTDSS".to_string(), "已是最新版本".to_string()),
        UpdateCheckOutcome::Failed => ("MTDSS".to_string(), "检查更新失败".to_string()),
    }
}

/// 更新设置两档切换后的 UI 落定态(勾选/使能;决策表的输出面)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrefsUi {
    /// 「自动更新」勾选框落定值。
    pub auto_check_checked: bool,
    /// 「自动安装」勾选框落定值。
    pub auto_install_checked: bool,
    /// 「自动安装」使能落定值(仅自动更新开启时可勾)。
    pub auto_install_enabled: bool,
}

/// 「自动更新」切换决策表(纯函数,D4 三档联动):当前检查/安装值 ×
/// 点击目标 × set 成败 → 两档的 UI 落定。
/// - 成功开:检查勾上;安装项恢复使能、勾选不动(开检查不反向拉起安装);
/// - 成功关:检查勾下;安装项连带去勾+禁用(daemon 侧
///   `set_update_check(false)` 连带 auto_install=false,UI 跟进);
/// - 失败:设置零改动,两项回读权威值(开机自启同款回滚模式;权威值即
///   入参快照——失败意味着设置未动)。
pub fn update_check_toggle_ui(
    check_current: bool,
    install_current: bool,
    target: bool,
    set_ok: bool,
) -> UpdatePrefsUi {
    match (set_ok, target) {
        (true, true) => UpdatePrefsUi {
            auto_check_checked: true,
            auto_install_checked: install_current,
            auto_install_enabled: true,
        },
        (true, false) => UpdatePrefsUi {
            auto_check_checked: false,
            auto_install_checked: false,
            auto_install_enabled: false,
        },
        (false, _) => UpdatePrefsUi {
            auto_check_checked: check_current,
            auto_install_checked: install_current,
            auto_install_enabled: check_current,
        },
    }
}

/// 「自动安装」切换的勾选落定(纯函数):成功 = 目标态,失败回读当前;
/// 使能不受本项影响(只随「自动更新」联动,见 [`update_check_toggle_ui`])。
pub fn update_install_toggle_checked(install_current: bool, target: bool, set_ok: bool) -> bool {
    if set_ok {
        target
    } else {
        install_current
    }
}

/// 32×32 RGBA(4096 字节)实心圆图标字节:圆内 alpha=255 涂主色/灰,
/// 圆外 alpha=0 全透明。纯函数,跨平台可测。
pub fn tray_icon_rgba(colored: bool) -> Vec<u8> {
    let rgb = if colored {
        COLOR_LOGGED_IN
    } else {
        COLOR_LOGGED_OUT
    };
    let center = (ICON_SIZE as f64 - 1.0) / 2.0;
    let r2 = ICON_RADIUS * ICON_RADIUS;
    let mut buf = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let inside = (x as f64 - center) * (x as f64 - center)
                + (y as f64 - center) * (y as f64 - center)
                <= r2;
            buf.extend_from_slice(&[rgb[0], rgb[1], rgb[2], if inside { 255 } else { 0 }]);
        }
    }
    buf
}

/// 托盘线程句柄:Drop 即收尾(置退出标志并 join 泵线程;线程内同线程
/// drop TrayIcon,任务栏图标即时移除)。shutdown 与异常路径共用同一入口,
/// 不留双通道。
pub(crate) struct TrayHandle {
    quit: std::sync::Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        self.quit
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// 专属线程装配托盘并起消息泵;`quit_tx` 是「退出」菜单项的信号出口,
/// Daemon 持接收端交 main 与 Ctrl-C 同台 select。图标/菜单创建与泵同
/// 线程,满足 tray-icon 的 Windows 要求。
#[cfg(windows)]
pub(crate) fn spawn_tray(
    state: std::sync::Arc<crate::state::DaemonState>,
    quit_tx: tokio::sync::watch::Sender<bool>,
) -> anyhow::Result<TrayHandle> {
    use anyhow::Context as _;
    let quit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let join = {
        let quit = quit.clone();
        std::thread::Builder::new()
            .name("mtdss-tray".into())
            .spawn(move || {
                if let Err(err) = tray_thread(state, quit, quit_tx) {
                    tracing::error!(%err, "托盘线程异常退出");
                }
            })
            .context("启动托盘线程失败")?
    };
    Ok(TrayHandle {
        quit,
        join: Some(join),
    })
}

/// 32×32 托盘图标(实心圆,按登录态取色)。
#[cfg(windows)]
fn make_icon(colored: bool) -> anyhow::Result<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(tray_icon_rgba(colored), ICON_SIZE, ICON_SIZE)
        .map_err(|e| anyhow::anyhow!("构建托盘图标失败: {e}"))
}

/// 托盘图标事件 → 是否拉起主窗(纯函数;入参类型随 tray-icon 的
/// cfg(windows) 依赖门控,单测见 `windows_tray_event_tests`):
/// DoubleClick(任意键)→ true;单击/移入/移动/移出 → false。
/// Windows 侧语义:左双击唤起主窗(Explorer 惯例);单击不抢动作,
/// 右键菜单照旧。
#[cfg(windows)]
fn tray_event_opens_main(event: &tray_icon::TrayIconEvent) -> bool {
    matches!(event, tray_icon::TrayIconEvent::DoubleClick { .. })
}

/// 托盘线程主体:建菜单与图标 → 消息泵循环 → 收尾 drop。
/// 菜单(设计 §5.1 + Task 14):打开主窗口 / 登录·登出 /
/// 设置▸(开机自启·自动更新·自动安装)/ 检查更新… / 退出;
/// 托盘图标双击 = 唤起主窗(fix-wave:TrayIconEvent 通道接线)。
#[cfg(windows)]
fn tray_thread(
    state: std::sync::Arc<crate::state::DaemonState>,
    quit: std::sync::Arc<std::sync::atomic::AtomicBool>,
    quit_tx: tokio::sync::watch::Sender<bool>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, Submenu};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let logged_in = state.logged_in();
    // 双图标预建,登录翻转时仅 clone 交换(set_icon 接 Option<Icon>)。
    let icon_colored = make_icon(true)?;
    let icon_gray = make_icon(false)?;

    // 菜单(全部保留句柄:登录项要改文案,勾选项要回滚勾选,退出项比对 id)
    let menu = Menu::new();
    let open_item = MenuItem::new("打开主窗口", true, None);
    let login_item = MenuItem::new(if logged_in { "登出" } else { "登录" }, true, None);
    let settings_sub = Submenu::new("设置", true);
    let autostart_item = CheckMenuItem::new("开机自启", true, state.autostart(), None);
    // 更新设置两档(Task 14):种子取 settings 权威值;自动安装的使能随
    // 自动更新联动(D4)——仅在检查开启时可勾。
    let auto_check_item = CheckMenuItem::new("自动更新", true, state.update_auto_check(), None);
    let auto_install_item = CheckMenuItem::new(
        "自动安装",
        state.update_auto_check(),
        state.update_auto_install(),
        None,
    );
    let check_update_item = MenuItem::new("检查更新…", true, None);
    let quit_item = MenuItem::new("退出", true, None);
    settings_sub
        .append_items(&[&autostart_item, &auto_check_item, &auto_install_item])
        .context("装配「设置」子菜单失败")?;
    menu.append_items(&[
        &open_item,
        &login_item,
        &settings_sub,
        &check_update_item,
        &quit_item,
    ])
    .context("装配托盘菜单失败")?;

    // menu_on_left_click 关断(fix-wave):默认 true 时左键按下即弹模态
    // TrackPopupMenu,双击的第二次按下被菜单循环吞掉,WM_LBUTTONDBLCLK
    // 永远到不了托盘窗——双击唤起形同虚设。关断后左双击干净产出
    // DoubleClick 事件(下方 TrayIconEvent 通道消费),右键菜单不受影响
    // (WM_RBUTTONDOWN 无条件弹菜单)。
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("MTDSS")
        .with_icon(if logged_in {
            icon_colored.clone()
        } else {
            icon_gray.clone()
        })
        .build()
        .context("创建托盘图标失败")?;

    // 登录态 watch:托盘线程同步轮询 has_changed(tokio watch 接收端
    // 的同步面),先借初始值避免陈旧值误报。
    let mut login_rx = state.login_watch();
    let _ = login_rx.borrow_and_update();

    // 作业计数 watch(Task 13):tooltip「N 个传输中」面,与 login watch
    // 同一同步轮询样板(建队 +1 / 终态 -1)。
    let mut count_rx = state.jobs_count_rx();
    let _ = count_rx.borrow_and_update();

    // 「检查更新…」的 async 桥(Task 14):托盘线程无 tokio 上下文,备一个
    // 专属 current-thread 运行时同步阻塞跑 check。check 只触服务内部同步
    // 锁与自带超时的 HTTP 客户端,不依赖守护运行时的任务面,异运行时阻塞
    // 安全;点击期间菜单事件经通道排队,泵恢复后照常消化(有界:清单
    // 超时 15s,收尾后积压事件一次批处理)。
    let update_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("创建托盘更新检查运行时失败")?;

    loop {
        if quit.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        // 菜单事件:muda 在 DispatchMessageW 里转入全局通道,此处逐条消化
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *open_item.id() {
                // 后台拉起 ui;已在运行时由 ui 侧单实例插件聚焦既有主窗
                crate::uilaunch::open_main_window();
                continue;
            }
            if event.id == *login_item.id() {
                if state.logged_in() {
                    // 已登录即登出:会话清理 + 登录态翻转,图标/文案随
                    // login watch 循环自动换,此处不手动补 UI
                    if let Err(err) = state.logout() {
                        tracing::warn!(%err, "托盘登出失败");
                    }
                } else {
                    // 未登录:拉起主窗,登录引导在 UI 侧完成
                    crate::uilaunch::open_main_window();
                }
                continue;
            }
            if event.id == *autostart_item.id() {
                // 勾选态以 settings 为权威:目标 = 当前取反;失败回滚勾选框
                let target = !state.autostart();
                match state.set_autostart(target) {
                    Ok(()) => {
                        autostart_item.set_checked(target);
                        tracing::info!(enabled = target, "托盘切换开机自启");
                    }
                    Err(err) => {
                        tracing::warn!(%err, "托盘切换开机自启失败");
                        autostart_item.set_checked(state.autostart());
                    }
                }
                continue;
            }
            if event.id == *auto_check_item.id() {
                // 勾选态以 settings 为权威:目标 = 当前取反;成功才落勾,
                // 失败回读(开机自启回滚模式);关检查连带关安装(daemon 侧
                // 耦合),两档的勾选/使能按决策表一次落定
                let check_current = state.update_auto_check();
                let install_current = state.update_auto_install();
                let target = !check_current;
                let set_ok = state.set_update_check(target).is_ok();
                let ui = update_check_toggle_ui(check_current, install_current, target, set_ok);
                auto_check_item.set_checked(ui.auto_check_checked);
                auto_install_item.set_checked(ui.auto_install_checked);
                auto_install_item.set_enabled(ui.auto_install_enabled);
                if set_ok {
                    tracing::info!(auto_check = target, "托盘切换更新自动检查");
                } else {
                    tracing::warn!("托盘切换更新自动检查失败");
                }
                continue;
            }
            if event.id == *auto_install_item.id() {
                // 同回滚模式;使能不动(只随「自动更新」联动)
                let install_current = state.update_auto_install();
                let target = !install_current;
                let set_ok = state.set_update_install(target).is_ok();
                auto_install_item
                    .set_checked(update_install_toggle_checked(install_current, target, set_ok));
                if set_ok {
                    tracing::info!(auto_install = target, "托盘切换更新自动安装");
                } else {
                    tracing::warn!("托盘切换更新自动安装失败");
                }
                continue;
            }
            if event.id == *check_update_item.id() {
                // 强制检查(state.update() 句柄派生,沿 login/logout 直调
                // 形态):同步阻塞至多清单超时;结果三分支 → 通知(失败细节
                // 只进日志)+ 有新版拉起更新窗(open_update_window 自带
                // 更新进行中守卫)
                let svc = state.update();
                let outcome = match update_rt.block_on(svc.check(true)) {
                    Ok(v) => update_check_outcome(&v),
                    Err(err) => {
                        tracing::warn!(
                            code = err.code,
                            message = %err.message,
                            "托盘检查更新失败"
                        );
                        UpdateCheckOutcome::Failed
                    }
                };
                let (title, body) = update_check_notify(&outcome);
                crate::notify::notify_one(&title, &body);
                if matches!(outcome, UpdateCheckOutcome::Available(_)) {
                    crate::uilaunch::open_update_window();
                }
                continue;
            }
            if event.id == *quit_item.id() {
                tracing::info!("托盘「退出」:请求守护进程收尾");
                let _ = quit_tx.send(true);
                continue;
            }
        }
        // 托盘图标事件通道(fix-wave):双击 = 唤起主窗(经单实例插件在
        // ui 侧聚焦/恢复既有主窗,与「打开主窗口」菜单项同一入口);
        // Enter/Move/Leave 是鼠标滑过高频噪音,逐条丢弃不动作。
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if tray_event_opens_main(&event) {
                crate::uilaunch::open_main_window();
            }
        }
        // 登录翻转 → 换图标 + 改菜单文案
        if login_rx.has_changed().unwrap_or(false) {
            let logged = *login_rx.borrow_and_update();
            tracing::info!(logged, "登录态变化,切换托盘图标");
            let icon = if logged {
                icon_colored.clone()
            } else {
                icon_gray.clone()
            };
            if let Err(err) = tray.set_icon(Some(icon)) {
                tracing::warn!(%err, "切换托盘图标失败");
            }
            login_item.set_text(if logged { "登出" } else { "登录" });
        }
        // 作业计数变化(Task 13)→ 更新 tooltip「MTDSS — N 个传输中」
        // (0 → 静态「MTDSS」)。set_tooltip 失败仅 warn:tooltip 是锦上
        // 添花面,不进错误路径。
        if count_rx.has_changed().unwrap_or(false) {
            let n = *count_rx.borrow_and_update();
            if let Err(err) = tray.set_tooltip(Some(tray_tooltip(n))) {
                tracing::warn!(count = n, %err, "更新托盘 tooltip 失败");
            }
        }
        // 泵:有消息立即泵干;无消息 ~150ms 超时让出(退出标志/事件通道
        // 的轮询空档)。MWMO_INPUTAVAILABLE:已有未处理输入也立即返回,
        // 避免泵清空后新到消息漏唤醒。注意 windows-sys 0.59 里这些原语
        // 全在 Win32::UI::WindowsAndMessaging(非 Threading/Foundation)。
        unsafe {
            let wait = windows_sys::Win32::UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                150,
                windows_sys::Win32::UI::WindowsAndMessaging::QS_ALLINPUT,
                windows_sys::Win32::UI::WindowsAndMessaging::MWMO_INPUTAVAILABLE,
            );
            if wait == windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
                let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG =
                    std::mem::zeroed();
                while windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW(
                    &mut msg,
                    std::ptr::null_mut(),
                    0,
                    0,
                    windows_sys::Win32::UI::WindowsAndMessaging::PM_REMOVE,
                ) != 0
                {
                    windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                    windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
            } else if wait == u32::MAX {
                // WAIT_FAILED:消息队列失效(线程/桌面异常),继续轮询也
                // 无产出,按异常收尾留痕。
                anyhow::bail!("win32 消息泵失败(WAIT_FAILED)");
            }
        }
    }
    // 收尾:同线程 drop TrayIcon(Windows 上 Shell_NotifyIcon 移除必须
    // 由创建线程执行),任务栏图标即时消失。
    drop(tray);
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tray_event_tests {
    //! 托盘图标事件 → 动作判定(纯函数)单测:构造真实 `TrayIconEvent`
    //! 逐变体钉死「双击拉起主窗、其余不动作」。tray-icon 是 cfg(windows)
    //! 依赖,本模块随之门控(Linux 交叉检查构建不进依赖图)。
    use super::tray_event_opens_main;
    use tray_icon::dpi::PhysicalPosition;
    use tray_icon::{MouseButton, Rect, TrayIconEvent, TrayIconId};

    /// 最小事件坐标面(判定只看变体,坐标恒零)。
    fn pos() -> PhysicalPosition<f64> {
        PhysicalPosition::new(0.0, 0.0)
    }

    #[test]
    fn double_click_any_button_opens_main_window() {
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            let ev = TrayIconEvent::DoubleClick {
                id: TrayIconId::new("mtdss"),
                position: pos(),
                rect: Rect::default(),
                button,
            };
            assert!(tray_event_opens_main(&ev), "双击({button:?})必须拉起主窗");
        }
    }

    #[test]
    fn click_enter_move_leave_never_open() {
        let mk = || TrayIconId::new("mtdss");
        let events = [
            TrayIconEvent::Click {
                id: mk(),
                position: pos(),
                rect: Rect::default(),
                button: MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
            },
            TrayIconEvent::Click {
                id: mk(),
                position: pos(),
                rect: Rect::default(),
                button: MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Down,
            },
            TrayIconEvent::Enter { id: mk(), position: pos(), rect: Rect::default() },
            TrayIconEvent::Move { id: mk(), position: pos(), rect: Rect::default() },
            TrayIconEvent::Leave { id: mk(), position: pos(), rect: Rect::default() },
        ];
        for ev in &events {
            assert!(!tray_event_opens_main(ev), "非双击事件({ev:?})不得拉起主窗");
        }
    }
}
