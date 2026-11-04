//! RPC 方法分派:把 JSON-RPC 方法映射到守护进程状态操作。
//!
//! `hello`/`status`/`devices` 为查询(`devices` 为现拉:REST 5s 超时,失败
//! 回退缓存 stale=true,Task 7;M4a Task 5 加 1s 单飞备忘,并发 RPC 共享
//! 一次 REST 拉取);`login.*` 为写路径(REST 进出 + 状态
//! 翻转);`send`/`set_recv_dir` 为传输面(send 在 RPC 内长调用跑完整编排,
//! 并发第二传经发送互斥立即拒绝);`list`/`stat` 为远程浏览面、`delete`/
//! `move`/`copy` 为同机操作面(本机短路 core fs——fs 调用经 CtrlPool 内
//! spawn_blocking 落阻塞线程池,其余经 CtrlPool 打洞,M3b/M3c);`transfer.*`
//! 为作业化传输面(M3c Task 6/7/8:start[upload/download/local_copy/
//! remote_copy]/list/pause/resume/cancel/wait;v2-P2 Task 6 增 reinitiate
//! ——失败/中断任务翻新重发;v2-P2 Task 7 增 clear——按记录类型清残留,
//! 在线回执/离线失败保留;作业异步驱动,不经 send 互
//! 斥——并发合法,进度/状态经 `event.transfer` 广播);
//! `autostart.*` 为设置面(Task 9:Windows 落 HKCU Run 注册表,非 Windows
//! 仅随 settings 持久化);`login.*` 的 V3-1 状态机面:verify 新形态
//! (status 判别:ok/approval/limit)、续接 op(submit_code/status/evict/
//! cancel)、扫码面(qr_begin/qr_status)、受信终端应答(approval.respond)
//! 与账号数据 KV 透传(kv.get/put;404 typed not_found)——jwt/refresh 恒
//! 不出 IPC,中间态续接凭据由 daemon 槽内跟踪(spec §4.2 全表);
//! `update.prefs.get/set` 读写更新器偏好(检查/
//! 安装开关,M4b Task 10;关检查连带关安装,矛盾组合 -32602);
//! `update.check/state/apply` 为更新核心面(M4b Task 11:强制检查/状态
//! 快照/下载至 ready;M4 仅 Windows——非 Windows 全臂 UNSUPPORTED_PLATFORM);
//! 未知方法回
//! -32601。错误码约定:-32602 参数缺失/
//! 非法;1 业务失败(验证码错误、服务器拒绝、传输失败报告、远端浏览/操作
//! 失败、跨设备拒绝、transfer.* 语义校验[策略超限/dir 非法/任务不存在或
//! 已终态]等),message 携带上下文;2 发送互斥占用(仅遗留 send);3 对端
//! 离线(list/stat/delete/move/copy,目录快照 online=false 或信令未就绪;
//! transfer.clear 的对端残留清理同此码位)。
//! 事件:send 的进度逐行广播 `event.send_log`;作业的状态翻转(即发)与
//! 节流进度(≥400ms)广播 `event.transfer`;登录/登出翻转成功后由 state
//! 层广播 `event.login {"logged_in":bool}`(登出幂等成功亦发,UI 据此全量
//! 刷新/回引导态)。

use mtdss_ipc::{Dispatcher, EventFrame, RpcError};
use serde_json::{json, Value};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::ctrl::{CtrlErr, CtrlPool};
use crate::jobs::check_policy_limits;
use crate::session::Session;
use crate::state::{DaemonState, DEVICES_MEMO_TTL, PendingLogin};

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

/// 守护进程分派器:持共享状态的读写视图与控制通道连接池。
pub(crate) struct ApiDispatcher {
    state: Arc<DaemonState>,
    ctrl: Arc<CtrlPool>,
}

impl ApiDispatcher {
    pub(crate) fn new(state: Arc<DaemonState>, ctrl: Arc<CtrlPool>) -> Self {
        Self { state, ctrl }
    }
}

