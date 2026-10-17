//! RPC 方法分派:把 JSON-RPC 方法映射到守护进程状态操作。
//!
//! `hello`/`status`/`devices` 为查询;`login.*` 为写路径(REST 进出 + 状态
//! 翻转);`send`/`set_recv_dir` 为传输面(send 在 RPC 内长调用跑完整编排,
//! 并发第二传经发送互斥立即拒绝);`autostart.*` 为设置面(Task 9:Windows
//! 落 HKCU Run 注册表,非 Windows 仅随 settings 持久化);未知方法回
//! -32601。错误码约定:-32602 参数缺失/非法;1 业务失败(验证码错误、
//! 服务器拒绝、传输失败报告等),message 携带上下文;2 发送互斥占用
//! (传输进行中)。

use mtdss_ipc::{Dispatcher, EventFrame, RpcError};
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
/// 发送互斥占用(并发第二个 send)错误码。
const ERR_BUSY: i64 = 2;
/// 事件方法名:发送进度逐行推送(flow 的 out_report 每行一条)。
const EV_SEND_LOG: &str = "event.send_log";

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
                "send" => send(&state, params).await,
                "set_recv_dir" => set_recv_dir(&state, params).await,
                "autostart.get" => Ok(autostart_get(&state)),
                "autostart.set" => autostart_set(&state, params).await,
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

// ---------- 传输面(send / set_recv_dir) ----------

/// 从 params 取字符串数组字段(逐项必须为字符串)。
fn need_str_array(params: &Value, key: &str) -> Result<Vec<String>, RpcError> {
    let items: Option<Vec<String>> = params
        .get(key)
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        });
    match items {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: {key} 须为非空字符串数组"),
        }),
    }
}

/// `send {files:[String], to:String}` → `{}`。在 RPC 内长调用跑完整发送
/// 编排(ipc 分派对每请求各自 await,长调用合法;同连接后续请求排队,并发
/// 调用方另开连接)。并发第二传经发送互斥 try_lock 立即拒绝 code=2「传输
/// 进行中」,不排队不打断在途第一传。out_report 每行一条 event.send_log
/// 广播;成功 `{}`,失败 code=1 且 message 为失败报告全文。
async fn send(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let files = need_str_array(&params, "files")?;
    let to = need_str(&params, "to")?;
    let Some(session) = state.session() else {
        return Err(RpcError {
            code: ERR_BUSINESS,
            message: "未登录,请先 login.verify".into(),
        });
    };
    // 互斥先于编排:占用即拒,长传输期间不累积排队请求
    let Some(_permit) = state.try_send_permit() else {
        return Err(RpcError {
            code: ERR_BUSY,
            message: "传输进行中".into(),
        });
    };

    let home = state.home();
    let paths: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
    let opts = state.send_opts();
    let events = state.events();
    let report = move |line: &str| {
        let _ = events.send(EventFrame {
            method: EV_SEND_LOG.into(),
            params: json!({ "line": line }),
        });
    };
    crate::flow::run_send_with(&session, &home, &paths, &to, &report, &opts)
        .await
        .map(|()| json!({}))
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("{e:#}"),
        })
}

/// `set_recv_dir {dir}` → `{}`。state 即时更新 + settings.json 持久化;
/// 常驻接收的当前等待轮被通知打断重进,下一轮即用新目录。只持久化字符串
/// 不落盘目录——目录可用性由接收落盘时兜底,坏路径走传输失败报告。
async fn set_recv_dir(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let dir = need_str(&params, "dir")?;
    if dir.trim().is_empty() {
        return Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: "缺少或非法参数: dir 不能为空".into(),
        });
    }
    state
        .set_recv_dir(PathBuf::from(&dir))
        .map(|()| json!({}))
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("设置接收目录失败: {e:#}"),
        })
}

// ---------- 开机自启(autostart.*) ----------

/// 从 params 取布尔字段;缺失或类型错即 -32602。
fn need_bool(params: &Value, key: &str) -> Result<bool, RpcError> {
    params
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: {key}"),
        })
}

/// `autostart.get {}` → `{"enabled":bool}`。读 settings.autostart(设置面
/// 权威);Windows 上 set 成功后注册表与该值同步。
fn autostart_get(state: &DaemonState) -> Value {
    json!({ "enabled": state.autostart() })
}

/// `autostart.set {enabled}` → `{}`。先落注册表(Windows;失败即整体
/// 回退,设置不动),再更新 settings 并持久化。托盘「开机自启」勾选走
/// 同一 state.set_autostart 路径,语义完全一致。
async fn autostart_set(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let enabled = need_bool(&params, "enabled")?;
    state
        .set_autostart(enabled)
        .map(|()| json!({}))
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("设置开机自启失败: {e:#}"),
        })
}

/// HKCU 开机自启 Run 键路径(Windows 用户级,无需提权)。
#[cfg(windows)]
pub(crate) const AUTOSTART_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Run 键里代表 MTDSS 的值名。
#[cfg(windows)]
pub(crate) const AUTOSTART_RUN_VALUE: &str = "MTDSS";

/// Windows:把当前 exe(带引号完整路径)写入 HKCU Run 的 MTDSS 值;
/// 关闭即删除该值(不存在视为幂等成功)。删除不回滚:Run 键本身保留。
#[cfg(windows)]
pub(crate) fn registry_set_autostart(enabled: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    // create_subkey 已存在即打开(带读写权限),键缺失则建(Run 键常规在场)
    let run = hkcu
        .create_subkey(AUTOSTART_RUN_KEY)
        .with_context(|| format!("打开注册表键 HKCU\\{AUTOSTART_RUN_KEY} 失败"))?
        .0;
    if enabled {
        let exe =
            std::env::current_exe().context("定位当前可执行文件失败(current_exe)")?;
        let quoted = format!("\"{}\"", exe.display());
        run.set_value(AUTOSTART_RUN_VALUE, &quoted)
            .with_context(|| format!("写入 Run 值 {AUTOSTART_RUN_VALUE} 失败"))?;
    } else {
        match run.delete_value(AUTOSTART_RUN_VALUE) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(anyhow::Error::new(err)
                    .context(format!("删除 Run 值 {AUTOSTART_RUN_VALUE} 失败")));
            }
        }
    }
    Ok(())
}

/// 非 Windows:M3a 开机自启无注册表面(Linux/macOS 自启机制不在本里程碑),
/// 仅随 settings.json 持久化;RPC 语义保持一致(set/get 仍闭环)。
#[cfg(not(windows))]
pub(crate) fn registry_set_autostart(_enabled: bool) -> anyhow::Result<()> {
    Ok(())
}
