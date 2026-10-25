//! fdpass 集成测试:SCM_RIGHTS 收发 round-trip(测试为 cfg(unix),fd 传递
//! 依赖 Unix 域套接字)。覆盖:fd 随帧到达 handler 且可写、无 fd 帧正常
//! 通行、应答反向携带 fd、超限 payload 断连(客户端本地拒绝 + 服务端断连
//! 双臂)、一帧多 fd / 帧内二次 fd / 单帧 9 fd 触发 MSG_CTRUNC 的防御断连
//! (附收到的 fd 全部关闭的泄漏可观测断言)、非法 JSON 断连,以及 kind
//! 常量钉值。
//! 防御路径用原始 socket 直打:测试侧独立再实现线格式,不与实现共享拼帧
//! 代码(防镜像盲区);阻塞读一律挪进 spawn_blocking,防饿死
//! current_thread 测试运行时上的 serve 任务。

#![cfg(unix)]

use mtdss_ipc::fdpass::{
    serve_fdpass, FdMsg, FdMsgHandler, FdPassClient, KIND_ACK, KIND_DEST_COMMIT, KIND_DEST_FD,
    KIND_SOURCE_FD, MAX_PAYLOAD_BYTES,
};
use serde_json::json;
use std::future::Future;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// handler 写入被传递 fd 的问候字节(客户端从管道另一端读回应断言的内容)。
const GREETING: &[u8] = b"hello from handler";
/// 防御断言读超时:超时即「服务端未断连/未关 fd」,以测试失败显形而非悬挂。
const ASSERT_TIMEOUT: Duration = Duration::from_secs(5);

/// handler 写入被传递 fd 的问候字节,再回 0x81 ack(回显 request_id)。
struct GreetHandler;

impl FdMsgHandler for GreetHandler {
    fn on_msg(&self, msg: FdMsg, reply: mtdss_ipc::fdpass::FdReply) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            assert_eq!(msg.kind, KIND_SOURCE_FD, "服务端应收到 0x01");
            let fd = msg.fd.expect("0x01 帧必须携带 fd");
            let mut f = std::fs::File::from(fd);
            f.write_all(GREETING).expect("向收到的 fd 写入问候");
            reply
                .send(FdMsg {
                    kind: KIND_ACK,
                    payload: json!({"request_id": msg.payload["request_id"], "ok": true}),
                    fd: None,
                })
                .expect("回 ack 不应失败");
        })
    }
}

/// 无 fd 帧通行验证:断言帧无 fd,回 0x81 ack,并计数。
struct NoFdHandler(Arc<AtomicUsize>);

impl FdMsgHandler for NoFdHandler {
    fn on_msg(&self, msg: FdMsg, reply: mtdss_ipc::fdpass::FdReply) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        // 返回的 future 须 'static(与 Dispatcher 同款):先克隆状态再进 async move
        let seen = self.0.clone();
        Box::pin(async move {
            assert!(msg.fd.is_none(), "无 fd 帧到达 handler 时 fd 应为 None");
            seen.fetch_add(1, Ordering::SeqCst);
            reply
                .send(FdMsg {
                    kind: KIND_ACK,
                    payload: json!({"request_id": msg.payload["request_id"], "ok": true}),
                    fd: None,
                })
                .expect("回 ack 不应失败");
        })
    }
}

/// 防御用例用 handler:纯计数,正常应答 ack;违规帧(超限/多 fd/非法
/// JSON)不应触达本 handler。
struct CountingHandler(Arc<AtomicUsize>);

impl FdMsgHandler for CountingHandler {
    fn on_msg(&self, msg: FdMsg, reply: mtdss_ipc::fdpass::FdReply) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        // 返回的 future 须 'static(与 Dispatcher 同款):先克隆状态再进 async move
        let seen = self.0.clone();
        Box::pin(async move {
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = reply.send(FdMsg {
                kind: KIND_ACK,
                payload: json!({"ok": true}),
                fd: None,
            });
            let _ = msg; // msg.fd(若有)随之 Drop 关闭
        })
    }
}

/// 应答反向携带 fd:自建管道,先向写端写问候,再把读端作为应答 fd 回流
/// (验证 daemon→客户端方向的 SCM_RIGHTS)。
struct FdBackHandler;