impl Dispatcher for ApiDispatcher {
    fn dispatch(
        &self,
        method: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>> {
        let state = self.state.clone();
        let ctrl = self.ctrl.clone();
        Box::pin(async move {
            match method.as_str() {
                "hello" => Ok(hello(&state)),
                "status" => Ok(status(&state)),
                "devices" => devices(&state).await,
                "login.request_code" => login_request_code(&state, params).await,
                "login.verify" => login_verify(&state, params).await,
                "login.submit_code" => login_submit_code(&state, params).await,
                "login.status" => login_status(&state, params).await,
                "login.evict" => login_evict(&state, params).await,
                "login.cancel" => login_cancel(&state).await,
                "login.qr_begin" => login_qr_begin(&state, params).await,
                "login.qr_status" => login_qr_status(&state, params).await,
                "approval.respond" => approval_respond(&state, params).await,
                "kv.get" => kv_get(&state, params).await,
                "kv.put" => kv_put(&state, params).await,
                "login.logout" => login_logout(&state).await,
                "send" => send(&state, params).await,
                "set_recv_dir" => set_recv_dir(&state, params).await,
                "transfer.start" => transfer_start(&state, params).await,
                "transfer.list" => Ok(transfer_list(&state)),
                "transfer.pause" => transfer_set_paused(&state, params, true).await,
                "transfer.resume" => transfer_set_paused(&state, params, false).await,
                "transfer.cancel" => transfer_cancel(&state, params).await,
                "transfer.wait" => transfer_wait(&state, params).await,
                "transfer.reinitiate" => transfer_reinitiate(&state, params).await,
                "transfer.clear" => transfer_clear(&state, params).await,
                "autostart.get" => Ok(autostart_get(&state)),
                "autostart.set" => autostart_set(&state, params).await,
                "update.prefs.get" => Ok(update_prefs_get(&state)),
                "update.prefs.set" => update_prefs_set(&state, params),
                "update.check" => update_check(&state).await,
                "update.state" => update_state(&state),
                "update.apply" => update_apply(&state, params).await,
                "list" => ctrl_list(&state, &ctrl, params).await,
                "stat" => ctrl_stat(&state, &ctrl, params).await,
                "delete" => ctrl_delete(&state, &ctrl, params).await,
                "move" => ctrl_move(&state, &ctrl, params).await,
                "copy" => ctrl_copy(&state, &ctrl, params).await,
                other => Err(RpcError {
                    code: ERR_METHOD_NOT_FOUND,
                    message: format!("method not found: {other}"),
                    data: None,
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

/// `status` 的 recv_dir 投影(v2-P3 T6 平台参数化):android = 收件箱
/// 哨兵(收件目的地固定,UI 呈现「收件箱」);桌面 = 实际接收目录字符串。
/// 参数化本体使契约 golden 能在桌面 host 上钉**双变体**(status 与
/// status.resp.android 的 recv_dir 值同源于此);生产调用点传入编译期
/// 常量 `cfg!`(桌面构建零分支、零行为面变化)。
fn recv_dir_display(android: bool, actual: &std::path::Path) -> String {
    if android {
        mtdss_core::transport::intake::INBOX_SENTINEL.to_string()
    } else {
        actual.to_string_lossy().into_owned()
    }
}

/// `status`:运行态快照。online 取自身连接态(在线循环维护,Task 6 接线)。
fn status(state: &DaemonState) -> Value {
    let session = state.session();
    json!({
        "logged_in": session.is_some(),
        "online": state.online(),
        "server": session.as_ref().map(|s| s.server.clone()),
        "device": device_value(session.as_ref()),
        "recv_dir": recv_dir_display(cfg!(target_os = "android"), &state.recv_dir()),
    })
}

/// `devices`:现拉(Task 7)——REST 目录为权威,5s 超时内命中即更新缓存并
/// 返回 stale=false(顺带刷新枢纽公钥缓存,与在线循环同一投影口径);现拉
/// 失败/超时回退缓存快照 stale=true。未登录无缓存语义,返回空列表
/// stale=false(没有「陈旧」可言)。每台恰含 device_id/name/platform/
/// online,本机条目 online 恒取本地连接态(不取服务器 presence)。
/// M4a Task 5 单飞:并发 RPC 在 memo 锁上排队,队首持锁现拉并落 memo,
/// 后来者醒来见新鲜 memo(age < DEVICES_MEMO_TTL)即复用——一次 REST 拉
/// 取服务整屏并发;失败/超时不落 memo(无负缓存),各调用沿原路径回退。
async fn devices(state: &Arc<DaemonState>) -> Result<Value, RpcError> {
    const FRESH_TIMEOUT: Duration = Duration::from_secs(5);
    let Some(session) = state.session() else {
        return Ok(json!({ "devices": [], "stale": false }));
    };
    // 单飞入口:持 memo 锁完成「查 memo → 过窗则现拉 → 落 memo」。tokio
    // Mutex 因持锁跨 await 而选;现拉撞外层 5s 超时取消本 future 时 guard
    // 随之释放(tokio Mutex 取消安全),排队者获锁重试,无死锁、无半截 memo。
    let memo_arc = state.devices_memo();
    let mut memo = memo_arc.lock().await;
    // 新鲜 memo 直接复用:以当下在线态重投影快照(本机条目 online 取本地
    // 连接态);公钥缓存不必重建——填 memo 的那次现拉已 load_pks,窗口内
    // 目录数据同一份。
    if let Some(raw) = memo
        .as_ref()
        .filter(|m| m.0.elapsed() < DEVICES_MEMO_TTL)
        .map(|(_, raw)| raw.clone())
    {
        drop(memo);
        let snap =
            crate::hub::build_snapshot(&raw, state.self_device_id().as_deref(), state.online());
        return Ok(json!({ "devices": snap, "stale": false }));
    }
    match tokio::time::timeout(FRESH_TIMEOUT, crate::flow::fetch_devices(&session)).await {
        Ok(Ok(raw)) => {
            *memo = Some((std::time::Instant::now(), raw.clone()));
            drop(memo);
            let snap = crate::hub::build_snapshot(
                &raw,
                state.self_device_id().as_deref(),
                state.online(),
            );
            state.set_devices(snap.clone());
            state.hub().load_pks(&raw);
            Ok(json!({ "devices": snap, "stale": false }))
        }
        Ok(Err(e)) => {
            tracing::warn!(%e, "devices 现拉失败,回退缓存快照");
            Ok(json!({
                "devices": state.devices().unwrap_or_default(),
                "stale": true,
            }))
        }
        Err(_) => {
            tracing::warn!("devices 现拉超时({FRESH_TIMEOUT:?}),回退缓存快照");
            Ok(json!({
                "devices": state.devices().unwrap_or_default(),
                "stale": true,
            }))
        }
    }
}

// ---------- login.*(V3-1 登录状态机:直入/审批/踢选/扫码四路同一收口) ----------

/// 从 params 取字符串字段;缺失或类型错即 -32602。值为字符串即可(允许
/// 空串):list/stat 的 `path` 以空串表示根卷,复用本 helper,不另设变体。
fn need_str(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: {key}"),
            data: None,
        })
}

/// 可选字符串路径参数(自签 CA 证书路径)。
fn opt_path(params: &Value, key: &str) -> Option<PathBuf> {
    params.get(key).and_then(|v| v.as_str()).map(PathBuf::from)
}

/// LoginStep → IPC 投影(status 判别面,spec §4.2 全表)。jwt/refresh 恒
/// 不出 IPC(凭据只进会话落盘);`name` 为 ok 形态的终端名(verify 用请求
/// 参数,续接 op 用槽内候选名;缺席为 null);limit 的 devices 即服务器
/// 清单(踢选屏数据源,键序随 serde 字母序)。
fn step_json(step: &crate::account::LoginStep, name: Option<&str>) -> Value {
    use crate::account::LoginStep as S;
    match step {
        S::Ok(ok) => json!({
            "status": "ok",
            "device_id": ok.device_id,
            "name": name,
        }),
        S::Pending { expires_at } => json!({
            "status": "pending",
            "expires_at": expires_at,
        }),
        S::Approval { session_id, expires_at } => json!({
            "status": "approval",
            "session_id": session_id,
            "expires_at": expires_at,
        }),
        S::Limit { enroll_token, devices } => json!({
            "status": "limit",
            "enroll_token": enroll_token,
            "devices": devices,
        }),
        S::Denied => json!({ "status": "denied" }),
        S::Expired => json!({ "status": "expired" }),
    }
}

/// 裁定收口:ok → 会话落盘翻转(续接槽清空)+ ok 投影;其余形态原样投影
/// (approval/limit 的续接凭据已由调用方记入待登录槽)。登录互斥锁由调用
/// 方持(与 verify/logout 同锁,登录态的磁盘与内存翻转原子)。
async fn settle_login(
    state: &DaemonState,
    pending: PendingLogin,
    step: crate::account::LoginStep,
    name: Option<String>,
) -> Result<Value, RpcError> {
    use crate::account::LoginStep as S;
    match step {
        S::Ok(ok) => {
            let device_id = ok.device_id.clone();
            let session = Session {
                server: pending.server,
                jwt: ok.jwt,
                refresh: ok.refresh,
                device_id: device_id.clone(),
                ca: pending.ca,
            };
            state.login(session).map_err(|e| RpcError {
                code: ERR_BUSINESS,
                message: format!("写入会话失败: {e:#}"),
                data: None,
            })?;
            // 收口成功即清槽:登录面结束,残留的中间态凭据不得复用
            // (服务器侧单次凭据亦已消费)。
            state.take_pending();
            let name = name.or(pending.name);
            // ok 投影在此内联(凭据束已被会话消费,只剩 device_id/name 面)
            Ok(json!({
                "status": "ok",
                "device_id": device_id,
                "name": name,
            }))
        }
        step => Ok(step_json(&step, None)),
    }
}

/// `login.request_code {server, email, ca?}` → `{"code":String}` 或 `{}`。
/// dev 回显门开时响应带验证码(本地/e2e 自动预填),产线真实投递响应为
/// `{}`(码走邮件,客户端手输;spec §3.6「有则填」)。server/email/ca 存
/// 入 state 作 pending,verify 阶段按 email 取回。
async fn login_request_code(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let server = need_str(&params, "server")?;
    let email = need_str(&params, "email")?;
    let ca = opt_path(&params, "ca");
    let code = crate::account::request_code(&server, &email, ca.as_deref())
        .await
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("请求验证码失败: {e:#}"),
            data: None,
        })?;
    state.set_pending_login(PendingLogin {
        server,
        email: Some(email),
        ca,
        ..PendingLogin::default()
    });
    Ok(match code {
        Some(c) => json!({ "code": c }),
        None => json!({}),
    })
}

/// `login.verify {email, code, name?}` → status 判别形态:`{"status":"ok",
/// device_id,name}`(直入,会话落盘)/ `{"status":"approval",session_id,
/// expires_at}`(审批挂起,受信终端弹窗或输码)/ `{"status":"limit",
/// enroll_token,devices}`(配额超额,踢选屏)。name 缺省用本机主机名;
/// 身份种子不存在则生成落盘(0600)。旧服务器容错:响应缺 `status` 字段
/// 按 ok 解析(account::parse_login_step)。
///
/// 登录全程持登录互斥锁(身份生成 → REST → 会话落盘翻转原子化);中间态
/// (approval/limit)把续接凭据记回待登录槽——后续 submit_code/status/
/// evict 凭凭据查槽,server/CA 不必重传。
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
        data: None,
    })?;

    let _guard = state.lock_login().await;
    let id = crate::account::load_identity(&state.home()).map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("加载本机身份失败: {e:#}"),
        data: None,
    })?;
    let pubkey_hex = hex::encode(id.public_key().as_bytes());

    let step = crate::account::verify(
        &pending.server,
        &email,
        &code,
        &pubkey_hex,
        &name,
        pending.ca.as_deref(),
    )
    .await
    .map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("验证失败: {e:#}"),
        data: None,
    })?;

    // 中间态记槽:审批会话号/踢选凭据 + 候选名(续接 op 的 ok 投影回显)。
    // 槽已被别的登录流覆盖(update_pending_for 返回 false)时仍照常投影
    // ——本 verify 的会话在服务器侧依然有效,客户端可直接轮询(槽重登记后
    // submit_code 才需重验)。
    match &step {
        crate::account::LoginStep::Approval { session_id, expires_at } => {
            state.update_pending_for(&email, |p| {
                p.session_id = Some(session_id.clone());
                p.name = Some(name.clone());
                p.expires_at = Some(*expires_at);
            });
        }
        crate::account::LoginStep::Limit { enroll_token, .. } => {
            state.update_pending_for(&email, |p| {
                p.enroll_token = Some(enroll_token.clone());
                p.name = Some(name.clone());
            });
        }
        _ => {}
    }

    match step {
        // 直入:即落盘翻转(take 沿旧序在落盘成功之后——失败时槽保留,
        // 客户端重试 verify 不至死路)
        step @ crate::account::LoginStep::Ok(_) => {
            settle_login(state, pending, step, Some(name)).await
        }
        step => Ok(step_json(&step, None)),
    }
}

