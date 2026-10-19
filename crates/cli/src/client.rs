//! CLI ↔ 守护进程桥:连接/自动拉起 + RPC 命令层。
//!
//! `DaemonClient::connect_or_spawn` 三段式:先 ≤1s 试连(已有 daemon 在场
//! 即直接复用)→ 失败则 `find_daemon_bin` 找 mtdssd 后台拉起(Windows 加
//! `CREATE_NO_WINDOW` 防闪控制台;环境整体透传,`MTDSS_HOME` 天然一致,
//! `--no-tray` 不传——CLI 拉起即默认带托盘,Linux 无托盘编译)→ 持 Child
//! 句柄轮询重连:子进程退出立即失败(可行动文案);SPAWN_WAIT 未就绪但
//! 子进程仍在 → stderr 提示一次(首跑安全软件扫描常见),继续轮到硬顶
//! SPAWN_WAIT_HARD。找不到 mtdssd 报 CliError code=2(退出码语义:2 =
//! 前置条件不满足)。登录守门:未登录且命令不在豁免集(login/status)时
//! 同样 code=2,文案指引 `mtdss login`。
//!
//! 命令层 async 函数即 main 的分发目标,也是 ipc_cli.rs 的测试入口;
//! main 只保留 clap 解析与登录的 stdin 交互。所有 RPC 均显式限时包装
//! (IpcClient::call 无内建超时;send 长调用除外——其活性由 daemon 侧
//! 重试预算与连接断开兜底)。

