/// 进程内引擎宿主(设计 §5.1 的 Swift 消费面):T2 C ABI 三件套
/// (mtdss_ios_start / mtdss_ios_stop / mtdss_ios_ipc_path,crates/ios-bridge)
/// 经注入闭包消费 + IPC 就绪轮询(hello 背避重试,总时长受 ``readyTimeout``
/// 约束,默认 5s)。
///
/// 注入而非直绑:swift test 跑在 macOS 宿主,C 符号只存在于 iOS 静态库——
/// 闭包注入让宿主单测用伪实现覆盖全部启动语义;App 目标(ioS/app)以
/// `@_silgen_name` 直绑真符号,构造 `Bindings` 注入本类型(Ruling E:宿主
/// 逻辑归 Kit,绑定归 App)。
///
/// 语义:
/// - ``start(home:)``:起引擎线程 → 解析 UDS 路径(两段调用)→ 轮询 hello
///   直至引擎就绪;引擎已在运行(T2 `ERR_START_RUNNING`=2)按幂等 boot 放行;
///   就绪前 socket 未出现/连接拒/调用无应答都按「未就绪」重试。
/// - ``stop()``:透传 mtdss_ios_stop(优雅停引擎;监督重启路径 = stop 后再
///   start)。
/// - 单次 hello 调用带独立超时闸(T3 已知面:IpcClient 挂起调用无内建超时)。

import Foundation

public struct EngineHost: Sendable {

    /// T2 C ABI 同型闭包集(签名逐参对照 crates/ios-bridge/src/lib.rs)。
    public struct Bindings: Sendable {
        /// `mtdss_ios_start(home)` → 0=线程已起;1/2/3=入参/已运行/启动失败。
        public let start: @Sendable (UnsafePointer<CChar>?) -> Int32
        /// `mtdss_ios_stop()` → 0=已收尾;非 0=引擎未在运行。
        public let stop: @Sendable () -> Int32
        /// `mtdss_ios_ipc_path(home, out, len)` → 所需长度(含 NUL);缓冲不足
        /// 不写仍返回所需长度;负值=入参错。
        public let ipcPath: @Sendable (UnsafePointer<CChar>?, UnsafeMutablePointer<CChar>?, Int) -> Int32

        public init(
            start: @escaping @Sendable (UnsafePointer<CChar>?) -> Int32,
            stop: @escaping @Sendable () -> Int32,
            ipcPath: @escaping @Sendable (UnsafePointer<CChar>?, UnsafeMutablePointer<CChar>?, Int) -> Int32,
        ) {
            self.start = start
            self.stop = stop
            self.ipcPath = ipcPath
        }
    }

    /// 启动失败面。
    public enum StartError: Error, Equatable, Sendable {
        /// 引擎线程起而复拒(T2:1=入参非法,3=装配/线程失败;细节在引擎侧
        /// tracing 日志)。
        case startRejected(Int32)
        /// ipc_path 派生失败(T2 负值错误码)。
        case ipcPathUnavailable(Int32)
        /// 就绪轮询超时(引擎线程起后 `readyTimeout` 内 hello 未通)。
        case helloTimeout
    }

    /// T2 `ERR_START_RUNNING`:引擎已在运行 → 幂等 boot,不视为失败。
    /// (App 生命周期反复 ensureEngine 时,二次 start 不炸。)
    static let alreadyRunning: Int32 = 2

    private let bindings: Bindings
    /// UDS 路径 → IPC 客户端(宿主测试注入伪 daemon;App 侧走默认 UDS 端点)。
    private let makeClient: @Sendable (String) -> IpcClient
    /// 就绪轮询总时长上界(brief:hello 重试 ≤5s 背避)。
    public let readyTimeout: TimeInterval

    public init(
        bindings: Bindings,
        makeClient: @escaping @Sendable (String) -> IpcClient = { IpcClient(endpoint: UnixSocketEndpoint(path: $0)) },
        readyTimeout: TimeInterval = 5,
    ) {
        self.bindings = bindings
        self.makeClient = makeClient
        self.readyTimeout = readyTimeout
    }

    // MARK: 对外面

    /// 起引擎并等待 IPC 就绪;返回与引擎通话的客户端(Repository 包一层即用)。
    /// `home` = MTDSS 数据目录(引擎的 settings/session/inbox 与 UDS 均在其下)。
    public func start(home: String) async throws -> IpcClient {
        let rc = home.withCString { bindings.start($0) }
        guard rc == 0 || rc == Self.alreadyRunning else {
            throw StartError.startRejected(rc)
        }
        let path = try resolveIpcPath(home)
        return try await pollHello(path: path)
    }

    /// 优雅停引擎(透传 T2;0=已收尾)。监督重启 = stop 后再 start。
    @discardableResult
    public func stop() -> Int32 {
        bindings.stop()
    }

    // MARK: 内部

    /// 两段调用解析 UDS 路径(T2 契约:首调只取所需长度,再按值分配重调)。
    private func resolveIpcPath(_ home: String) throws -> String {
        let needed = home.withCString { bindings.ipcPath($0, nil, 0) }
        guard needed > 0 else { throw StartError.ipcPathUnavailable(needed) }
        var buffer = [CChar](repeating: 0, count: Int(needed))
        let written = home.withCString { bindings.ipcPath($0, &buffer, buffer.count) }
        guard written == needed else { throw StartError.ipcPathUnavailable(written) }
        return String(cString: buffer)
    }

    /// hello 背避轮询:每次尝试换新客户端(失败尝试可能留悬挂 pending,
    /// T3 已知面——新客户端零包袱);100ms 起步倍增,400ms 封顶。
    private func pollHello(path: String) async throws -> IpcClient {
        let deadline = Date().addingTimeInterval(readyTimeout)
        var delayNs: UInt64 = 100_000_000
        while true {
            let client = makeClient(path)
            if await helloOnce(client) {
                return client
            }
            await client.close()
            guard Date() < deadline else { throw StartError.helloTimeout }
            try? await Task.sleep(nanoseconds: delayNs)
            delayNs = min(delayNs * 2, 400_000_000)
        }
    }

    /// 单次 hello(带独立超时闸,防 T3 已知面:无应答调用永久挂起)。
    private func helloOnce(_ client: IpcClient) async -> Bool {
        let attemptCap = min(readyTimeout, 2.0)
        do {
            _ = try await Self.withTimeout(seconds: attemptCap) {
                try await client.send(Ops.hello)
            }
            return true
        } catch {
            return false
        }
    }

    /// 带总时长的 await(task 组赛跑;Kotlin withTimeout 同象)。
    static func withTimeout<T: Sendable>(
        seconds: TimeInterval,
        _ run: @escaping @Sendable () async throws -> T,
    ) async throws -> T {
        struct TimeoutError: Error {}
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await run() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                throw TimeoutError()
            }
            let first = try await group.next()!
            group.cancelAll()
            return first
        }
    }
}