/// `login.submit_code {session_id, code}` → 同 verify 新形态。新终端直接输
/// 受信终端弹窗上的审批码 settle:approve-code 成功(或 409 已被并发他路
/// 裁决)后即时轮询 login-status 单次领票收口——领到 ok 即登录;码错/会话
/// 未知等业务失败 code 1(带 HTTP 上下文)。
async fn login_submit_code(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let session_id = need_str(&params, "session_id")?;
    let code = need_str(&params, "code")?;
    let pending =
        state.pending_by_session(&session_id).ok_or_else(|| RpcError {
            code: ERR_INVALID_PARAMS,
            message: "没有该审批会话的待登录记录,请先调用 login.verify".into(),
            data: None,
        })?;
    let _guard = state.lock_login().await;
    match crate::account::approve_code(&pending.server, &session_id, &code, pending.ca.as_deref())
        .await
    {
        Ok(crate::account::ApproveOutcome::Approved)
        | Ok(crate::account::ApproveOutcome::AlreadySettled) => {
            // settle 后即时领票:approved 单次,并发他路先领时本侧按
            // Expired/Denied 形态呈现(轮询面无错误通道)
            let step = crate::account::login_status(&pending.server, &session_id, pending.ca.as_deref())
                .await
                .map_err(|e| RpcError {
                    code: ERR_BUSINESS,
                    message: format!("审批码已受理但领票失败: {e:#}"),
                    data: None,
                })?;
            settle_login(state, pending, step, None).await
        }
        Err(e) => Err(RpcError {
            code: ERR_BUSINESS,
            message: format!("提交审批码失败: {e:#}"),
            data: None,
        }),
    }
}

/// `login.status {session_id}` → 同 verify 新形态。审批轮询收口(服务端
/// push 不进 V3-1):approval=继续等、ok=领票登录、denied/expired=终态文案、
/// limit=转踢选屏。轮询终态是正常投影,不是 RPC 错误。
async fn login_status(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let session_id = need_str(&params, "session_id")?;
    let pending = state.pending_by_session(&session_id).ok_or_else(|| RpcError {
        code: ERR_INVALID_PARAMS,
        message: "没有该审批会话的待登录记录,请先调用 login.verify".into(),
        data: None,
    })?;
    let _guard = state.lock_login().await;
    let step = crate::account::login_status(&pending.server, &session_id, pending.ca.as_deref())
        .await
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("查询审批状态失败: {e:#}"),
            data: None,
        })?;
    // limit 中间态同样记槽(审批通过但配额超额的转场),踢选凭据可续接
    if let crate::account::LoginStep::Limit { enroll_token, .. } = &step {
        state.update_pending_session(&session_id, |p| {
            p.enroll_token = Some(enroll_token.clone());
        });
    }
    settle_login(state, pending, step, None).await
}

/// `login.evict {enroll_token, device_id}` → 同 verify ok 形态。踢选屏动作:
/// 消费单次 enroll_token 删除所选设备并即时腾位,本机 enroll 收口登录。
async fn login_evict(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let enroll_token = need_str(&params, "enroll_token")?;
    let device_id = need_str(&params, "device_id")?;
    let pending = state.pending_by_enroll(&enroll_token).ok_or_else(|| RpcError {
        code: ERR_INVALID_PARAMS,
        message: "没有该踢选凭据的待登录记录,请先调用 login.verify".into(),
        data: None,
    })?;
    let _guard = state.lock_login().await;
    let step = crate::account::evict(
        &pending.server,
        &enroll_token,
        &device_id,
        pending.ca.as_deref(),
    )
    .await
    .map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("踢选失败: {e:#}"),
        data: None,
    })?;
    settle_login(state, pending, step, None).await
}

/// `login.cancel {}` → `{}`。作废本地待登录续接槽(审批/踢选/扫码中间态
/// 一并清);服务端会话无取消端点,靠自身 TTL 收口(审批 300s/踢选凭据
/// 600s/扫码 120s),到期后原凭据全部失效。
async fn login_cancel(state: &DaemonState) -> Result<Value, RpcError> {
    state.take_pending();
    Ok(json!({}))
}

/// `login.qr_begin {server, ca?, name?}` → `{"qr_url",token,expires_at}`。
/// PC 扫码登录发起:铸 128bit 单次 token(120s),二维码内容由 daemon 拼
/// `https://www.mtdss.top/qr?t=<token>`(spec §3.2,服务端不参与)。身份
/// 种子不存在则生成落盘(0600);server/ca/name 存入待登录槽,qr_status
/// 凭 token 查槽。
async fn login_qr_begin(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let server = need_str(&params, "server")?;
    let ca = opt_path(&params, "ca");
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => gethostname::gethostname().to_string_lossy().into_owned(),
    };
    let _guard = state.lock_login().await;
    let id = crate::account::load_identity(&state.home()).map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("加载本机身份失败: {e:#}"),
        data: None,
    })?;
    let pubkey_hex = hex::encode(id.public_key().as_bytes());
    let b = crate::account::qr_begin(&server, &pubkey_hex, &name, ca.as_deref())
        .await
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("发起扫码登录失败: {e:#}"),
            data: None,
        })?;
    let token = b.qr_token.clone();
    state.set_pending_login(PendingLogin {
        server,
        ca,
        email: None,
        name: Some(name),
        session_id: None,
        enroll_token: None,
        qr_token: Some(token.clone()),
        expires_at: Some(b.expires_at),
    });
    Ok(json!({
        "qr_url": crate::account::qr_url(&token),
        "token": token,
        "expires_at": b.expires_at,
    }))
}

/// `login.qr_status {token}` → 同 verify 新形态(pending/ok/denied/expired/
/// limit)。PC 轮询收口:ok 单次领票即登录;pending 是正常投影(继续轮询),
/// 不是 RPC 错误。
async fn login_qr_status(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let token = need_str(&params, "token")?;
    let pending = state.pending_by_qr(&token).ok_or_else(|| RpcError {
        code: ERR_INVALID_PARAMS,
        message: "没有该扫码会话的待登录记录,请先调用 login.qr_begin".into(),
        data: None,
    })?;
    let _guard = state.lock_login().await;
    let step = crate::account::qr_status(&pending.server, &token, pending.ca.as_deref())
        .await
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("查询扫码状态失败: {e:#}"),
            data: None,
        })?;
    settle_login(state, pending, step, None).await
}