impl FdMsgHandler for FdBackHandler {
    fn on_msg(&self, msg: FdMsg, reply: mtdss_ipc::fdpass::FdReply) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let (pipe_rx, mut pipe_tx) = UnixStream::pair().expect("建管道对");
            pipe_tx.write_all(GREETING).expect("向写端写问候");
            drop(pipe_tx); // 数据已在管道缓冲,关闭写端
            reply
                .send(FdMsg {
                    kind: KIND_ACK,
                    payload: json!({"request_id": msg.payload["request_id"], "ok": true}),
                    fd: Some(OwnedFd::from(pipe_rx)),
                })
                .expect("回带 fd 的 ack 不应失败");
        })
    }
}

/// 起 serve_fdpass 实例(返回后 fds.sock 未必已建,连接侧用 connect_retry 兜底)。
fn spawn_server(home: &Path, handler: Arc<dyn FdMsgHandler>) {
    let home = home.to_path_buf();
    tokio::spawn(async move {
        if let Err(err) = serve_fdpass(&home, handler).await {
            eprintln!("serve_fdpass failed: {err:#}");
        }
    });
}

/// 重试连接直到服务端就绪(上限 2s):bind 与测试首次连接间存在调度竞态。
async fn connect_retry(home: &Path) -> FdPassClient {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match FdPassClient::connect(home).await {
            Ok(c) => return c,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => panic!("连接 fds.sock 失败: {e:#}"),
        }
    }
}

/// 在阻塞线程上执行闭包并等待结果。std 阻塞读若直接跑在 async 任务里,
/// 会饿死 current_thread 测试运行时上的 serve 任务(accept 无从推进),
/// 防御断连用例全部假死;spawn_blocking 让出执行权,serve 与断言读并行。
async fn on_blocking<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.expect("阻塞断言任务不应 panic")
}

/// 帧头:`kind u8 | len u32 BE`。测试侧独立再实现线格式(不与实现共享
/// 拼帧代码),防「实现与测试共享同一段代码」的镜像盲区。
fn frame_header(kind: u8, len: usize) -> [u8; 5] {
    let len = len as u32;
    [kind, (len >> 24) as u8, (len >> 16) as u8, (len >> 8) as u8, len as u8]
}

/// 原始 sendmsg(在 on_blocking 内调用):不受 FdPassClient 接口与客户端
/// 本地上限约束,可携 0..N 个 fd——正是用来打服务端防御路径(超限/多
/// fd/非法 JSON)。测试帧均极小,sendmsg 不应部分写,发生即报错。
fn raw_sendmsg(sock: &UnixStream, bufs: &[&[u8]], fds: &[RawFd]) -> io::Result<()> {
    let mut iov: Vec<libc::iovec> = bufs
        .iter()
        .map(|b| libc::iovec {
            iov_base: b.as_ptr() as *mut libc::c_void,
            iov_len: b.len(),
        })
        .collect();
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = iov.len() as _;
    // usize 背书对齐:cmsghdr 解引用的对齐前提(与实现侧同理由)
    let mut control: Vec<usize> = Vec::new();
    if !fds.is_empty() {
        let space = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as libc::c_uint) } as usize;
        control.resize(space.div_ceil(std::mem::size_of::<usize>()), 0);
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        // SAFETY:控制缓冲非空且 usize 对齐,cmsg 指向其头部,空间按
        // CMSG_SPACE 备足
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        if cmsg.is_null() {
            return Err(io::Error::other("测试侧 CMSG_FIRSTHDR 返回空"));
        }
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as libc::c_uint) as _;
            let data = libc::CMSG_DATA(cmsg).cast::<libc::c_int>();
            for (i, fd) in fds.iter().enumerate() {
                *data.add(i) = *fd;
            }
        }
    }
    let total: usize = bufs.iter().map(|b| b.len()).sum();
    // SAFETY:iov/control/msg 均为本函数局部,存活覆盖 sendmsg 调用全程
    let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != total {
        return Err(io::Error::other(format!(
            "sendmsg 部分写 {n}/{total}:测试帧极小,不应发生"
        )));
    }
    Ok(())
}

/// 阻塞读原始 socket 至对端断开:EOF(0 字节)或 ConnectionReset(对端关
/// 闭时接收缓冲尚有未读数据,Linux 发 RST)皆算已断连;读超时(对端未
/// 断连)以错误抛出,由调用侧 expect 成测试失败——防御行为缺失的信号。
fn read_eof_or_reset(sock: &mut UnixStream) -> io::Result<()> {
    sock.set_read_timeout(Some(ASSERT_TIMEOUT))?;
    let mut buf = [0u8; 1];
    match sock.read(&mut buf) {
        Ok(0) => Ok(()),
        Ok(_) => Err(io::Error::other("预期断连,却读到数据")),
        Err(e) if e.kind() == ErrorKind::ConnectionReset => Ok(()),
        Err(e) => Err(e),
    }
}

