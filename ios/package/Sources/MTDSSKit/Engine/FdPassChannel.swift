/// fdpass 查询/发布应答通道(Swift 客户端,与 crates/ipc/src/fdpass.rs 及
/// Android Kotlin 客户端共享同一线格式,两侧实现必须逐字节一致):
///
/// ```text
/// kind u8 | payload_len u32(大端) | payload(UTF-8 JSON,≤ 64 KiB)
/// ```
///
/// iOS 应答面(P4a §6.5 查询通道,统一 id 计数器由 daemon 侧持有;fd 不进
/// 线格式,0x04/0x05/0x06/0x07 均无随帧 fd;R7 起照片桥两帧同面;V4-1 T10
/// 起 seek 源桥 0x0D 交付形**随帧 1 fd**,SCM_RIGHTS 同传):
/// - `0x04 dest_query`:`{request_id, exists, size}`(App 查收件箱真实行;
///   F7 失败方向铁律由调用方 InboxPublisher 收口——超时/异常引擎侧按不在场)
/// - `0x05 publish_result`:`{error?, ok, request_id}`(字母序;App move
///   暂存件进收件箱后回话)
/// - `0x06 photo_list`:`{entries?, error?, ok, request_id, truncated?}`
///   (字母序;R7 照片列表应答,ok:false = 桥不可用/未授权)
/// - `0x07 photo_export`:`{error?, ok, path?, request_id}`(字母序;R7 照片
///   导出应答,path = App 导出临时件绝对路径,引擎读发后 unlink)
/// - `0x0B thumb_data`:`{error?, ok, path?, request_id}`(V4-1 T9/T11 缩略
///   图应答,**无 fd**,字母序;交付形 `{ok, path, request_id}`——path =
///   App 生成件绝对路径 `{home}/thumb-serve/`,引擎读字节入源端缓存后
///   unlink;拒答形 `{error, ok, request_id}`,引擎按「桥不可用」落既有
///   回退——缩略图缺位只是降级,不回错误帧。**桥号双端钉死**:fdpass
///   kind 是 daemon 单一派发表,Android(T9)/iOS(T11)同用 0x0B)
/// - `0x0D seek_fd`:`{error?, ok?, request_id}`(V4-1 T10 可 seek fd 源应答;
///   交付形 `{request_id}` + 1 fd——SCM_RIGHTS 随帧,可 seek 只读;拒开形
///   `{error, ok, request_id}` 字母序无 fd。**桥号双端钉死**:fdpass kind
///   是 daemon 单一派发表,Android(T8)/iOS(T10)同用 0x0D——「iOS 桥号按
///   现状顺延」口径据此收口,0x0B 缩略图/0x0C 转码已为 T11/T13 预留)
/// - `0x0C rendition_data`:`{error?, index?, ok, path?, request_id}`(V4-1
///   T14 转码控制应答,**无 fd**,字母序——open/cancel 回执 `{ok, request_id}`,
///   段就绪回告 `+{index, path}`,拒答/管线失败 `{error, ok:false}`。Android
///   T13/iOS T14 同号,T13 线形逐字节同面)
///
/// 连接纪律:每次应答一连接(connect → write → read ack → close)。daemon
/// 侧逐连接独立 actor、应答按 request_id 交付等待方,不要求客户端长连——
/// 无状态化免去重连/断线监督面(Android 长连形态的 iOS 等价简化,3s/30s
/// 应答窗内连接寿命足够)。
///
/// 阻塞 socket 调用全部落在脱离协作线程池的 detached 任务上(UDS 本地回环
/// 微秒级;读侧带 SO_RCVTIMEO 兜底,不死等)。

import Foundation