/// `approval.respond {session_id, allow}` → `{}`。受信终端弹窗应答(Bearer
/// 面须为该账号在线受信设备):allow=false 一票否决。
async fn approval_respond(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let session_id = need_str(&params, "session_id")?;
    let allow = need_bool(&params, "allow")?;
    let session = state.session().ok_or_else(|| RpcError {
        code: ERR_BUSINESS,
        message: "未登录,无法应答审批请求".into(),
        data: None,
    })?;
    crate::account::approval_respond(
        &session.server,
        &session.jwt,
        &session_id,
        allow,
        session.ca.as_deref(),
    )
    .await
    .map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("应答审批失败: {e:#}"),
        data: None,
    })?;
    Ok(json!({}))
}

// ---------- 账号数据 KV 透传(spec §3.8;V3-3 消费值语义) ----------

/// `kv.get {key}` → `{"value":String}`。404 = 合法未命(无此键)→
/// typed 错误 `data.kind = "not_found"`(操作端类型化判定,同 fs 稳定码
/// 房风);其余失败 code 1。
async fn kv_get(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let key = need_str(&params, "key")?;
    let session = state.session().ok_or_else(|| RpcError {
        code: ERR_BUSINESS,
        message: "未登录,账号数据不可用".into(),
        data: None,
    })?;
    match crate::account::kv_get(&session, &key).await {
        Ok(Some(value)) => Ok(json!({ "value": value })),
        Ok(None) => Err(RpcError::with_data(
            ERR_BUSINESS,
            format!("无此键: {key}"),
            json!({ "kind": "not_found" }),
        )),
        Err(e) => Err(RpcError {
            code: ERR_BUSINESS,
            message: format!("读取账号数据失败: {e:#}"),
            data: None,
        }),
    }
}

/// `kv.put {key, value}` → `{"ok":true}`。UPSERT 本账号 KV(value ≤256KiB
/// 由服务端校验);成功后同账号其他在线端收 data_changed 通告(服务端推)。
async fn kv_put(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let key = need_str(&params, "key")?;
    let value = need_str(&params, "value")?;
    let session = state.session().ok_or_else(|| RpcError {
        code: ERR_BUSINESS,
        message: "未登录,账号数据不可用".into(),
        data: None,
    })?;
    crate::account::kv_put(&session, &key, &value)
        .await
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("写入账号数据失败: {e:#}"),
            data: None,
        })?;
    Ok(json!({ "ok": true }))
}

/// `login.logout {}` → `{}`。幂等:未登录同样返回成功(删不存在的
/// session.json 不算错);磁盘删除失败才报业务错。与 verify 同锁:串行化
/// 登录态翻转,登出不插队在途 verify 的落盘中间。
async fn login_logout(state: &DaemonState) -> Result<Value, RpcError> {
    let _guard = state.lock_login().await;
    state.logout().map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("登出失败: {e:#}"),
        data: None,
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
            data: None,
        }),
    }
}

/// 可选字符串参数(存在且为字符串才取;空串按未提供处理,与「缺省 =
/// 旧行为」同义)。
fn opt_str(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

/// `send {files:[String], to:String, dir?:String}` → `{}`。在 RPC 内长调用
/// 跑完整发送编排(ipc 分派对每请求各自 await,长调用合法;同连接后续请求
/// 排队,并发调用方另开连接)。`dir` 为可选定向落盘目录(M3c Task 5,随
/// ConnectRequest 走密封信封;缺省/空 = 旧行为,合法性由接收方判定,非法
/// 回退默认收件目录)。并发第二传经发送互斥 try_lock 立即拒绝 code=2
/// 「传输进行中」,不排队不打断在途第一传。out_report 每行一条
/// event.send_log 广播;成功 `{}`,失败 code=1 且 message 为失败报告全文。
async fn send(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let files = need_str_array(&params, "files")?;
    let to = need_str(&params, "to")?;
    let Some(session) = state.session() else {
        return Err(RpcError {
            code: ERR_BUSINESS,
            message: "未登录,请先 login.verify".into(),
            data: None,
        });
    };
    // 互斥先于编排:占用即拒,长传输期间不累积排队请求
    let Some(_permit) = state.try_send_permit() else {
        return Err(RpcError {
            code: ERR_BUSY,
            message: "传输进行中".into(),
            data: None,
        });
    };

    let home = state.home();
    let paths: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
    // 源表物化(v2-P3 T4):顶层输入展开成 (SourceFile::Path, rel) 表
    // (collect_items 逻辑零改动,行为与泛化前逐字节一致;bridge 形态
    // 不进本 RPC——走 transfer.start 的作业面)
    let sources = match crate::flow::path_sources(&paths).await {
        Ok(v) => v,
        Err(e) => {
            return Err(RpcError {
                code: ERR_BUSINESS,
                message: format!("{e:#}"),
                data: None,
            })
        }
    };
    let opts = state.send_opts();
    let events = state.events();
    let report = move |line: &str| {
        let _ = events.send(EventFrame {
            method: EV_SEND_LOG.into(),
            params: json!({ "line": line }),
        });
    };
    // 定向规格:本任务只接 dir(其余接线:job_id 留空——Task 6/7 的 jobs
    // 面经 jobs RPC 携带后再填;空表策略线上不携带,legacy 等价;
    // display_name(V3-1 §6)按代表名规则现算——遗留 send 无作业卡,同一
    // 规则保证新接收端会话行与 jobs 面传输两端同名)
    let spec = crate::flow::SendSpec {
        target_dir: opt_str(&params, "dir").unwrap_or_default(),
        job_id: String::new(),
        display_name: crate::jobs::representative_name(
            &paths[0].to_string_lossy(),
            paths.len(),
        ),
        policy: Default::default(),
    };
    // 遗留 send 无暂停/取消/中继标记面(SendCtl 全空——含 on_relay None,
    // 零行为差;作业化传输走 transfer.start,由 jobs 驱动注入 ctl)
    let ctl = crate::flow::SendCtl::default();
    crate::flow::run_send_with(
        &session, &home, state.hub(), sources, &to, &spec, &report, &opts, &ctl,
    )
    .await
    .map(|()| json!({}))
    .map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: format!("{e:#}"),
        data: None,
    })
}

/// `set_recv_dir {dir}` → `{}`。state 即时更新 + settings.json 持久化;
/// 常驻接收的当前等待轮被通知打断重进,下一轮即用新目录。只持久化字符串
/// 不落盘目录——目录可用性由接收落盘时兜底,坏路径走传输失败报告。
///
/// android(v2-P3 T6):收件箱固定(目的地是 provider 侧 fd 桥,spec
/// §5.2)——改收件目录的业务在安卓端不存在,业务错误回绝;桌面路径与
/// 文案零变化(`inbox_fixed()` 桌面恒 false,编译期折叠)。文案常量经
/// lib.rs `#[doc(hidden)]` 导出,契约 golden 据此在桌面 host 生成
/// android 变体帧(set_recv_dir.resp.android.json)。
async fn set_recv_dir(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let dir = need_str(&params, "dir")?;
    if inbox_fixed() {
        return Err(RpcError {
            code: ERR_BUSINESS,
            message: crate::ANDROID_SET_RECV_DIR_ERR.into(),
            data: None,
        });
    }
    if dir.trim().is_empty() {
        return Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: "缺少或非法参数: dir 不能为空".into(),
            data: None,
        });
    }
    state
        .set_recv_dir(PathBuf::from(&dir))
        .map(|()| json!({}))
        .map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("设置接收目录失败: {e:#}"),
            data: None,
        })
}

/// 平台判定:android = 收件箱固定(true);桌面 = false。编译期常量
/// (`cfg!`),桌面构建零分支开销。
fn inbox_fixed() -> bool {
    cfg!(target_os = "android")
}

