//! 托盘纯函数与开机自启行为测试(Task 9)。
//!
//! 纯函数(`tray_icon_png` 四态选择 / `tray_tooltip`)跨平台可测;`autostart.set`
//! 的注册表语义为 cfg(windows) 真写 HKCU(测试自带现场快照与恢复);
//! RPC 往返全平台闭环(非 Windows 无注册表,值仅随 settings.json 持久化)。
//!
//! 环境注:部分受管 Windows(EDR/组策略)会拦截对 HKCU Run 键的任何写入
//! (连 .NET 直写同样在 ~25s 后报 os error 5)。此类环境以 `registry_writable`
//! 探测识别:被拦截时注册表往返测试显式跳过,RPC 往返测试改测「写注册表
//! 失败 → 业务错误 + 设置不半程翻转」的一致性语义;常规 Windows 环境跑全量。

use mtdss_daemon::tray::{taskbar_light, tray_icon_png, tray_tooltip};
use mtdss_daemon::uilaunch::launch_allowed;
use mtdss_daemon::{Daemon, DaemonOpts};
use mtdss_ipc::{IpcClient, RpcError};
use std::path::Path;
use std::time::Duration;

/// 调用超时:IpcClient::call 无内建超时,测试自带上界防悬挂。
const CALL_TIMEOUT: Duration = Duration::from_secs(5);


/// 临时 home + 显式 recv_dir 的启动选项(托盘恒关:测试不依赖桌面会话)。
fn opts(home: &Path) -> DaemonOpts {
    DaemonOpts {
        recv_dir: home.join("inbox"),
        tray: false,
        authorized_roots: None,
        inbox_staging: None,
        // 桌面形态:无收件箱发布面(v2-P5 T5b,哨兵回退默认收件目录)
        inbox_publish: false,
            photo_serve: false,
    }
}

