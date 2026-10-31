/// MTDSS daemon 的 JSON-RPC 客户端(NDJSON 线格式,一行一帧;协议事实对照
/// crates/ipc/src/lib.rs):
/// - 请求 `{"jsonrpc":"2.0","id":N,"method":…,"params":…}`(键序 = serde_json
///   字节序,首调 id=1 与 golden 逐字节锚对齐);
/// - 应答 `{"id":N,"result":…}` / `{"id":N,"error":{code,message,data?}}` →
///   按 id 路由到挂起调用;错误 → RpcError(kind 取自 data.kind);
/// - 通知帧(无 id)→ ``events`` 事件流(类型化;未知方法静默跳过);
/// - 单帧上限 8 MiB,超限即断连(镜像 daemon 的 read_capped_line 口径);
/// - 坏帧(无法解析)直接丢弃,连接不断(镜像 Rust 读半工口径)。
///
/// Swift actor(镜像 Kotlin IpcClient 的并发语义:并发调用按 id 路由互不串扰,
/// 整帧写入互斥不交错)。断连后在飞调用立即失败(transport -32000);下次
/// 调用惰性重建连接。断连自愈重连(后台退避 + 补拉)留待 UI 任务按需叠加——
/// 接口不因之变化。
///
/// 生命周期:``close()`` 关停一切并幂等;close 后调用抛 IpcConnectError,
/// 事件流终结。

import Foundation

/// 连接建立失败(传输层,尚未成帧)。
public struct IpcConnectError: Error, Sendable, CustomStringConvertible {
    public let message: String
    public init(_ message: String) {
        self.message = message
    }
    public var description: String { message }
}

/// 单帧超限(镜像 mtdss_ipc read_capped_line:累计超 8 MiB 未遇换行即断连)。
struct FrameTooLargeError: Error {}

