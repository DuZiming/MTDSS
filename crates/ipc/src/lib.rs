//! mtdss-ipc:本机 IPC 上的 JSON-RPC(NDJSON 线格式)服务端与客户端。
//!
//! 传输层用 `interprocess` 本地套接字(Windows 命名管道 / Unix 域套接字),
//! 线格式为 NDJSON——一行一帧:
//! - 请求:  `{"jsonrpc":"2.0","id":N,"method":"...","params":...}`
//! - 响应:  `{"id":N,"result":...}` 或 `{"id":N,"error":{"code":i64,"message":"...","data"?…}}`
//! - 事件:  通知帧 `{"jsonrpc":"2.0","method":"...","params":...}`(无 id)
//!
//! 服务端为每个连接拆「读半工 + 写半工」两个任务;事件向所有连接广播,
//! 写队列满的慢消费者立即断连,保证一个卡死的客户端不拖垮其他连接。
//! 单帧上限 8 MiB,超限断连,防止异常对端撑爆内存。

pub mod consts;

use anyhow::Context as _;
use interprocess::local_socket::{
    traits::tokio::{Listener as _, Stream as _},
    tokio::{RecvHalf, SendHalf, Stream},
    GenericFilePath, ListenerOptions, ToFsName,
};
// sha256 仅用于 Windows 管道名的 home 指纹;Unix 侧 socket_path 为纯路径拼接
#[cfg(windows)]
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};

/// 单帧(单行)字节上限:8 MiB。
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// 每连接写队列容量;事件广播以 try_send 入队,满即断连。
const OUT_CHANNEL_CAP: usize = 64;
/// 客户端事件广播容量;订阅方消费过慢时收到 Lagged。
const CLIENT_EVENT_CAP: usize = 256;