/// 限时调用助手:传输错误/超时直接 panic,RpcError 交给调用方断言。
async fn call(
    client: &IpcClient,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, RpcError> {
    tokio::time::timeout(CALL_TIMEOUT, client.call(method, params))
        .await
        .expect("RPC 必须在超时内完成")
}

// ---------- 纯函数(跨平台) ----------

/// 托盘图标四态选择(V3-2 单色双弧,纯函数;final review C4:除非空/合法
/// PNG/两两不同外,还需**逐字节钉死哪个组合配哪张资产**——把 match 四臂
/// 互换(浅色任务栏→白图标=隐身)时本测必须红)。
#[test]
fn tray_icon_png_four_states_distinct() {
    let on_dark = tray_icon_png(true, false);
    let dim_dark = tray_icon_png(false, false);
    let on_light = tray_icon_png(true, true);
    let dim_light = tray_icon_png(false, true);
    // 深色任务栏 → 白色资产(light);浅色任务栏 → 深靛资产(dark);
    // 登录=on(不透明),登出=dim(40%)。资产字节与 brand/exported 逐一同源。
    assert_eq!(
        on_dark,
        include_bytes!("../../../brand/exported/tray-light-on-32.png") as &[u8],
        "登录/深色条 → 白色不透明资产"
    );
    assert_eq!(
        dim_dark,
        include_bytes!("../../../brand/exported/tray-light-dim-32.png") as &[u8],
        "登出/深色条 → 白色 40% 资产"
    );
    assert_eq!(
        on_light,
        include_bytes!("../../../brand/exported/tray-dark-on-32.png") as &[u8],
        "登录/浅色条 → 深靛不透明资产"
    );
    assert_eq!(
        dim_light,
        include_bytes!("../../../brand/exported/tray-dark-dim-32.png") as &[u8],
        "登出/浅色条 → 深靛 40% 资产"
    );
    assert_ne!(on_dark, on_light, "深浅条资产必须不同");
}

/// 任务栏主题探测(V3-2):返回 bool(浅色=true)且不 panic——具体值
/// 随本机主题走,只钉「可探测、有兜底」的契约(读失败按浅色)。
#[test]
fn taskbar_light_probe_returns_without_panic() {
    let _ = taskbar_light();
}

/// 托盘 tooltip 计数文案(Task 13,纯函数;V3-2 品牌 → Synshu):
/// >0 → 「Synshu — N 个传输中」,0 → 静态「Synshu」。
#[test]
fn tray_tooltip_counts_active_jobs() {
    assert_eq!(tray_tooltip(0), "Synshu", "无进行中作业 → 静态文案");
    assert_eq!(tray_tooltip(1), "Synshu — 1 个传输中");
    assert_eq!(tray_tooltip(3), "Synshu — 3 个传输中");
}

// ---------- 更新设置三档联动 + 检查更新三分支(Task 14,纯函数) ----------

// 决策/文案选择面(与托盘装配解耦的纯函数,Linux CI 同测):
// 三档联动决策表 + 检查更新三分支的状态投影与通知文案选择器。
use mtdss_daemon::tray::{
    update_check_notify, update_check_outcome, update_check_toggle_ui,
    update_install_toggle_checked, UpdateCheckOutcome, UpdatePrefsUi,
};

/// 「自动更新」切换的独立复述决策表(与实现解耦,测试即规格):遍历
/// 全组合(当前检查 × 当前安装 × 点击目标 × set 成败),按 D4 联动规格
/// 逐格钉死两档的期望 UI 态。
#[test]
fn update_check_toggle_decision_table() {
    for check_current in [false, true] {
        for install_current in [false, true] {
            for target in [false, true] {
                for set_ok in [false, true] {
                    let got =
                        update_check_toggle_ui(check_current, install_current, target, set_ok);
                    let expect = if set_ok {
                        if target {
                            // 开检查:勾上;安装项恢复使能、勾选不动
                            //(开检查不反向拉起安装)
                            UpdatePrefsUi {
                                auto_check_checked: true,
                                auto_install_checked: install_current,
                                auto_install_enabled: true,
                            }
                        } else {
                            // 关检查:勾下;安装连带去勾+禁用(daemon 侧
                            // set_update_check(false) 连带 auto_install=false)
                            UpdatePrefsUi {
                                auto_check_checked: false,
                                auto_install_checked: false,
                                auto_install_enabled: false,
                            }
                        }
                    } else {
                        // set 失败 = 设置零改动:两项回读权威值(开机自启
                        // 回滚模式;使能只随权威的检查开关)
                        UpdatePrefsUi {
                            auto_check_checked: check_current,
                            auto_install_checked: install_current,
                            auto_install_enabled: check_current,
                        }
                    };
                    assert_eq!(
                        got, expect,
                        "决策表失配(check={check_current}, install={install_current}, \
                         target={target}, ok={set_ok})"
                    );
                }
            }
        }
    }
}

/// 简报钉死的头牌案例:自动更新与自动安装都勾着 → 取消勾选自动更新
/// (成功)→ 自动安装连带去勾 + 禁用;失败则一切原样(回读)。
#[test]
fn update_check_toggle_uncheck_couples_install_off_and_disabled() {
    let ui = update_check_toggle_ui(true, true, false, true);
    assert!(!ui.auto_check_checked, "检查项落勾 = 关");
    assert!(!ui.auto_install_checked, "安装项必须连带去勾");
    assert!(!ui.auto_install_enabled, "安装项必须连带禁用");

    let ui = update_check_toggle_ui(true, true, false, false);
    assert!(ui.auto_check_checked, "失败回读:检查项保持原勾选");
    assert!(ui.auto_install_checked, "失败回读:安装项保持原勾选");
    assert!(ui.auto_install_enabled, "失败回读:安装项保持原使能");
}

/// 「自动安装」切换:成功 = 目标态,失败回读当前;与检查/目标以外的
/// 组合无关(使能只随「自动更新」联动)。
#[test]
fn update_install_toggle_checked_follows_set_result() {
    for install_current in [false, true] {
        for target in [false, true] {
            for set_ok in [false, true] {
                let got = update_install_toggle_checked(install_current, target, set_ok);
                let expect = if set_ok { target } else { install_current };
                assert_eq!(
                    got, expect,
                    "失配(install={install_current}, target={target}, ok={set_ok})"
                );
            }
        }
    }
}

/// check() 成功返回的状态 JSON → 三分支投影:available+latest →
/// Available(latest);idle → UpToDate;缺 state / 幂等挡回的中间态 →
/// UpToDate。Err 分支不经投影(调用方直接落 Failed),不在此测。
#[test]
fn update_check_outcome_maps_state_json() {
    let got = update_check_outcome(&serde_json::json!({ "state": "available", "latest": "1.2.3" }));
    assert_eq!(
        got,
        UpdateCheckOutcome::Available("1.2.3".to_string()),
        "available + latest → 携版本号的 Available"
    );
    for state in ["idle", "downloading", "pending_install", "ready", "installing"] {
        let got = update_check_outcome(&serde_json::json!({ "state": state }));
        assert_eq!(got, UpdateCheckOutcome::UpToDate, "state={state} → UpToDate");
    }
    let got = update_check_outcome(&serde_json::json!({}));
    assert_eq!(got, UpdateCheckOutcome::UpToDate, "缺 state 防御性落 UpToDate");
}

/// 三分支通知文案选择器:正文为简报钉死的三句;有新版带版本号,
/// 标题沿自动检查通知先例;V3-2 品牌切换 → Synshu。
#[test]
fn update_check_notify_texts() {
    let (title, body) = update_check_notify(&UpdateCheckOutcome::Available("1.2.3".to_string()));
    assert_eq!((title.as_str(), body.as_str()), ("Synshu 更新可用", "发现新版本 1.2.3"));
    let (title, body) = update_check_notify(&UpdateCheckOutcome::UpToDate);
    assert_eq!((title.as_str(), body.as_str()), ("Synshu", "已是最新版本"));
    let (title, body) = update_check_notify(&UpdateCheckOutcome::Failed);
    assert_eq!((title.as_str(), body.as_str()), ("Synshu", "检查更新失败"));
}

// ---------- uilaunch:find_ui_bin 注入单测(Task 9) ----------

// 可注入纯函数测试面:env 值与 exe 路径参数化,规避测试进程内
// std::env::set_var 的并行竞态(set_var 影响整个进程,默认 harness 的
// 并行测试线程共享一块环境块,互相污染);debug 兜底逐级仍读真实
// CARGO_MANIFEST_DIR(daemon crate 的清单目录,进程级不可注入——该路
// 以「优雅返回」口径测,见第三测注释)。
use mtdss_daemon::uilaunch::find_ui_bin_impl;

/// env MTDSS_UI_BIN 最高优先级且原样采用:镜像 find_daemon_bin 对
/// MTDSSD_BIN 的口径,显式覆盖不验证存在性——指向不存在的临时路径
/// 也必须原样返回(存在性留给 spawn 阶段报错)。
#[test]
fn find_ui_bin_env_override() {
    let tmp = tempfile::tempdir().unwrap();
    // 故意不创建该文件:env 是显式覆盖,解析层不验存在性
    let fake = tmp.path().join("some-ui-placeholder");
    let r = find_ui_bin_impl(Some(fake.to_string_lossy().into_owned()), None);
    assert_eq!(
        r.as_deref(),
        Some(fake.as_path()),
        "MTDSS_UI_BIN 必须最高优先级且原样采用"
    );
}

/// exe 同目录命中 Synshu[.exe](发布布局:V3-2 起 bundle mainBinaryName
/// 改名,发布时分发到同一目录):临时目录造假宿主 exe 路径 + 同目录写入
/// Synshu[.exe],注入后必须恰返回该兄弟路径(宿主 exe 本体是否存在不
/// 参与解析,只取其父目录)。
#[test]
fn find_ui_bin_exe_sibling() {
    let tmp = tempfile::tempdir().unwrap();
    let sibling_name = if cfg!(windows) { "Synshu.exe" } else { "Synshu" };
    let sibling = tmp.path().join(sibling_name);
    std::fs::write(&sibling, b"fake ui").unwrap();
    let r = find_ui_bin_impl(None, Some(tmp.path().join("whatever-host.exe")));
    assert_eq!(
        r.as_deref(),
        Some(sibling.as_path()),
        "同目录 Synshu[.exe] 必须被命中"
    );
}

/// env 缺失 + 同目录无 ui:debug 构建下逐级找 CARGO_MANIFEST_DIR 祖先的
/// ui/src-tauri/target/{debug,release}/ui[.exe]。该清单目录指向 daemon
/// crate 且进程级不可注入,本路无法确定性断言:ui 已构建的开发机命中、
/// 干净机返回 None,均合法——只断言穷尽后优雅返回(不 panic),命中时
/// 必为 ui 可执行文件名;前两路(env/同目录)已由上面两测确定性覆盖,
/// 逐级查找的路径拼装由本机实际构建形态经冒烟验证(Task 11)。
#[test]
fn find_ui_bin_no_sibling_falls_through_gracefully() {
    // 嵌套不存在目录:同目录路必不命中,必然落到 env 兜底之后的逐级找
    let tmp = tempfile::tempdir().unwrap();
    let r = find_ui_bin_impl(None, Some(tmp.path().join("no-such-dir").join("host.exe")));
    // 兜底命中可 Synshu[.exe](tauri build --debug 改名产物)或 ui[.exe]
    // (cargo 原生名)——双名皆合法 ui 可执行文件(V3-2 双名契约)。
    let expects: &[&str] = if cfg!(windows) {
        &["Synshu.exe", "ui.exe"]
    } else {
        &["Synshu", "ui"]
    };
    if let Some(p) = r {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap();
        assert!(
            expects.contains(&name),
            "兜底命中必须是 ui 可执行文件名(得到 {name})"
        );
    }
}

/// 唤起守卫(Task 12 / D11):更新进行中(标记在场)→ 拒绝唤起——安装器
/// 即将接管,新窗只会对着将死的 IPC 端点。纯决策面(launch_allowed),
/// 与 find_ui_bin_impl 同款注入口径,不真 spawn。
#[test]
fn update_in_progress_blocks_ui_launch() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(launch_allowed(tmp.path()), "无标记必须放行唤起");
    std::fs::write(mtdss_ipc::update_marker_path(tmp.path()), "1.0.1").unwrap();
    assert!(!launch_allowed(tmp.path()), "更新进行中必须拒绝唤起");
}

