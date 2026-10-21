//! ipc_bridge:把 mtdss-ipc(JSON-RPC over 本地套接字)桥接为 Tauri 命令与窗口事件。
//!
//! 连接策略(setup 钩子拉起常驻 pump 任务):
//! 1. 先直连 `mtdss_home()` 派生的套接字(daemon 已在 → 直接用);
//! 2. 失败 → `find_daemon_bin()` 找 mtdssd 并后台拉起(Windows 隐藏窗口;
//!    环境原样继承;不传 --no-tray,托盘随 daemon 正常出现),随后 ≤10s
//!    内每 500ms 重试连接;
//! 3. 连上后订阅事件泵,把 `event.online`/`event.devices`/`event.send_log`/
//!    `event.login`/`event.transfer`/`event.notify` 转发为窗口事件
//!    `daemon://online`/`daemon://devices`/`daemon://send_log`/`daemon://login`/
//!    `daemon://transfer`/`daemon://notify`(Task 13);`event.quit`(守护
//!    进程优雅退出,fix-wave Defect D)不经前端,桥直接退出 app 关掉窗口;
//!    另有 5s 心跳(RPC hello),探活失败即断开重进外层循环重连——崩溃/
//!    被杀没有 `event.quit`,走离线态不关窗。
//!
//! 命令面(Task 12 起十三 + 拖出落点;Task 13 增四):daemon_hello /
//! status / devices / list / stat / delete / fs_move / fs_copy /
//! transfer_start / transfer_list / transfer_pause / transfer_resume /
//! transfer_cancel / login_request_code / login_verify / logout 全部经
//! `rpc()` 包装:通用命令 10s 超时;list/stat/fs_move/fs_copy 走浏览超时
//! (RPC_TIMEOUT_BROWSE,35s,ctrl 单请求往返 15s + 打洞 10s 的合法上界);
//! transfer_start 起始校验快、transfer_list 读注册表快、pause/resume/cancel
//! 置门/置 watch 即返,通用超时。另有本机命令 clipboard_files(CF_HDROP
//! 直读,Task 12)、pick_folder(目录选择器)与 pick_ca_cert(登录表单
//! 自签 CA 证书选择器;fix-wave Defect B)——均为 tauri-plugin-dialog /
//! Win32 包装;RpcError → 错误串(形态见 `rpc_err`:无 data 人读串,带
//! data 整体 JSON)。

use anyhow::Context as _;
use mtdss_ipc::{find_daemon_bin, mtdss_home, EventFrame, IpcClient, RpcError};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tokio::sync::RwLock;

/// RPC 调用超时:UI 不悬挂在无响应的 daemon 上。
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// list/stat 浏览专用超时:daemon 侧冷启动远端浏览合法上界约 30s
/// (REST 目录拉取 15s + 打洞 10s + 请求 15s 链路),35s 留余量;
/// 其余命令仍用 10s 保持响应灵敏。
const RPC_TIMEOUT_BROWSE: Duration = Duration::from_secs(35);
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

/// RpcError → 前端错误串:无 `data` 保持 `code N: message` 人读形态
/// (离线/超时/未连接/参数错等桥接面字节不变);FsError 驱动的错误
/// (M4a-6 起 `data = {"kind": <FsError 稳定码>}`)整体序列化为 JSON
/// 文本,`data.kind` 随报文到达前端——UI 侧 daemon.ts `toRpcError` 双
/// 形态还原,确认梯 RECYCLE_UNAVAILABLE 与冲突预扫 NOT_FOUND 的类型化
/// 判定依此(标记串子串匹配已废,双端不留双轨)。
fn rpc_err(e: RpcError) -> String {
    if e.data.is_some() {
        serde_json::to_string(&e).unwrap_or_else(|_| format!("code {}: {}", e.code, e.message))
    } else {
        format!("code {}: {}", e.code, e.message)
    }
}

/// 单次 RPC 调用:超时 + 错误整形。所有命令走这里;list/stat 传
/// `RPC_TIMEOUT_BROWSE`(冷启动远端浏览链路较长),其余传 `RPC_TIMEOUT`。
async fn rpc(
    state: &State<'_, DaemonBridge>,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let client = state.client().await?;
    match tokio::time::timeout(timeout, client.call(method, params)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(rpc_err(e)),
        Err(_) => Err(format!(
            "code -32000: RPC 调用超时({}s): {method}",
            timeout.as_secs()
        )),
    }
}