/// 当前已连接客户端数(serve 每连接 ±1:accept 后 +1,写循环退出 -1)。
/// 进程级全局;唯一合法消费面是 [`connected_clients`](更新编排等待 UI 全部
/// 断开),计数维护完全内聚在 serve/write_loop,不外借裸计数器。
static CONN_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 连接计数的 -1 端(Drop 即减):写循环任意收尾路径(EOF/写失败/慢消费
/// 者/读半工终局,乃至 panic 展开)都经 Drop 配对 serve accept 处的 +1。
struct CountGuard;
impl Drop for CountGuard {
    fn drop(&mut self) {
        CONN_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 当前已连接客户端数(更新编排的「UI 全部断开」判定数据源)。
pub fn connected_clients() -> usize {
    CONN_COUNT.load(Ordering::SeqCst)
}

/// 更新进行中标记路径:`{home}/update-in-progress`(内容 = 目标版本)。
/// 由 daemon 在 Installing 序列原子写入,重启后开机消费(已更新通知/未完成
/// 告警);cli/uilaunch 的拉起守卫也读它。纯路径拼接,跨平台可编译。
pub fn update_marker_path(home: &Path) -> PathBuf {
    home.join("update-in-progress")
}

/// 重启 UI 标记路径:`{home}/restart-ui`(内容无关)。Installing 序列在
/// 有已连客户端时写入(更新退出会关掉 UI),新 daemon 开机消费:清除 +
/// 唤回主窗。纯路径拼接,跨平台可编译。
pub fn restart_ui_marker_path(home: &Path) -> PathBuf {
    home.join("restart-ui")
}

/// 读更新进行中标记:Some(目标版本) / None(不存在或读失败——两种情况
/// 同口径,守卫面与开机消费都不得因坏标记失败)。
pub fn update_in_progress(home: &Path) -> Option<String> {
    std::fs::read_to_string(update_marker_path(home))
        .ok()
        .map(|s| s.trim().to_string())
}

/// 等待中的响应表:id → 应答 oneshot。
type PendingMap = HashMap<u64, oneshot::Sender<Result<serde_json::Value, RpcError>>>;

/// MTDSS 数据目录:`$MTDSS_HOME` 覆盖，否则 `$HOME`/`%USERPROFILE%` 下的 `.mtdss`。
pub fn mtdss_home() -> PathBuf {
    if let Ok(p) = std::env::var("MTDSS_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".mtdss")
}

/// 由 home 派生本地套接字名。Windows:`\\.\pipe\mtdss-` + home 的
/// 「绝对化 + 词法规范化」路径串的 sha256 前 8 个十六进制字符(不 canonicalize,
/// home 允许尚不存在);Unix:`{home}/daemon.sock`。
/// serve 与 IpcClient 由同一函数同口径派生,两侧不会错位。
pub fn socket_path(home: &Path) -> String {
    #[cfg(windows)]
    {
        let digest = Sha256::digest(normalize_absolute(home).to_string_lossy().as_bytes());
        let tag = &hex::encode(digest)[..8];
        format!(r"\\.\pipe\mtdss-{tag}")
    }
    #[cfg(unix)]
    {
        home.join("daemon.sock").to_string_lossy().into_owned()
    }
}

/// 绝对化 + 词法规范化:相对路径拼当前目录,再逐组件消除 `.` 与 `..`。
/// 纯字符串操作,不触碰文件系统(不 canonicalize——home 可能尚不存在)。
#[cfg(windows)]
fn normalize_absolute(p: &Path) -> PathBuf {
    use std::path::Component;

    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// JSON-RPC 错误对象(`{"code":i64,"message":...}`,可选 `data` 附结构化
/// 数据)。daemon 在 FsError 驱动的错误上附 `data = {"kind": <fs 稳定码>}`
/// (NOT_FOUND/PERMISSION_DENIED/INVALID_PATH/RECYCLE_UNAVAILABLE/…),CLI
/// 据此类型化判定;`data` 缺省时序列化省略、反序列化缺省 None(旧版报文
/// 无该字段亦兼容)。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// 无附加数据的错误(参数校验、业务失败、传输层故障等)。
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// 携带结构化附加数据 `data` 的错误(daemon 面:`{"kind": <fs 稳定码>}`)。
    pub fn with_data(code: i64, message: impl Into<String>, data: serde_json::Value) -> Self {
        Self { code, message: message.into(), data: Some(data) }
    }

    /// `data.kind` 字符串(FsError 稳定码判定键;无 data / 非对象 / kind
    /// 非字符串 → None)。
    pub fn kind(&self) -> Option<&str> {
        self.data.as_ref()?.get("kind")?.as_str()
    }
}

/// 推送事件帧:`{"method":"...","params":...}`(线上会补 `"jsonrpc":"2.0"` 前缀,无 id)。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventFrame {
    pub method: String,
    pub params: serde_json::Value,
}

/// 请求分发器:由守护进程实现;返回 boxed Future 以支持异步处理。
pub trait Dispatcher: Send + Sync {
    fn dispatch(
        &self,
        method: String,
        params: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, RpcError>> + Send>,
    >;
}

/// 当前进程用户 SID 的字符串形(SDDL `A;;GA;;;{sid}` 条目用)。链路:
/// OpenProcessToken(本进程令牌,TOKEN_QUERY)→ GetTokenInformation(TokenUser,
/// 先探长再取)→ ConvertSidToStringSidW(LocalFree 收 PWSTR 缓冲)。任一步
/// 失败返回 None,调用方回退默认 DACL 并留痕——收紧失败不得破坏 IPC。
#[cfg(windows)]
pub(crate) fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY:GetCurrentProcess 恒返回伪句柄;OpenProcessToken 写回令牌句柄
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
    }
    // 令牌句柄的收尾守卫:以下任何早退路径都经 Drop 关闭
    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            // SAFETY:有效令牌句柄,CloseHandle 幂等无害
            unsafe { CloseHandle(self.0) };
        }
    }
    let _token = TokenHandle(token);

    // SAFETY:GetTokenInformation 空缓冲探长(预期 ERROR_INSUFFICIENT_BUFFER,
    // 返回 0),再按 needed 分配后实取。缓冲以 u64 背书:TOKEN_USER 起始含
    // PSID(指针),须 8 对齐,Vec<u8> 无对齐保证。buf 生存期内 TOKEN_USER
    // 借用其内存
    unsafe {
        let mut needed = 0u32;
        if GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) != 0 {
            return None; // 空缓冲不应成功,拒绝歧义
        }
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut needed,
        ) == 0
        {
            return None;
        }
        let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(token_user.User.Sid, &mut sid_str) == 0 {
            return None;
        }
        let result = (|| {
            let mut len = 0usize;
            while *sid_str.add(len) != 0 {
                len += 1;
            }
            let wide = std::slice::from_raw_parts(sid_str, len);
            if wide.is_empty() {
                return None;
            }
            Some(String::from_utf16_lossy(wide))
        })();
        LocalFree(sid_str.cast());
        result
    }
}