// ---------- 开机自启:RPC 往返(跨平台;非 Windows 无注册表) ----------

/// 本机能否写 HKCU Run(Windows;进程内一次性探测,结果全测试共享)。
#[cfg(windows)]
fn registry_writable() -> bool {
    use std::sync::OnceLock;
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let Ok(run) = hkcu.open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ | KEY_SET_VALUE,
        ) else {
            return false;
        };
        // 用独立探针值名,不碰被测的 MTDSS 值;探毕即清。
        match run.set_value("MTDSS_TEST_PROBE", &"probe".to_string()) {
            Ok(()) => {
                let _ = run.delete_value("MTDSS_TEST_PROBE");
                true
            }
            Err(_) => false,
        }
    })
}

/// HKCU Run 是进程全局共享面:本二进制内两个注册表测试(本函数与
/// `windows_registry::autostart_set_true_writes_run_key_then_false_removes`)
/// 被默认 harness 并发调度,必须互斥触碰 Run\MTDSS,否则交错读写会间歇红。
/// std Mutex 一次性初始化;持锁跨 await(await_holding_lock)是测试内
/// 串行化的本意——被序列化的正是持锁期间发出的 RPC 的服务端注册表写入,
/// 无第三方取锁者,无死锁面。
static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn autostart_get_set_roundtrip_rpc() {
    #[cfg(windows)]
    let can_write_registry = registry_writable();
    #[cfg(not(windows))]
    let can_write_registry = true;

    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
    let client = IpcClient::connect(home.path()).await.unwrap();

    // 初始:默认 false(get 只读 settings,不触注册表,无需持锁)
    let r = call(&client, "autostart.get", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(r["enabled"], false, "全新 home 缺省 autostart=false");

    // 参数校验:缺 enabled → -32602(参数层拒绝,不触注册表,无需持锁)
    let err = call(&client, "autostart.set", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, -32602, "缺 enabled 必须回参数错误");

    {
        // 写注册表的段持全局锁:与 registry 往返测试互斥
        let _registry = REGISTRY_LOCK.lock().unwrap();
        if can_write_registry {
            // 常规环境全链:set true → get true → settings.json 持久化
            let r = call(&client, "autostart.set", serde_json::json!({ "enabled": true }))
                .await
                .unwrap();
            assert_eq!(r, serde_json::json!({}), "set 返回空对象");
            let r = call(&client, "autostart.get", serde_json::json!({}))
                .await
                .unwrap();
            assert_eq!(r["enabled"], true);
            let settings: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(home.path().join("settings.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(settings["autostart"], true, "settings.json 必须持久化");
        } else {
            // 受管环境(EDR 拦 Run 键写):set true 必须业务失败(code 1)且
            // 设置不被半程翻转——注册表先行、失败即整体回退的一致性语义。
            let err = call(&client, "autostart.set", serde_json::json!({ "enabled": true }))
                .await
                .unwrap_err();
            assert_eq!(err.code, 1, "注册表写入被拦截时 set 必须业务失败");
            let r = call(&client, "autostart.get", serde_json::json!({}))
                .await
                .unwrap();
            assert_eq!(r["enabled"], false, "失败的 set 不得翻转设置");
        }
    }

    {
        // set false 回环同样写注册表(删除),持锁与另一测试互斥
        let _registry = REGISTRY_LOCK.lock().unwrap();
        call(&client, "autostart.set", serde_json::json!({ "enabled": false }))
            .await
            .unwrap();
        let r = call(&client, "autostart.get", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(r["enabled"], false);
    }

    daemon.shutdown().await.unwrap();
}

// ---------- 开机自启:注册表真实往返(cfg windows) ----------

#[cfg(windows)]
mod windows_registry {
    use super::*;

    /// HKCU Run 键(与 api.rs 的实现常量独立复述,测试即规格)。
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// Run 键里代表 Synshu 的值名(V3-2 品牌切换:原「MTDSS」值由启动
    /// 清扫迁移,见 api.rs sweep_legacy_autostart)。
    const RUN_VALUE: &str = "Synshu";

    /// 直读 HKCU Run 的 MTDSS 值;键或值不存在为 None。
    fn read_value() -> Option<String> {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let run = hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ).ok()?;
        run.get_value(RUN_VALUE).ok()
    }

    /// 尽力写/删 Run 值(现场恢复用;drop 路径内不容 panic)。
    fn write_value_best_effort(v: Option<&str>) {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
            match v {
                Some(v) => {
                    let _ = run.set_value(RUN_VALUE, &v.to_string());
                }
                None => {
                    let _ = run.delete_value(RUN_VALUE);
                }
            }
        }
    }

    /// 现场恢复守卫:构造时快照,drop 时尽力还原(断言 panic 也不丢现场)。
    struct RestoreOriginal(Option<String>);

    impl Drop for RestoreOriginal {
        fn drop(&mut self) {
            write_value_best_effort(self.0.as_deref());
        }
    }

    /// 真写 HKCU 的端到端:set true → 值存在且等于带引号的当前 exe 全路径;
    /// set false → 值删除。走 RPC(state.set_autostart)即生产同一路径。
    /// 受管 Windows 拦截 Run 键写入时显式跳过(见文件头环境注)。
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn autostart_set_true_writes_run_key_then_false_removes() {
        if !registry_writable() {
            eprintln!(
                "SKIP: 本机策略拦截 HKCU Run 写入(EDR/组策略),注册表真实往返留待常规 Windows 环境验证"
            );
            return;
        }
        // 整段持全局锁(快照→写→断言→删→断言→现场恢复全程独占):
        // 与 roundtrip 测试的注册表写入段互斥。guard 声明在 RestoreOriginal
        // 之前,drop 逆序保证恢复动作仍在持锁区间内(panic 展开同理)。
        let _registry = REGISTRY_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let daemon = Daemon::start(home.path(), opts(home.path())).await.unwrap();
        let client = IpcClient::connect(home.path()).await.unwrap();
        // 现场快照 + 守卫恢复(测试写的是真实 HKCU,必须自带清理)
        let _guard = RestoreOriginal(read_value());

        // set true → Run 值出现,内容 = 带引号的当前 exe 完整路径
        call(&client, "autostart.set", serde_json::json!({ "enabled": true }))
            .await
            .expect("set true 必须成功");
        let expected = format!("\"{}\"", std::env::current_exe().unwrap().display());
        let v = read_value().expect("set true 后 HKCU Run 的 MTDSS 值必须存在");
        assert_eq!(v, expected, "Run 值必须是带引号的完整路径");

        // set false → Run 值删除
        call(&client, "autostart.set", serde_json::json!({ "enabled": false }))
            .await
            .expect("set false 必须成功");
        assert!(
            read_value().is_none(),
            "set false 后 HKCU Run 的 MTDSS 值必须删除"
        );

        daemon.shutdown().await.unwrap();
    }
}
