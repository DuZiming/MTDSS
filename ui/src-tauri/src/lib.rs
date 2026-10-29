//! MTDSS 主窗口壳(Tauri v2):前端(React)经本 crate 的桥接命令访问
//! daemon IPC。见 ipc_bridge 模块文档:启动即尝试连接/拉起 mtdssd,
//! 事件转发为 `daemon://*` 窗口事件(守护进程优雅退出 `event.quit` 则由
//! 桥直接关窗,fix-wave Defect D),RPC 命令带超时(通用 10s,浏览/更新
//! 检查 35s)。
//! 单实例(tauri-plugin-single-instance):二次启动不新开进程窗,
//! 唤起既有主窗并聚焦——托盘「打开主窗口」重复点击即走此路聚焦;
//! 二次启动带 `--update-page`(托盘「检查更新…」拉起更新窗,Task 14)
//! 另转发 `daemon://open-update`,前端据此打开更新页覆盖层。冷启动
//! (无既有实例)同参数直接启动时,经 `OPEN_UPDATE_REQUESTED` 标记 +
//! `take_open_update_request` 命令由前端挂载后拉取消费。

use std::sync::atomic::{AtomicBool, Ordering};

mod dropresolve;
mod ipc_bridge;

/// 冷启动 `--update-page` 路由标记(Task 15):ui.exe 带 `--update-page`
/// 直接启动且无既有实例可转发(单实例回调只在**第一个**实例进程内触发)
/// → 置位。由前端挂载后的 [`take_open_update_request`] 拉取消费(取走即
/// 清)。不直接 emit:setup 先于 webview 加载完成,事件会落在前端监听器
/// 注册之前而丢失(冷启动 WebView2 可达数秒),拉取式消费与加载耗时
/// 彻底解耦。
static OPEN_UPDATE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// 冷启动 `--update-page` 路由标记的取走口(Task 15):true = 本次进程
/// 以 `--update-page` 启动且尚未消费,前端应打开更新页覆盖层。取走即清
/// (幂等:二次调用恒 false,防重复开页)。
#[tauri::command]
fn take_open_update_request() -> bool {
    OPEN_UPDATE_REQUESTED.swap(false, Ordering::AcqRel)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 冷启动 `--update-page` 路由:置标记(见 OPEN_UPDATE_REQUESTED 注释)。
    if std::env::args().any(|a| a == "--update-page") {
        OPEN_UPDATE_REQUESTED.store(true, Ordering::Release);
    }
    tauri::Builder::default()
        // 单实例必须最先注册(官方建议):第二实例启动即触达回调——
        // 唤起既有主窗(恢复最小化 + 显示 + 聚焦)后自身退出;窗口缺失
        //(理论不可达:tauri.conf.json 唯一窗口 label 缺省为 "main")则
        // 静默放过。unminimize 必须先行(fix-wave):Windows 的 show()
        // 只置可见位、不反转最小化态(SW_SHOW ≠ SW_RESTORE),最小化的
        // 主窗此前仅 show+set_focus 时表现为「托盘唤起无响应」。
        // `--update-page` 路由(Task 15):托盘「检查更新…」拉起 ui.exe
        // 带参 → 本回调转发 `daemon://open-update`,由既有前端打开更新
        // 页覆盖层(与唤起聚焦并存:更新页叠在既有界面上层)。
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            use tauri::{Emitter as _, Manager as _};
            if args.iter().any(|a| a == "--update-page")
                && let Err(err) = app.emit("daemon://open-update", ())
            {
                tracing::warn!(%err, "单实例路由:更新页打开事件发送失败");
            }
            if let Some(win) = app.get_webview_window("main") {
                if let Err(err) = win.unminimize() {
                    tracing::warn!(%err, "单实例唤起:恢复最小化主窗失败");
                }
                if let Err(err) = win.show() {
                    tracing::warn!(%err, "单实例唤起:显示主窗失败");
                }
                if let Err(err) = win.set_focus() {
                    tracing::warn!(%err, "单实例唤起:聚焦主窗失败");
                }
            }
        }))
        .plugin(tauri_plugin_opener::init())
        // 系统目录选择器(Task 12「下载到本机…」):经 ipc_bridge::pick_folder
        // 包装为 Tauri 命令,前端不直接依赖插件的 JS 命令面
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 常驻 IPC 泵:连接/拉起 daemon + 事件转发;桥状态注册进 app
            ipc_bridge::setup(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc_bridge::daemon_hello,
            ipc_bridge::status,
            ipc_bridge::devices,
            ipc_bridge::list,
            ipc_bridge::stat,
            ipc_bridge::delete,
            ipc_bridge::fs_move,
            ipc_bridge::fs_copy,
            ipc_bridge::transfer_start,
            ipc_bridge::transfer_list,
            ipc_bridge::transfer_pause,
            ipc_bridge::transfer_resume,
            ipc_bridge::transfer_cancel,
            ipc_bridge::transfer_reinitiate,
            ipc_bridge::transfer_clear,
            ipc_bridge::update_check,
            ipc_bridge::update_state,
            ipc_bridge::update_apply,
            ipc_bridge::clipboard_files,
            ipc_bridge::copy_text,
            ipc_bridge::pick_folder,
            ipc_bridge::pick_ca_cert,
            ipc_bridge::login_request_code,
            ipc_bridge::login_verify,
            ipc_bridge::logout,
            take_open_update_request,
            dropresolve::drop_resolve_at_cursor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