// ---------- 传输作业面(transfer.*) ----------

/// 可选字符串数组参数(transfer.start 的 skip/overwrite):缺省/Null →
/// 空表;存在则逐项必须为字符串。
fn opt_str_list(params: &Value, key: &str) -> Result<Vec<String>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str().map(str::to_owned).ok_or_else(|| RpcError {
                    code: ERR_INVALID_PARAMS,
                    message: format!("缺少或非法参数: {key} 须为字符串数组"),
                    data: None,
                })
            })
            .collect(),
        Some(_) => Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: {key} 须为字符串数组"),
            data: None,
        }),
    }
}

/// `transfer.start` upload(bridge)的 items 条目解析(v2-P3 T4):
/// `[{ref:String, rel:String, size:u64, mtime:i64}]` →
/// [`crate::jobs::BridgeItem`](字段缺失/类型不符 → -32602;语义校验
/// [rel 合法/重复/ref 空]在 jobs 注册表收口,与路径形态的预扫校验同位)。
fn need_bridge_items(params: &Value) -> Result<Vec<crate::jobs::BridgeItem>, RpcError> {
    let arr = params
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError {
            code: ERR_INVALID_PARAMS,
            message: "缺少或非法参数: items 须为数组".into(),
            data: None,
        })?;
    let bad = |i: usize, what: &str| RpcError {
        code: ERR_INVALID_PARAMS,
        message: format!("缺少或非法参数: items[{i}] 的 {what} 缺失或类型不符"),
        data: None,
    };
    let mut items = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        let r#ref = v
            .get("ref")
            .and_then(Value::as_str)
            .ok_or_else(|| bad(i, "ref"))?
            .to_owned();
        let rel = v
            .get("rel")
            .and_then(Value::as_str)
            .ok_or_else(|| bad(i, "rel"))?
            .to_owned();
        let size = v
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad(i, "size"))?;
        let mtime = v
            .get("mtime")
            .and_then(Value::as_i64)
            .ok_or_else(|| bad(i, "mtime"))?;
        items.push(crate::jobs::BridgeItem {
            r#ref,
            rel,
            size,
            mtime,
        });
    }
    Ok(items)
}

/// `transfer.start` → `{"job_id":"<u64 字符串>"}`。M3c Task 6 起作业化
/// 传输:upload 形态 `{kind:"upload",files,dir,skip?,overwrite?,cut?}`
/// 或(v2-P3 T4)`{kind:"upload",source:"bridge",items:[{ref,rel,size,
/// mtime}],dir,skip?,overwrite?,cut?}`(fd 桥:元数据先行,驱动起步
/// 逐文件 fs.request_source 懒解析 fd);
/// download 形态 `{kind:"download",device,paths,dir,skip?,overwrite?,cut?}`
/// (device = 源设备,paths = 远端绝对路径,dir = 本机落盘目录;下载源不能
/// 是本机,本机复制请用 local_copy);local_copy 形态
/// `{kind:"local_copy",paths,dir,cut?,skip?,overwrite?}`(纯本地,不要求
/// 登录;M4b 移交债①起收冲突策略,skip/overwrite 经
/// [`policy_from_params`]——自带 limits 校验);remote_copy
/// 形态 `{kind:"remote_copy","from":dev_id,"paths":[…],"to":dev_id,
/// "dir":abs@to,"skip":[],"overwrite":[],"cut":false}`(Task 8:from 设备
/// 把 paths 直传给 to 设备的 dir,数据不经本机落地;IPC 契约字段面钉死)。
/// 语义校验(device≠本机、paths 非空、dir 合法、策略上限、预扫为空、未登
/// 录)在 jobs 注册表收口,Err → code 1。作业不经遗留 send 互斥(并发合法;
/// code 2 只属遗留 send)。
async fn transfer_start(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let kind = need_str(&params, "kind")?;
    match kind.as_str() {
        "upload" => {
            // 源形态判别(v2-P3 T4):缺省或 "files" = 路径形态(桌面,
            // 语义零变——files/to/dir 校验次序与泛化前一致);
            // "bridge" = fd 桥形态(items 元数据 + 懒 fd,Android
            // 客户端)。其余值显式拒(契约面防静默走错形态)。
            let started: Result<String, RpcError> = match params
                .get("source")
                .and_then(Value::as_str)
            {
                None | Some("files") => {
                    let files = need_str_array(&params, "files")?;
                    let to = need_str(&params, "to")?;
                    let dir = need_str(&params, "dir")?;
                    let policy = mtdss_core::transport::intake::TransferPolicy {
                        skip: opt_str_list(&params, "skip")?,
                        overwrite: opt_str_list(&params, "overwrite")?,
                    };
                    let cut = params.get("cut").and_then(Value::as_bool).unwrap_or(false);
                    let paths: Vec<PathBuf> =
                        files.into_iter().map(PathBuf::from).collect();
                    state
                        .jobs()
                        .start_upload(state, paths, &to, &dir, policy, cut)
                        .await
                        .map_err(|m| RpcError {
                            code: ERR_BUSINESS,
                            message: m,
                            data: None,
                        })
                }
                Some("bridge") => {
                    let items = need_bridge_items(&params)?;
                    let to = need_str(&params, "to")?;
                    let dir = need_str(&params, "dir")?;
                    let policy = mtdss_core::transport::intake::TransferPolicy {
                        skip: opt_str_list(&params, "skip")?,
                        overwrite: opt_str_list(&params, "overwrite")?,
                    };
                    let cut = params.get("cut").and_then(Value::as_bool).unwrap_or(false);
                    state
                        .jobs()
                        .start_upload_bridge(state, items, &to, &dir, policy, cut)
                        .await
                        .map_err(|m| RpcError {
                            code: ERR_BUSINESS,
                            message: m,
                            data: None,
                        })
                }
                Some(other) => Err(RpcError {
                    code: ERR_INVALID_PARAMS,
                    message: format!("缺少或非法参数: 未知 source 形态: {other}"),
                    data: None,
                }),
            };
            started
                .map(|id| json!({ "job_id": id }))
        }
        "download" => {
            let device = need_str(&params, "device")?;
            let paths = need_str_array(&params, "paths")?;
            let dir = need_str(&params, "dir")?;
            let policy = mtdss_core::transport::intake::TransferPolicy {
                skip: opt_str_list(&params, "skip")?,
                overwrite: opt_str_list(&params, "overwrite")?,
            };
            let cut = params.get("cut").and_then(Value::as_bool).unwrap_or(false);
            state
                .jobs()
                // old_rels 恒空:重发编排携旧 rel 集的入口只在 jobs::
                // reinitiate(transfer.reinitiate),transfer.start 是首发面
                .start_download(state, &device, paths, &dir, policy, cut, &[])
                .map(|id| json!({ "job_id": id }))
                .map_err(|m| RpcError {
                    code: ERR_BUSINESS,
                    message: m,
                    data: None,
                })
        }
        "local_copy" => {
            let paths = need_str_array(&params, "paths")?;
            let dir = need_str(&params, "dir")?;
            let cut = params.get("cut").and_then(Value::as_bool).unwrap_or(false);
            // 冲突策略(M4b 移交债①):policy_from_params 自带 limits 校验
            // (超限 code 1,与 upload/download/remote_copy 臂同口径)。
            let policy = policy_from_params(&params)?;
            let local: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            state
                .jobs()
                .start_local_copy(state, local, &dir, cut, policy)
                .await
                .map(|id| json!({ "job_id": id }))
                .map_err(|m| RpcError {
                    code: ERR_BUSINESS,
                    message: m,
                    data: None,
                })
        }
        "remote_copy" => {
            let from = need_str(&params, "from")?;
            let paths = need_str_array(&params, "paths")?;
            let to = need_str(&params, "to")?;
            let dir = need_str(&params, "dir")?;
            let policy = mtdss_core::transport::intake::TransferPolicy {
                skip: opt_str_list(&params, "skip")?,
                overwrite: opt_str_list(&params, "overwrite")?,
            };
            let cut = params.get("cut").and_then(Value::as_bool).unwrap_or(false);
            state
                .jobs()
                // old_rels 恒空:同 download 臂,重发编排入口只在 reinitiate
                .start_remote_copy(state, &from, paths, &to, &dir, policy, cut, &[])
                .map(|id| json!({ "job_id": id }))
                .map_err(|m| RpcError {
                    code: ERR_BUSINESS,
                    message: m,
                    data: None,
                })
        }
        other => Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: 未知或未支持的 kind: {other}"),
            data: None,
        }),
    }
}

