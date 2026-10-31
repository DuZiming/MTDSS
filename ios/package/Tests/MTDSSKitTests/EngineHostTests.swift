import XCTest
@testable import MTDSSKit

/// EngineHost 单测(macOS 宿主;brief T4 Step 1):C ABI 经注入伪闭包消费,
/// socket 面经 makeClient 注入伪 daemon(FakeDaemon 内存管道)——swift test
/// 不触真引擎(iOS-only 符号),引擎就绪/未就绪/已运行/超时四路语义全覆盖。
final class EngineHostTests: XCTestCase {

    /// T2 C ABI 伪实现:契约逐条模仿(含 ipc_path 两段调用「缓冲不足不写、
    /// 返回所需长度」语义),记录调用痕迹供断言。
    final class MockEngine: @unchecked Sendable {
        private let lock = NSLock()
        private var startHomeStorage: String?
        private var startCallsStorage = 0
        private var startResultStorage: Int32 = 0
        private var stopCallsStorage = 0
        private var ipcPathValueStorage = "/fake/home/daemon.sock"
        private var ipcPathCallsStorage: [(outWasNull: Bool, len: Int)] = []

        var startHome: String? { lock.withLock { startHomeStorage } }
        var startCalls: Int { lock.withLock { startCallsStorage } }
        var startResult: Int32 {
            get { lock.withLock { startResultStorage } }
            set { lock.withLock { startResultStorage = newValue } }
        }
        var stopCalls: Int { lock.withLock { stopCallsStorage } }
        var ipcPathValue: String {
            get { lock.withLock { ipcPathValueStorage } }
            set { lock.withLock { ipcPathValueStorage = newValue } }
        }
        var ipcPathCalls: [(outWasNull: Bool, len: Int)] { lock.withLock { ipcPathCallsStorage } }

        /// T2 契约同型:mtdss_ios_start。
        lazy var bindings: EngineHost.Bindings = .init(
            start: { [weak self] home in
                guard let self else { return 1 }
                let home = home.map(String(cString:))
                return self.lock.withLock {
                    self.startHomeStorage = home
                    self.startCallsStorage += 1
                    return self.startResultStorage
                }
            },
            stop: { [weak self] in
                guard let self else { return 1 }
                return self.lock.withLock {
                    self.stopCallsStorage += 1
                    return 0
                }
            },
            ipcPath: { [weak self] home, out, outLen in
                guard let self else { return -1 }
                guard let home else { return -1 }
                let path = self.lock.withLock {
                    self.ipcPathCallsStorage.append((out == nil, outLen))
                    return self.ipcPathValueStorage
                }
                let needed = path.utf8.count + 1 // 含 NUL(T2 契约)
                if let out, outLen >= needed {
                    let bytes = Array(path.utf8)
                    for (i, b) in bytes.enumerated() { out[i] = CChar(bitPattern: b) }
                    out[bytes.count] = 0
                }
                return Int32(needed)
            },
        )
    }

    /// connect 即失败的端点(模拟引擎线程未起、socket 文件尚未出现)。
    private struct UnreadyEndpoint: SocketEndpoint {
        func connect() throws -> any SocketConnection { throw IpcConnectError("引擎未就绪") }
    }

    /// 应答 hello 的伪 daemon 端点(golden hello.resp 原帧回写;新客户端首调
    /// id=1 与 golden 逐字节对齐)。
    private func helloDaemon() -> FakeDaemon {
        let fake = FakeDaemon()
        fake.onLine = { side, line in
            guard let parsed = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .object(let obj) = parsed,
                  case .number(let raw)? = obj["id"],
                  let id = UInt64(raw) else { return }
            side.sendLine(Goldens.withId(Goldens.read("hello.resp.json"), id))
        }
        return fake
    }

    // MARK: 用例

    func testStartBootsEnginePollsHelloAndReturnsWorkingClient() async throws {
        let engine = MockEngine()
        let fake = helloDaemon()
        let madePaths = PathLog()
        let host = EngineHost(
            bindings: engine.bindings,
            makeClient: { path in
                madePaths.append(path)
                return IpcClient(endpoint: fake)
            },
        )

        let client = try await host.start(home: "/fake/home")

        // start 经闭包收到 home;socket 路径经 ipc_path 两段调用解析
        XCTAssertEqual(engine.startHome, "/fake/home")
        XCTAssertEqual(engine.startCalls, 1)
        XCTAssertEqual(madePaths.all, ["/fake/home/daemon.sock"])
        // 两段调用:先 (out=nil, 0) 探长度,再真缓冲取路径
        let calls = engine.ipcPathCalls
        XCTAssertEqual(calls.count, 2)
        XCTAssertEqual(calls[0].outWasNull, true)
        XCTAssertEqual(calls[0].len, 0)
        XCTAssertEqual(calls[1].outWasNull, false)
        XCTAssertTrue(calls[1].len >= "/fake/home/daemon.sock".utf8.count + 1)
        // 返回的客户端真通(golden hello:logged_in=true)
        let hello = try await withTimeout(5) { try await client.send("hello") }
        let parsed = try HelloResult(hello)
        XCTAssertEqual(parsed.loggedIn, true)
        await client.close()
    }

