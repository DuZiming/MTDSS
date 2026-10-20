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
//! main 只保留 clap 解析与登录/冲突问询/永久删除确认的 stdin 交互
//! (Task 7 put/get、Task 8 cp、Task 9 rm/mv:库层出 parse_scp_target/
//! parse_cp_target/parse_same_device_targets/move_pairs/conflict_prompt_data/
//! parse_conflict_answer/need_force_confirm 纯数据,问询循环留在 main)。
//! 所有 RPC 均显式限时包装(IpcClient::call 无内建超时;send 长调用与
//! transfer.wait 终态等待除外——其活性由 daemon 侧重试预算/作业 Notify
//! 唤醒与连接断开兜底)。

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

// ---------- put / get(Task 7:scp 形目标 + 作业化传输) ----------

/// scp 形目标解析(纯函数):`设备:远端绝对路径` → (设备, 路径)。
///
/// 规则:首冒号切分;无冒号 = 裸路径拒;第二枚冒号只允许出现在路径段的
/// 盘符位(第 2 字符,Windows `D:\x`)——`dev:D:\a:b` 这类多冒号拒;空
/// 设备段/空路径段拒。Windows 下 `C:\foo` 这类**裸盘符路径**形似目标
/// (单冒号 + 单字母设备段),按裸路径拒——设备名恰为单字母的场景改用
/// 完整 id 或多字符名(Unix 无此歧义,不做该启发)。
pub fn parse_scp_target(s: &str) -> std::result::Result<(String, String), CliError> {
    const HINT: &str = "目标须形如 设备:路径(如 dev_b:D:\\dir 或 dev:/srv/dir)";
    let Some(i) = s.find(':') else {
        return Err(CliError {
            code: 1,
            message: format!("裸路径缺少设备段:{HINT}"),
        });
    };
    let dev = s[..i].trim().to_string();
    let path = s[i + 1..].trim().to_string();
    if dev.is_empty() || path.is_empty() {
        return Err(CliError {
            code: 1,
            message: format!("目标设备段与路径段都不能为空:{HINT}"),
        });
    }
    // 路径段冒号纪律:至多一枚,且只能落在盘符位(第 2 字符,Windows
    // `D:\x`);首位或更后的冒号(`D:\a:b`、`:x`)一律拒
    let colons: Vec<usize> = path.match_indices(':').map(|(i, _)| i).collect();
    if colons.len() > 1 || colons.first() == Some(&0) {
        return Err(CliError {
            code: 1,
            message: format!("目标路径段含非法冒号:{HINT}"),
        });
    }
    #[cfg(windows)]
    if dev.len() == 1
        && dev.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !path.contains(':')
    {
        // 单字母设备段 + 单冒号 + 路径段无盘符:更像裸盘符路径的误输
        return Err(CliError {
            code: 1,
            message: format!("裸路径(盘符形态)缺少设备段:{HINT}"),
        });
    }
    Ok((dev, path))
}

/// 冲突问询的用户动作(纯数据;交互段留 main.rs,库层不碰终端)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    /// 全部跳过(同名条目不落盘,接收端按 Skip 计)。
    SkipAll,
    /// 全部覆盖(同名条目清障后原位落盘)。
    OverwriteAll,
    /// 取消整个传输(不发起)。
    Cancel,
}

/// 冲突问询的纯数据(文案):列出冲突条目与三个选项的应答字母。
pub struct ConflictPrompt {
    /// 完整问询文案(多行;交互段原样打印)。
    pub text: String,
}

/// 冲突问询文案(纯函数):逐条列出冲突(>20 条截断计数),末行给
/// [s]跳过 / [o]覆盖 / [c]取消 选项。
pub fn conflict_prompt_data(conflicts: &[String]) -> ConflictPrompt {
    let mut text = format!("目标处已存在 {} 个同名条目:\n", conflicts.len());
    for c in conflicts.iter().take(20) {
        text.push_str(&format!("  {c}\n"));
    }
    if conflicts.len() > 20 {
        text.push_str(&format!("  …(其余 {} 项略)\n", conflicts.len() - 20));
    }
    text.push_str("[s] 全部跳过  [o] 全部覆盖  [c] 取消");
    ConflictPrompt { text }
}