/// 更新进行中守卫(Task 12 / D11,纯决策面可测):update-in-progress 标记
/// 在场 → 拒绝拉起新 daemon(安装器即将接管;桥保持既有离线重试节奏,
/// 更新收尾后自然恢复)。
fn spawn_blocked(home: &std::path::Path) -> bool {
    mtdss_ipc::update_in_progress(home).is_some()
}

/// 后台拉起 mtdssd:隐藏控制台窗口(Windows),环境原样继承,
/// 不传 --no-tray(托盘正常拉起)。daemon 已在运行时新实例会因
/// 套接字占用自行退出,无害。更新进行中(标记在场)不拉起(Err 由
/// pump 既有臂 warn 留痕)。
fn spawn_daemon() -> anyhow::Result<std::path::PathBuf> {
    if spawn_blocked(&mtdss_home()) {
        return Err(anyhow::anyhow!("更新进行中,请稍候"));
    }
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
        // 连接建立信号:冷启动场景下前端挂载时的三连 RPC 会先失败
        // (daemon 尚未拉起),而 daemon 事件只在状态翻转时发、ipc serve
        // 对新连接也不重放快照——没有这个信号,前端会永久滞留引导态。
        // 每次(重)连成功都发,前端以重拉全量作幂等响应。
        if let Err(e) = handle.emit("daemon://connected", json!({})) {
            tracing::warn!(err = %e, "连接信号发送失败");
        }
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

/// daemon 事件的桥侧动作([`event_action`] 的返回面,单测见 tests)。
#[derive(Debug, PartialEq, Eq)]
enum EventAction {
    /// 转发为窗口事件(名字随值;参数原样透传给前端)。
    Forward(&'static str),
    /// 桥侧直接动作:守护进程优雅退出(`event.quit`)→ 关掉 UI。
    QuitApp,
    /// 未知事件:忽略。
    Ignore,
}

/// daemon 事件方法名 → 桥动作(纯函数)。既有事件映射窗口事件名,与
/// daemon 侧 `EventFrame` 契约逐字对齐;`event.quit`(fix-wave Defect D)
/// 是守护进程**优雅退出**(托盘「退出」/Ctrl-C 汇入 `Daemon::shutdown`
/// 入口)的专属信号——桥直接退出 app(窗口随之关闭),不经前端:前端
/// 挂死/无窗状态也能收口。崩溃/被杀不会有此帧,断连仍走既有探活失败 →
/// 重连循环 + 前端 M3b 离线态,绝不因 IPC 断连连坐关窗。未知方法忽略。
fn event_action(method: &str) -> EventAction {
    match method {
        "event.online" => EventAction::Forward("daemon://online"),
        "event.devices" => EventAction::Forward("daemon://devices"),
        "event.send_log" => EventAction::Forward("daemon://send_log"),
        "event.login" => EventAction::Forward("daemon://login"),
        // Task 13:传输作业事件(状态翻转即发/进度 ≥400ms 节流)与接收
        // 完成通知(OS 通知 daemon 侧已发,UI 仅窗内回显同文案)。
        "event.transfer" => EventAction::Forward("daemon://transfer"),
        "event.notify" => EventAction::Forward("daemon://notify"),
        "event.quit" => EventAction::QuitApp,
        _ => EventAction::Ignore,
    }
}

/// daemon 事件 → 窗口事件 / 桥动作(映射见 [`event_action`])。
fn forward(handle: &AppHandle, frame: EventFrame) {
    match event_action(&frame.method) {
        EventAction::Forward(event) => {
            if let Err(e) = handle.emit(event, frame.params) {
                tracing::warn!(%event, err = %e, "窗口事件转发失败");
            }
        }
        EventAction::QuitApp => {
            tracing::info!("收到 event.quit(守护进程优雅退出),关闭 UI");
            handle.exit(0);
        }
        EventAction::Ignore => {}
    }
}

/// 在 setup 钩子里调用:装配桥状态并拉起常驻 pump 任务。
/// 管理的是 `DaemonBridge` 本体(命令侧 `State<DaemonBridge>` 由此解析),
/// pump 任务经 `handle.state::<DaemonBridge>()` 拿到同一实例写入客户端。
pub fn setup(handle: AppHandle) {
    handle.manage(DaemonBridge::default());
    tauri::async_runtime::spawn(pump(handle));
}

// ---------- Tauri 命令(九个) ----------

/// `hello`:握手/探活 → {version, logged_in, device}。
#[tauri::command]
pub async fn daemon_hello(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "hello", json!({}), RPC_TIMEOUT).await
}

/// `status`:运行态快照 → {logged_in, online, server, device, recv_dir}。
#[tauri::command]
pub async fn status(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "status", json!({}), RPC_TIMEOUT).await
}

