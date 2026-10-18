//! ipc_bridge:把 mtdss-ipc(JSON-RPC over 本地套接字)桥接为 Tauri 命令与窗口事件。
//!
//! 连接策略(setup 钩子拉起常驻 pump 任务):
//! 1. 先直连 `mtdss_home()` 派生的套接字(daemon 已在 → 直接用);
//! 2. 失败 → `find_daemon_bin()` 找 mtdssd 并后台拉起(Windows 隐藏窗口;
//!    环境原样继承;不传 --no-tray,托盘随 daemon 正常出现),随后 ≤10s
//!    内每 500ms 重试连接;
//! 3. 连上后订阅事件泵,把 `event.online`/`event.devices`/`event.send_log`
//!    转发为窗口事件 `daemon://online`/`daemon://devices`/`daemon://send_log`;
//!    另有 5s 心跳(RPC hello),探活失败即断开重进外层循环重连。
//!
//! 六个 Tauri 命令(hello/status/devices/login_request_code/login_verify/
//! logout)全部经 `rpc()` 包装:10s 超时,RpcError → `code N: message`。

use anyhow::Context as _;
use mtdss_ipc::{find_daemon_bin, mtdss_home, EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tokio::sync::RwLock;

/// RPC 调用超时:UI 不悬挂在无响应的 daemon 上。
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// 拉起 daemon 后的连接重试窗口。
const CONNECT_WINDOW: Duration = Duration::from_secs(10);
/// 连接重试间隔。
const CONNECT_RETRY: Duration = Duration::from_millis(500);
/// 心跳探活间隔(daemon 死亡检测)。
const HEARTBEAT: Duration = Duration::from_secs(5);

/// 全局桥状态:当前可用的 IPC 客户端(pump 任务维护,命令只读)。
#[derive(Default)]
pub struct DaemonBridge {
    client: RwLock<Option<Arc<IpcClient>>>,
}

impl DaemonBridge {
    async fn client(&self) -> Result<Arc<IpcClient>, String> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| "code -32000: daemon 未连接".to_string())
    }
}

/// RpcError → 前端可读字符串 `code N: message`。
fn rpc_err(e: RpcError) -> String {
    format!("code {}: {}", e.code, e.message)
}

/// 单次 RPC 调用:10s 超时 + 错误整形。所有命令走这里。
async fn rpc(
    state: &State<'_, DaemonBridge>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let client = state.client().await?;
    match tokio::time::timeout(RPC_TIMEOUT, client.call(method, params)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(rpc_err(e)),
        Err(_) => Err(format!(
            "code -32000: RPC 调用超时({}s): {method}",
            RPC_TIMEOUT.as_secs()
        )),
    }
}