/// 冲突问询应答解析(纯函数):s/o/c 大小写不敏感、容忍首尾空白;其余
/// (含空串)None,交互段重问。
pub fn parse_conflict_answer(ans: &str) -> Option<ConflictAction> {
    match ans.trim() {
        "s" | "S" => Some(ConflictAction::SkipAll),
        "o" | "O" => Some(ConflictAction::OverwriteAll),
        "c" | "C" | "q" | "Q" => Some(ConflictAction::Cancel),
        _ => None,
    }
}

/// put 冲突预扫(经 daemon `list` RPC):本地输入的顶层名与远端目标目录
/// 的既有条目求交。远端目录不存在(NOT_FOUND)= 无冲突(接收端会建出);
/// 其余错误(离线 code 3 等)原样上抛。
pub async fn scan_put_conflicts(
    c: &DaemonClient,
    device: &str,
    dir: &str,
    files: &[PathBuf],
) -> std::result::Result<Vec<String>, CliError> {
    let local_names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if local_names.len() != files.len() {
        return Err(code1(anyhow::anyhow!("输入存在无文件名的路径(卷根等)")));
    }
    let remote = rpc(c, "list", json!({ "device": device, "path": dir })).await;
    let entries: Vec<Value> = match remote {
        Ok(v) => v["entries"].as_array().cloned().unwrap_or_default(),
        Err(e) if e.message.contains("NOT_FOUND") => Vec::new(), // 远端目录不存在 = 无冲突
        Err(e) => return Err(e),
    };
    let hits: Vec<String> = entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .filter(|n| local_names.iter().any(|l| l == *n))
        .map(str::to_owned)
        .collect();
    Ok(hits)
}