/// `devices`:设备目录快照 → {devices:[{device_id,name,platform,online}]}。
#[tauri::command]
pub async fn devices(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "devices", json!({}), RPC_TIMEOUT).await
}

/// `list {device,path}` → {entries,truncated}(path 空串 = 根卷;
/// 对端离线报 `code 3: …`,其余浏览失败 `code 1: …`;冷启动远端浏览
/// 链路长,走浏览超时 RPC_TIMEOUT_BROWSE)。
#[tauri::command]
pub async fn list(
    state: State<'_, DaemonBridge>,
    device: String,
    path: String,
) -> Result<Value, String> {
    rpc(
        &state,
        "list",
        json!({ "device": device, "path": path }),
        RPC_TIMEOUT_BROWSE,
    )
    .await
}

/// `stat {device,path}` → {entry}(参数与错误映射同 list,单条查询;
/// 走浏览超时 RPC_TIMEOUT_BROWSE,同 list)。
#[tauri::command]
pub async fn stat(
    state: State<'_, DaemonBridge>,
    device: String,
    path: String,
) -> Result<Value, String> {
    rpc(
        &state,
        "stat",
        json!({ "device": device, "path": path }),
        RPC_TIMEOUT_BROWSE,
    )
    .await
}

/// `delete {device,paths,force}` → {deleted,recycled,failed}(同机操作面:
/// 回收优先,可回收成功即静默;设施级回收不可用整体拒绝 code 1 且报文含
/// 「回收站不可用」标记,操作端确认后 force=true 整批重发;对端离线
/// code 3。本机回收/永久删为快操作,走通用超时 RPC_TIMEOUT)。
#[tauri::command]
pub async fn delete(
    state: State<'_, DaemonBridge>,
    device: String,
    paths: Vec<String>,
    force: bool,
) -> Result<Value, String> {
    rpc(
        &state,
        "delete",
        json!({ "device": device, "paths": paths, "force": force }),
        RPC_TIMEOUT,
    )
    .await
}

/// `fs_move {device, pairs:[{from,to}], skip?, overwrite?}` →
/// {moved,failed,skipped}(Task 12 device_copy 剪切面:同机 ctrl move,无
/// 网络传输;对远端设备走 ctrl 打洞链路,长尾同浏览 →
/// RPC_TIMEOUT_BROWSE)。skip/overwrite 为可选冲突策略表(M4a-4,UI 冲突
/// 对话框裁决;判定键 = pairs 的 `to` 原串,引擎侧匹配),缺省不携键 =
/// legacy(与旧请求报文逐字节同形);daemon 响应回带 skipped 名单,原样
/// 透传。
#[tauri::command]
pub async fn fs_move(
    state: State<'_, DaemonBridge>,
    device: String,
    pairs: Vec<MovePair>,
    skip: Option<Vec<String>>,
    overwrite: Option<Vec<String>>,
) -> Result<Value, String> {
    let mut params = json!({
        "device": device,
        "pairs": pairs.iter().map(|p| json!({ "from": p.from, "to": p.to })).collect::<Vec<_>>(),
    });
    if let Some(s) = skip {
        params["skip"] = json!(s);
    }
    if let Some(o) = overwrite {
        params["overwrite"] = json!(o);
    }
    rpc(&state, "move", params, RPC_TIMEOUT_BROWSE).await
}