/// 作业快照 → list 条目 JSON(字段面与 event.transfer 一致,另带 src/dst
/// 与显示名 name——v2-P2 Task 4,UI 任务名列数据源)。
fn job_json(j: &crate::jobs::JobSnap) -> Value {
    json!({
        "id": j.id,
        "kind": j.kind,
        "name": j.name,
        "state": j.state,
        "files_done": j.files_done,
        "files_total": j.files_total,
        "bytes_done": j.bytes_done,
        "bytes_total": j.bytes_total,
        "speed_bps": j.speed_bps,
        "skipped": j.skipped,
        // 中继兜底标记(v2-P1c):与 event.transfer 同面,UI 角标数据源。
        "relay": j.relay,
        "src": j.src,
        "dst": j.dst,
        "error": j.error,
    })
}

/// 持久记录 → list 条目 JSON(与 job_json 同形;speed_bps/skipped/relay
/// 是 live 独有面,store 历史行以零值/默认补位。src/dst 按各 kind 建卡时
/// 的投影规则重建:upload 源端 = 本机路径不带设备前缀,其余带 from 前缀;
/// download 目标端 = 本机目录不带前缀,其余带 to 前缀)。
fn task_rec_json(r: &crate::taskstore::TaskRec) -> Value {
    let joined = r.src_paths.join(", ");
    let src = if r.kind == "upload" {
        joined
    } else {
        format!("{}:{joined}", r.from_dev)
    };
    let dst = if r.kind == "download" {
        r.target_dir.clone()
    } else {
        format!("{}:{}", r.to_dev, r.target_dir)
    };
    json!({
        "id": r.id,
        "kind": r.kind,
        "name": r.name,
        "state": r.state,
        "files_done": r.files_done,
        "files_total": r.files_total,
        "bytes_done": r.bytes_done,
        "bytes_total": r.bytes_total,
        "speed_bps": 0,
        "skipped": 0,
        "relay": false,
        "src": src,
        "dst": dst,
        "error": r.error,
    })
}

/// `transfer.list {}` → `{"jobs":[…]}`(v2-P2 Task 4:持久层打底 + live
/// 快照按 id 覆盖合并——live 权威盖掉同 id 的陈旧持久态,store 独有行
/// (重启中断/历史)照常出现;按 id 序号升序)。
pub(crate) fn transfer_list(state: &DaemonState) -> Value {
    let mut jobs: Vec<Value> = state
        .tasks()
        .list()
        .into_iter()
        .map(|r| task_rec_json(&r))
        .collect();
    for snap in state.jobs().snapshot() {
        match jobs
            .iter()
            .position(|j| j["id"].as_str() == Some(snap.id.as_str()))
        {
            Some(i) => jobs[i] = job_json(&snap),
            None => jobs.push(job_json(&snap)),
        }
    }
    jobs.sort_by_key(|j| crate::jobs::job_seq(j["id"].as_str().unwrap_or("")));
    json!({
        "jobs": jobs,
    })
}

/// `transfer.pause|resume {id}` → `{}`。作业缺失或已终态 → code 1
/// (不可再暂停/恢复)。
async fn transfer_set_paused(
    state: &Arc<DaemonState>,
    params: Value,
    paused: bool,
) -> Result<Value, RpcError> {
    let id = need_str(&params, "id")?;
    if state.jobs().set_paused(&id, paused) {
        Ok(json!({}))
    } else {
        Err(RpcError {
            code: ERR_BUSINESS,
            message: format!(
                "任务不存在或已终态,无法{}: {id}",
                if paused { "暂停" } else { "恢复" }
            ),
            data: None,
        })
    }
}

/// `transfer.cancel {id}` → `{}`。语义:置 watch + abort 驱动任务(重试
/// 循环一并杀,不复活);作业缺失或已终态 → code 1。
async fn transfer_cancel(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let id = need_str(&params, "id")?;
    if state.jobs().cancel(&id) {
        Ok(json!({}))
    } else {
        Err(RpcError {
            code: ERR_BUSINESS,
            message: format!("任务不存在或已终态,无法取消: {id}"),
            data: None,
        })
    }
}

/// `transfer.wait {id}` →
/// `{state,error,files_done,files_total,bytes_total,skipped}`
/// 终态长轮询(Notify 唤醒,作业终态落定即返回)。任务不存在 → code 1
/// (报文「作业不存在: <id>」,立即返回不长轮询)。
async fn transfer_wait(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let id = need_str(&params, "id")?;
    let snap = state.jobs().wait(&id).await.map_err(|e| RpcError {
        code: ERR_BUSINESS,
        message: e,
        data: None,
    })?;
    Ok(json!({
        "state": snap.state,
        "error": snap.error,
        "files_done": snap.files_done,
        "files_total": snap.files_total,
        "bytes_total": snap.bytes_total,
        "skipped": snap.skipped,
    }))
}

/// `transfer.reinitiate {id}` → `{}`(v2-P2 Task 6/8):失败/中断任务翻新
/// 重发——upload 为本地部分预检 + dropped 残留清理;download/remote_copy
/// 为重发编排(orchestrate 携 old_rels,源端同一 partial 机制预检,dropped
/// 由源端统一清接收端残留,spec §5.1)。校验(不存在/种类/状态面/幂等门)
/// 在 jobs 注册表收口,Err → code 1。
async fn transfer_reinitiate(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let id = need_str(&params, "id")?;
    state
        .jobs()
        .reinitiate(state, &id)
        .await
        .map(|_new_id| json!({}))
        .map_err(|m| RpcError {
            code: ERR_BUSINESS,
            message: m,
            data: None,
        })
}

/// `transfer.clear {id}` → `{}`(v2-P2 Task 7,spec §5.2「各清各的」):
/// download 本地清目标目录 `.mtdss` 残留(不依赖对端);upload/remote_copy
/// 对端清理回执 Ok 才删记录(对端离线 code 3,任务保留可重试);终态均可
/// 清(v2-P5 T5b,Ruling K:done 拒清与 spec「清除已完成」冲突,引擎侧放
/// 开),live 态(queued/running/paused/publishing)仍拒清(先取消或等终
/// 态)。语义与码位收口在 jobs 的 `ClearErr`。
async fn transfer_clear(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let id = need_str(&params, "id")?;
    state
        .jobs()
        .clear(state, &id)
        .await
        .map(|()| json!({}))
        .map_err(|e| {
            let (code, message) = e.rpc();
            RpcError::new(code, message)
        })
}

// ---------- 远程浏览(list / stat) ----------

/// `list {device, path}` → `{"entries":[DirEntry...],"truncated":bool}`。
/// device/path 皆必填(-32602),path 允许空串 = 根卷。本机 device_id 短路
/// 直调 core fs(不经网络);其余经 CtrlPool 打洞——对端离线 code 3,
/// 对端 FsError/打洞失败等其余错误 code 1(映射收口在 CtrlErr::rpc)。
async fn ctrl_list(
    state: &Arc<DaemonState>,
    ctrl: &Arc<CtrlPool>,
    params: Value,
) -> Result<Value, RpcError> {
    let device = need_str(&params, "device")?;
    let path = need_str(&params, "path")?;
    match ctrl.list(state, &device, &path).await {
        Ok(out) => Ok(json!({
            "entries": out.entries.iter().map(dir_entry_json).collect::<Vec<_>>(),
            "truncated": out.truncated,
        })),
        Err(e) => Err(ctrl_rpc_err(&e)),
    }
}