/// get 冲突预扫(经 daemon `stat` RPC + 本地 read_dir):远端路径的顶层名
/// 与本机落盘目录的既有条目求交。远端路径缺失即业务错误(stat 原样上抛);
/// 本机目录不存在 = 无冲突(接收端会建出)。
pub async fn scan_get_conflicts(
    c: &DaemonClient,
    device: &str,
    remote_paths: &[String],
    local_dir: &Path,
) -> std::result::Result<Vec<String>, CliError> {
    let mut remote_names: Vec<String> = Vec::with_capacity(remote_paths.len());
    for p in remote_paths {
        let v = rpc(
            c,
            "stat",
            json!({ "device": device, "path": p }),
        )
        .await?;
        let name = v["entry"]["name"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| code1(anyhow::anyhow!("stat 响应缺条目: {v}")))?;
        remote_names.push(name);
    }
    let local: std::collections::HashSet<String> = std::fs::read_dir(local_dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    Ok(remote_names.into_iter().filter(|n| local.contains(n)).collect())
}

/// 发起 put(transfer.start upload):files 为本地输入,dir 为远端目标
/// 目录(绝对形态由 daemon 校验);返回 job_id。
pub async fn start_put(
    c: &DaemonClient,
    files: &[PathBuf],
    device: &str,
    dir: &str,
    skip: &[String],
    overwrite: &[String],
) -> std::result::Result<String, CliError> {
    let r = rpc(
        c,
        "transfer.start",
        json!({
            "kind": "upload",
            "files": files.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "to": device,
            "dir": dir,
            "skip": skip,
            "overwrite": overwrite,
            "cut": false,
        }),
    )
    .await?;
    job_id_of(&r)
}

/// 发起 get(transfer.start download):paths 为远端绝对路径,dir 为本机
/// 落盘目录(绝对形态由 daemon 校验);返回 job_id。
pub async fn start_get(
    c: &DaemonClient,
    device: &str,
    paths: &[String],
    dir: &Path,
    skip: &[String],
    overwrite: &[String],
) -> std::result::Result<String, CliError> {
    let r = rpc(
        c,
        "transfer.start",
        json!({
            "kind": "download",
            "device": device,
            "paths": paths,
            "dir": dir.to_string_lossy(),
            "skip": skip,
            "overwrite": overwrite,
            "cut": false,
        }),
    )
    .await?;
    job_id_of(&r)
}

/// start 响应 → job_id(缺字段 = daemon 契约破坏,按环境故障报)。
fn job_id_of(v: &Value) -> std::result::Result<String, CliError> {
    v["job_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| code1(anyhow::anyhow!("transfer.start 响应缺 job_id: {v}")))
}

/// `transfer.wait` 阻塞至终态(**不套限时**:传输时长合法,活性由作业
/// Notify 唤醒与 IPC 连接断开兜底)。返回终态快照
/// `{state,error,files_done,files_total,bytes_total}`。
pub async fn wait_job(c: &DaemonClient, id: &str) -> std::result::Result<Value, CliError> {
    c.ipc
        .call("transfer.wait", json!({ "id": id }))
        .await
        .map_err(from_rpc)
}

// ---------- cp(Task 8:四态分发) ----------

/// cp 源/目标解析(纯函数):`设备:路径` → 远端(设备段回传);裸路径 →
/// 本机(设备段 None)。Windows 单字母盘符形态(`C:\x`)按本机路径(与
/// [`parse_scp_target`] 的裸盘符守卫同口径;单字母设备名场景改用完整 id,
/// Unix 无此歧义不做该启发);其余 scp 形非法(空段/多冒号)原样报错
/// ——cp 不吞错误静默按本机解释。
pub fn parse_cp_target(s: &str) -> std::result::Result<(Option<String>, String), CliError> {
    // Windows 裸盘符形态:首冒号在第 2 字符且设备段为单字母 → 本机路径
    #[cfg(windows)]
    if let Some(i) = s.find(':') {
        if i == 1 && s[..1].chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            return Ok((None, s.to_string()));
        }
    }
    if !s.contains(':') {
        return Ok((None, s.to_string()));
    }
    let (dev, path) = parse_scp_target(s)?;
    Ok((Some(dev), path))
}

/// cp 的跨侧冲突预扫(同机糖与 remote_copy 共用):逐源路径 stat(from
/// 设备;同机糖时即本机短路)取顶层名,to 设备目标目录 list 取既有条目名
/// 求交。**顶层粒度**(V1 已知限制,嵌套同名交接收端引擎安全网);目标
/// 目录缺失(NOT_FOUND)= 无冲突(接收端会建出),其余错误原样上抛。
pub async fn scan_cross_conflicts(
    c: &DaemonClient,
    from_dev: &str,
    paths: &[String],
    to_dev: &str,
    dir: &str,
) -> std::result::Result<Vec<String>, CliError> {
    let mut src_names: Vec<String> = Vec::with_capacity(paths.len());
    for p in paths {
        let v = rpc(c, "stat", json!({ "device": from_dev, "path": p })).await?;
        let name = v["entry"]["name"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| code1(anyhow::anyhow!("stat 响应缺条目: {v}")))?;
        src_names.push(name);
    }
    let entries: Vec<Value> = match rpc(c, "list", json!({ "device": to_dev, "path": dir })).await {
        Ok(v) => v["entries"].as_array().cloned().unwrap_or_default(),
        Err(e) if e.message.contains("NOT_FOUND") => Vec::new(), // 目标目录不存在 = 无冲突
        Err(e) => return Err(e),
    };
    Ok(entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .filter(|n| src_names.iter().any(|l| l == *n))
        .map(str::to_owned)
        .collect())
}

/// 发起 cp(transfer.start remote_copy):from 设备把 paths 直传给 to 设备
/// 的 dir(数据不经本机落地);cut 恒 false(CLI cp 无剪切旗标);返回
/// job_id。
pub async fn start_remote_copy(
    c: &DaemonClient,
    from: &str,
    paths: &[String],
    to: &str,
    dir: &str,
    skip: &[String],
    overwrite: &[String],
) -> std::result::Result<String, CliError> {
    let r = rpc(
        c,
        "transfer.start",
        json!({
            "kind": "remote_copy",
            "from": from,
            "paths": paths,
            "to": to,
            "dir": dir,
            "skip": skip,
            "overwrite": overwrite,
            "cut": false,
        }),
    )
    .await?;
    job_id_of(&r)
}

/// 同机复制糖(`copy` RPC):同设备复制不经传输面;skip/overwrite 为
/// 冲突策略表(M4a-3,表项 = 目标路径判定键,经 [`join_remote`] 换算;
/// 空 = legacy 接收端自动改名)。返回原始响应供打印(`skipped` 回带跳过)。
pub async fn copy_remote_cmd(
    c: &DaemonClient,
    device: &str,
    paths: &[String],
    dir: &str,
    skip: &[String],
    overwrite: &[String],
) -> std::result::Result<Value, CliError> {
    rpc(
        c,
        "copy",
        json!({
            "device": device,
            "paths": paths,
            "dir": dir,
            "skip": skip,
            "overwrite": overwrite,
        }),
    )
    .await
}

// ---------- rm / mv(Task 9:同机操作面薄壳 + 永久删除确认梯) ----------

/// 永久删除升级判定标记串:daemon 侧 `FsError::RecycleUnavailable` 的
/// thiserror 文案前缀,经 ctrl 原样透传到 RPC 报文。CLI/UI 据此把「回收
/// 优先删除」升级为「确认后永久删」(Task 9 确认梯)。
pub const RECYCLE_UNAVAILABLE_MARKER: &str = "回收站不可用";

/// 永久删除升级判定(纯函数):报文含「回收站不可用」标记串 → 需操作端
/// 确认后 force 重发;其余错误一律 false。T7 已知债:daemon 无结构化错误
/// 码面,标记串子串匹配有意为之(daemon 侧报文形态由 thiserror 文案钉死,
/// 不随版本漂移)。
pub fn need_force_confirm(message: &str) -> bool {
    message.contains(RECYCLE_UNAVAILABLE_MARKER)
}

/// 多目标同设备解析(纯函数,rm/mv/get 共用):逐个 scp 形目标解析(非法
/// 形态原样报错不吞),全部须同一台设备;返回 (设备, 路径表)。空表拒绝
/// (clap required 之外的防御兜底)。
pub fn parse_same_device_targets(
    targets: &[String],
) -> std::result::Result<(String, Vec<String>), CliError> {
    let mut dev: Option<String> = None;
    let mut paths = Vec::with_capacity(targets.len());
    for t in targets {
        let (d, p) = parse_scp_target(t)?;
        match &dev {
            None => dev = Some(d),
            Some(prev) if *prev == d => {}
            Some(prev) => {
                return Err(CliError {
                    code: 1,
                    message: format!("多个目标须同一台设备:「{prev}」与「{d}」不符"),
                });
            }
        }
        paths.push(p);
    }
    let dev = dev.ok_or_else(|| CliError { code: 2, message: "至少需要一个目标".into() })?;
    Ok((dev, paths))
}

/// 远端路径拼接(纯函数,pub:cp/mv 同机糖把冲突名换算成引擎判定键也
/// 走同一规则——见 core fs `policy_key` 文档):目录 + 条目名。分隔符随
/// 目录形态(含 `\` 用 `\`,否则 `/`——目标设备平台未知,不能依赖宿主
/// `std::path`;两种形态在 Windows 的 std::fs 与 core fs normalize 下均
/// 合法);目录自带尾分隔符时不重复。
pub fn join_remote(dir: &str, name: &str) -> String {
    let sep = if dir.contains('\\') { '\\' } else { '/' };
    if dir.ends_with('\\') || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}{sep}{name}")
    }
}