use anyhow::{Context, Result};
use mtdss_ipc::{IpcClient, RpcError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 首次试连上限(已有 daemon 时快速接管)。
const CONNECT_TRY: Duration = Duration::from_secs(1);
/// 自动拉起后的常规就绪预算;超窗而子进程仍活 → 提示一次并继续等。
const SPAWN_WAIT: Duration = Duration::from_secs(10);
/// 自动拉起的硬顶(首跑安全软件扫描可拖到分钟级);到点仍未就绪即失败。
const SPAWN_WAIT_HARD: Duration = Duration::from_secs(60);
/// 重连轮询间隔。
const SPAWN_POLL: Duration = Duration::from_millis(250);
/// hello/普通命令的单次限时(ipc call 无内建超时)。
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 未登录守门文案(常量便于测试断言)。
pub const NOT_LOGGED_IN_HINT: &str = "未登录:请先 `mtdss login`";
/// 找不到守护进程文案(安装与 MTDSSD_BIN 指引)。
pub const NO_DAEMON_HINT: &str =
    "找不到 mtdssd:请安装 MTDSS 或设置 MTDSSD_BIN 指向 mtdssd 可执行文件";

/// CLI 错误:消息即用户面中文文案,code 即进程退出码(2 = 前置条件不满足:
/// 未登录/找不到 daemon;1 = 业务失败/环境故障)。
#[derive(Debug)]
pub struct CliError {
    /// 建议进程退出码。
    pub code: i32,
    /// 用户面文案(多行允许,send 失败报告为多行全文)。
    pub message: String,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

/// anyhow 失败归一为退出码 1(保留完整错误链)。
fn code1(e: anyhow::Error) -> CliError {
    CliError { code: 1, message: format!("{e:#}") }
}

/// RPC 业务失败归一为退出码 1(message 即 daemon 携带的报告全文)。
fn from_rpc(e: RpcError) -> CliError {
    CliError { code: 1, message: e.message }
}

/// 与守护进程的会话:IPC 客户端 + `hello` 握手快照(version/logged_in/device)。
pub struct DaemonClient {
    pub ipc: IpcClient,
    pub hello: Value,
}

impl DaemonClient {
    /// 连接 home 上的守护进程;不在场则自动拉起(时序见模块注释)。
    /// 找不到 mtdssd 可执行文件 → CliError code=2;其余失败 code=1。
    pub async fn connect_or_spawn(home: &Path) -> std::result::Result<Self, CliError> {
        // ① 已有 daemon 在场:≤1s 试连,连上即直接复用(不触碰发现逻辑)
        if let Some(c) = try_connect_bounded(home, CONNECT_TRY).await {
            return Self::handshake(c).await;
        }
        // ② 发现并后台拉起 mtdssd(Child 句柄留着做活性监视)
        let bin = mtdss_ipc::find_daemon_bin().ok_or_else(|| CliError {
            code: 2,
            message: NO_DAEMON_HINT.into(),
        })?;
        let mut child = spawn_daemon(&bin).map_err(code1)?;
        tracing::info!(bin = %bin.display(), "已自动拉起守护进程,等待就绪");
        // ③ 轮询重连:子进程退出即快速失败;软窗(SPAWN_WAIT)过后仍活则
        // stderr 提示一次继续等,硬顶(SPAWN_WAIT_HARD)到点仍未就绪才放弃。
        let soft = tokio::time::Instant::now() + SPAWN_WAIT;
        let hard = tokio::time::Instant::now() + SPAWN_WAIT_HARD;
        let mut noticed = false;
        loop {
            if let Some(c) = try_connect_bounded(home, CONNECT_TRY).await {
                return Self::handshake(c).await;
            }
            if let Some(status) =
                child.try_wait().map_err(|e| code1(anyhow::anyhow!("轮询守护进程状态失败: {e}")))?
            {
                return Err(code1(anyhow::anyhow!(
                    "守护进程提前退出(status: {status}),请查看其日志,或手动运行 mtdssd 排查原因"
                )));
            }
            let now = tokio::time::Instant::now();
            if now >= hard {
                return Err(code1(anyhow::anyhow!(
                    "守护进程未在 {SPAWN_WAIT_HARD:?} 内就绪({})",
                    mtdss_ipc::socket_path(home)
                )));
            }
            if !noticed && now >= soft {
                noticed = true;
                eprintln!(
                    "守护进程 {SPAWN_WAIT:?} 内未就绪但仍在运行:首跑安全软件扫描可能拖慢启动,继续等待(上限 {SPAWN_WAIT_HARD:?})…"
                );
            }
            tokio::time::sleep(SPAWN_POLL).await;
        }
    }

    /// 连上后的握手:`hello` 快照进 client(登录守门与版本判断的数据源)。
    async fn handshake(c: IpcClient) -> std::result::Result<Self, CliError> {
        let hello = match tokio::time::timeout(CALL_TIMEOUT, c.call("hello", json!({}))).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(from_rpc(e)),
            Err(_) => return Err(code1(anyhow::anyhow!("hello 超时({CALL_TIMEOUT:?})"))),
        };
        Ok(Self { ipc: c, hello })
    }
}

/// 限时连接:成功返回 Some;失败/超时一律 None(interprocess 对占用中的
/// 管道会自旋等待,显式限时防悬挂;与 daemon 侧 try_connect 同款防御)。
async fn try_connect_bounded(home: &Path, t: Duration) -> Option<IpcClient> {
    match tokio::time::timeout(t, IpcClient::connect(home)).await {
        Ok(Ok(c)) => Some(c),
        _ => None,
    }
}

/// 后台拉起 mtdssd:环境整体透传(MTDSS_HOME/MTDSS_TEST_* 等子进程自读),
/// stdin 置空(守护进程不读 stdin);stdout/stderr 继承(缺省 stderr 日志
/// 对用户可见,也便于排障)。Windows 加 CREATE_NO_WINDOW:GUI 场景拉起
/// 不闪黑色控制台。返回的 Child 由 connect_or_spawn 持有做活性监视
/// (退出即快速失败);进程本身独立常驻,生命周期归用户。
fn spawn_daemon(bin: &Path) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(bin);
    cmd.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().with_context(|| format!("spawn {}", bin.display()))
}

