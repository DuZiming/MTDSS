//! fdpass:fd 传递侧通道(Android Kotlin 客户端 ↔ daemon,SCM_RIGHTS)。
//!
//! 「文件本体走 fd」的旁路通道:JSON-RPC 通道只传元数据,文件内容经本通道
//! 以 fd 形式直传(daemon 不经 IPC 缓冲拷贝文件字节)。daemon 侧跑
//! [`serve_fdpass`] 监听 `{home}/fds.sock`;线上对端是 Android Kotlin 实现,
//! Rust 侧的 [`FdPassClient`] 与其共享同一线格式,供测试与 mock provider
//! 同源对打。
//!
//! # 线格式(与 Android Kotlin 实现共享,两侧实现必须逐字节一致)
//!
//! ```text
//! kind u8 | payload_len u32(大端) | payload(UTF-8 JSON,≤ 64 KiB)
//! ```
//!
//! fd 不进线格式,经 SCM_RIGHTS 控制消息随帧附带(0 或 1 个);应答同格式
//! 回流,同样可带 fd。kind 常量:
//!
//! | 值   | 名          | 方向          | payload                     | fd |
//! |------|-------------|---------------|-----------------------------|----|
//! | 0x01 | source_fd   | Kotlin→daemon | `{request_id}`              | 1  |
//! | 0x02 | dest_fd     | Kotlin→daemon | `{request_id}`              | 1  |
//! | 0x03 | dest_commit | daemon→Kotlin | `{request_id, done}`(T6)   | 0  |
//! | 0x04 | dest_query  | Kotlin→daemon | `{request_id, exists, size}`(F7,无 fd) | 0  |
//! | 0x81 | ack         | daemon→Kotlin | `{request_id, ok, error?}`  | 0  |
//!
//! # 实现口径
//!
//! 裸 `libc::sendmsg/recvmsg`(interprocess 不暴露 ancillary data):accepted
//! 连接转阻塞模式,每连接一个 `spawn_blocking` actor(读主循环,handler
//! 异步逻辑经 `Handle::block_on` 桥接)+ 一个写线程(消费应答队列)。单帧
//! payload 上限 64 KiB,超限断连;协议异常(随帧 fd 数不为 0/1、帧截断、
//! 非法 JSON、应答超限)一律断连,防御性收口,不让坏对端拖垮 daemon。

use std::future::Future;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::thread;

use anyhow::{Context as _, Result};
use tokio::net::UnixListener;

/// 帧头长度:`kind u8 | len u32 BE`。
const HEADER_LEN: usize = 5;
/// 单帧 payload(JSON)字节上限:64 KiB(元数据足够)。
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
/// 每连接应答队列容量;满则 `FdReply::send` 阻塞,形成对慢消费者的背压。
const OUT_QUEUE_CAP: usize = 64;

/// 0x01 source_fd:Kotlin→daemon,上传源 fd(payload `{request_id}` + 1 fd)。
pub const KIND_SOURCE_FD: u8 = 0x01;
/// 0x02 dest_fd:Kotlin→daemon,下载目标 fd(payload `{request_id}` + 1 fd)。
pub const KIND_DEST_FD: u8 = 0x02;
/// 0x03 dest_commit:daemon→Kotlin,落盘确认(payload `{request_id, done}`,T7)。
pub const KIND_DEST_COMMIT: u8 = 0x03;
/// 0x04 dest_query:Kotlin→daemon,在场查询应答(payload `{request_id, exists,
/// size}`,**无 fd**;设备场 F7——收件箱 final_state 的 provider 在场证明:
/// 行在且 size 吻合→ exists,否则 false 让引擎走 FULL 重收)。
pub const KIND_DEST_QUERY: u8 = 0x04;
/// 0x81 ack:daemon→Kotlin 应答(payload `{request_id, ok, error?}`)。
pub const KIND_ACK: u8 = 0x81;

/// recvmsg 的附加标志:Linux/Android 用 MSG_CMSG_CLOEXEC,收到的 fd 自带
/// close-on-exec,防 daemon 未来向子进程 exec 时句柄外泄;macOS/iOS 的 libc
/// 无此常量,传 0(P5 iOS 落地时应补逐 fd 的 fcntl FD_CLOEXEC)。
#[cfg(any(target_os = "linux", target_os = "android"))]
const RECVMSG_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const RECVMSG_FLAGS: libc::c_int = 0;