/// fs_move 的 pairs 元素(serde 反序列化形;daemon 侧同形)。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MovePair {
    pub from: String,
    pub to: String,
}

/// `fs_copy {device, paths, dir, skip?, overwrite?}` →
/// {copied,failed,skipped}(Task 12 device_copy 复制面:同机 ctrl copy,
/// fs::copy_paths;超时口径同 fs_move)。策略面同 fs_move(M4a-4):判定
/// 键 = 引擎 policy_key(dir, name) 形(目标目录拼名),UI 侧经 lib/paste
/// 换算后透传;缺省不携键 = legacy。
#[tauri::command]
pub async fn fs_copy(
    state: State<'_, DaemonBridge>,
    device: String,
    paths: Vec<String>,
    dir: String,
    skip: Option<Vec<String>>,
    overwrite: Option<Vec<String>>,
) -> Result<Value, String> {
    let mut params = json!({ "device": device, "paths": paths, "dir": dir });
    if let Some(s) = skip {
        params["skip"] = json!(s);
    }
    if let Some(o) = overwrite {
        params["overwrite"] = json!(o);
    }
    rpc(&state, "copy", params, RPC_TIMEOUT_BROWSE).await
}

/// `transfer_start {…}` → {job_id}(Task 12):transfer.start 透传,四形态
/// (upload/download/local_copy/remote_copy)的参数面在 daemon 侧收口。
/// 起始只做同步校验即返 job_id(驱动在 daemon 后台跑),通用超时足够。
#[tauri::command]
pub async fn transfer_start(
    state: State<'_, DaemonBridge>,
    params: Value,
) -> Result<Value, String> {
    rpc(&state, "transfer.start", params, RPC_TIMEOUT).await
}

/// `transfer_list {}` → {jobs:[…]}(Task 13 卡片种子:transfer.list 全量
/// 快照透传;读注册表快,通用超时足够)。
#[tauri::command]
pub async fn transfer_list(state: State<'_, DaemonBridge>) -> Result<Value, String> {
    rpc(&state, "transfer.list", json!({}), RPC_TIMEOUT).await
}

/// `transfer_pause {id}`(Task 13):暂停透传;作业缺失/已终态/remote_copy
/// 无数据面 → daemon 业务错误(code 1)原样上抛,卡片侧以提示呈现。
#[tauri::command]
pub async fn transfer_pause(
    state: State<'_, DaemonBridge>,
    id: String,
) -> Result<Value, String> {
    rpc(&state, "transfer.pause", json!({ "id": id }), RPC_TIMEOUT).await
}

/// `transfer_resume {id}`(Task 13):恢复透传,错误面同 transfer_pause。
#[tauri::command]
pub async fn transfer_resume(
    state: State<'_, DaemonBridge>,
    id: String,
) -> Result<Value, String> {
    rpc(&state, "transfer.resume", json!({ "id": id }), RPC_TIMEOUT).await
}

/// `transfer_cancel {id}`(Task 13):取消透传;缺失/已终态 → 业务错误。
#[tauri::command]
pub async fn transfer_cancel(
    state: State<'_, DaemonBridge>,
    id: String,
) -> Result<Value, String> {
    rpc(&state, "transfer.cancel", json!({ "id": id }), RPC_TIMEOUT).await
}

/// `clipboard_files {}` → {files:[…]}(Windows 直读系统剪贴板 CF_HDROP;
/// 其余平台恒空表)。Explorer 复制 → MTDSS 粘贴上传的反向入口;空表 =
/// 无文件剪贴板(粘贴 no-op,不放大为错误)。剪贴板 Win32 调用毫秒级但
/// 可能被他进程短暂持有,落 spawn_blocking 免占异步 worker。
#[cfg(windows)]
#[tauri::command]
pub async fn clipboard_files() -> Result<Value, String> {
    tokio::task::spawn_blocking(clipboard_imp::read_files)
        .await
        .map(|files| json!({ "files": files }))
        .map_err(|e| format!("剪贴板读取任务失败:{e}"))
}