/// 线上 kind 常量(与 crates/ipc/src/fdpass.rs 一致;0x02/0x03 已随收件
/// fd 桥退役,不复用)。
public enum FdPassKind {
    /// source_fd(Kotlin→daemon 上行;iOS 沙箱直通形态不使用)。
    public static let sourceFD: UInt8 = 0x01
    /// dest_query:在场查询应答(App→daemon)。
    public static let destQuery: UInt8 = 0x04
    /// publish_result:发布结果应答(App→daemon)。
    public static let publishResult: UInt8 = 0x05
    /// photo_list:照片列表应答(App→daemon;R7 照片图库虚拟根,0x06)。
    public static let photoList: UInt8 = 0x06
    /// photo_export:照片导出应答(App→daemon;R7,0x07)。
    public static let photoExport: UInt8 = 0x07
    /// thumb_data:缩略图应答(App→daemon;V4-1 T9/T11,0x0B——**恒无 fd**,
    /// JPEG 字节走临时件 `{home}/thumb-serve/`,64 KiB payload 装不下千
    /// 像素图;Android/iOS 同号,见头注桥号钉面)。
    public static let thumbData: UInt8 = 0x0B
    /// seek_fd:可 seek fd 源应答(App→daemon;V4-1 T10,0x0D——Android/iOS
    /// 同号,桥号勘误见头注)。
    public static let seekFD: UInt8 = 0x0D
    /// rendition_data:转码控制应答(App→daemon;V4-1 T13/T14,0x0C——
    /// **恒无 fd**,段字节走临时件 `{home}/rendition-serve/{session}/`,字节
    /// 面与 0x0B 同轨;Android/iOS 同号,见头注桥号钉面)。
    public static let renditionData: UInt8 = 0x0C
    /// ack(daemon→App,受理回执)。
    public static let ack: UInt8 = 0x81
}

/// 一条 fdpass 帧。
public struct FdPassFrame: Equatable, Sendable {
    public let kind: UInt8
    public let payload: Data

    public init(kind: UInt8, payload: Data) {
        self.kind = kind
        self.payload = payload
    }

    /// 单帧 payload JSON 上限(元数据足够;超限按协议异常)。
    /// field r26e 帽提 8 MiB(与引擎侧 MAX_PAYLOAD_BYTES 同步;照片列表 10_000 帽 JSON)。
    public static let maxPayloadBytes = 8 * 1024 * 1024

    /// 编码:`kind u8 | len u32 BE | payload`。超限 payload 拒编。
    public static func encode(kind: UInt8, payload: Data) throws -> Data {
        try checked(kind: kind, payload: payload)
    }

    /// 校验 + 编码(调用方已有的编码路径复用)。
    public static func checked(kind: UInt8, payload: Data) throws -> Data {
        guard payload.count <= maxPayloadBytes else {
            throw FdPassError.oversized(count: payload.count)
        }
        var out = Data(capacity: 5 + payload.count)
        out.append(kind)
        var len = UInt32(payload.count).bigEndian
        withUnsafeBytes(of: &len) { out.append(contentsOf: $0) }
        out.append(payload)
        return out
    }

    /// 解码单帧:头 5 字节(kind + 大端长度)+ 恰长 payload。截断/长度超限
    /// 即抛(协议异常,调用方弃帧)。
    public static func decode(_ data: Data) throws -> FdPassFrame {
        guard data.count >= 5 else {
            throw FdPassError.truncated
        }
        let kind = data[data.startIndex]
        let len = data.subdata(in: (data.startIndex + 1)..<(data.startIndex + 5))
            .withUnsafeBytes { $0.load(as: UInt32.self) }.bigEndian
        guard Int(len) <= maxPayloadBytes else {
            throw FdPassError.oversized(count: Int(len))
        }
        guard data.count >= 5 + Int(len) else {
            throw FdPassError.truncated
        }
        let payload = data.subdata(in: (data.startIndex + 5)..<(data.startIndex + 5 + Int(len)))
        return FdPassFrame(kind: kind, payload: payload)
    }
}

public enum FdPassError: Error, Equatable {
    case oversized(count: Int)
    case truncated
    case connectFailed(String)
    case ioFailed(String)
}

/// fdpass 应答通道抽象(宿主单测注入桩;生产 = FdPassChannel)。
public protocol FdPassAnswering: Sendable {
    /// 发一帧并等 daemon 的 ack 回执(ack 的 ok:false = 等待方已超时/缺席,
    /// 调用方仅留痕——发布事实以引擎侧终态为准)。
    func send(kind: UInt8, payload: Data) async throws -> FdPassFrame
}

/// fd 随帧应答通道(V4-1 T10 seek 源桥;SCM_RIGHTS 交付——0x0D 交付形 =
/// `{request_id}` + 1 个可 seek 只读 fd,拒开形无 fd)。宿主单测注入记录桩;
/// 生产 = FdPassChannel。
public protocol FdPassFdAnswering: FdPassAnswering, Sendable {
    /// 发一帧并等 ack;`fileDescriptor` 非 nil 时经 sendmsg + SCM_RIGHTS
    /// 随帧交付(daemon 侧 recvmsg 收编为引擎自有副本——发送返回后本侧即可
    /// 关闭;调用方负责本侧句柄生命周期)。
    func send(kind: UInt8, payload: Data, fileDescriptor: Int32?) async throws -> FdPassFrame
}

