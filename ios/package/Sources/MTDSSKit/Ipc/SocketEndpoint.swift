/// 传输层抽象:一条到 daemon 的连接工厂(测试注入内存管道,实机 POSIX Unix
/// domain socket;镜像 android/.../ipc/SocketFactory.kt)。IpcClient 的全部
/// 连接语义只依赖本抽象,不感知传输介质。

import Foundation

public protocol SocketEndpoint: Sendable {
    /// 建立一条到 daemon 的连接;失败抛错。可为阻塞调用。
    func connect() throws -> any SocketConnection
}

/// 一条双工连接:读侧由 IpcClient 读线程独占;写侧由写闸串行整帧写出。
public protocol SocketConnection: AnyObject, Sendable {
    /// 阻塞读一批已到达字节(至少 1 字节);空 Data = 对端关闭(EOF)。
    func read() throws -> Data
    /// 尽力整段写出(内部循环补写;对端关闭抛错)。
    func write(_ data: Data) throws
    /// 关闭(幂等;阻塞中的 read 随后返回 EOF/抛错)。
    func close()
}

/// 实机端点:POSIX Unix domain socket。路径口径与 mtdss_ipc::socket_path 的
/// unix 分支一致:`{home}/daemon.sock`(macOS 宿主测试与 iOS 同码)。
public struct UnixSocketEndpoint: SocketEndpoint {
    public let path: String

    public init(path: String) {
        self.path = path
    }

    /// 由 home 派生 socket 路径(与 crates/ipc socket_path unix 分支同口径)。
    public static func socketPath(home: String) -> String {
        (home as NSString).appendingPathComponent("daemon.sock")
    }

    public func connect() throws -> any SocketConnection {
        try PosixSocketConnection.connect(path: path)
    }
}

/// POSIX AF_UNIX SOCK_STREAM 连接;Darwin 下置 SO_NOSIGPIPE(写对端关闭返回
/// EPIPE 而非杀进程),close 先 shutdown 以解除他线程阻塞读。
final class PosixSocketConnection: SocketConnection, @unchecked Sendable {
    private let fd: Int32
    private let lock = NSLock()
    private var closed = false

    private init(fd: Int32) {
        self.fd = fd
    }

    static func connect(path: String) throws -> PosixSocketConnection {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw IpcConnectError("ipc socket 创建失败 errno=\(errno)")
        }
        let conn = PosixSocketConnection(fd: fd)
        do {
            var addr = sockaddr_un()
            addr.sun_family = sa_family_t(AF_UNIX)
            let pathBytes = Array(path.utf8)
            guard pathBytes.count < MemoryLayout.size(ofValue: addr.sun_path) else {
                throw IpcConnectError("ipc socket 路径过长: \(path)")
            }
            withUnsafeMutableBytes(of: &addr.sun_path) { dst in
                _ = pathBytes.withUnsafeBufferPointer { src in
                    memcpy(dst.baseAddress, src.baseAddress, pathBytes.count) // 尾部零已由零初始化保证
                }
            }
            let result = withUnsafePointer(to: &addr) { p in
                p.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                    Darwin.connect(fd, sa, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else {
                throw IpcConnectError("ipc connect 失败 errno=\(errno): \(path)")
            }
            #if canImport(Darwin)
            var nosigpipe: Int32 = 1
            _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &nosigpipe, socklen_t(MemoryLayout<Int32>.size))
            #endif
            return conn
        } catch {
            Darwin.close(fd)
            throw error
        }
    }

    func read() throws -> Data {
        var buf = [UInt8](repeating: 0, count: 64 * 1024)
        var got: Int = -1
        buf.withUnsafeMutableBytes { raw in
            repeat {
                got = Darwin.read(fd, raw.baseAddress, raw.count)
            } while got < 0 && errno == EINTR
        }
        if got > 0 {
            return Data(buf.prefix(got))
        }
        if got == 0 {
            return Data() // EOF
        }
        throw IpcConnectError("ipc read 失败 errno=\(errno)")
    }

    func write(_ data: Data) throws {
        let bytes = [UInt8](data)
        try bytes.withUnsafeBufferPointer { bp in
            var sent = 0
            while sent < bytes.count {
                let n = Darwin.write(fd, bp.baseAddress! + sent, bytes.count - sent)
                if n > 0 {
                    sent += n
                    continue
                }
                if n < 0 && errno == EINTR { continue }
                throw IpcConnectError("ipc write 失败 errno=\(errno)")
            }
        }
    }

    func close() {
        lock.lock()
        defer { lock.unlock() }
        guard !closed else { return }
        closed = true
        shutdown(fd, SHUT_RDWR) // 解除他线程的阻塞读
        Darwin.close(fd)
    }

    deinit { close() }
}