/// 源路径末段条目名(纯函数):按两种分隔符取最后一段(与宿主平台无关,
/// 见 [`join_remote`]);卷根形(`C:`/`C:\`/`/`)无条目名 → None。
fn base_name(p: &str) -> Option<&str> {
    let trimmed = p.trim_end_matches(['\\', '/']);
    let name = trimmed.rsplit(['\\', '/']).next().unwrap_or("");
    if name.is_empty() {
        return None;
    }
    // Windows 卷根(`C:`):整卷不可作为移动条目
    if name.len() == 2 && name.ends_with(':') && name.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    Some(name)
}

/// mv 的目标对构造(纯函数):同设备源路径表 + 目录形 `--to` → `move` 的
/// pairs(from 原样,to = dir/源末段名——源移入目标目录)。卷根等无名源
/// 逐条拒绝(批前整体短路,不发起任何 RPC,无副作用)。
pub fn move_pairs(
    sources: &[String],
    dir: &str,
) -> std::result::Result<Vec<(String, String)>, CliError> {
    let mut pairs = Vec::with_capacity(sources.len());
    for s in sources {
        let name = base_name(s).ok_or_else(|| CliError {
            code: 1,
            message: format!("mv 源缺少条目名(卷根等不可移动):{s}"),
        })?;
        pairs.push((s.clone(), join_remote(dir, name)));
    }
    Ok(pairs)
}