/// `stat {device, path}` → `{"entry": DirEntry}`。参数与错误映射同
/// [`ctrl_list`](无截断概念,单条查询)。
async fn ctrl_stat(
    state: &Arc<DaemonState>,
    ctrl: &Arc<CtrlPool>,
    params: Value,
) -> Result<Value, RpcError> {
    let device = need_str(&params, "device")?;
    let path = need_str(&params, "path")?;
    match ctrl.stat(state, &device, &path).await {
        Ok(d) => Ok(json!({ "entry": dir_entry_json(&d) })),
        Err(e) => Err(ctrl_rpc_err(&e)),
    }
}

// ---------- 同机操作面(delete / move / copy) ----------

/// 跨设备移动/复制目标的拒绝文案(钉死):跨设备走复制/传输面(download/
/// orchestrate,Task 7/8),不是 ctrl 同机操作面。
const CROSS_DEVICE_REJECTED: &str = "跨设备请用复制/传输";

/// 可选目标设备参数 `to_device` 的同机守卫(move/copy):缺省或等于
/// `device` 放行;不同设备直接 code 1 拒绝——同机操作面不接受跨设备,
/// 在任何 fs 动作之前短路(拒绝即无副作用)。
fn guard_same_device(params: &Value, device: &str) -> Result<(), RpcError> {
    match params.get("to_device").and_then(Value::as_str) {
        None => Ok(()),
        Some(td) if td == device => Ok(()),
        Some(_) => Err(RpcError {
            code: ERR_BUSINESS,
            message: CROSS_DEVICE_REJECTED.into(),
            data: None,
        }),
    }
}

/// move 的 `pairs` 参数:[{"from","to"}…] 非空对象数组 → [(from, to)];
/// 缺键/类型错/空数组一律 -32602。
fn need_move_pairs(params: &Value, key: &str) -> Result<Vec<(String, String)>, RpcError> {
    let items: Option<Vec<(String, String)>> = params
        .get(key)
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .map(|v| {
                    let from = v.get("from").and_then(Value::as_str)?;
                    let to = v.get("to").and_then(Value::as_str)?;
                    Some((from.to_string(), to.to_string()))
                })
                .collect()
        });
    match items {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("缺少或非法参数: {key} 须为非空 {{from,to}} 对象数组"),
            data: None,
        }),
    }
}

/// PathOpFailure → JSON 投影(字段名与 proto PathOpFailure 一一对应)。
fn failure_json(f: &mtdss_proto::ctrl::PathOpFailure) -> Value {
    json!({ "path": f.path, "code": f.code, "message": f.message })
}

/// `delete {device, paths:[], force:bool}` →
/// `{"deleted":n,"recycled":bool,"failed":[{path,code,message}]}`。
/// 回收优先(force=false),设施级回收不可用沿 CtrlError 透传
/// (code 1,`data.kind = RECYCLE_UNAVAILABLE`,操作端类型化判定确认后
/// force 重发);逐条失败随 failed 表,批不半途弃废。
async fn ctrl_delete(
    state: &Arc<DaemonState>,
    ctrl: &Arc<CtrlPool>,
    params: Value,
) -> Result<Value, RpcError> {
    let device = need_str(&params, "device")?;
    let paths = need_str_array(&params, "paths")?;
    let force = need_bool(&params, "force")?;
    let r = ctrl
        .delete(state, &device, &paths, force)
        .await
        .map_err(|e| ctrl_rpc_err(&e))?;
    Ok(json!({
        "deleted": r.deleted,
        "recycled": r.recycled,
        "failed": r.failed.iter().map(failure_json).collect::<Vec<_>>(),
    }))
}

/// `move {device, pairs:[{from,to}], skip?, overwrite?, to_device?}` →
/// `{"moved":n,"failed":[…],"skipped":[…]}`。仅同设备(to_device 缺省 =
/// device;不同设备 code 1 拒绝,见 [`guard_same_device`])。skip/overwrite
/// 为可选冲突策略表(M4a-3,缺省空 = legacy),经 [`check_policy_limits`]
/// 限流(超限 code 1,与 transfer.* 策略校验同口径)。
async fn ctrl_move(
    state: &Arc<DaemonState>,
    ctrl: &Arc<CtrlPool>,
    params: Value,
) -> Result<Value, RpcError> {
    let device = need_str(&params, "device")?;
    let pairs = need_move_pairs(&params, "pairs")?;
    let policy = policy_from_params(&params)?;
    guard_same_device(&params, &device)?;
    let r = ctrl
        .move_(state, &device, &pairs, &policy)
        .await
        .map_err(|e| ctrl_rpc_err(&e))?;
    Ok(json!({
        "moved": r.moved,
        "failed": r.failed.iter().map(failure_json).collect::<Vec<_>>(),
        "skipped": r.skipped,
    }))
}

/// `copy {device, paths:[], dir, skip?, overwrite?, to_device?}` →
/// `{"copied":n,"failed":[…],"skipped":[…]}`。同设备远端复制
/// (fs::copy_paths_with,不经网络传输面);to_device 守卫同
/// [`ctrl_move`];skip/overwrite 策略面同 [`ctrl_move`]。
async fn ctrl_copy(
    state: &Arc<DaemonState>,
    ctrl: &Arc<CtrlPool>,
    params: Value,
) -> Result<Value, RpcError> {
    let device = need_str(&params, "device")?;
    let paths = need_str_array(&params, "paths")?;
    let dir = need_str(&params, "dir")?;
    let policy = policy_from_params(&params)?;
    guard_same_device(&params, &device)?;
    let r = ctrl
        .copy(state, &device, &paths, &dir, &policy)
        .await
        .map_err(|e| ctrl_rpc_err(&e))?;
    Ok(json!({
        "copied": r.copied,
        "failed": r.failed.iter().map(failure_json).collect::<Vec<_>>(),
        "skipped": r.skipped,
    }))
}

/// 可选冲突策略表(skip/overwrite;M4a-3):缺键/null → 空策略(legacy,
/// 线上不带 policy 字段——旧端互通面);类型错逐项 -32602;合计条数与
/// 单条长度经 [`check_policy_limits`] 限流(超限 code 1,与 transfer.*
/// 策略校验同口径)。
fn policy_from_params(params: &Value) -> Result<mtdss_core::transport::intake::TransferPolicy, RpcError> {
    let skip = opt_str_array(params, "skip")?;
    let overwrite = opt_str_array(params, "overwrite")?;
    let policy = mtdss_core::transport::intake::TransferPolicy { skip, overwrite };
    check_policy_limits(&policy).map_err(|m| RpcError {
        code: ERR_BUSINESS,
        message: m,
        data: None,
    })?;
    Ok(policy)
}

/// 从 params 取可选字符串数组(缺键/null → 空表;逐项必须为字符串)。
fn opt_str_array(params: &Value, key: &str) -> Result<Vec<String>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => match v.as_array() {
            Some(a) => a
                .iter()
                .map(|x| {
                    x.as_str().map(str::to_owned).ok_or_else(|| RpcError {
                        code: ERR_INVALID_PARAMS,
                        message: format!("缺少或非法参数: {key} 须为字符串数组"),
                        data: None,
                    })
                })
                .collect(),
            None => Err(RpcError {
                code: ERR_INVALID_PARAMS,
                message: format!("缺少或非法参数: {key} 须为字符串数组"),
                data: None,
            }),
        },
    }
}