/// Windows 管道安全描述符:显式 DACL 仅授 当前用户 + SYSTEM + Administrators
/// 全部访问权(`D:P` 保护 DACL 不被父对象继承改写),替代 CreateNamedPipe
/// 默认 DACL 对 Everyone/匿名只读的放行(此前同机他用户可被动接收事件
/// 广播)。构造全走安全封装:SDDL → SecurityDescriptor 用 interprocess 的
/// `deserialize`(内部 ConvertStringSecurityDescriptorToSecurityDescriptorW
/// 并转为绝对格式),无本侧 unsafe;SID 查询失败/转换失败均返回 None。
#[cfg(windows)]
fn pipe_security_descriptor(
) -> Option<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    let sid = current_user_sid()?;
    let sddl = format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)(A;;GA;;;BA)");
    let sddl_wide = widestring::U16CString::from_str(&sddl).ok()?;
    interprocess::os::windows::security_descriptor::SecurityDescriptor::deserialize(&sddl_wide)
        .ok()
}

/// 在 home 派生的本地套接字上监听并服务。连接逐个 accept;每个连接拆
/// 读半工(解析请求 → 分发 → 响应进写队列)与写半工(写队列 + 事件广播合并写出)。
/// Unix 下 socket 文件权限 0600;Windows 下显式 SDDL 限当前用户/SYSTEM/
/// Administrators 全权(见 pipe_security_descriptor;构造失败回退默认 DACL)。
pub async fn serve(
    home: &Path,
    dispatcher: Arc<dyn Dispatcher>,
    event_tx: broadcast::Sender<EventFrame>,
) -> anyhow::Result<()> {
    let name = socket_path(home);
    std::fs::create_dir_all(home)
        .with_context(|| format!("create dir {}", home.display()))?;
    #[cfg(unix)]
    {
        // 残留的旧 socket 文件会让 bind 报地址占用,先清理(best-effort)
        let sock = home.join("daemon.sock");
        if sock.exists() {
            let _ = std::fs::remove_file(&sock);
        }
    }
    // Windows 侧 SDDL 收紧分支会重赋值 options 才需要 mut;非 Windows 编译
    // 单元里 cfg 块不存在,绑定永不写入——unix 下静默 unused-mut(否则
    // clippy -D warnings 红,Windows 本机编译不可见)。
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut options = ListenerOptions::new().name(name.as_str().to_fs_name::<GenericFilePath>()?);
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::ListenerOptionsExt as _;
        match pipe_security_descriptor() {
            Some(sd) => options = options.security_descriptor(sd),
            None => {
                tracing::warn!(
                    "管道安全描述符构造失败,回退默认 DACL(同机其他用户可被动读事件广播)"
                );
            }
        }
    }
    let listener = options
        .create_tokio()
        .with_context(|| format!("listen on {name}"))?;
    #[cfg(unix)]
    {
        // socket 文件仅本用户可读写,防止同机他人连接
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            home.join("daemon.sock"),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    loop {
        let stream = listener.accept().await.context("ipc accept")?;
        // 连接计数 +1(更新编排等「UI 全部断开」的判定面);-1 在写循环收尾
        CONN_COUNT.fetch_add(1, Ordering::SeqCst);
        let (read_half, write_half) = stream.split();
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(OUT_CHANNEL_CAP);
        let ev_rx = event_tx.subscribe();
        // 读半工与写半工各持一个 out_tx 克隆;读半工退出时它那份 drop,
        // 写半工的 out_rx.recv() 随之归 None 而收尾。
        // 注:写半工自己恒持一个 sender,out_rx 对它永不闭合——「None 即断」
        // 实际不可达;写半工的可靠收尾靠 read_gone(对端消失,读半工终局)
        // 直达断开,否则对端走人后连接实例常驻(命名管道名随之占死,守护
        // 进程重启即撞 bind 失败),只能等下一帧事件写失败才顺带退出。
        let (read_gone_tx, read_gone_rx) = tokio::sync::watch::channel(false);
        let read_out_tx = out_tx.clone();
        let read_dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            read_loop(read_half, read_out_tx, read_dispatcher).await;
            let _ = read_gone_tx.send(true);
        });
        tokio::spawn(write_loop(write_half, out_tx, out_rx, ev_rx, read_gone_rx));
    }
}