/// 阻塞读探测管道至 EOF:对端连同其全部副本均已关闭。若服务端泄漏任一
/// 副本,读超时以错误抛出——fd 泄漏的可观测判据。
fn read_eof(sock: &mut UnixStream) -> io::Result<()> {
    sock.set_read_timeout(Some(ASSERT_TIMEOUT))?;
    let mut buf = [0u8; 1];
    match sock.read(&mut buf) {
        Ok(0) => Ok(()),
        Ok(_) => Err(io::Error::other("预期 EOF,却读到数据")),
        Err(e) => Err(e),
    }
}

/// 主链路:客户端发 0x01(source_fd)携管道写端,handler 向收到的 fd 写问候,
/// 客户端从管道读端读回断言内容;应答为 0x81 ack 且 request_id 回显。
#[tokio::test]
async fn fd_passes_through_and_handler_writes_to_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    spawn_server(dir.path(), Arc::new(GreetHandler));
    let mut client = connect_retry(dir.path()).await;

    let (mut pipe_rx, pipe_tx) = UnixStream::pair().expect("建管道对");
    let reply = client
        .send_msg(KIND_SOURCE_FD, json!({"request_id": 42}), Some(OwnedFd::from(pipe_tx)))
        .await
        .expect("send_msg 不应失败");
    assert_eq!(reply.kind, KIND_ACK, "应答 kind 应为 0x81");
    assert_eq!(reply.payload["request_id"], 42, "request_id 应回显");
    assert_eq!(reply.payload["ok"], true);
    assert!(reply.fd.is_none(), "ack 应答不携带 fd");

    let mut buf = vec![0u8; GREETING.len()];
    pipe_rx.read_exact(&mut buf).expect("从管道读端读问候");
    assert_eq!(buf, GREETING, "handler 写入 fd 的字节应原样读回");
}

/// 无 fd 消息:transport 层不校验 kind-fd 对应,0 fd 帧正常到达 handler 并收到应答。
#[tokio::test]
async fn message_without_fd_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let seen = Arc::new(AtomicUsize::new(0));
    spawn_server(dir.path(), Arc::new(NoFdHandler(seen.clone())));
    let mut client = connect_retry(dir.path()).await;

    let reply = client
        .send_msg(KIND_SOURCE_FD, json!({"request_id": 7}), None)
        .await
        .expect("无 fd send_msg 不应失败");
    assert_eq!(reply.kind, KIND_ACK);
    assert_eq!(reply.payload["request_id"], 7);
    assert_eq!(seen.load(Ordering::SeqCst), 1, "handler 应恰好被调用一次");
}

/// 超限 payload(len > 64 KiB)双臂:
/// (a) FdPassClient 本地拒绝:payload 序列化后超限即拒发,不毒害连接,
///     后续正常帧照常 round-trip;
/// (b) 原始 socket 绕过客户端防护直发超限帧头:服务端读到 len 即断连,
///     handler 不被触达。
#[tokio::test]
async fn oversized_payload_disconnects_without_reaching_handler() {
    let dir = tempfile::tempdir().expect("tempdir");
    let seen = Arc::new(AtomicUsize::new(0));
    spawn_server(dir.path(), Arc::new(CountingHandler(seen.clone())));
    let mut client = connect_retry(dir.path()).await;

    // (a) 客户端本地拒绝:超限 payload 不上线,连接保持可用
    let pad = "x".repeat(MAX_PAYLOAD_BYTES + 1024);
    let rejected = client
        .send_msg(KIND_SOURCE_FD, json!({"pad": pad}), None)
        .await;
    assert!(rejected.is_err(), "超限 payload 应在客户端本地被拒绝");
    let reply = client
        .send_msg(KIND_SOURCE_FD, json!({"request_id": 1}), None)
        .await
        .expect("本地拒绝后连接应仍可用");
    assert_eq!(reply.kind, KIND_ACK);
    assert_eq!(seen.load(Ordering::SeqCst), 1, "仅第二帧应触达 handler");

    // (b) 原始 socket 直发超限帧头(只发帧头:服务端见 len 超限即应断连)
    drop(client);
    let mut raw = UnixStream::connect(dir.path().join("fds.sock")).expect("raw 连接 fds.sock");
    let header = frame_header(KIND_SOURCE_FD, MAX_PAYLOAD_BYTES + 1024);
    on_blocking(move || {
        raw_sendmsg(&raw, &[&header], &[])?;
        read_eof_or_reset(&mut raw)
    })
    .await
    .expect("超限帧头应致服务端断连");
    assert_eq!(seen.load(Ordering::SeqCst), 1, "超限帧不应触达 handler");
}

