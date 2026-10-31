/// fdpass 查询/发布应答通道(Swift 客户端,与 crates/ipc/src/fdpass.rs 及
/// Android Kotlin 客户端共享同一线格式,两侧实现必须逐字节一致):
///
/// ```text
/// kind u8 | payload_len u32(大端) | payload(UTF-8 JSON,≤ 64 KiB)
/// ```
///
/// iOS 应答面(P4a §6.5 查询通道,统一 id 计数器由 daemon 侧持有;fd 不进
/// 线格式,0x04/0x05 均无随帧 fd):
/// - `0x04 dest_query`:`{request_id, exists, size}`(App 查收件箱真实行;
///   F7 失败方向铁律由调用方 InboxPublisher 收口——超时/异常引擎侧按不在场)
/// - `0x05 publish_result`:`{error?, ok, request_id}`(字母序;App move
///   暂存件进收件箱后回话)
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
    public static let maxPayloadBytes = 64 * 1024

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

/// 生产通道:`{home}/fds.sock` 上的按应答一连接客户端。
public struct FdPassChannel: FdPassAnswering {

    public let socketPath: String
    /// ack 读窗(应答受理回执;3s 覆盖 0x04 的全窗,读不到不影响事实)。
    let ackTimeout: TimeInterval

    public init(socketPath: String, ackTimeout: TimeInterval = 3) {
        self.socketPath = socketPath
        self.ackTimeout = ackTimeout
    }

    /// JSON 便利面:sortedKeys 序列化(0x05 的 error?/ok/request_id 字母序
    /// 与 Kotlin golden 同口径)。
    public func send(kind: UInt8, json: [String: Any]) async throws -> FdPassFrame {
        let payload = try JSONSerialization.data(withJSONObject: json, options: [.sortedKeys])
        return try await send(kind: kind, payload: payload)
    }

    public func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
        let frame = try FdPassFrame.checked(kind: kind, payload: payload)
        let path = socketPath
        let timeout = ackTimeout
        return try await Task.detached(priority: .userInitiated) {
            try blockingSend(path: path, frame: frame, ackTimeout: timeout)
        }.value
    }

    /// 阻塞收发(脱离协作线程池;connect 失败快速失败,与 Rust 客户端同口径)。
    static func blockingSend(path: String, frame: Data, ackTimeout: TimeInterval) throws -> FdPassFrame {
        let fd = Darwin.socket(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0)
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
        var tv = timeval(tv_sec: Int(timeout), tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        let sent = frame.withUnsafeBytes { raw in
            write(fd, raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
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
}
