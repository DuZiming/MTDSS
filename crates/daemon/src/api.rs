//! RPC 方法分派:把 JSON-RPC 方法映射到守护进程状态读取。
//!
//! 本任务实现 `hello` 与 `status` 两个查询方法;未知方法回 -32601。
//! 后续任务(login/send/recv)在此扩展,写路径经 state 的可变接口进入。

use mtdss_ipc::{Dispatcher, RpcError};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::session::Session;
use crate::state::DaemonState;

/// RPC 层对外报出的协议版本(hello.version)。
const RPC_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 未知方法的 JSON-RPC 标准错误码。
const ERR_METHOD_NOT_FOUND: i64 = -32601;

/// 守护进程分派器:持共享状态的只读视图。
pub(crate) struct ApiDispatcher {
    state: Arc<DaemonState>,
}

impl ApiDispatcher {
    pub(crate) fn new(state: Arc<DaemonState>) -> Self {
        Self { state }
    }
}

impl Dispatcher for ApiDispatcher {
    fn dispatch(
        &self,
        method: String,
        _params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
        let state = self.state.clone();
        Box::pin(async move {
            match method.as_str() {
                "hello" => Ok(hello(&state)),
                "status" => Ok(status(&state)),
                other => Err(RpcError {
                    code: ERR_METHOD_NOT_FOUND,
                    message: format!("method not found: {other}"),
                }),
            }
        })
    }
}

/// 设备字段:未登录为 null;session.json 无设备名字段,name 置 null
/// (Task 4 登录引入名字来源后再回填)。
fn device_value(session: Option<&Session>) -> Value {
    match session {
        None => Value::Null,
        Some(s) => json!({ "device_id": s.device_id, "name": Value::Null }),
    }
}

/// `hello`:握手/探活,报协议版本与登录态。
fn hello(state: &DaemonState) -> Value {
    let session = state.session();
    json!({
        "version": RPC_VERSION,
        "logged_in": state.logged_in(),
        "device": device_value(session.as_ref()),
    })
}

/// `status`:运行态快照(online 本任务恒 false,信令接线在 Task 4/5)。
fn status(state: &DaemonState) -> Value {
    let session = state.session();
    json!({
        "logged_in": session.is_some(),
        "online": false,
        "server": session.as_ref().map(|s| s.server.clone()),
        "device": device_value(session.as_ref()),
        "recv_dir": state.recv_dir().to_string_lossy(),
    })
}
