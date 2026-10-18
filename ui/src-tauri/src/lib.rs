//! MTDSS 主窗口壳(Tauri v2):前端(React)经本 crate 的桥接命令访问
//! daemon IPC。见 ipc_bridge 模块文档:启动即尝试连接/拉起 mtdssd,
//! 事件转发为 `daemon://*` 窗口事件,八个 RPC 命令带 10s 超时。

mod ipc_bridge;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            ipc_bridge::login_request_code,
            ipc_bridge::login_verify,
            ipc_bridge::logout,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