/// 登录守门:`exempt` 为 true 的命令(login/status)在未登录态放行;
/// 否则未登录即 CliError code=2(文案见 NOT_LOGGED_IN_HINT)。登录后恒放行。
pub fn ensure_logged_in(c: &DaemonClient, exempt: bool) -> std::result::Result<(), CliError> {
    if exempt || c.hello["logged_in"].as_bool().unwrap_or(false) {
        return Ok(());
    }
    Err(CliError { code: 2, message: NOT_LOGGED_IN_HINT.into() })
}

// ---------- 命令层(main 分发目标;ipc_cli.rs 测试入口) ----------

/// `login.request_code`:返回 dev 模式服务器回显的验证码(正式模式下该码
/// 经服务器其他渠道投递,回显行为随服务器演进)。
pub async fn login_request_code(
    c: &DaemonClient,
    server: &str,
    email: &str,
    ca: Option<&Path>,
) -> std::result::Result<String, CliError> {
    let mut params = json!({ "server": server, "email": email });
    if let Some(ca) = ca {
        params["ca"] = json!(ca.to_string_lossy());
    }
    let r = rpc(c, "login.request_code", params).await?;
    match r["code"].as_str() {
        Some(code) => Ok(code.to_string()),
        None => Err(code1(anyhow::anyhow!("login.request_code 响应缺 code: {r}"))),
    }
}

/// `login.verify`:成功返回 {device_id,name}。
pub async fn login_verify(
    c: &DaemonClient,
    email: &str,
    code: &str,
    name: Option<&str>,
) -> std::result::Result<Value, CliError> {
    let mut params = json!({ "email": email, "code": code });
    if let Some(n) = name {
        params["name"] = json!(n);
    }
    rpc(c, "login.verify", params).await
}

/// `login.logout`(daemon 侧幂等:未登录同样成功)。
pub async fn login_logout(c: &DaemonClient) -> std::result::Result<(), CliError> {
    rpc(c, "login.logout", json!({})).await.map(|_| ())
}

/// `devices`:目录快照 {devices:[{device_id,name,platform,online}]}。
pub async fn devices_cmd(c: &DaemonClient) -> std::result::Result<Value, CliError> {
    rpc(c, "devices", json!({})).await
}

/// `status`:运行态快照 {logged_in,online,server,device,recv_dir}。
pub async fn status_cmd(c: &DaemonClient) -> std::result::Result<Value, CliError> {
    rpc(c, "status", json!({})).await
}

/// 设备定位(纯函数):id 精确命中优先(目录里 device_id 恒唯一,命中即
/// 用,名字空间同串不抢);否则按 name 精确匹配——0 台 → 列出可用设备
/// (id+name 每行一台)的 Err;>1 台 → 提示歧义并列全部匹配 device_id 的
/// Err;恰 1 台 → 其 id。退出码 1(业务失败)。
pub fn resolve_device(
    devices: &[Value],
    name_or_id: &str,
) -> std::result::Result<String, CliError> {
    if devices.iter().any(|d| d["device_id"].as_str() == Some(name_or_id)) {
        return Ok(name_or_id.to_string());
    }
    let by_name: Vec<&Value> = devices
        .iter()
        .filter(|d| d["name"].as_str() == Some(name_or_id))
        .collect();
    match by_name.as_slice() {
        [d] => Ok(d["device_id"].as_str().unwrap_or_default().to_string()),
        [] => {
            let mut msg = format!("找不到设备「{name_or_id}」,可用设备:");
            for d in devices {
                msg.push_str(&format!(
                    "\n{}\t{}",
                    d["device_id"].as_str().unwrap_or("?"),
                    d["name"].as_str().unwrap_or(""),
                ));
            }
            Err(CliError { code: 1, message: msg })
        }
        many => {
            let ids: Vec<&str> = many
                .iter()
                .filter_map(|d| d["device_id"].as_str())
                .collect();
            Err(CliError {
                code: 1,
                message: format!(
                    "设备名「{name_or_id}」对应多台设备,请改用 device_id:\n{}",
                    ids.join("\n")
                ),
            })
        }
    }
}