/// DirEntryData → JSON 投影(字段名与 proto DirEntry 一一对应;children
/// 为 v2-P5 T5b 增量可选面——目录项在场的直接子项数,文件项/未知**整键
/// 缺省**,与 proto3 optional 线上语义一致,客户端缺键即退化纯日期副文)。
fn dir_entry_json(d: &mtdss_core::fs::DirEntryData) -> Value {
    let mut v = json!({
        "name": d.name,
        "path": d.path,
        "size": d.size,
        "mtime_ms": d.mtime_ms,
        "is_dir": d.is_dir,
    });
    if let Some(c) = d.children {
        v["children"] = json!(c);
    }
    v
}

/// CtrlErr → RPC 错误:离线 code 3;其余 code 1;对端错误 message 加
/// 「远端: 」前缀(码位与前缀收口在 CtrlErr::rpc)。FsError 驱动的错误
/// (CtrlErr::Peer,code 即 fs 稳定码)附 `data = {"kind": code}`——操作端
/// 类型化判定面(确认梯 RECYCLE_UNAVAILABLE、冲突预扫 NOT_FOUND),标记串
/// 子串匹配已废。
fn ctrl_rpc_err(e: &CtrlErr) -> RpcError {
    let (code, message) = e.rpc();
    match e {
        CtrlErr::Peer { code: kind, .. } => {
            RpcError::with_data(code, message, json!({ "kind": kind }))
        }
        _ => RpcError::new(code, message),
    }
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
            data: None,
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
            data: None,
        })
}

// ---------- 更新偏好(update.prefs.*) ----------

/// 可选布尔参数(存在且为布尔才取;缺省/Null/类型不符一律按未提供处理,
/// opt_str 同形——「缺省 = 不动」)。
fn opt_bool(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(Value::as_bool)
}

/// `update.prefs.get {}` → `{"auto_check":bool,"auto_install":bool}`。
/// 读 settings(D8 默认:检查开/安装关;老 settings.json 前向兼容同值)。
fn update_prefs_get(state: &DaemonState) -> Value {
    json!({
        "auto_check": state.update_auto_check(),
        "auto_install": state.update_auto_install(),
    })
}

/// `update.prefs.set {auto_check?, auto_install?}` → `{}`(Option 字段缺省
/// 不动)。D4 父子耦合收口:关检查连带落安装(state.set_update_check);
/// 任何「检查关 + 安装开」的组合——同帧矛盾,或检查已关时跨帧单开安装
/// (Task 11 收口的 D4 残面)——按落定后的生效检查开关统一 -32602 拒绝,
/// 不落半程态。
fn update_prefs_set(state: &DaemonState, params: Value) -> Result<Value, RpcError> {
    let check = opt_bool(&params, "auto_check");
    let install = opt_bool(&params, "auto_install");
    let effective_check = check.unwrap_or_else(|| state.update_auto_check());
    if install == Some(true) && !effective_check {
        return Err(RpcError {
            code: ERR_INVALID_PARAMS,
            message: "检查已关闭时不得开启 auto_install(D4 父子耦合)".into(),
            data: None,
        });
    }
    if let Some(c) = check {
        state.set_update_check(c).map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("设置更新偏好失败: {e:#}"),
            data: None,
        })?;
    }
    if let Some(i) = install {
        state.set_update_install(i).map_err(|e| RpcError {
            code: ERR_BUSINESS,
            message: format!("设置更新偏好失败: {e:#}"),
            data: None,
        })?;
    }
    Ok(json!({}))
}

// ---------- 更新面(update.*;Task 11,M4 仅 Windows) ----------

/// 非 Windows 垫片:M4 更新仅 Windows,update.* 全臂以类型化错误回绝
/// (`data.kind=UNSUPPORTED_PLATFORM`,code 1)——操作端据此隐藏更新入口,
/// 而不是收到未知方法;非 Windows 构建不进 update 模块依赖图。
#[cfg(not(windows))]
fn update_unsupported() -> RpcError {
    RpcError::with_data(
        ERR_BUSINESS,
        "M4 更新仅支持 Windows",
        json!({ "kind": "UNSUPPORTED_PLATFORM" }),
    )
}

/// `update.check {}` → 强制检查(`{base}/manifest.json`)→ 状态 JSON。
#[cfg(windows)]
async fn update_check(state: &Arc<DaemonState>) -> Result<Value, RpcError> {
    state.update().check(true).await
}

/// 非 Windows 垫片。
#[cfg(not(windows))]
async fn update_check(_state: &Arc<DaemonState>) -> Result<Value, RpcError> {
    Err(update_unsupported())
}

/// `update.state {}` → 状态 JSON(idle/available/downloading/ready/
/// pending_install/…/failed;事件同形)。
#[cfg(windows)]
fn update_state(state: &Arc<DaemonState>) -> Result<Value, RpcError> {
    Ok(state.update().state())
}

/// 非 Windows 垫片。
#[cfg(not(windows))]
fn update_state(_state: &Arc<DaemonState>) -> Result<Value, RpcError> {
    Err(update_unsupported())
}

/// `update.apply {version?}` → 状态 JSON(ready)。version 缺省 = 已检得
/// 的清单缓存;显式 version 走 `/win/{v}/manifest.json` 钉扎(M4 无防回滚)。
#[cfg(windows)]
async fn update_apply(state: &Arc<DaemonState>, params: Value) -> Result<Value, RpcError> {
    let version = opt_str(&params, "version");
    state.update().apply(version, state).await
}

/// 非 Windows 垫片。
#[cfg(not(windows))]
async fn update_apply(_state: &Arc<DaemonState>, _params: Value) -> Result<Value, RpcError> {
    Err(update_unsupported())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// v2-P3 T6(recv_dir 双变体):android 投影恒收件箱哨兵,桌面投影
    /// 原样透传——契约 golden 的 status.resp.android 变体与桌面 status
    /// 同源于本函数(桌面 host 上钉死两形态)。
    #[test]
    fn recv_dir_display_pins_both_variants() {
        assert_eq!(
            recv_dir_display(true, std::path::Path::new("/mtdss/任意目录")),
            mtdss_core::transport::intake::INBOX_SENTINEL,
            "android 投影恒哨兵(目录参数被忽略)"
        );
        assert_eq!(
            recv_dir_display(false, std::path::Path::new("/mtdss/任意目录")),
            "/mtdss/任意目录",
            "桌面投影原样透传"
        );
        // android set_recv_dir 文案锚(与 lib.rs 导出的契约常量同一份)
        assert_eq!(crate::ANDROID_SET_RECV_DIR_ERR, "安卓端收件箱固定");
    }

    /// dir_entry_json 的 children 投影(v2-P5 T5b):目录项带整数键,文件
    /// 项/未知(None)整键缺省——与 proto3 optional 线上语义一致;其余
    /// 字段面不变。serde_json 键序字母序,children 落首(与 golden 样本
    /// 同一实现序列化,逐字节可比)。
    #[test]
    fn dir_entry_json_children_optional_key() {
        let dir = mtdss_core::fs::DirEntryData {
            name: "文档 目录".into(),
            path: "/远端/文档 目录".into(),
            size: 0,
            mtime_ms: 1_760_000_000_000,
            is_dir: true,
            children: Some(2),
        };
        assert_eq!(
            dir_entry_json(&dir),
            json!({
                "name": "文档 目录", "path": "/远端/文档 目录", "size": 0,
                "mtime_ms": 1_760_000_000_000i64, "is_dir": true, "children": 2,
            }),
            "目录项带 children 整数"
        );
        let file = mtdss_core::fs::DirEntryData {
            children: None,
            ..dir
        };
        assert_eq!(
            dir_entry_json(&file),
            json!({
                "name": "文档 目录", "path": "/远端/文档 目录", "size": 0,
                "mtime_ms": 1_760_000_000_000i64, "is_dir": true,
            }),
            "children 为 None 时整键缺省(非 null)"
        );
    }
}