public extension FdPassAnswering {
    /// JSON 便利面:sortedKeys 序列化(0x05 的 error?/ok/request_id 字母序
    /// 与 Kotlin golden 同口径)。
    func send(kind: UInt8, json: [String: Any]) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(withJSONObject: json, options: [.sortedKeys])
        return try await send(kind: kind, payload: payload)
    }

    /// 0x0B thumb_data 应答(V4-1 T9/T11 缩略图桥;**无 fd**——JPEG 字节走
    /// 临时件,64 KiB payload 装不下千像素图,0x07/0x09 同轨):[path] 非
    /// nil = 交付生成件 `{ok, path, request_id}` 字母序(与 Kotlin
    /// exportReplyPayload 同形;path = 生成件绝对路径 `{home}/thumb-serve/`,
    /// 引擎读字节入源端缓存后 unlink,App 挂载清扫兜底);nil = 拒答/不可
    /// 解 `{error, ok, request_id}` 字母序(Kotlin refusePayload 同形,
    /// [error] 词面透传,daemon 落既有回退——缩略图缺位只是降级)。
    ///
    /// **thumb 通道单实例**:daemon 侧 ThumbBridge 单 request_id 计数器,
    /// 与源/查询/照片/服务/seek 通道各立(Kotlin FdPassChannel 六实例纪律
    /// 同面)。
    func sendThumbData(requestId: UInt64, path: String?, error: String? = nil) async throws -> FdPassFrame {
        let json: [String: Any] =
            if let path {
                [
                    "ok": true,
                    "path": path,
                    "request_id": Int(clamping: requestId),
                ]
            } else {
                [
                    "error": error ?? "App 无法生成缩略图",
                    "ok": false,
                    "request_id": Int(clamping: requestId),
                ]
            }
        let payload = try JSONSerialization.data(withJSONObject: json, options: [.sortedKeys])
        return try await send(kind: FdPassKind.thumbData, payload: payload)
    }
    /// 0x0C rendition_data 应答族(V4-1 T13/T14 转码桥;**无 fd**——段字
    /// 节走临时件 `{home}/rendition-serve/{session}/{index}.m4s`,0x0B 同轨):
    /// - 回执(open/cancel 受理;管线已受理/cancel 幂等照成):`{ok,
    ///   request_id}` 字母序(Kotlin FdWire.renditionAckPayload 同形);
    /// - 段就绪回告:`{index, ok, path, request_id}` 字母序——request_id =
    ///   开启时的 request_id(即 App 侧会话键,daemon 按 (request_id, index)
    ///   记账段路径),index 0 = init 段(T12 段序约定),path = 段文件落盘
    ///   绝对路径,引擎按需读已产段(T12「读已产段」语义);
    /// - 拒答/管线失败:`{error, ok:false, request_id}` 字母序(Kotlin
    ///   refusePayload 同形,[error] 词面透传——open 待答窗内 = 拒答落
    ///   unsupported,此后 = 管线失败标记,已产段照读新段即 Err)。
    ///
    /// **rendition 通道单实例**:daemon 侧 RenditionBridge 单 request_id
    /// 计数器,与源/查询/照片/服务/seek/thumb 通道各立(Kotlin FdPassChannel
    /// 七实例纪律同面)。
    func sendRenditionAck(requestId: UInt64) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(
            withJSONObject: ["ok": true, "request_id": Int(clamping: requestId)],
            options: [.sortedKeys])
        return try await send(kind: FdPassKind.renditionData, payload: payload)
    }

    /// 0x0C 段就绪回告(见 `sendRenditionAck` 注;index 0 = init 段)。
    func sendRenditionSegment(requestId: UInt64, index: Int, path: String) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(
            withJSONObject: [
                "index": index,
                "ok": true,
                "path": path,
                "request_id": Int(clamping: requestId),
            ], options: [.sortedKeys])
        return try await send(kind: FdPassKind.renditionData, payload: payload)
    }

    /// 0x0C 拒答/管线失败形(见 `sendRenditionAck` 注)。
    func sendRenditionRefuse(requestId: UInt64, error: String) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(
            withJSONObject: [
                "error": error,
                "ok": false,
                "request_id": Int(clamping: requestId),
            ], options: [.sortedKeys])
        return try await send(kind: FdPassKind.renditionData, payload: payload)
    }

    /// 0x0C 管线自然完毕形(field 修:`{eof, ok, request_id}`——App 转码
    /// EOF 后通知;引擎记会话完毕,此后越界段答段越界,查看端 playlist
    /// 据此封 ENDLIST)。
    func sendRenditionEof(requestId: UInt64) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(
            withJSONObject: [
                "eof": true,
                "ok": true,
                "request_id": Int(clamping: requestId),
            ], options: [.sortedKeys])
        return try await send(kind: FdPassKind.renditionData, payload: payload)
    }
}

