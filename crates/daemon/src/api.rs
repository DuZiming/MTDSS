//! RPC 方法分派:把 JSON-RPC 方法映射到守护进程状态操作。
//!
//! `hello`/`status` 为查询;`login.*` 为写路径(REST 进出 + 状态翻转);
//! 未知方法回 -32601。错误码约定:-32602 参数缺失/非法;1 业务失败
//! (验证码错误、服务器拒绝等),message 携带服务器上下文。

use mtdss_ipc::{Dispatcher, RpcError};
use serde_json::{json, Value};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::session::Session;
use crate::state::{DaemonState, PendingLogin};

/// RPC 层对外报出的协议版本(hello.version)。
const RPC_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 未知方法的 JSON-RPC 标准错误码。
const ERR_METHOD_NOT_FOUND: i64 = -32601;
/// 参数缺失/非法的 JSON-RPC 标准错误码。
const ERR_INVALID_PARAMS: i64 = -32602;
/// 业务失败(服务器拒绝/本地落盘失败等)错误码。
const ERR_BUSINESS: i64 = 1;

/// 守护进程分派器:持共享状态的读写视图。
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
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
        let state = self.state.clone();
        Box::pin(async move {
            match method.as_str() {
                "hello" => Ok(hello(&state)),
                "status" => Ok(status(&state)),
                "devices" => Ok(devices(&state)),
                "login.request_code" => login_request_code(&state, params).await,
                "login.verify" => login_verify(&state, params).await,
                "login.logout" => login_logout(&state).await,
                other => Err(RpcError {
                    code: ERR_METHOD_NOT_FOUND,
                    message: format!("method not found: {other}"),
                }),
            }
        })
    }
}

/// 设备字段:未登录为 null;session.json 无设备名字段,name 置 null
/// (名字权威在服务器目录,Task 6 在线循环经 devices 查询时再补)。
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

/// `status`:运行态快照。online 取自身连接态(在线循环维护,Task 6 接线)。
fn status(state: &DaemonState) -> Value {
    let session = state.session();
    json!({
        "logged_in": session.is_some(),
        "online": state.online(),
        "server": session.as_ref().map(|s| s.server.clone()),
        "device": device_value(session.as_ref()),
        "recv_dir": state.recv_dir().to_string_lossy(),
    })
}

/// `devices`:在线循环维护的目录快照(连接建立全量 + Presence 增量刷新;
/// 未登录或尚无快照为空数组)。每台恰含 device_id/name/platform/online,
/// 本机恒在列表且 online=自身连接态。RPC 只读缓存,不现拉 REST——快照的
/// 新鲜度由在线循环负责(连接建立与 presence 增量时刷新)。
fn devices(state: &DaemonState) -> Value {
    let list = state.devices().unwrap_or_default();
    json!({ "devices": list })
}

// ---------- login.* ----------

/// 从 params 取字符串字段;缺失或类型错即 -32602。
fn need_str(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: {key}"),
        })
}

/// 可选字符串路径参数(自签 CA 证书路径)。
fn opt_path(params: &Value, key: &str) -> Option<PathBuf> {
    params.get(key).and_then(|v| v.as_str()).map(PathBuf::from)
}

/// `login.request_code {server, email, ca?}` → `{"code":String}`。
/// dev 模式服务器把验证码回显在响应里,原样透传;server/email/ca 存入
/// state 作 pending,verify 阶段按 email 取回。
async fn login_request_code(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let server = need_str(&params, "server")?;
    let email = need_str(&params, "email")?;
    let ca = opt_path(&params, "ca");
    let code = crate::account::request_code(&server, &email, ca.as_deref())
        .await
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("请求验证码失败: {e:#}"),
        })?;
    state.set_pending_login(PendingLogin {
        server,
        email,
        ca,
    });
    Ok(json!({ "code": code }))
}

/// `login.verify {email, code, name?}` → `{"device_id","name"}`。
/// name 缺省用本机主机名;身份种子不存在则生成落盘(0600)。登录全程持
/// 登录互斥锁(身份生成 → REST → 会话落盘翻转原子化):并发 verify 串行
/// 后后来者看到已存在的 seed(同一身份),磁盘与会话永不分歧;logout 同锁,
/// 登出不会插队在途 verify 的翻转中间。pending 窥视在进锁前同步完成——
/// 并发 verify 两侧都拿到快照,随后靠服务器码单次有效决出唯一成功者
/// (失败侧报 401 上下文,而非「无待登录记录」)。
async fn login_verify(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let email = need_str(&params, "email")?;
    let code = need_str(&params, "code")?;
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => gethostname::gethostname().to_string_lossy().into_owned(),
    };
    let pending = state.pending_login_for(&email).ok_or_else(|| RpcError {
        code: ERR_INVALID_PARAMS,
        message: format!("没有 {email} 的待登录记录,请先调用 login.request_code"),
    })?;

    let _guard = state.lock_login().await;
    let id = crate::account::load_identity(&state.home()).map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("加载本机身份失败: {e:#}"),
    })?;
    let pubkey_hex = hex::encode(id.public_key().as_bytes());

    let (jwt, refresh, device_id) =
        crate::account::verify(&pending.server, &email, &code, &pubkey_hex, &name, pending.ca.as_deref())
            .await
            .map_err(|e| RpcError {
                code: ERR_BUSINESS,
                message: format!("验证失败: {e:#}"),
            })?;

    let session = Session {
        server: pending.server,
        jwt,
        refresh,
        device_id: device_id.clone(),
        ca: pending.ca,
    };
    state.login(session).map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("写入会话失败: {e:#}"),
    })?;
    // take 在落盘成功之后:state.login 失败(磁盘满/权限)时 pending 保留,
    // 客户端重试 verify 不至死路。
    state.take_pending_login_for(&email);
    Ok(json!({ "device_id": device_id, "name": name }))
}

/// `login.logout {}` → `{}`。幂等:未登录同样返回成功(删不存在的
/// session.json 不算错);磁盘删除失败才报业务错。与 verify 同锁:串行化
/// 登录态翻转,登出不插队在途 verify 的落盘中间。
async fn login_logout(state: &DaemonState) -> Result<Value, RpcError> {
    let _guard = state.lock_login().await;
    state.logout().map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("登出失败: {e:#}"),
    })?;
    Ok(json!({}))
}