/// 反向 fd:应答携带 daemon 建的管道读端,客户端从收到的 fd 读出问候字节。
#[tokio::test]
async fn reply_can_carry_fd_back_to_client() {
    let dir = tempfile::tempdir().expect("tempdir");
    spawn_server(dir.path(), Arc::new(FdBackHandler));
    let mut client = connect_retry(dir.path()).await;

    let reply = client
        .send_msg(KIND_SOURCE_FD, json!({"request_id": 9}), None)
        .await
        .expect("send_msg 不应失败");
    assert_eq!(reply.kind, KIND_ACK);
    let fd = reply.fd.expect("应答应携带 fd");
    let mut f = std::fs::File::from(fd);
    let mut buf = vec![0u8; GREETING.len()];
    f.read_exact(&mut buf).expect("从应答 fd 读问候");
    assert_eq!(buf, GREETING, "反向 SCM_RIGHTS 传递的字节应原样读回");
}

/// 一帧多 fd(协议允许 0/1):单次 sendmsg 携 2 个 fd 的合法帧,服务端应
/// 防御性断连、收到的 fd 全部关闭(探测管道读端 EOF 可观测——泄漏任一
/// 副本即悬挂超时)、handler 不被触达;监听器存活,新连接照常 round-trip。
#[tokio::test]
async fn two_fds_in_one_frame_disconnects_and_closes_extras() {
    let dir = tempfile::tempdir().expect("tempdir");
    let seen = Arc::new(AtomicUsize::new(0));
    spawn_server(dir.path(), Arc::new(CountingHandler(seen.clone())));
    let _gate = connect_retry(dir.path()).await; // 确保服务端已监听

    let mut raw = UnixStream::connect(dir.path().join("fds.sock")).expect("raw 连接 fds.sock");
    let (mut probe1, fd1_peer) = UnixStream::pair().expect("建探测管道 1");
    let (mut probe2, fd2_peer) = UnixStream::pair().expect("建探测管道 2");
    let payload = br#"{"request_id": 5}"#;
    let header = frame_header(KIND_SOURCE_FD, payload.len());
    let fds = [fd1_peer.as_raw_fd(), fd2_peer.as_raw_fd()];
    on_blocking(move || {
        raw_sendmsg(&raw, &[&header, payload], &fds)?;
        drop(fd1_peer); // 发送侧副本撤除:服务端若关闭其副本,探测端即 EOF
        drop(fd2_peer);
        read_eof_or_reset(&mut raw)?;
        read_eof(&mut probe1)?;
        read_eof(&mut probe2)
    })
    .await
    .expect("双 fd 帧应被断连且多余 fd 被关闭");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "违规帧不应触达 handler");

    // 坏连接不拖垮监听器:新连接照常服务
    let mut client = FdPassClient::connect(dir.path()).await.expect("重连 fds.sock");
    let reply = client
        .send_msg(KIND_SOURCE_FD, json!({"request_id": 6}), None)
        .await
        .expect("断连后新连接应照常服务");
    assert_eq!(reply.kind, KIND_ACK);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

/// 帧内二次 fd(帧头携 1 个 + payload 再携 1 个,帧内累计 2):同样按协议
/// 违规断连,两个 fd 都关闭。覆盖 recv 侧「帧内 fd 累计」判定路径(与
/// 单 sendmsg 双 fd 的 cmsg 解析路径互补)。
#[tokio::test]
async fn second_fd_within_same_frame_disconnects_and_closes_both() {
    let dir = tempfile::tempdir().expect("tempdir");
    let seen = Arc::new(AtomicUsize::new(0));
    spawn_server(dir.path(), Arc::new(CountingHandler(seen.clone())));
    let _gate = connect_retry(dir.path()).await; // 确保服务端已监听

    let mut raw = UnixStream::connect(dir.path().join("fds.sock")).expect("raw 连接 fds.sock");
    let (mut probe1, fd1_peer) = UnixStream::pair().expect("建探测管道 1");
    let (mut probe2, fd2_peer) = UnixStream::pair().expect("建探测管道 2");
    let payload = br#"{"request_id": 5}"#;
    let header = frame_header(KIND_SOURCE_FD, payload.len());
    let fds = [fd1_peer.as_raw_fd(), fd2_peer.as_raw_fd()];
    on_blocking(move || {
        raw_sendmsg(&raw, &[&header], &[fds[0]])?;
        drop(fd1_peer);
        raw_sendmsg(&raw, &[payload], &[fds[1]])?;
        drop(fd2_peer);
        read_eof_or_reset(&mut raw)?;
        read_eof(&mut probe1)?;
        read_eof(&mut probe2)
    })
    .await
    .expect("帧内二次 fd 应被断连且两 fd 均关闭");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "违规帧不应触达 handler");
}