public extension FdPassFdAnswering {
    /// 0x0D 交付形:`{request_id}` + 1 fd(字母序;与 Kotlin seekFdPayload
    /// 同形)。
    func sendSeekFd(requestId: UInt64, fileDescriptor: Int32) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(
            withJSONObject: ["request_id": Int(clamping: requestId)], options: [.sortedKeys])
        return try await send(kind: FdPassKind.seekFD, payload: payload, fileDescriptor: fileDescriptor)
    }

    /// 0x0D 拒开形:`{error, ok, request_id}` 字母序,无 fd(与 Kotlin
    /// refusePayload 同形;error 词面透传——iCloud 未卸载项恒
    /// `icloud_not_local`,引擎落既有导出回退)。
    func sendSeekRefuse(requestId: UInt64, error: String) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(
            withJSONObject: [
                "error": error,
                "ok": false,
                "request_id": Int(clamping: requestId),
            ], options: [.sortedKeys])
        return try await send(kind: FdPassKind.seekFD, payload: payload, fileDescriptor: nil)
    }
}

/// 生产通道:`{home}/fds.sock` 上的按应答一连接客户端。
public struct FdPassChannel: FdPassFdAnswering {

    public let socketPath: String
    /// ack 读窗(应答受理回执;3s 覆盖 0x04 的全窗,读不到不影响事实)。
    let ackTimeout: TimeInterval

    public init(socketPath: String, ackTimeout: TimeInterval = 3) {
        self.socketPath = socketPath
        self.ackTimeout = ackTimeout
    }

