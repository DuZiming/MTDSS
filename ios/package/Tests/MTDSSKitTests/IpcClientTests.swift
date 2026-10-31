import XCTest
import MTDSSKit

/// IpcClient 状态机单测(伪 server = 内存管道,见 FakeDaemon):hello→响应配对
/// (请求帧逐字节对齐 golden)、并发按 id 路由互不串扰、错误帧→RpcError(kind
/// 取自 data.kind)、事件流投递且与响应路由互不干扰、坏帧丢弃连接存活、超限帧
/// 断连且在飞失败、连接失败快速失败、close 终态。字节级 golden 全形态对齐见
/// ContractGoldenTests。
final class IpcClientTests: XCTestCase {

    struct DaemonDownError: Error {}
    struct TestTimeoutError: Error {}

    /// 伪 server 应答脚本助手:应答 golden 帧(id 重写为请求 id)。
    private func replyWithGolden(_ name: String) -> FakeDaemon.LineHandler {
        { conn, line in
            let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self))
            guard case .number(let raw)? = frame?["id"], let id = UInt64(raw) else { return }
            conn.sendLine(Goldens.withId(Goldens.read(name), id))
        }
    }

    /// 条件轮询直至成立(默认 5s 上界,届满抛错)。
    private func awaitUntil(timeout: TimeInterval = 5, cond: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !cond() {
            guard Date() < deadline else { throw TestTimeoutError() }
            try await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// 从请求行提取 id(伪 server 脚本用)。
    private func requestId(of line: Data) -> UInt64? {
        let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self))
        guard case .number(let raw)? = frame?["id"] else { return nil }
        return UInt64(raw)
    }

    // ── 测试 ────────────────────────────────────────────────────────────────

    func testHelloRequestByteExactAndTypedResponse() async throws {
        let fake = FakeDaemon()
        fake.onLine = replyWithGolden("hello.resp.json")
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        let hello = try await MtdssRepository(client: client).hello()
        XCTAssertEqual(hello.version, "1.1.0")
        XCTAssertTrue(hello.loggedIn)
        XCTAssertEqual(hello.device?.deviceId, "10ba682c8ad13513971e8b56881aab8b")
        // golden 文件内容即线上帧(含尾换行);客户端首调 id=1 与 golden 同
        let conn = try XCTUnwrap(fake.connections.first)
        XCTAssertEqual(conn.received.count, 1)
        XCTAssertEqual(conn.received.first, Goldens.read("hello.req.json"))
        await client.close()
    }

    func testConcurrentCallsRouteById() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            // 睡 20ms 让全部请求入队在飞,应答集中回——最大化路由竞态窗口
            Thread.sleep(forTimeInterval: 0.02)
            let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self))
            guard case .number(let id)? = frame?["id"],
                  case .string(let marker)? = frame?["params"]?["marker"] else { return }
            let resp = JSONValue.object([
                "id": .number(id),
                "result": .object(["marker": .string(marker), "n": .number(id)]),
            ])
            conn.sendLine(Data((resp.canonical + "\n").utf8))
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        let n = 32
        let results = try await withThrowingTaskGroup(of: (String, Int64).self) { group in
            for k in 1...n {
                group.addTask {
                    let v = try await client.send("echo", params: .object(["marker": .string("m\(k)")]))
                    return (try v.req("marker").asString(), try v.req("n").asInt64())
                }
            }
            var out: [(String, Int64)] = []
            for try await r in group { out.append(r) }
            return out
        }
        // 各调用收到自己的 marker 回显;id 恰好覆盖 1...n(无重复无丢失)
        XCTAssertEqual(results.count, n)
        XCTAssertEqual(Set(results.map { $0.0 }), Set((1...n).map { "m\($0)" }))
        XCTAssertEqual(Set(results.map { $0.1 }), Set(Int64(1)...Int64(n)))
        await client.close()
    }

    func testErrorEnvelopeBecomesRpcErrorWithKind() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let id = self.requestId(of: line) else { return }
            conn.sendLine("{\"id\":\(id),\"error\":{\"code\":7,\"message\":\"没找到\",\"data\":{\"kind\":\"NOT_FOUND\"}}\n")
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        do {
            _ = try await client.send("op1")
            XCTFail("应抛 RpcError")
        } catch let e as RpcError {
            XCTAssertEqual(e.code, 7)
            XCTAssertEqual(e.message, "没找到")
            XCTAssertEqual(e.kind, "NOT_FOUND")
        }
        // 无 data 形态:kind 为空
        fake.onLine = { conn, line in
            guard let id = self.requestId(of: line) else { return }
            conn.sendLine("{\"id\":\(id),\"error\":{\"code\":3,\"message\":\"设备离线: x\"}}\n")
        }
        do {
            _ = try await client.send("op2")
            XCTFail("应抛 RpcError")
        } catch let e as RpcError {
            XCTAssertEqual(e.code, 3)
            XCTAssertEqual(e.message, "设备离线: x")
            XCTAssertNil(e.kind)
        }
        await client.close()
    }

    func testEventsDeliveredAndDemuxedFromResponses() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let id = self.requestId(of: line) else { return }
            // 先塞两个事件帧再回应答:验证读循环按帧分拣
            conn.sendLine(Goldens.read("event.login.json"))
            conn.sendLine(Goldens.read("event.notify.json"))
            conn.sendLine("{\"id\":\(id),\"result\":{\"ok\":true}}\n")
        }
        let client = IpcClient(endpoint: fake)
        let collector = EventCollector(client.events) // 订阅先于连接
        try await client.connect()
        let r = try await client.send("x")
        XCTAssertEqual(try r.req("ok").asBool(), true)
        let events = await collector.waitCount(2)
        XCTAssertEqual(events.count, 2)
        XCTAssertEqual(events.first, .login(loggedIn: true))
        XCTAssertEqual(events.last, .notify(title: "MTDSS", body: "«契约对端» 向 /mtdss/contract-inbox 写入 2 个文件"))
        await client.close()
    }

    func testGarbageLineSkippedConnectionSurvives() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let id = self.requestId(of: line) else { return }
            // 坏帧(无法解析)直接丢弃,连接不断(镜像 Rust 读半工口径)
            conn.sendLine(Data("not-json\n".utf8))
            conn.sendLine("{\"id\":\(id),\"result\":{\"ok\":true}}\n")
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        let v1 = try await client.send("op")
        XCTAssertEqual(try v1.req("ok").asBool(), true)
        // 同一连接继续可用
        let v2 = try await client.send("op")
        XCTAssertEqual(try v2.req("ok").asBool(), true)
        XCTAssertEqual(fake.connections.count, 1)
        await client.close()
    }

    func testOversizedFrameDisconnectsAndFailsInFlight() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, _ in
            // 8 MiB + 1 字节无换行:超单帧上限 → 客户端须断连(镜像 daemon 口径)
            conn.sendLine(Data(repeating: UInt8(ascii: "x"), count: 8 * 1024 * 1024 + 1))
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        do {
            _ = try await client.send("op")
            XCTFail("应抛传输错误")
        } catch let e as RpcError {
            XCTAssertEqual(e.code, RpcError.transportCode)
        }
        // 断连后下次调用惰性重建连接(FakeDaemon 记为第 2 条)
        _ = try? await client.send("op2")
        try await awaitUntil { fake.connections.count == 2 }
        await client.close()
    }

    func testConnectFailureThrowsFastThenRecovers() async throws {
        let fake = FakeDaemon()
        fake.connectFailure = DaemonDownError()
        let client = IpcClient(endpoint: fake)
        do {
            try await client.connect()
            XCTFail("应抛 IpcConnectError")
        } catch let e as IpcConnectError {
            XCTAssertFalse(e.message.isEmpty)
        }
        fake.connectFailure = nil // daemon 起来了
        try await client.connect()
        fake.onLine = replyWithGolden("hello.resp.json")
        let hello = try await MtdssRepository(client: client).hello()
        XCTAssertEqual(hello.version, "1.1.0")
        await client.close()
    }

    func testCloseFailsPendingAndIsTerminal() async throws {
        let fake = FakeDaemon()
        fake.onLine = { _, _ in } // 悬挂不答 → 在飞调用靠 close 失败
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        let inFlight = Task { try await client.send("slow") }
        let conn = try XCTUnwrap(fake.connections.first)
        try await awaitUntil { !conn.received.isEmpty } // 请求已上管道

        // 事件流终结观测(订阅先于 close)
        let drained = Task { () -> Int in
            var count = 0
            for await _ in client.events { count += 1 }
            return count
        }

        await client.close()
        do {
            _ = try await inFlight.value
            XCTFail("在飞调用应失败")
        } catch {
            // 传输错误(关闭兜底失败)
        }
        // close 后调用失败
        do {
            _ = try await client.send("x")
            XCTFail("close 后调用应失败")
        } catch {
            // IpcConnectError(已关闭)
        }
        // 事件流已终结(for-await 自然返回)
        let eventCount = try await withTimeout(5) { try await drained.value }
        XCTAssertEqual(eventCount, 0)
        // 幂等
        await client.close()
    }

    func testEventHubFansOutToSubscribers() async throws {
        let fake = FakeDaemon()
        let client = IpcClient(endpoint: fake)
        let hub = EventHub(client.events)
        let s1 = EventCollector(hub.subscribe())
        let s2 = EventCollector(hub.subscribe())
        try await client.connect()
        fake.connections.first?.sendLine(Goldens.read("event.online.json"))
        let e1 = await s1.waitCount(1)
        let e2 = await s2.waitCount(1)
        XCTAssertEqual(e1.first, .online(online: true))
        XCTAssertEqual(e2.first, .online(online: true))
        hub.close()
        await client.close()
    }
}

/// 带总时长的 await(防测试悬挂拖死 swift test)。
private func withTimeout<T: Sendable>(_ seconds: TimeInterval, _ run: @escaping @Sendable () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await run() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw IpcClientTests.TestTimeoutError()
        }
        let first = try await group.next()!
        group.cancelAll()
        return first
    }
}