/// recv 控制缓冲的 fd 槽位数:对端单次塞入超过此数,内核置 MSG_CTRUNC
/// (已装入的仍须本侧关闭),与「帧内 fd 数超 1」一起按协议异常断连。
const CMSG_FD_SLOTS: usize = 8;

/// 一条 fdpass 帧:kind + JSON payload + 随帧 fd(0/1 个)。
#[derive(Debug)]
pub struct FdMsg {
    pub kind: u8,
    pub payload: serde_json::Value,
    pub fd: Option<OwnedFd>,
}

/// 应答手柄:handler 借此把一帧(同格式,可带 fd)回给本连接客户端。
/// 底层为有界队列,满则 `send` 阻塞(慢消费者背压);连接已断则报错。
/// `Clone`(v2-P3 T6):0x03 dest_commit 经同一连接**事后**送回——handler
/// 把手柄克隆暂存,commit 时原路推送(服务端主动帧不经请求即达)。
#[derive(Clone)]
pub struct FdReply {
    tx: std::sync::mpsc::SyncSender<FdMsg>,
}

impl FdReply {
    /// 回一帧给客户端;连接关闭(或队列再也不会被消费)时报错。
    pub fn send(&self, msg: FdMsg) -> Result<()> {
        self.tx
            .send(msg)
            .map_err(|_| anyhow::anyhow!("fdpass 连接已关闭,应答未送达"))
    }
}

/// fdpass 消息处理器:daemon 实现;每收到一帧调用一次,处理完(含应答)
/// 才读下一帧。返回 boxed Future 以支持异步处理(与 [`crate::Dispatcher`] 同款)。
pub trait FdMsgHandler: Send + Sync {
    fn on_msg(&self, msg: FdMsg, reply: FdReply) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// 在 `{home}/fds.sock` 上监听并服务 fdpass 通道。socket 文件权限 0600,
/// bind 前清理残留(与 [`crate::serve`] 同款)。每连接一个阻塞 actor 任务。
pub async fn serve_fdpass(home: &Path, handler: Arc<dyn FdMsgHandler>) -> Result<()> {
    let sock = home.join("fds.sock");
    std::fs::create_dir_all(home).with_context(|| format!("create dir {}", home.display()))?;
    // 残留的旧 socket 文件会让 bind 报地址占用,先清理(best-effort)
    if sock.exists() {
        let _ = std::fs::remove_file(&sock);
    }
    let listener = UnixListener::bind(&sock).with_context(|| format!("listen on {}", sock.display()))?;
    // socket 文件仅本用户可读写,防止同机他人连接
    let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
    loop {
        let (stream, _) = listener.accept().await.context("fdpass accept")?;
        let stream = stream.into_std().context("fdpass into_std")?;
        // 阻塞模式:actor 跑在阻塞线程,sendmsg/recvmsg 直接同步调用
        stream.set_nonblocking(false).context("fdpass set_nonblocking")?;
        let handler = handler.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || conn_actor(stream, handler, handle));
    }
}

/// 逐连接 actor:读主循环解析帧 → 交 handler(block_on 桥接)→ 应答经队列
/// 交写线程。串行处理(一帧处理完才读下一帧),与 fd 单向交接语义匹配。
fn conn_actor(stream: UnixStream, handler: Arc<dyn FdMsgHandler>, handle: tokio::runtime::Handle) {
    // 写半独立 fd:读写各持一个描述符,互不干扰
    let writer = match stream.try_clone() {
        Ok(w) => w,
        Err(err) => {
            tracing::debug!(%err, "fdpass try_clone 失败,连接丢弃");
            return;
        }
    };
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel::<FdMsg>(OUT_QUEUE_CAP);
    thread::spawn(move || writer_loop(writer, reply_rx));
    loop {
        match read_frame(&stream) {
            Ok(Some(msg)) => {
                // handler 的异步逻辑在阻塞线程上桥接执行;on_msg 返回即视为
                // 本帧处理完(含应答已入队)。注:current_thread 运行时上
                // Handle::block_on 仅由本线程推进该 future,依赖同运行时其他
                // 任务推进的 handler 会停滞——daemon 为 #[tokio::main] 多线程
                // 运行时,不受影响;测试 handler 均不含 await。
                handle.block_on(handler.on_msg(msg, FdReply { tx: reply_tx.clone() }));
            }
            Ok(None) => break, // 对端正常关闭
            Err(err) => {
                tracing::debug!(%err, "fdpass 连接结束");
                break;
            }
        }
    }
    // 本端 tx drop;handler 未暂存 reply 时,写线程排干队列后自然退出。
    // 不 join 写线程:handler 若永持 FdReply,写线程随其悬挂并持有连接 fd,
    // 但 accept 线程不受牵连——与 ipc 慢消费者断连口径一致的防御取舍。
    drop(reply_tx);
}