/// `delete {device,paths,force}`(rm 的 RPC 面):回收优先(force=false),
/// 设施级回收不可用 → code 1 且报文含「回收站不可用」标记(操作端
/// [`need_force_confirm`] 确认后 force 重发);逐条失败随 failed 表,
/// 批不半途弃废。响应 `{deleted,recycled,failed}`。
pub async fn rm_cmd(
    c: &DaemonClient,
    device: &str,
    paths: &[String],
    force: bool,
) -> std::result::Result<Value, CliError> {
    rpc(
        c,
        "delete",
        json!({ "device": device, "paths": paths, "force": force }),
    )
    .await
}

/// `move {device,pairs,skip,overwrite}`(mv 的 RPC 面):同设备移动/改名
/// (rename 直通,跨卷对调用方透明)。跨设备由 CLI 参数层短路指引 cp;
/// daemon 侧 to_device 守卫是最后防线(本面不传 to_device)。skip/overwrite
/// 为冲突策略表(M4a-3,表项 = 目标路径判定键,经 [`join_remote`] 换算;
/// 空 = legacy)。响应 `{moved,failed,skipped}`。
pub async fn mv_cmd(
    c: &DaemonClient,
    device: &str,
    pairs: &[(String, String)],
    skip: &[String],
    overwrite: &[String],
) -> std::result::Result<Value, CliError> {
    let pairs: Vec<Value> = pairs
        .iter()
        .map(|(from, to)| json!({ "from": from, "to": to }))
        .collect();
    rpc(
        c,
        "move",
        json!({ "device": device, "pairs": pairs, "skip": skip, "overwrite": overwrite }),
    )
    .await
}

/// cp/mv 同机糖的 ask 落定(纯决策,不碰终端;问询文案与应答解析复用
/// [`conflict_prompt_data`]/[`parse_conflict_answer`],终端循环留 main
/// ——`next_answer` 是应答源注入面,生产传 stdin 读行,管道测试传预置
/// 应答序列):无冲突 → 空表直通(不应答源);逐次取应答解析,非法应答
/// 忽略重问;s → 全表 skip、o → 全表 overwrite、c → code 0 干净取消;
/// 应答流耗尽(EOF)同取消。读入失败的 code 1 由应答源自带(Err 直传)。
pub fn resolve_conflicts_ask(
    conflicts: &[String],
    mut next_answer: impl FnMut() -> std::result::Result<Option<String>, CliError>,
) -> std::result::Result<(Vec<String>, Vec<String>), CliError> {
    if conflicts.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    loop {
        let Some(line) = next_answer()? else {
            return Err(CliError { code: 0, message: "已取消".into() });
        };
        match parse_conflict_answer(&line) {
            Some(ConflictAction::SkipAll) => return Ok((conflicts.to_vec(), Vec::new())),
            Some(ConflictAction::OverwriteAll) => return Ok((Vec::new(), conflicts.to_vec())),
            Some(ConflictAction::Cancel) => {
                return Err(CliError { code: 0, message: "已取消".into() });
            }
            None => continue, // 非法应答重问
        }
    }
}