/// 连接读半工:逐帧解析请求并分发,响应写入写队列。
/// EOF、行超限或 IO 错误即结束——out_tx 随之 drop,写半工随之收尾。
async fn read_loop(
    read_half: RecvHalf,
    out_tx: mpsc::Sender<Vec<u8>>,
    dispatcher: Arc<dyn Dispatcher>,
) {
    let mut reader = BufReader::new(read_half);
    let mut line = Vec::new();
    loop {
        match read_capped_line(&mut reader, &mut line, MAX_FRAME_BYTES).await {
            Ok(true) => {}
            Ok(false) => break, // 对端正常关闭
            Err(err) => {
                tracing::debug!(%err, "ipc 读半工结束");
                break;
            }
        }
        let response = handle_frame(&dispatcher, &line).await;
        let mut bytes = serde_json::to_vec(&response).expect("Value 序列化不会失败");
        bytes.push(b'\n');
        if out_tx.send(bytes).await.is_err() {
            break; // 写半工已退出
        }
    }
}

/// 处理一帧请求,返回恰好一帧应写回的响应(id 回显;无法解析时 id 为 null)。
async fn handle_frame(dispatcher: &Arc<dyn Dispatcher>, line: &[u8]) -> serde_json::Value {
    let req: serde_json::Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(err) => {
            // 无法解析:按 JSON-RPC 约定回 Parse error,id 为 null
            return json!({
                "id": null,
                "error": {"code": -32700i64, "message": format!("parse error: {err}")},
            });
        }
    };
    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let Some(method) = req.get("method").and_then(|m| m.as_str()).map(str::to_owned) else {
        return json!({
            "id": id,
            "error": {"code": -32600i64, "message": "invalid request: missing method"},
        });
    };
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    match dispatcher.dispatch(method, params).await {
        Ok(result) => json!({"id": id, "result": result}),
        Err(err) => json!({"id": id, "error": err}),
    }
}

/// 连接写半工:把写队列中的响应帧与广播事件合并写出。
/// 事件入队用 try_send:写队列满(慢消费者)或通道关闭即断开该连接,
/// 保证一个卡死的客户端不拖垮其他连接。
/// `read_gone`:读半工终局信号(EOF/错误/任务收尾)——对端已走,立即断开
/// 本连接并释放管道实例。写半工恒持一个 out_tx,out_rx 对它永不闭合,
/// 「None 即断」不可达;没有本信号,对端走人后的连接只能等下一帧事件
/// 写失败才退出(事件静默期即永久占住管道名)。
async fn write_loop(
    mut write_half: SendHalf,
    out_tx: mpsc::Sender<Vec<u8>>,
    mut out_rx: mpsc::Receiver<Vec<u8>>,
    mut ev_rx: broadcast::Receiver<EventFrame>,
    mut read_gone: tokio::sync::watch::Receiver<bool>,
) {
    // select 臂内不能再用被另一臂 future 独占借用的 out_rx,事件先暂存,select 后处理
    let mut pending_event: Option<Result<EventFrame, broadcast::error::RecvError>> = None;
    // 连接计数 -1 端:Drop 即减,与 serve accept 处的 +1 配对
    let _conn_guard = CountGuard;
    loop {
        // biased:写队列优先——读半工退出(out_rx 归 None)时立即收尾,不被事件等待拖住
        tokio::select! {
            biased;
            maybe = out_rx.recv() => match maybe {
                Some(bytes) => {
                    if write_half.write_all(&bytes).await.is_err() {
                        break;
                    }
                    if write_half.flush().await.is_err() {
                        break;
                    }
                }
                None => break, // 读半工退出,连接收尾
            },
            _ = read_gone.changed() => {
                if *read_gone.borrow() {
                    break; // 读半工终局:对端已走,断开并释放管道实例
                }
            }
            evt = ev_rx.recv() => pending_event = Some(evt),
        }
        match pending_event.take() {
            None => {}
            Some(Ok(frame)) => {
                // 事件帧 = 通知帧:补 "jsonrpc":"2.0",无 id
                let mut line = json!({
                    "jsonrpc": "2.0",
                    "method": frame.method,
                    "params": frame.params,
                })
                .to_string()
                .into_bytes();
                line.push(b'\n');
                if out_tx.try_send(line).is_err() {
                    // 队列满或已关闭:该连接过慢,跳帧并断连
                    tracing::warn!("ipc 慢消费者,断开该连接");
                    break;
                }
            }
            Some(Err(broadcast::error::RecvError::Lagged(n))) => {
                // 广播通道因该连接消费过慢而丢帧:同样按慢消费者断连
                tracing::warn!(skipped = n, "ipc 事件通道滞后,断开该连接");
                break;
            }
            Some(Err(broadcast::error::RecvError::Closed)) => break,
        }
    }
}