/// 写线程:消费应答队列,逐帧写出。序列化失败或应答超限按协议异常断连。
fn writer_loop(writer: UnixStream, rx: std::sync::mpsc::Receiver<FdMsg>) {
    for msg in rx {
        let payload = match serde_json::to_vec(&msg.payload) {
            Ok(v) => v,
            Err(err) => {
                tracing::debug!(%err, "fdpass 应答序列化失败,断开连接");
                break;
            }
        };
        if payload.len() > MAX_PAYLOAD_BYTES {
            tracing::debug!(len = payload.len(), "fdpass 应答超限,断开连接");
            break;
        }
        let fd_raw = msg.fd.as_ref().map(AsRawFd::as_raw_fd);
        if let Err(err) = write_frame(writer.as_raw_fd(), msg.kind, &payload, fd_raw) {
            tracing::debug!(%err, "fdpass 写线程结束");
            break;
        }
    }
}

/// 读一帧:`kind u8 | len u32 BE | JSON` + 可选随帧 fd。`Ok(None)` 表示对端
/// 在帧边界正常关闭;其余错误(超限/截断/非法 JSON/fd 数异常)一律视为
/// 协议异常,调用方应断连。
///
/// 读取严格按帧边界切分(帧头 5 字节、payload 恰 len 字节,recvmsg 缓冲
/// 永不跨帧):SCM_RIGHTS 的 fd 随携它的 sendmsg 的首数据字节所在的
/// recvmsg 交付,帧边界对齐读取保证收到的 fd 无歧义归属当前帧——这是
/// `fd_out` 跨多次 recvmsg 累计仍安全的不变式。
fn read_frame(stream: &UnixStream) -> Result<Option<FdMsg>> {
    let fd = stream.as_raw_fd();
    let mut header = [0u8; HEADER_LEN];
    let mut have = 0usize;
    let mut fd_seen: Option<OwnedFd> = None;
    while have < HEADER_LEN {
        let n = recv_some(fd, &mut header[have..], &mut fd_seen)?;
        if n == 0 {
            if have == 0 && fd_seen.is_none() {
                return Ok(None); // 帧边界上的干净 EOF
            }
            anyhow::bail!("fdpass 对端中途关闭(帧头截断)");
        }
        have += n;
    }
    let kind = header[0];
    let len = u32::from_be_bytes(header[1..HEADER_LEN].try_into().expect("帧头长度恒为 5")) as usize;
    if len > MAX_PAYLOAD_BYTES {
        anyhow::bail!("fdpass 帧 payload {len} 字节,超 {MAX_PAYLOAD_BYTES} 上限");
    }
    let mut payload = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        let n = recv_some(fd, &mut payload[got..], &mut fd_seen)?;
        if n == 0 {
            anyhow::bail!("fdpass 对端中途关闭(payload 截断)");
        }
        got += n;
    }
    let payload: serde_json::Value = serde_json::from_slice(&payload)
        .context("fdpass payload 非法 JSON")?;
    Ok(Some(FdMsg { kind, payload, fd: fd_seen }))
}