/// 非 Windows:恒空表(保编译;MTDSS UI 仅 Windows,此为防御分支)。
#[cfg(not(windows))]
#[tauri::command]
pub async fn clipboard_files() -> Result<Value, String> {
    Ok(json!({ "files": [] }))
}

/// `pick_folder {}` → {dir:String|null}(Task 12「下载到本机…」落点:系统
/// 目录选择器,tauri-plugin-dialog 包装)。回调式 API 经 oneshot 归位
/// async;用户取消 → dir:null。Mutex<Option<Sender>> 保证 FnOnce 回调至多
/// 发送一次(对话框被系统吞掉回调的防御面)。
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = std::sync::Mutex::new(Some(tx));
    app.dialog().file().pick_folder(move |folder| {
        // 锁中毒 = 对话框回调已消费过发送槽:直接放弃(至多一次语义)。
        if let Ok(mut guard) = tx.lock()
            && let Some(tx) = guard.take()
        {
            let dir =
                folder.and_then(|fp| fp.into_path().ok().map(|p| p.to_string_lossy().into_owned()));
            let _ = tx.send(dir);
        }
    });
    let picked = rx.await.map_err(|_| "目录选择器未返回结果".to_string())?;
    Ok(json!({ "dir": picked }))
}

/// `pick_ca_cert {}` → {path:String|null}(登录表单自签 CA 证书选择;系统
/// 文件选择器,tauri-plugin-dialog 包装,回调归位与 [`pick_folder`] 同款)。
/// 不加扩展名过滤:CA 可为 .pem/.crt/.cer 等任意形态(选错文件由 daemon
/// `login.request_code` 报错,错误串原样回显到表单),用户取消 → path:null。
#[tauri::command]
pub async fn pick_ca_cert(app: AppHandle) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = std::sync::Mutex::new(Some(tx));
    app.dialog().file().pick_file(move |file| {
        // 锁中毒 = 对话框回调已消费过发送槽:直接放弃(至多一次语义)。
        if let Ok(mut guard) = tx.lock()
            && let Some(tx) = guard.take()
        {
            let path = file
                .and_then(|fp| fp.into_path().ok())
                .map(|p| p.to_string_lossy().into_owned());
            let _ = tx.send(path);
        }
    });
    let picked = rx.await.map_err(|_| "文件选择器未返回结果".to_string())?;
    Ok(json!({ "path": picked }))
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
        RPC_TIMEOUT,
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
        RPC_TIMEOUT,
    )
    .await
}

/// `login.logout`:登出(daemon 侧幂等)。
#[tauri::command]
pub async fn logout(state: State<'_, DaemonBridge>) -> Result<(), String> {
    rpc(&state, "login.logout", json!({}), RPC_TIMEOUT).await.map(|_| ())
}

