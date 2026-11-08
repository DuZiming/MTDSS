//! cmd_settings:设置面 Tauri 命令桥(V3-4 §5:Windows 设置对话框)。
//!
//! 三个命令全部经既有 IPC 泵(`ipc_bridge::rpc`,通用 10s 超时)透传
//! daemon RPC:设置读写在 daemon 侧收口(`settings.get`/`settings.set`,
//! 持久化权威 = 引擎 settings.json);授权根热注册转发 `roots.set`(T1
//! op,载荷由前端装配——信任模式开 = 收件箱+所选目录,关 = 空集直通;
//! daemon 即时换守卫,免「重启生效」窗)。本模块零本地逻辑,存在意义
//! 与 kv_get/kv_put 同:给前端一枚稳定命令面,参数形态归 Tauri 桥。

use crate::ipc_bridge::{rpc, DaemonBridge, RPC_TIMEOUT};
use serde_json::{json, Value};
use tauri::State;

/// `settings_get {}` → `settings.get` 透传:五字段投影(allow_remote_delete/
/// show_hidden/trust_mode/trusted_roots/device_name)。
#[tauri::command]
pub async fn settings_get(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "settings.get", json!({}), RPC_TIMEOUT).await
}

/// `settings_set {params}` → `settings.set` 透传:补丁写入(缺省键 daemon
/// 侧不动;params 原样透传)。信任段热生效不在本命令——切换由前端随后
/// 调 [`roots_set`]。
#[tauri::command]
pub async fn settings_set(
    state: State<'_, DaemonBridge>,
    params: Value,
) -> Result<Value, String> {
    rpc(&state, "settings.set", params, RPC_TIMEOUT).await
}

/// `roots_set {roots}` → `roots.set` 透传:授权根热注册(roots 数组原样
/// 包参;非法根 daemon 侧丢弃留痕 fail-closed,空集 = 直通)。
#[tauri::command]
pub async fn roots_set(state: State<'_, DaemonBridge>, roots: Value) -> Result<Value, String> {
    rpc(&state, "roots.set", json!({ "roots": roots }), RPC_TIMEOUT).await
}