public actor IpcClient {

    /// 单帧(单行)字节上限:8 MiB(镜像 daemon MAX_FRAME_BYTES)。
    public static let maxFrameBytes = 8 * 1024 * 1024

    private let endpoint: any SocketEndpoint
    private var connection: (any SocketConnection)?
    /// 等待中的响应表:id → 应答 continuation。
    private var pending: [UInt64: CheckedContinuation<JSONValue, Error>] = [:]
    private var nextId: UInt64 = 1 // 首调 id=1,与 golden 逐字节锚对齐
    private var closed = false
    /// 整帧写入互斥:并发 send 的帧不交错(镜像 Rust 写半工互斥)。
    private let writeGate = NSLock()

    /// 服务端推送事件(类型化)。单消费流;多方订阅用 EventHub 扇出。
    public nonisolated let events: AsyncStream<IpcEvent>
    private let eventContinuation: AsyncStream<IpcEvent>.Continuation

    public init(endpoint: any SocketEndpoint) {
        self.endpoint = endpoint
        (self.events, self.eventContinuation) = AsyncStream.makeStream(
            of: IpcEvent.self,
            bufferingPolicy: .bufferingNewest(256), // 慢订阅丢旧帧,不拖垮读循环
        )
    }

    deinit {
        connection?.close()
        eventContinuation.finish()
    }

    // MARK: 对外面

    /// 建立连接(幂等;已连即复用)。失败抛 IpcConnectError,不重试。
    public func connect() throws {
        if closed { throw IpcConnectError("ipc 客户端已关闭") }
        if connection != nil { return }
        let conn: any SocketConnection
        do {
            conn = try endpoint.connect()
        } catch {
            throw IpcConnectError("ipc 连接失败: \(error)")
        }
        connection = conn
        startReadLoop(conn)
    }

    /// 原始调用:result 值原样返回;错误帧转 RpcError;断连/关闭转传输错误。
    /// 首调自动建连(镜像 Kotlin ensureConnected)。
    public func send(_ method: String, params: JSONValue? = nil) async throws -> JSONValue {
        if closed { throw IpcConnectError("ipc 客户端已关闭") }
        if connection == nil {
            try connect()
        }
        let id = nextId
        nextId &+= 1
        let frame = buildRequestLine(id: id, method: method, params: params)
        print("T3DBG: send \(method) id=\(id)")
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation {
                (cont: CheckedContinuation<JSONValue, Error>) in
                // 注册与写帧同处一个无挂起临界区(续体闭包同步内联执行),
                // 响应先到/断连先至的竞态由此闭合。
                pending[id] = cont
                do {
                    try writeFrame(frame)
                } catch {
                    pending.removeValue(forKey: id)
                    breakCurrentConnection() // 写失败 = 连接已坏:其余在飞一并失败
                    cont.resume(throwing: RpcError.transport("ipc 写入失败: \(error)"))
                    return
                }
            }
        } onCancel: {
            Task { await self.failPendingCall(id: id, error: CancellationError()) }
        }
    }

    /// 关停:断开连接、失败一切在飞调用、终结事件流。幂等。
    public func close() {
        guard !closed else { return }
        closed = true
        if let conn = connection {
            connection = nil
            conn.close()
        }
        failAllPending(RpcError.transport("ipc 客户端已关闭"))
        eventContinuation.finish()
    }

    // MARK: 内部:写帧

    private func writeFrame(_ frame: Data) throws {
        guard let conn = connection else {
            throw RpcError.transport("ipc 连接已断开")
        }
        writeGate.lock()
        defer { writeGate.unlock() }
        try conn.write(frame)
    }

    // MARK: 内部:读循环

    private func startReadLoop(_ conn: any SocketConnection) {
        let reader = CappedLineReader(connection: conn, cap: Self.maxFrameBytes)
        Thread.detachNewThread { [weak self] in
            while true {
                do {
                    guard let line = try reader.readLine() else { break } // EOF
                    guard let self else { return } // 客户端已释放:线程收尾
                    Task { await self.handleLine(line) } // 同步分拣:actor 队列保序
                } catch {
                    // 超限帧/读错误:按协议违约断连(与 daemon 对端口径对称)
                    guard let self else { return }
                    Task { await self.breakConnection(conn) }
                    return
                }
            }
            // 服务端主动断连:读侧 EOF,统一走断连清理
            guard let self else { return }
            Task { await self.breakConnection(conn) }
        }
    }

    /// 读循环分拣:响应帧按 id 路由;通知帧进事件流;坏帧丢弃(镜像 Rust 读半工)。
    private func handleLine(_ line: Data) {
        print("T3DBG: handleLine \(line.count)B")
        let text = String(decoding: line, as: UTF8.self)
        guard let frame = try? JSONParser.parse(text), case .object(let obj) = frame else {
            print("T3DBG: handleLine dropped unparsable")
            return // 无法解析的帧直接丢弃
        }
        if case .number(let raw)? = obj["id"], let id = UInt64(raw) {
            // 响应帧:按 id 路由;无人认领(过期/重复)即忽略
            print("T3DBG: handleLine resp id=\(id) pending=\(pending.keys.sorted())")
            guard let cont = pending.removeValue(forKey: id) else { return }
            switch JSONRpc.parseResponse(frame) {
            case .success(let v): cont.resume(returning: v)
            case .failure(let e): cont.resume(throwing: e)
            }
        } else {
            // 通知帧(无 id):缺 method/params 的畸形事件帧丢弃(镜像 Rust)
            guard case .string(let method)? = obj["method"], let params = obj["params"] else { return }
            if let event = IpcEvent.parse(method: method, params: params) {
                eventContinuation.yield(event)
            }
        }
    }

    // MARK: 内部:断连与收尾

    /// 读循环收尾:仅当断的是当前连接才清理(陈旧连接无事可做)。幂等。
    private func breakConnection(_ conn: any SocketConnection) {
        guard connection === conn, !closed else { return }
        breakCurrentConnection()
    }

    /// 清当前连接、失败一切在飞。
    private func breakCurrentConnection() {
        guard let conn = connection else { return }
        connection = nil
        conn.close()
        failAllPending(RpcError.transport("ipc 连接已断开"))
    }

    private func failAllPending(_ error: Error) {
        let all = pending
        pending.removeAll()
        for cont in all.values {
            cont.resume(throwing: error)
        }
    }

    private func failPendingCall(id: UInt64, error: Error) {
        if let cont = pending.removeValue(forKey: id) {
            cont.resume(throwing: error)
        }
    }
}

/// 带上限的行读取(镜像 mtdss_ipc::read_capped_line):读至 '\n'(不含),
/// 累计长度超 cap 即抛 FrameTooLargeError(防异常对端撑爆内存);
/// EOF → nil(残余无换行的截断帧同样按关闭处理)。
final class CappedLineReader: @unchecked Sendable {
    private let connection: any SocketConnection
    private let cap: Int
    private var buffer = [UInt8]()
    /// buffer 内已扫描段(无换行)长度,避免整帧重扫。
    private var scanned = 0

    init(connection: any SocketConnection, cap: Int) {
        self.connection = connection
        self.cap = cap
    }

    /// 返回一行(不含 '\n');EOF → nil。
    func readLine() throws -> Data? {
        while true {
            if let nl = buffer[scanned...].firstIndex(of: 0x0A) {
                let line = Data(buffer[0..<nl])
                buffer.removeFirst(nl + 1)
                scanned = 0
                if line.count > cap {
                    throw FrameTooLargeError() // 换行收尾的超限行同样违约(镜像 Kotlin 逐段累计口径)
                }
                return line
            }
            scanned = buffer.count
            if buffer.count > cap {
                throw FrameTooLargeError() // 无换行先超限:即刻断,不无限攒内存
            }
            let chunk = try connection.read()
            if chunk.isEmpty {
                return nil // EOF(残余按关闭丢弃)
            }
            buffer.append(contentsOf: chunk)
        }
    }
}
