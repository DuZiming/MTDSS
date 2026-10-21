//! MTDSS 主窗口壳(Tauri v2):前端(React)经本 crate 的桥接命令访问
//! daemon IPC。见 ipc_bridge 模块文档:启动即尝试连接/拉起 mtdssd,
//! 事件转发为 `daemon://*` 窗口事件,九个 RPC 命令带 10s 超时。
//! 单实例(tauri-plugin-single-instance):二次启动不新开进程窗,
//! 唤起既有主窗并聚焦——托盘「打开主窗口」重复点击即走此路聚焦。

mod dropresolve;
mod ipc_bridge;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例必须最先注册(官方建议):第二实例启动即触达回调——
        // 唤起既有主窗(恢复最小化 + 显示 + 聚焦)后自身退出;窗口缺失
        //(理论不可达:tauri.conf.json 唯一窗口 label 缺省为 "main")则
        // 静默放过。unminimize 必须先行(fix-wave):Windows 的 show()
        // 只置可见位、不反转最小化态(SW_SHOW ≠ SW_RESTORE),最小化的
        // 主窗此前仅 show+set_focus 时表现为「托盘唤起无响应」。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager as _;
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
            ipc_bridge::clipboard_files,
            ipc_bridge::pick_folder,
            ipc_bridge::login_request_code,
            ipc_bridge::login_verify,
            ipc_bridge::logout,
            dropresolve::drop_resolve_at_cursor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