    func testStartRejectedPropagatesCodeAndSkipsHello() async {
        let engine = MockEngine()
        engine.startResult = 3 // ERR_START_FAILED(T2)
        let made = Atomics()
        let host = EngineHost(
            bindings: engine.bindings,
            makeClient: { _ in made.next(); return IpcClient(endpoint: UnreadyEndpoint()) },
        )

        do {
            _ = try await host.start(home: "/fake/home")
            XCTFail("start=3 应抛 StartError")
        } catch let error as EngineHost.StartError {
            XCTAssertEqual(error, .startRejected(3))
        } catch {
            XCTFail("错误类型不对: \(error)")
        }
        XCTAssertEqual(made.value, 0, "start 被拒不得再碰 socket")
    }

    func testAlreadyRunningProceedsToHello() async throws {
        let engine = MockEngine()
        engine.startResult = 2 // ERR_START_RUNNING(T2):幂等 boot
        let fake = helloDaemon()
        let host = EngineHost(
            bindings: engine.bindings,
            makeClient: { _ in IpcClient(endpoint: fake) },
        )

        let client = try await host.start(home: "/fake/home")
        let hello = try await withTimeout(5) { try await client.send("hello") }
        XCTAssertEqual(try HelloResult(hello).loggedIn, true)
        await client.close()
    }

    func testEngineNotReadyRetriesUntilHelloSucceeds() async throws {
        let engine = MockEngine()
        // 前 2 次客户端建在未就绪端点上(connect 即抛),第 3 次起伪 daemon 应答
        let fake = helloDaemon()
        let counter = Atomics()
        let host = EngineHost(
            bindings: engine.bindings,
            makeClient: { _ in
                counter.next() < 2
                    ? IpcClient(endpoint: UnreadyEndpoint())
                    : IpcClient(endpoint: fake)
            },
            readyTimeout: 10,
        )

        let client = try await host.start(home: "/fake/home")
        XCTAssertEqual(counter.value, 3, "前 2 次未就绪,第 3 次应成功")
        let hello = try await withTimeout(5) { try await client.send("hello") }
        XCTAssertEqual(try HelloResult(hello).loggedIn, true)
        await client.close()
    }

    func testHelloNotAnsweredTimesOut() async {
        let engine = MockEngine()
        let silent = FakeDaemon() // 无 onLine:请求被吞,永远不应答
        let host = EngineHost(
            bindings: engine.bindings,
            makeClient: { _ in IpcClient(endpoint: silent) },
            readyTimeout: 0.5,
        )

        let began = Date()
        do {
            _ = try await host.start(home: "/fake/home")
            XCTFail("不应答必须超时")
        } catch let error as EngineHost.StartError {
            XCTAssertEqual(error, .helloTimeout)
        } catch {
            XCTFail("错误类型不对: \(error)")
        }
        XCTAssertLessThan(Date().timeIntervalSince(began), 4, "超时应受 readyTimeout 约束")
    }

    func testStopPassesThrough() {
        let engine = MockEngine()
        let host = EngineHost(bindings: engine.bindings)
        XCTAssertEqual(host.stop(), 0)
        XCTAssertEqual(engine.stopCalls, 1)
    }
}

/// 微型计数器(makeClient 调用序号;@Sendable 闭包可安全读写)。
private final class Atomics: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    var value: Int { lock.withLock { n } }
    func next() -> Int {
        lock.lock()
        defer { lock.unlock() }
        n += 1
        return n
    }
}

/// 字符串收集器(路径痕迹;@Sendable 闭包可安全追加)。
private final class PathLog: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String] = []
    func append(_ s: String) {
        lock.withLock { items.append(s) }
    }
    var all: [String] { lock.withLock { items } }
}

/// 带总时长的 await(防测试悬挂拖死 swift test;与 IpcClientTests 同款)。
private func withTimeout<T: Sendable>(_ seconds: TimeInterval, _ run: @escaping @Sendable () async throws -> T) async throws -> T {
    struct TestTimeoutError: Error {}
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await run() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw TestTimeoutError()
        }
        let first = try await group.next()!
        group.cancelAll()
        return first
    }
}