/// 从缓冲读取器读一行(到 `\n` 为止,不含分隔符),累计长度超过 `cap` 即报错,
/// 防止异常对端撑爆内存。`Ok(false)` 表示对端正常关闭(EOF)。
async fn read_capped_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut Vec<u8>,
    cap: usize,
) -> io::Result<bool> {
    out.clear();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            // EOF:残余无换行的内容视为截断帧,按关闭处理
            return Ok(false);
        }
        match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                if out.len() + i > cap {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
                }
                out.extend_from_slice(&chunk[..i]);
                std::pin::Pin::new(&mut *reader).consume(i + 1);
                return Ok(true);
            }
            None => {
                if out.len() + chunk.len() > cap {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
                }
                out.extend_from_slice(chunk);
                let n = chunk.len();
                std::pin::Pin::new(&mut *reader).consume(n);
            }
        }
    }
}

/// 本地 JSON-RPC 客户端:请求按自增 id 路由,推送事件经内部广播通道分发。
pub struct IpcClient {
    /// 写半;并发 call 共用,互斥串行化完整帧写入,防止帧交错。
    write: Arc<tokio::sync::Mutex<SendHalf>>,
    /// 等待响应的 oneshot 表(id → 应答)。
    pending: Arc<Mutex<PendingMap>>,
    next_id: AtomicU64,
    /// 推送事件广播:读半工收到通知帧后 send;订阅方由 events() 产生。
    event_tx: broadcast::Sender<EventFrame>,
    /// 读半工任务句柄;Drop 时中止——interprocess 的 split 为共享句柄
    /// (两半都 drop 才真正关闭),读半工任务持 RecvHalf 阻塞等待,若不
    /// 中止,客户端即使整体 drop 连接也永不关闭,对端与命名管道实例
    /// 随之常驻(后续连接 ERROR_PIPE_BUSY)。中止读半工即释放其半,
    /// 与已随 drop 释放的写半合并,句柄关闭、连接收尾。
    read_task: tokio::task::JoinHandle<()>,
}

impl Drop for IpcClient {
    fn drop(&mut self) {
        self.read_task.abort();
    }
}

