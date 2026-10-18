//! mtdss-ipc:本机 IPC 上的 JSON-RPC(NDJSON 线格式)服务端与客户端。
//!
//! 传输层用 `interprocess` 本地套接字(Windows 命名管道 / Unix 域套接字),
//! 线格式为 NDJSON——一行一帧:
//! - 请求:  `{"jsonrpc":"2.0","id":N,"method":"...","params":...}`
//! - 响应:  `{"id":N,"result":...}` 或 `{"id":N,"error":{"code":i64,"message":"..."}}`
//! - 事件:  通知帧 `{"jsonrpc":"2.0","method":"...","params":...}`(无 id)
//!
//! 服务端为每个连接拆「读半工 + 写半工」两个任务;事件向所有连接广播,
//! 写队列满的慢消费者立即断连,保证一个卡死的客户端不拖垮其他连接。
//! 单帧上限 8 MiB,超限断连,防止异常对端撑爆内存。

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

/// JSON-RPC 错误对象(`{"code":i64,"message":...}`)。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
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

/// 在 home 派生的本地套接字上监听并服务。连接逐个 accept;每个连接拆
/// 读半工(解析请求 → 分发 → 响应进写队列)与写半工(写队列 + 事件广播合并写出)。
/// Unix 下 socket 文件权限 0600(Windows 命名管道默认 ACL 即本用户)。
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
    let listener = ListenerOptions::new()
        .name(name.as_str().to_fs_name::<GenericFilePath>()?)
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
        let (read_half, write_half) = stream.split();
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(OUT_CHANNEL_CAP);
        let ev_rx = event_tx.subscribe();
        // 读半工与写半工各持一个 out_tx 克隆;读半工退出时它那份 drop,
        // 写半工的 out_rx.recv() 随之归 None 而收尾
        tokio::spawn(read_loop(read_half, out_tx.clone(), dispatcher.clone()));
        tokio::spawn(write_loop(write_half, out_tx, out_rx, ev_rx));
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
async fn write_loop(
    mut write_half: SendHalf,
    out_tx: mpsc::Sender<Vec<u8>>,
    mut out_rx: mpsc::Receiver<Vec<u8>>,
    mut ev_rx: broadcast::Receiver<EventFrame>,
) {
    // select 臂内不能再用被另一臂 future 独占借用的 out_rx,事件先暂存,select 后处理
    let mut pending_event: Option<Result<EventFrame, broadcast::error::RecvError>> = None;
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
            return Err(RpcError {
                code: -32000,
                message: format!("ipc 写入失败: {err}"),
            });
        }
        match rx.await {
            Ok(reply) => reply,
            Err(_) => Err(RpcError {
                code: -32000,
                message: "ipc 连接已断开".into(),
            }),
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
                                Err(e) => Err(RpcError {
                                    code: -32000,
                                    message: format!("malformed error frame: {e}"),
                                }),
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
                let _ = tx.send(Err(RpcError {
                    code: -32000,
                    message: "ipc 连接已断开".into(),
                }));
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