/// recvmsg 一次:读满 `buf` 尽可能多的字节,并解析随帧 SCM_RIGHTS fd。
/// 收到 fd 的口径:整帧最多 1 个;任何一次读取里出现第 2 个 fd、fd 已收过
/// 又来新的、或 MSG_CTRUNC(对端一次塞了超量 fd,内核截断)都按协议异常
/// 断连——收到的 fd 全部关闭,绝不交给 handler。
fn recv_some(fd: RawFd, buf: &mut [u8], fd_out: &mut Option<OwnedFd>) -> Result<usize> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    // 控制缓冲预置 CMSG_FD_SLOTS 个 fd 位(探测超量,见常量注释)。
    // 缓冲以 usize 背书对齐:cmsghdr 首字段为 usize 长度、对齐要求不高于
    // usize,Vec<u8> 无对齐保证,直接 cast 解引用属未定义行为(同 lib.rs
    // current_user_sid 的 u64 背书理由)。
    let space = unsafe {
        libc::CMSG_SPACE((CMSG_FD_SLOTS * std::mem::size_of::<libc::c_int>()) as libc::c_uint)
    } as usize;
    let mut control = vec![0usize; space.div_ceil(std::mem::size_of::<usize>())];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1 as _;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    let n = unsafe { libc::recvmsg(fd, &mut msg, RECVMSG_FLAGS) };
    if n < 0 {
        return Err(io::Error::last_os_error()).context("fdpass recvmsg");
    }
    // 解析 cmsg,收集 SCM_RIGHTS 附带的 fd(所有权归本侧,Drop 即关闭)
    let mut fds: Vec<OwnedFd> = Vec::new();
    // SAFETY:控制缓冲与 msghdr 均为本次调用所得,CMSG_* 宏族按内核契约遍历
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let dlen = ((*cmsg).cmsg_len as usize) - libc::CMSG_LEN(0) as usize;
                let count = dlen / std::mem::size_of::<libc::c_int>();
                let data = libc::CMSG_DATA(cmsg).cast::<libc::c_int>();
                for i in 0..count {
                    // 内核保证 cmsg_len 覆盖 count 个 c_int;逐一收为 OwnedFd,
                    // 任何后续错误路径都经 Drop 关闭,不泄漏
                    let raw = *data.add(i);
                    fds.push(OwnedFd::from_raw_fd(raw));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    let truncated = msg.msg_flags & libc::MSG_CTRUNC != 0;
    let seen_before = usize::from(fd_out.is_some());
    if truncated || fds.len() + seen_before > 1 {
        let got = fds.len() + seen_before;
        drop(fds); // 防御性关闭全部收到的 fd
        anyhow::bail!(
            "fdpass 帧随附 fd 数异常(协议允许 0/1,本次累计 {got}{})",
            if truncated { ",内核缓冲截断" } else { "" }
        );
    }
    if let Some(f) = fds.pop() {
        *fd_out = Some(f);
    }
    Ok(n as usize)
}

/// 写一整帧:帧头 + payload 一次 sendmsg 写出,可附带 1 个 SCM_RIGHTS fd。
/// 阻塞 socket 上的 sendmsg 仍可能部分写(对端缓冲吃紧),循环补写;fd 随
/// 数据首字节交付,故补写必须摘除 cmsg,避免重复交付同一 fd。
fn write_frame(fd: RawFd, kind: u8, payload: &[u8], fd_to_send: Option<libc::c_int>) -> io::Result<()> {
    let mut header = [0u8; HEADER_LEN];
    header[0] = kind;
    header[1..HEADER_LEN].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    let mut iov = [
        libc::iovec {
            iov_base: header.as_mut_ptr().cast(),
            iov_len: header.len(),
        },
        libc::iovec {
            iov_base: payload.as_ptr() as *mut libc::c_void,
            iov_len: payload.len(),
        },
    ];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = 2 as _;
    // 控制缓冲声明在分支外:寿命必须覆盖下方整个 sendmsg 重试循环。
    // 若收进 if let 块内,块尾 Drop 即把缓冲归还分配器,msg_control 悬垂,
    // sendmsg 从被复用的堆内存读到垃圾 cmsg(实测 cmsg_len/level/type 全随
    // 机 → 内核 EINVAL;单线程裸进程可因堆未被复用而侥幸通过,多线程测试
    // 进程必现——这正是本侧通道曾经的实弹 bug)。
    let mut control: Vec<usize> = Vec::new();
    if let Some(fdx) = fd_to_send {
        let space =
            unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as libc::c_uint) } as usize;
        // usize 背书对齐(理由见 recv_some 同款注释)
        control.resize(space.div_ceil(std::mem::size_of::<usize>()), 0);
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        // SAFETY:控制缓冲已备,CMSG_FIRSTHDR 恒返回缓冲头且空间足够
        if cmsg.is_null() {
            return Err(io::Error::other("fdpass CMSG_FIRSTHDR 返回空"));
        }
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as libc::c_uint) as _;
            *(libc::CMSG_DATA(cmsg).cast::<libc::c_int>()) = fdx;
        }
    }
    let total = header.len() + payload.len();
    let mut sent = 0usize;
    loop {
        let n = unsafe { libc::sendmsg(fd, &msg, 0) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue; // 信号打断,一字节未出,cmsg 保持武装重试
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "fdpass sendmsg 返回 0"));
        }
        sent += n as usize;
        if sent >= total {
            return Ok(());
        }
        // 已有字节(连同随附 fd)出门:摘除 cmsg,补写剩余
        msg.msg_control = std::ptr::null_mut();
        msg.msg_controllen = 0;
        if sent < header.len() {
            iov[0].iov_base = header[sent..].as_mut_ptr().cast();
            iov[0].iov_len = header.len() - sent;
        } else {
            let p = sent - header.len();
            iov[0].iov_base = payload[p..].as_ptr() as *mut libc::c_void;
            iov[0].iov_len = payload.len() - p;
            iov[1].iov_len = 0;
        }
    }
}