/// 一次 sendmsg 塞 9 个 fd(超 recv 侧控制缓冲的 8 个槽位):内核置
/// MSG_CTRUNC(装不下的由内核直接关闭),已装入的 8 个随「帧随附 fd 数
/// 超 1」防御性全关并断连;handler 不被触达,9 条探测管道全部 EOF(泄漏
/// 任一副本即读超时)。钉死「控制缓冲按槽位预置、CTRUNC 视为协议异常」
/// 的内核级行为——压缩槽位或漏判 CTRUNC 都会在此现形。
#[tokio::test]
async fn nine_fds_in_one_frame_hits_ctrunc_and_disconnects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let seen = Arc::new(AtomicUsize::new(0));
    spawn_server(dir.path(), Arc::new(CountingHandler(seen.clone())));
    let _gate = connect_retry(dir.path()).await; // 确保服务端已监听

    let mut raw = UnixStream::connect(dir.path().join("fds.sock")).expect("raw 连接 fds.sock");
    // 9 条探测管道:一端送进 SCM_RIGHTS,另一端留作 EOF 探测
    let mut probes = Vec::new();
    let mut peers = Vec::new();
    for _ in 0..9 {
        let (probe, peer) = UnixStream::pair().expect("建探测管道");
        probes.push(probe);
        peers.push(peer);
    }
    let payload = br#"{"request_id": 5}"#;
    let header = frame_header(KIND_SOURCE_FD, payload.len());
    let fds: Vec<RawFd> = peers.iter().map(|p| p.as_raw_fd()).collect();
    on_blocking(move || -> std::io::Result<()> {
        raw_sendmsg(&raw, &[&header, payload], &fds)?;
        drop(peers); // 发送侧全部撤除:服务端与内核若各归各位,探测端即 EOF
        read_eof_or_reset(&mut raw)?;
        for p in probes.iter_mut() {
            read_eof(p)?;
        }
        Ok(())
    })
    .await
    .expect("9-fd 帧应触发 CTRUNC 断连且收到的 fd 全部关闭");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "违规帧不应触达 handler");
}

/// payload 非法 JSON:协议异常,断连,handler 不被触达。
#[tokio::test]
async fn invalid_json_payload_disconnects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let seen = Arc::new(AtomicUsize::new(0));
    spawn_server(dir.path(), Arc::new(CountingHandler(seen.clone())));
    let _gate = connect_retry(dir.path()).await; // 确保服务端已监听

    let mut raw = UnixStream::connect(dir.path().join("fds.sock")).expect("raw 连接 fds.sock");
    let payload = b"{not json";
    let header = frame_header(KIND_SOURCE_FD, payload.len());
    on_blocking(move || {
        raw_sendmsg(&raw, &[&header, payload], &[])?;
        read_eof_or_reset(&mut raw)
    })
    .await
    .expect("非法 JSON 帧应被断连");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "非法 JSON 不应触达 handler");
}

/// kind 常量钉值:与 Kotlin FdPassChannel.kt 逐字节共享的协议契约,改值
/// 即破坏跨端兼容;单帧 payload 上限同钉(与 ipc 主通道 8 MiB 帧上限独立)。
#[test]
fn kind_constants_pin_the_kotlin_shared_contract() {
    assert_eq!(KIND_SOURCE_FD, 0x01);
    assert_eq!(KIND_DEST_FD, 0x02);
    assert_eq!(KIND_DEST_COMMIT, 0x03);
    assert_eq!(KIND_ACK, 0x81);
    assert_eq!(MAX_PAYLOAD_BYTES, 64 * 1024);
}