/// 后台拉起 mtdssd:隐藏控制台窗口(Windows),环境原样继承,
/// 不传 --no-tray(托盘正常拉起)。daemon 已在运行时新实例会因
/// 套接字占用自行退出,无害。
fn spawn_daemon() -> anyhow::Result<std::path::PathBuf> {
    let bin = find_daemon_bin()
        .ok_or_else(|| anyhow::anyhow!("未找到 mtdssd 可执行文件(MTDSSD_BIN/发布目录/target)"))?;
    let mut cmd = std::process::Command::new(&bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .with_context(|| format!("拉起 {}", bin.display()))?;
    Ok(bin)
}

/// 尝试连接一次;失败返回 None(快速失败,不重试)。
async fn try_connect() -> Option<Arc<IpcClient>> {
    IpcClient::connect(&mtdss_home()).await.ok().map(Arc::new)
}

/// 常驻 pump:连接 → 事件转发 → 探活失败重连,永生循环。
async fn pump(handle: AppHandle) {
    let bridge = handle.state::<DaemonBridge>();
    loop {
        // 1) daemon 已在则直连;否则拉起后在窗口内重试
        let mut client = try_connect().await;
        if client.is_none() {
            match spawn_daemon() {
                Ok(bin) => tracing::info!("已拉起 daemon: {}", bin.display()),
                Err(e) => tracing::warn!("拉起 daemon 失败: {e:#}"),
            }
            let deadline = Instant::now() + CONNECT_WINDOW;
            while client.is_none() && Instant::now() < deadline {
                tokio::time::sleep(CONNECT_RETRY).await;
                client = try_connect().await;
            }
        }
        let Some(client) = client else {
            // 没有可执行文件可拉:等一会再来一轮(用户可能手动启动 daemon)
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        *bridge.client.write().await = Some(client.clone());
        tracing::info!("daemon IPC 已连接");

        // 2) 事件转发 + 心跳探活
        let mut events = client.events();
        let mut tick = tokio::time::interval(HEARTBEAT);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut alive = true;
        while alive {
            tokio::select! {
                evt = events.recv() => match evt {
                    Ok(frame) => forward(&handle, frame),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "事件泵滞后,跳过");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => alive = false,
                },
                _ = tick.tick() => {
                    // hello 探活:写失败/超时/断开都视作 daemon 不可用
                    let probe = tokio::time::timeout(
                        RPC_TIMEOUT,
                        client.call("hello", json!({})),
                    );
                    if matches!(probe.await, Ok(Err(_)) | Err(_)) {
                        tracing::warn!("daemon 探活失败,重连");
                        alive = false;
                    }
                }
            }
        }

        // 3) 断开:清状态,重进外层循环
        *bridge.client.write().await = None;
        tracing::info!("daemon IPC 已断开,等待重连");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// daemon 事件 → 窗口事件。方法名硬匹配,未知事件忽略。
fn forward(handle: &AppHandle, frame: EventFrame) {
    let event = match frame.method.as_str() {
        "event.online" => "daemon://online",
        "event.devices" => "daemon://devices",
        "event.send_log" => "daemon://send_log",
        _ => return,
    };
    if let Err(e) = handle.emit(event, frame.params) {
        tracing::warn!(%event, err = %e, "窗口事件转发失败");
    }
}

/// 在 setup 钩子里调用:装配桥状态并拉起常驻 pump 任务。
/// 管理的是 `DaemonBridge` 本体(命令侧 `State<DaemonBridge>` 由此解析),
/// pump 任务经 `handle.state::<DaemonBridge>()` 拿到同一实例写入客户端。
pub fn setup(handle: AppHandle) {
    handle.manage(DaemonBridge::default());
    tauri::async_runtime::spawn(pump(handle));
}

// ---------- Tauri 命令(六个) ----------

/// `hello`:握手/探活 → {version, logged_in, device}。
#[tauri::command]
pub async fn daemon_hello(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "hello", json!({})).await
}

/// `status`:运行态快照 → {logged_in, online, server, device, recv_dir}。
#[tauri::command]
pub async fn status(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "status", json!({})).await
}

/// `devices`:设备目录快照 → {devices:[{device_id,name,platform,online}]}。
#[tauri::command]
pub async fn devices(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "devices", json!({})).await
}

/// `login.request_code {server,email,ca?}` → {code}(dev 模式回显验证码)。
#[tauri::command]
pub async fn login_request_code(
    state: State<'_, DaemonBridge>,
    server: String,
    email: String,
    ca: Option<String>,
) -> Result<Value, String> {
    rpc(
        &state,
        "login.request_code",
        json!({ "server": server, "email": email, "ca": ca }),
    )
    .await
}

/// `login.verify {email,code,name?}` → {device_id,name}。name 缺省用主机名(daemon 侧)。
#[tauri::command]
pub async fn login_verify(
    state: State<'_, DaemonBridge>,
    email: String,
    code: String,
    name: Option<String>,
) -> Result<Value, String> {
    rpc(
        &state,
        "login.verify",
        json!({ "email": email, "code": code, "name": name }),
    )
    .await
}

/// `login.logout`:登出(daemon 侧幂等)。
#[tauri::command]
pub async fn logout(state: State<'_, DaemonBridge>) -> Result<(), String> {
    rpc(&state, "login.logout", json!({})).await.map(|_| ())
}