/// fdpass 客户端(Rust 侧测试 / mock provider 用,与 Kotlin 客户端同线格式):
/// 连接 `{home}/fds.sock`,一问一答。`send_msg` 独占 `&mut`,防并发调用
/// 在同一 socket 上交错帧。
pub struct FdPassClient {
    stream: UnixStream,
}

impl FdPassClient {
    /// 连接 home 下的 fds.sock;连不上立即返回 Err(快速失败,不重试)。
    pub async fn connect(home: &Path) -> Result<Self> {
        let path = home.join("fds.sock");
        let stream = tokio::net::UnixStream::connect(&path)
            .await
            .with_context(|| format!("connect to {}", path.display()))?;
        let stream = stream.into_std().context("fdpass client into_std")?;
        // 与服务端同口径:阻塞模式,收发在阻塞线程上同步调用
        stream.set_nonblocking(false).context("fdpass client set_nonblocking")?;
        Ok(Self { stream })
    }

    /// 发一帧(可携 1 个 fd)并等待应答帧。payload 超限在本侧即拒绝;
    /// 服务端断连(超限帧/协议异常)时返回 Err。
    pub async fn send_msg(
        &mut self,
        kind: u8,
        payload: serde_json::Value,
        fd: Option<OwnedFd>,
    ) -> Result<FdMsg> {
        let bytes = serde_json::to_vec(&payload).context("fdpass payload 序列化")?;
        if bytes.len() > MAX_PAYLOAD_BYTES {
            anyhow::bail!("fdpass payload {} 字节,超 {MAX_PAYLOAD_BYTES} 上限", bytes.len());
        }
        let stream = self.stream.try_clone().context("fdpass client try_clone")?;
        tokio::task::spawn_blocking(move || -> Result<FdMsg> {
            let fd_raw = fd.as_ref().map(AsRawFd::as_raw_fd);
            write_frame(stream.as_raw_fd(), kind, &bytes, fd_raw)
                .context("fdpass 发送失败")?;
            match read_frame(&stream).context("fdpass 接收失败")? {
                Some(msg) => Ok(msg),
                None => anyhow::bail!("fdpass 服务端关闭连接,无应答"),
            }
        })
        .await
        .context("fdpass 客户端任务失败")?
    }

    /// 读一帧服务端主动推送(v2-P3 T6:`0x03 dest_commit` 等不经本侧请求
    /// 先行到达的帧)。与 [`send_msg`] 的读面同源(阻塞线程上同步
    /// recvmsg);服务端关闭 / 协议异常 → Err。独占 `&mut`——与 send_msg
    /// 互斥,并发两读会互相偷帧;调用方对「应答与推送交错到达」的排序
    /// 自行容错(send_msg 的应答读也可能先收到推送帧)。
    pub async fn recv_msg(&mut self) -> Result<FdMsg> {
        let stream = self.stream.try_clone().context("fdpass client try_clone")?;
        tokio::task::spawn_blocking(move || match read_frame(&stream).context("fdpass 接收失败")? {
            Some(msg) => Ok(msg),
            None => anyhow::bail!("fdpass 服务端关闭连接"),
        })
        .await
        .context("fdpass 客户端任务失败")?
    }
}