impl IpcClient {
    /// 连接 home 派生的本地套接字;连不上立即返回 Err(快速失败,不重试)。
    pub async fn connect(home: &Path) -> anyhow::Result<Self> {
        let name = socket_path(home);
        let stream = Stream::connect(name.as_str().to_fs_name::<GenericFilePath>()?)
            .await
            .with_context(|| format!("connect to {name}"))?;
        let (read_half, write_half) = stream.split();
        let (event_tx, _) = broadcast::channel(CLIENT_EVENT_CAP);
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));
        // 读半工先行启动(直取共享件),句柄存入 client 供 Drop 中止
        let read_task = Self::spawn_read_loop(read_half, pending.clone(), event_tx.clone());
        Ok(Self {
            write: Arc::new(tokio::sync::Mutex::new(write_half)),
            pending,
            next_id: AtomicU64::new(1),
            event_tx,
            read_task,
        })
    }

    /// 发起一次 JSON-RPC 调用;id 自增,并发调用按 id 路由互不串扰。
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string()
        .into_bytes();
        line.push(b'\n');
        // 完整帧写入互斥,保证并发 call 的帧不交错
        let written = async {
            let mut w = self.write.lock().await;
            w.write_all(&line).await?;
            w.flush().await?;
            Ok::<(), io::Error>(())
        }
        .await;
        if let Err(err) = written {
            self.pending.lock().unwrap().remove(&id);
            return Err(RpcError::new(-32000, format!("ipc 写入失败: {err}")));
        }
        match rx.await {
            Ok(reply) => reply,
            Err(_) => Err(RpcError::new(-32000, "ipc 连接已断开")),
        }
    }

    /// 订阅推送事件。
    pub fn events(&self) -> broadcast::Receiver<EventFrame> {
        self.event_tx.subscribe()
    }

    /// 启动读半工:响应帧按 id 路由到等待中的 call;通知帧(无 id)进事件广播。
    /// 返回任务句柄供 Drop 中止(见 IpcClient.read_task 注释)。
    fn spawn_read_loop(
        read_half: RecvHalf,
        pending: Arc<Mutex<PendingMap>>,
        event_tx: broadcast::Sender<EventFrame>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut reader = BufReader::new(read_half);
            let mut line = Vec::new();
            loop {
                match read_capped_line(&mut reader, &mut line, MAX_FRAME_BYTES).await {
                    Ok(true) => {}
                    _ => break, // EOF / 超限 / IO 错误:连接结束
                }
                let frame: serde_json::Value = match serde_json::from_slice(&line) {
                    Ok(v) => v,
                    Err(err) => {
                        tracing::debug!(%err, "ipc 客户端丢弃无法解析的帧");
                        continue;
                    }
                };
                match frame.get("id").and_then(serde_json::Value::as_u64) {
                    Some(id) => {
                        // 响应帧:{"id":N,"result":...} 或 {"id":N,"error":{...}}
                        let reply = if let Some(err) = frame.get("error") {
                            match serde_json::from_value::<RpcError>(err.clone()) {
                                Ok(err) => Err(err),
                                Err(e) => Err(RpcError::new(
                                    -32000,
                                    format!("malformed error frame: {e}"),
                                )),
                            }
                        } else {
                            Ok(frame.get("result").cloned().unwrap_or(serde_json::Value::Null))
                        };
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(reply);
                        }
                    }
                    None => {
                        // 通知帧(无 id):事件
                        match serde_json::from_value::<EventFrame>(frame) {
                            Ok(evt) => {
                                let _ = event_tx.send(evt); // 无订阅者则丢弃
                            }
                            Err(err) => {
                                tracing::debug!(%err, "ipc 客户端丢弃畸形事件帧");
                            }
                        }
                    }
                }
            }
            // 连接断开:唤醒所有等待中的 call,以免悬挂
            for (_, tx) in pending.lock().unwrap().drain() {
                let _ = tx.send(Err(RpcError::new(-32000, "ipc 连接已断开")));
            }
        })
    }
}

/// 解析 mtdssd 守护进程可执行文件路径。解析序:
/// 1. 环境变量 `MTDSSD_BIN`(显式覆盖,原样采用);
/// 2. 当前可执行文件同目录的 `mtdssd[.exe]`(发布布局);
/// 3. debug 构建下从 `CARGO_MANIFEST_DIR` 逐级向上找 `target/{debug,release}/mtdssd[.exe]`
///    (cargo run / 测试场景)。
pub fn find_daemon_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MTDSSD_BIN") {
        return Some(PathBuf::from(p));
    }
    let file_name = if cfg!(windows) { "mtdssd.exe" } else { "mtdssd" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(file_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    #[cfg(debug_assertions)]
    {
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            for anc in PathBuf::from(manifest).ancestors() {
                for profile in ["debug", "release"] {
                    let cand = anc.join("target").join(profile).join(file_name);
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }
    }
    None
}

/// SDDL 收紧路径自证(仅 Windows):SID 可取且形状正确,描述符可构造——
/// 守住「静默回退默认 DACL」的盲区(同用户连通性由 ipc.rs 集成套件覆盖;
/// 跨用户拒绝不可 CI 化,由发布冒烟人工验证)。
#[cfg(all(test, windows))]
mod sddl_tests {
    use super::*;

    #[test]
    fn pipe_descriptor_builds_from_current_user_sid() {
        let sid = current_user_sid().expect("当前用户 SID 应可取");
        assert!(sid.starts_with("S-1-"), "SID 形状须为 S-1-…: {sid}");
        let sd = pipe_security_descriptor().expect("安全描述符应可构造");
        drop(sd); // 构造成功即证;Drop 走 interprocess 的释放路径
    }
}
