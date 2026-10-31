import Foundation
import MTDSSKit

/// SwiftPM 单测伪 daemon:实现 SocketEndpoint,每次 connect 配一对内存管道,
/// 客户端侧交还 IpcClient;服务端侧独立线程逐行读入并交 onLine 脚本处理
/// (对齐 android 测试 FakeDaemon 手法)。
///
/// - onLine:每条请求行(原始字节,含尾换行)到达时回调;nil = 静默吞掉;
///   抛错语义不可用(闭包非 throwing)——需要断连时直接 ServerSide.close()。
/// - connectFailure:非 nil 时所有连接尝试立即抛该错误(模拟 daemon 未就绪)。
/// - ServerSide.received:逐连接累积已收请求行,字节级断言用。
final class FakeDaemon: SocketEndpoint, @unchecked Sendable {

    typealias LineHandler = @Sendable (ServerSide, Data) -> Void

    private let lock = NSLock()
    private var onLineStorage: LineHandler?
    private var connectFailureStorage: Error?
    private var conns: [ServerSide] = []

    /// 每条请求行到达时回调(服务端线程上执行)。
    var onLine: LineHandler? {
        get { lock.withLock { onLineStorage } }
        set { lock.withLock { onLineStorage = newValue } }
    }

    /// connect() 注入失败(非 nil 即抛,模拟 daemon 未就绪/不可达)。
    var connectFailure: Error? {
        get { lock.withLock { connectFailureStorage } }
        set { lock.withLock { connectFailureStorage = newValue } }
    }

    /// 已建立的连接(服务端侧句柄,按建立顺序)。
    var connections: [ServerSide] { lock.withLock { conns } }

    func connect() throws -> any SocketConnection {
        let toClient = MemoryPipe() // server → client
        let toServer = MemoryPipe() // client → server
        let side: ServerSide
        lock.lock()
        if let f = connectFailureStorage {
            lock.unlock()
            throw f
        }
        side = ServerSide(inbox: toServer, outbox: toClient, index: conns.count)
        conns.append(side)
        lock.unlock()
        Thread.detachNewThread { side.readLoop(daemon: self) }
        return ClientConnection(inbox: toClient, outbox: toServer)
    }

    fileprivate func dispatch(line: Data, to side: ServerSide) {
        onLine?(side, line)
    }

    /// 服务端侧连接句柄:已收请求行累积 + 向客户端写行/主动断连。
    final class ServerSide: @unchecked Sendable {
        /// 连接序号(第几条连接,自 0 起)。
        let index: Int
        private let inbox: MemoryPipe  // 客户端请求进此管道
        private let outbox: MemoryPipe // 应答/事件由此管道出
        private let lock = NSLock()
        private var receivedStorage: [Data] = []

        fileprivate init(inbox: MemoryPipe, outbox: MemoryPipe, index: Int) {
            self.inbox = inbox
            self.outbox = outbox
            self.index = index
        }

        /// 已收请求行(原始字节,含尾换行)。
        var received: [Data] { lock.withLock { receivedStorage } }

        /// 向客户端写一行(字符串形式,自动补尾换行)。
        func sendLine(_ line: String) { sendLine(Data(line.utf8)) }

        /// 向客户端写一行(字节形式;无尾换行则补)。
        func sendLine(_ bytes: Data) {
            var line = bytes
            if line.last != UInt8(ascii: "\n") {
                line.append(UInt8(ascii: "\n"))
            }
            try? outbox.write(line)
        }

        /// 服务端主动断连:客户端读侧随后见 EOF。
        func close() {
            inbox.close()
            outbox.close()
        }

        fileprivate func readLoop(daemon: FakeDaemon) {
            var pending = [UInt8]()
            while true {
                guard let nl = pending.firstIndex(of: 0x0A) else {
                    let chunk = inbox.read()
                    if chunk.isEmpty { break } // EOF(残余无换行同丢弃)
                    pending.append(contentsOf: Array(chunk))
                    continue
                }
                let line = Data(pending[0...nl]) // 含尾换行——线上帧原文
                pending.removeFirst(nl + 1)
                lock.withLock { receivedStorage.append(line) }
                daemon.dispatch(line: line, to: self)
            }
            close()
        }
    }

    /// 客户端侧连接(交还 IpcClient 的双工流)。
    private final class ClientConnection: SocketConnection, @unchecked Sendable {
        private let inbox: MemoryPipe  // server → client(读)
        private let outbox: MemoryPipe // client → server(写)

        fileprivate init(inbox: MemoryPipe, outbox: MemoryPipe) {
            self.inbox = inbox
            self.outbox = outbox
        }

        func read() throws -> Data { inbox.read() }

        func write(_ data: Data) throws { try outbox.write(data) }

        func close() {
            inbox.close()
            outbox.close()
        }
    }
}

/// 内存双工管道(NSCondition 阻塞语义;JVM PipedInput/OutputStream 同象):
/// write 即时投递;read 阻塞至有数据,EOF(已关且排空)返回空 Data。
/// 纯用户态实现——无 OS 管道,无 SIGPIPE 顾虑,EOF 语义确定。
final class MemoryPipe: @unchecked Sendable {
    private let cond = NSCondition()
    private var buffer = Data()
    private var closed = false

    func write(_ data: Data) throws {
        cond.lock()
        defer { cond.unlock() }
        if closed {
            throw IpcConnectError("ipc 管道已关闭")
        }
        buffer.append(data)
        cond.signal()
    }

    /// 阻塞读一批已到达字节(全部);EOF → 空 Data。
    func read() -> Data {
        cond.lock()
        defer { cond.unlock() }
        while buffer.isEmpty && !closed {
            cond.wait()
        }
        if !buffer.isEmpty {
            let data = buffer
            buffer = Data()
            return data
        }
        return Data()
    }

    func close() {
        cond.lock()
        closed = true
        cond.broadcast()
        cond.unlock()
    }
}

/// 事件收集器:订阅事件流逐个收集(线程安全),供轮询断言。事件流带缓冲,
/// 订阅先于连接建立即可,无丢帧竞态(镜像 Kotlin 订阅落位闸的语义保障)。
final class EventCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [IpcEvent] = []
    private let task: Task<Void, Never>

    init(_ source: AsyncStream<IpcEvent>) {
        task = Task { [weak self] in
            for await event in source {
                guard let self else { return }
                self.lock.lock()
                self.events.append(event)
                self.lock.unlock()
            }
        }
    }

    /// 轮询直至收集到 n 条(默认 5s 上界,届满返回已收集部分)。
    func waitCount(_ n: Int, timeout: TimeInterval = 5) async -> [IpcEvent] {
        let deadline = Date().addingTimeInterval(timeout)
        while true {
            lock.lock()
            let current = events
            lock.unlock()
            if current.count >= n || Date() > deadline {
                return current
            }
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    deinit { task.cancel() }
}