/// `ls`:远端目录列表。device 为已定位的 device_id;path 空串 = 根卷。
/// 与其他命令同一限时包装(daemon 侧离线快错 code 3,文案随 RpcError 原样
/// 透出)。
pub async fn ls_cmd(
    c: &DaemonClient,
    device: &str,
    path: &str,
) -> std::result::Result<Value, CliError> {
    rpc(c, "list", json!({ "device": device, "path": path })).await
}

/// ls 行格式(纯函数,稳定可 grep):每条目一行 `{d|-}\t{name}\t{size}\t
/// {mtime_ms}`(d=目录);`truncated=true` 时末行追加截断提示。10000 与
/// daemon 侧 core fs 的 LIST_ENTRY_CAP 同口径(CLI 依赖白名单不含 core,
/// 硬编码并在此注记)。
pub fn format_ls(v: &Value) -> String {
    let mut out = String::new();
    for e in v["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let kind = if e["is_dir"].as_bool().unwrap_or(false) { "d" } else { "-" };
        out.push_str(&format!(
            "{kind}\t{}\t{}\t{}\n",
            e["name"].as_str().unwrap_or(""),
            e["size"].as_u64().unwrap_or(0),
            e["mtime_ms"].as_u64().unwrap_or(0),
        ));
    }
    if v["truncated"].as_bool() == Some(true) {
        out.push_str("…(结果已截断,仅显示前 10000 项)\n");
    }
    out
}

/// `set_recv_dir`(recv 命令的 RPC 面):目录字符串原样下发并持久化,
/// 目录可用性由接收落盘时兜底。
pub async fn set_recv_dir_cmd(c: &DaemonClient, dir: &Path) -> std::result::Result<(), CliError> {
    rpc(c, "set_recv_dir", json!({ "dir": dir.to_string_lossy() }))
        .await
        .map(|_| ())
}

/// send:先订阅事件再发起长调用,传输期间把 event.send_log 逐行打到
/// stdout(进度与旧 CLI 口径一致);返回即调用终局——成功 Ok(()),
/// 失败 Err(CliError,message 为逐文件失败报告全文)。
///
/// 不套外层超时:编排活性由 daemon 侧重试预算与 QUIC 空闲超时治理,
/// 连接断开时 ipc 层立即以错误收口,不悬挂。
pub async fn send_cmd(c: &DaemonClient, files: &[PathBuf], to: &str) -> std::result::Result<(), CliError> {
    let mut evs = c.ipc.events();
    let params = json!({
        "files": files.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "to": to,
    });
    let call = c.ipc.call("send", params);
    tokio::pin!(call);
    // 事件通道关闭后置 false,防止 select 臂在 Closed 上热旋
    let mut ev_open = true;
    let result = loop {
        tokio::select! {
            // biased:先把就绪的进度帧排干,再收调用终局
            biased;
            evt = async {
                if ev_open { evs.recv().await } else { std::future::pending().await }
            } => match evt {
                Ok(f) if f.method == "event.send_log" => {
                    println!("{}", f.params["line"].as_str().unwrap_or_default());
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("(进度日志滞后 {n} 行,已丢弃)");
                }
                Err(_) => ev_open = false, // 通道关闭:只等调用终局
            },
            r = &mut call => {
                // 终局前后可能仍有已入队进度帧:非阻塞排干,不打断收尾
                while let Ok(f) = evs.try_recv() {
                    if f.method == "event.send_log" {
                        println!("{}", f.params["line"].as_str().unwrap_or_default());
                    }
                }
                break r;
            }
        }
    };
    result.map(|_| ()).map_err(from_rpc)
}

/// 限时 RPC 包装:超时/断开/业务失败统一归一为 CliError code=1。
async fn rpc(
    c: &DaemonClient,
    method: &str,
    params: Value,
) -> std::result::Result<Value, CliError> {
    match tokio::time::timeout(CALL_TIMEOUT, c.ipc.call(method, params)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(from_rpc(e)),
        Err(_) => Err(code1(anyhow::anyhow!("{method} 超时({CALL_TIMEOUT:?})"))),
    }
}