// ---------------------------------------------------------------------------
// Windows 剪贴板 CF_HDROP 直读(Task 12;unsafe FFI 收口在本模块)。
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod clipboard_imp {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DragQueryFileW;

    /// 打开持有重试次数(剪贴板被他进程短暂持有是常态,Excel 等在剪贴板
    /// 上的渲染嗅探会挡几十毫秒)。
    const OPEN_ATTEMPTS: usize = 5;
    /// 重试间隔。
    const OPEN_RETRY_MS: u64 = 15;

    /// 直读系统剪贴板 CF_HDROP → 文件路径表。
    ///
    /// OpenClipboard(null owner) → IsClipboardFormatAvailable(CF_HDROP) →
    /// GetClipboardData → DragQueryFileW 逐条宽串 → CloseClipboard。全程
    /// **只读不取所有权**:GetClipboardData 返回的句柄归剪贴板所有,不得
    /// DragFinish/释放(那只在 OLE GetData 自持介质时才需要)。任何一步
    /// 失败/超窗 → 空表(空内部剪贴板的粘贴本就 no-op,不放大为错误),
    /// 绝不 panic。打开失败重试几轮后放弃;打开成功则 CloseClipboard 恒
    /// 配对(含中途返回路径)。
    pub fn read_files() -> Vec<String> {
        unsafe {
            if !open_with_retry() {
                return Vec::new();
            }
            let files = read_hdrop_paths();
            CloseClipboard();
            files
        }
    }

    /// OpenClipboard 带短重试:全部失败返回 false(放弃 = 空表)。
    unsafe fn open_with_retry() -> bool {
        unsafe {
            for attempt in 0..OPEN_ATTEMPTS {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    return true;
                }
                if attempt + 1 < OPEN_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(OPEN_RETRY_MS));
                }
            }
            false
        }
    }

    /// 已持剪贴板:CF_HDROP 可用则逐条取宽串路径(两段式查询:先长度后
    /// 内容);句柄不释放(见 read_files 注释)。
    unsafe fn read_hdrop_paths() -> Vec<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP as u32) == 0 {
                return Vec::new();
            }
            let handle = GetClipboardData(CF_HDROP as u32);
            if handle.is_null() {
                return Vec::new();
            }
            let hdrop: windows_sys::Win32::UI::Shell::HDROP = handle;
            // 0xFFFFFFFF = 取条目计数(不含结尾 NUL 的路径串数)。
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0);
                if len == 0 {
                    continue; // 防御:空名条目跳过
                }
                let mut buf = vec![0u16; len as usize + 1];
                let got = DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32);
                if got > 0 {
                    out.push(String::from_utf16_lossy(&buf[..got as usize]));
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// data.kind 必须随错误串到达前端(确认梯 RECYCLE_UNAVAILABLE / 冲突
    /// 预扫 NOT_FOUND 类型化判定的线面契约);无 data 错误保持
    /// `code N: message` 人读形态(离线/超时/未连接面字节不变)。
    #[test]
    fn rpc_err_forwards_data_kind_and_keeps_plain_form() {
        let e = RpcError::with_data(
            1,
            "远端: RECYCLE_UNAVAILABLE 回收站不可用: 设施未就绪",
            json!({ "kind": "RECYCLE_UNAVAILABLE" }),
        );
        let v: Value = serde_json::from_str(&rpc_err(e)).expect("带 data 错误应为 JSON 文本");
        assert_eq!(v["code"], 1);
        assert_eq!(
            v["data"]["kind"], "RECYCLE_UNAVAILABLE",
            "data.kind 丢失则 UI 确认梯失效"
        );

        assert_eq!(
            rpc_err(RpcError::new(3, "对端离线: dev-1")),
            "code 3: 对端离线: dev-1"
        );
    }

    /// 事件→动作映射(纯函数;fix-wave Defect D):既有六事件逐条钉死
    /// 窗口事件名(防映射字面量被手滑改动);`event.quit` 必须落在桥侧
    /// 退出动作(不走前端——优雅退出连前端卡死也要能关窗);未知/未来
    /// 事件一律忽略(不得误动作)。
    #[test]
    fn event_action_maps_known_events_and_quit() {
        let forwards = [
            ("event.online", "daemon://online"),
            ("event.devices", "daemon://devices"),
            ("event.send_log", "daemon://send_log"),
            ("event.login", "daemon://login"),
            ("event.transfer", "daemon://transfer"),
            ("event.notify", "daemon://notify"),
        ];
        for (method, window_event) in forwards {
            assert_eq!(
                event_action(method),
                EventAction::Forward(window_event),
                "{method} 的窗口事件映射被改动"
            );
        }
        assert_eq!(
            event_action("event.quit"),
            EventAction::QuitApp,
            "守护进程优雅退出必须是桥侧关窗动作"
        );
        assert_eq!(event_action("event.unknown"), EventAction::Ignore);
        assert_eq!(event_action(""), EventAction::Ignore);
    }

    /// 更新进行中守卫(Task 12 / D11):标记在场 → 拒绝拉起 daemon(安装器
    /// 即将接管;桥保持既有离线重试,由 pump 的 Err 臂留痕)。
    #[test]
    fn spawn_guard_blocks_while_update_in_progress() {
        let dir = std::env::temp_dir().join(format!("mtdss-bridge-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!spawn_blocked(&dir), "无标记必须放行拉起");
        std::fs::write(mtdss_ipc::update_marker_path(&dir), "1.0.1").unwrap();
        assert!(spawn_blocked(&dir), "更新进行中必须拒绝拉起 daemon");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