    public func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
        try await send(kind: kind, payload: payload, fileDescriptor: nil)
    }

    /// fd 随帧应答(0x0D seek 源;fileDescriptor = nil 落 plain write 同路)。
    public func send(kind: UInt8, payload: Data, fileDescriptor: Int32?) async throws -> FdPassFrame {
        let frame = try FdPassFrame.checked(kind: kind, payload: payload)
        let path = socketPath
        let timeout = ackTimeout
        return try await Task.detached(priority: .userInitiated) {
            try FdPassChannel.blockingSend(
                path: path, frame: frame, ackTimeout: timeout, fileDescriptor: fileDescriptor)
        }.value
    }

    /// 阻塞收发(脱离协作线程池;connect 失败快速失败,与 Rust 客户端同口径)。
    /// `fileDescriptor` 非 nil:帧经 sendmsg + SCM_RIGHTS 随帧交付——fd 在
    /// sendmsg 时复制内核引用,发送返回后本侧即可关闭;SCM_RIGHTS 随数据首
    /// 字节出门,**部分写即协议破损**(fd 已出门帧截断,引擎侧 recvmsg 收不
    /// 到整帧按协议异常断连)——UDS 本地回环 64KiB 内整帧一发即达,发生即报
    /// 错(与 Rust write_frame 的摘 cmsg 补写循环不同责,客户端面帧恒小)。
    static func blockingSend(
        path: String, frame: Data, ackTimeout: TimeInterval, fileDescriptor: Int32? = nil,
    ) throws -> FdPassFrame {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw FdPassError.connectFailed("socket: errno \(errno)")
        }
        defer { Darwin.close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < 104 else {
            throw FdPassError.connectFailed("socket path exceeds sun_path: \(bytes.count)")
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { buffer in
            bytes.withUnsafeBufferPointer { src in
                buffer.copyBytes(from: UnsafeRawBufferPointer(src).prefix(buffer.count - 1))
            }
        }
        let connected = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            throw FdPassError.connectFailed("connect \(path): errno \(errno)")
        }

        // 读侧超时兜底(ack 窗;写侧本地回环无窗)。
        var tv = timeval(tv_sec: Int(ackTimeout), tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        let sent: Int
        if let fileDescriptor {
            sent = try blockingSendWithFd(fd: fd, frame: frame, attached: fileDescriptor)
        } else {
            sent = frame.withUnsafeBytes { raw in
                write(fd, raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
            }
        }
        guard sent == frame.count else {
            throw FdPassError.ioFailed("write: \(sent)/\(frame.count)")
        }

        var header = [UInt8](repeating: 0, count: 5)
        var have = 0
        while have < 5 {
            let n = header.withUnsafeMutableBytes { raw in
                read(fd, raw.baseAddress!.advanced(by: have), 5 - have)
            }
            guard n > 0 else {
                throw FdPassError.ioFailed("read header: \(have)/5 (errno \(errno))")
            }
            have += n
        }
        let len = Int(header[1]) << 24 | Int(header[2]) << 16 | Int(header[3]) << 8 | Int(header[4])
        guard len <= FdPassFrame.maxPayloadBytes else {
            throw FdPassError.oversized(count: len)
        }
        var payload = [UInt8](repeating: 0, count: len)
        var got = 0
        while got < len {
            let n = payload.withUnsafeMutableBytes { raw in
                read(fd, raw.baseAddress!.advanced(by: got), len - got)
            }
            guard n > 0 else {
                throw FdPassError.ioFailed("read payload: \(got)/\(len)")
            }
            got += n
        }
        return FdPassFrame(kind: header[0], payload: Data(payload))
    }

    /// sendmsg 单次整帧 + SCM_RIGHTS(fd 随帧交付;Rust write_frame 的控制
    /// 缓冲同构——usize 背书对齐,此处用原生分配 8 字节对齐面)。
    private static func blockingSendWithFd(fd: Int32, frame: Data, attached: Int32) throws -> Int {
        try frame.withUnsafeBytes { raw -> Int in
            guard let base = raw.baseAddress else {
                throw FdPassError.ioFailed("frame buffer empty")
            }
            var iov = iovec(iov_base: UnsafeMutableRawPointer(mutating: base), iov_len: raw.count)
            let space = DarwinCmsg.space(MemoryLayout<Int32>.size)
            let control = UnsafeMutableRawPointer.allocate(
                byteCount: space, alignment: MemoryLayout<Int>.alignment)
            defer { control.deallocate() }
            control.initializeMemory(as: UInt8.self, repeating: 0, count: space)
            // 闭包体非 throwing(Stdlib 面),sendmsg 结果带回外抛。
            var sent = -1
            withUnsafeMutablePointer(to: &iov) { iovp in
                var msg = msghdr()
                msg.msg_iov = iovp
                msg.msg_iovlen = 1
                msg.msg_control = control
                msg.msg_controllen = socklen_t(space)
                let hdr = control.assumingMemoryBound(to: cmsghdr.self)
                // Darwin cmsghdr.cmsg_len = socklen_t(UInt32;现行 SDK)。
                hdr.pointee.cmsg_len = socklen_t(DarwinCmsg.len(MemoryLayout<Int32>.size))
                hdr.pointee.cmsg_level = SOL_SOCKET
                hdr.pointee.cmsg_type = SCM_RIGHTS
                DarwinCmsg.data(of: hdr).storeBytes(of: attached, as: Int32.self)
                sent = withUnsafeMutablePointer(to: &msg) { sendmsg(fd, $0, 0) }
            }
            guard sent >= 0 else {
                throw FdPassError.ioFailed("sendmsg: errno \(errno)")
            }
            guard sent == frame.count else {
                // 部分写:fd 已随首字节出门,帧截断=协议破损(见
                // blockingSend 注)——本面不补写(补写须摘 cmsg,Rust 面
                // 同责),直接失败暴露。
                throw FdPassError.ioFailed("sendmsg partial: \(sent)/\(frame.count)")
            }
            return sent
        }
    }
}

/// Darwin cmsg 宏族(Swift 不导入函数式宏,sys/socket.h CMSG_SPACE/CMSG_LEN/
/// CMSG_DATA 的手工同式;__DARWIN_ALIGN32 = 4 字节对齐,64 位 cmsghdr =
/// 16B,数据区随后——fd 随帧收发双侧同源):
/// - `space(l)` = CMSG_SPACE(l):分配控制缓冲的长度;
/// - `len(l)` = CMSG_LEN(l):单 fd 的实际 cmsg_len;
/// - `data(of:)` = CMSG_DATA:控制头后的数据区指针。
enum DarwinCmsg {
    static func align32(_ x: Int) -> Int { (x + 3) & ~3 }
    static var headerAligned: Int { align32(MemoryLayout<cmsghdr>.size) }
    static func space(_ len: Int) -> Int { headerAligned + align32(len) }
    static func len(_ len: Int) -> Int { headerAligned + len }
    static func data(of hdr: UnsafeMutablePointer<cmsghdr>) -> UnsafeMutableRawPointer {
        UnsafeMutableRawPointer(hdr) + headerAligned
    }
}
