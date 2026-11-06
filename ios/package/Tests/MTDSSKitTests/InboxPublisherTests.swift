import XCTest
@testable import MTDSSKit

/// T8 收件箱发布面单测(P4a 的 iOS 形态,spec §5.2;先红后绿):
/// - **两步发布 move**:fs.publish → 暂存终件 move 进收件箱本名(嵌套 rel
///   逐级建目录);撞名 `(n)` 改名(唯一名收口在 App 侧发布面);源缺席
///   → 错误(引擎按 0x05 ok:false 收「保存失败」,暂存保留待重试);
/// - **0x04 在场事实面**:收件箱真实行(在 + size 吻合 → exists);
/// - **事件帧组装**:fs.publish/fs.query_dest 事件的 0x05/0x04 应答帧
///   (payload 键字母序,与 Kotlin 客户端 golden 同口径);
/// - **事件解析**:daemon 帧参数 → IpcEvent(畸形参数同帧丢弃)。
final class InboxPublisherTests: XCTestCase {

    private var dir: String!
    private var inbox: String!
    private var staging: String!

    override func setUpWithError() throws {
        dir = NSTemporaryDirectory() + "mtdss-t8-pub-\(UUID().uuidString)"
        inbox = dir + "/Documents"
        staging = dir + "/home/.mtdss"
        try FileManager.default.createDirectory(atPath: inbox, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(atPath: staging, withIntermediateDirectories: true)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(atPath: dir)
    }

    // MARK: 两步发布 move

    func testPublishMovesStagedFileIntoInboxUnderFinalName() throws {
        let data = Data("收件内容".utf8)
        let staged = staging + "/发布 甲.bin"
        try data.write(to: URL(fileURLWithPath: staged))
        let files = DefaultInboxFilesystem()

        let landed = try files.moveAndRename(stagingPath: staged, rel: "发布 甲.bin", inbox: inbox)

        XCTAssertEqual(landed, inbox + "/发布 甲.bin")
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: landed)), data, "move 逐字节保真")
        XCTAssertFalse(FileManager.default.fileExists(atPath: staged), "暂存件不再留存(move 非拷贝)")
    }

    func testPublishNestedRelCreatesIntermediateDirs() throws {
        try Data("n".utf8).write(to: URL(fileURLWithPath: staging + "/乙.bin"))
        let files = DefaultInboxFilesystem()
        let landed = try files.moveAndRename(
            stagingPath: staging + "/乙.bin",
            rel: "嵌套/子/乙.bin",
            inbox: inbox,
        )
        XCTAssertEqual(landed, inbox + "/嵌套/子/乙.bin")
    }

    func testPublishConflictGetsNumericSuffix() throws {
        try Data("old".utf8).write(to: URL(fileURLWithPath: inbox + "/c.bin"))
        try Data("new".utf8).write(to: URL(fileURLWithPath: staging + "/c.bin"))
        let files = DefaultInboxFilesystem()

        let landed = try files.moveAndRename(stagingPath: staging + "/c.bin", rel: "c.bin", inbox: inbox)

        XCTAssertEqual(landed, inbox + "/c (1).bin", "撞名 (n) 改名,正身不被覆盖")
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: landed)), Data("new".utf8))
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: inbox + "/c.bin")), Data("old".utf8))
    }

    func testPublishConflictSuffixIncrements() throws {
        try Data("o".utf8).write(to: URL(fileURLWithPath: inbox + "/c.txt"))
        try Data("o1".utf8).write(to: URL(fileURLWithPath: inbox + "/c (1).txt"))
        try Data("n".utf8).write(to: URL(fileURLWithPath: staging + "/c.txt"))
        let files = DefaultInboxFilesystem()
        let landed = try files.moveAndRename(stagingPath: staging + "/c.txt", rel: "c.txt", inbox: inbox)
        XCTAssertEqual(landed, inbox + "/c (2).txt")
    }

    func testPublishMissingStagedFileThrows() throws {
        let files = DefaultInboxFilesystem()
        XCTAssertThrowsError(
            try files.moveAndRename(stagingPath: staging + "/无.bin", rel: "无.bin", inbox: inbox),
            "源缺席必须抛错(0x05 ok:false → 引擎「保存失败」暂存保留)"
        )
    }

    // MARK: 0x04 在场事实面

    func testPresenceChecksInboxTruthNotStaging() throws {
        try Data(repeating: 7, count: 42).write(to: URL(fileURLWithPath: inbox + "/a.bin"))
        try Data(repeating: 7, count: 42).write(to: URL(fileURLWithPath: staging + "/b.bin"))
        let files = DefaultInboxFilesystem()

        let hit = files.presence(rel: "a.bin", size: 42, inbox: inbox)
        XCTAssertTrue(hit.exists, "收件箱正身在场")
        XCTAssertEqual(hit.actualSize, 42)

        let sizeMismatch = files.presence(rel: "a.bin", size: 41, inbox: inbox)
        XCTAssertFalse(sizeMismatch.exists, "尺寸不符 = 不在场(F7 口径)")

        let stagingOnly = files.presence(rel: "b.bin", size: 42, inbox: inbox)
        XCTAssertFalse(stagingOnly.exists, "暂存件不是正身(杀 App 中间态不可见的语义锚)")

        let absent = files.presence(rel: "无.bin", size: 0, inbox: inbox)
        XCTAssertFalse(absent.exists)
    }

    // MARK: 事件应答帧

    func testPublisherAnswersPublishWithMoveAndAckPayload() async throws {
        try Data("pub".utf8).write(to: URL(fileURLWithPath: staging + "/x.bin"))
        let layout = EngineLayout(home: dir + "/home")
        let channel = RecordingChannel()
        let publisher = InboxPublisher(layout: layout, fdpass: channel, files: DefaultInboxFilesystem())

        let event = IpcEvent.publish(
            requestId: 9, rel: "x.bin",
            stagingPath: staging + "/x.bin", size: 3,
        )
        await publisher.handle(event)

        let frame = try XCTUnwrap(channel.frames.first)
        XCTAssertEqual(frame.kind, FdPassKind.publishResult)
        let json = try JSONSerialization.jsonObject(with: frame.payload) as? [String: Any]
        XCTAssertEqual(json?["ok"] as? Bool, true, "发布成功 ok:true")
        XCTAssertEqual(json?["request_id"] as? Int, 9)
        // 键字母序(error?/ok/request_id;与 Kotlin golden 同口径)
        let keys = try XCTUnwrap(JSONSerialization.jsonObject(with: frame.payload) as? [String: Any]).keys.sorted()
        XCTAssertEqual(keys, ["ok", "request_id"])
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: inbox + "/x.bin")), Data("pub".utf8))
    }

    func testPublisherAnswersPublishFailureWithoutMoving() async throws {
        let layout = EngineLayout(home: dir + "/home")
        let channel = RecordingChannel()
        let publisher = InboxPublisher(layout: layout, fdpass: channel, files: DefaultInboxFilesystem())

        await publisher.handle(
            .publish(requestId: 4, rel: "缺.bin", stagingPath: staging + "/缺.bin", size: 1),
        )

        let frame = try XCTUnwrap(channel.frames.first)
        XCTAssertEqual(frame.kind, FdPassKind.publishResult)
        let json = try JSONSerialization.jsonObject(with: frame.payload) as? [String: Any]
        XCTAssertEqual(json?["ok"] as? Bool, false)
        let error = try XCTUnwrap(json?["error"] as? String)
        XCTAssertFalse(error.isEmpty, "失败文案透传(引擎拼「保存失败」)")
        XCTAssertFalse(FileManager.default.fileExists(atPath: inbox + "/缺.bin"))
    }

    func testPublisherAnswersDestQueryFromInboxTruth() async throws {
        try Data(repeating: 1, count: 42).write(to: URL(fileURLWithPath: inbox + "/a.bin"))
        let layout = EngineLayout(home: dir + "/home")
        let channel = RecordingChannel()
        let publisher = InboxPublisher(layout: layout, fdpass: channel, files: DefaultInboxFilesystem())

        await publisher.handle(.destQuery(requestId: 2, rel: "a.bin", size: 42))
        await publisher.handle(.destQuery(requestId: 3, rel: "缺席.bin", size: 42))

        XCTAssertEqual(channel.frames.count, 2)
        for (frame, want) in zip(channel.frames, [true, false]) {
            XCTAssertEqual(frame.kind, FdPassKind.destQuery)
            let json = try JSONSerialization.jsonObject(with: frame.payload) as? [String: Any]
            XCTAssertEqual(json?["exists"] as? Bool, want)
            XCTAssertEqual(json?["size"] as? Int, want ? 42 : 0)
        }
    }

    // MARK: 目录行公开区建目录(V3-3 返工波 4 X6 修复轮)

    func testEnsureDirectoryTreeCreatesNestedDirsInPublicInbox() throws {
        let files = DefaultInboxFilesystem()

        let dir = try files.ensureDirectoryTree(rel: "docs/空巢/", inbox: inbox)

        XCTAssertEqual(dir, inbox + "/docs/空巢")
        var isDir: ObjCBool = false
        XCTAssertTrue(FileManager.default.fileExists(atPath: dir, isDirectory: &isDir))
        XCTAssertTrue(isDir.boolValue, "目录行以**目录**形态落公开区(绝不能落成文件)")
        // 幂等合并:重复建已存目录 = no-op 不报错(与引擎 create_dir_all 同语义)
        _ = try files.ensureDirectoryTree(rel: "docs/空巢/", inbox: inbox)
    }

    func testEnsureDirectoryTreeRejectsBadForms() throws {
        let files = DefaultInboxFilesystem()
        XCTAssertThrowsError(try files.ensureDirectoryTree(rel: "notadir", inbox: inbox), "无尾 '/' 非目录行")
        XCTAssertThrowsError(try files.ensureDirectoryTree(rel: "a//", inbox: inbox), "双尾 '/' 拒")
        XCTAssertThrowsError(try files.ensureDirectoryTree(rel: "a//b/", inbox: inbox), "中部空段拒")
        XCTAssertThrowsError(try files.ensureDirectoryTree(rel: "a/../b/", inbox: inbox), "\"..\" 严于 daemon 拒")
        XCTAssertThrowsError(try files.ensureDirectoryTree(rel: "/", inbox: inbox), "裸 '/' 拒")
        XCTAssertFalse(FileManager.default.fileExists(atPath: inbox + "/a"))
    }

    func testPublisherAnswersEnsureDirWithCreateAndAckPayload() async throws {
        let layout = EngineLayout(home: dir + "/home")
        let channel = RecordingChannel()
        let publisher = InboxPublisher(layout: layout, fdpass: channel, files: DefaultInboxFilesystem())

        await publisher.handle(.ensureDir(requestId: 21, rel: "空目录/"))

        let frame = try XCTUnwrap(channel.frames.first)
        XCTAssertEqual(frame.kind, FdPassKind.publishResult, "应答与发布共用 0x05 帧形状")
        let json = try JSONSerialization.jsonObject(with: frame.payload) as? [String: Any]
        XCTAssertEqual(json?["ok"] as? Bool, true)
        XCTAssertEqual(json?["request_id"] as? Int, 21)
        var isDir: ObjCBool = false
        XCTAssertTrue(FileManager.default.fileExists(atPath: inbox + "/空目录", isDirectory: &isDir))
        XCTAssertTrue(isDir.boolValue, "公开收件箱目录在场(不经暂存)")
    }

    func testPublisherAnswersEnsureDirFailureWithoutSideEffects() async throws {
        let layout = EngineLayout(home: dir + "/home")
        let channel = RecordingChannel()
        let publisher = InboxPublisher(layout: layout, fdpass: channel, files: DefaultInboxFilesystem())

        await publisher.handle(.ensureDir(requestId: 22, rel: "非目录行"))

        let frame = try XCTUnwrap(channel.frames.first)
        XCTAssertEqual(frame.kind, FdPassKind.publishResult)
        let json = try JSONSerialization.jsonObject(with: frame.payload) as? [String: Any]
        XCTAssertEqual(json?["ok"] as? Bool, false)
        XCTAssertNotNil(json?["error"] as? String, "失败文案透传(引擎「保存失败」收口)")
    }

    // MARK: 事件解析(daemon 帧 → IpcEvent)

    func testParsePublishAndQueryDestFrames() throws {
        let publish: JSONValue = .object([
            "request_id": .number("11"),
            "rel": .string("发布 名.bin"),
            "staging_path": .string("/home/.mtdss/发布 名.bin"),
            "size": .number("1024"),
        ])
        XCTAssertEqual(
            IpcEvent.parse(method: Ops.eventPublish, params: publish),
            .publish(requestId: 11, rel: "发布 名.bin", stagingPath: "/home/.mtdss/发布 名.bin", size: 1024),
        )
        let query: JSONValue = .object([
            "request_id": .number("12"),
            "rel": .string("a.bin"),
            "size": .number("42"),
        ])
        XCTAssertEqual(
            IpcEvent.parse(method: Ops.eventQueryDest, params: query),
            .destQuery(requestId: 12, rel: "a.bin", size: 42),
        )
        // fs.ensure_dir(V3-3 X6 修复轮):request_id + rel 两参
        let ensure: JSONValue = .object([
            "request_id": .number("13"),
            "rel": .string("空巢/"),
        ])
        XCTAssertEqual(
            IpcEvent.parse(method: Ops.eventEnsureDir, params: ensure),
            .ensureDir(requestId: 13, rel: "空巢/"),
        )
        // 畸形参数同帧丢弃(缺 staging_path / 缺 rel)
        let bad: JSONValue = .object(["request_id": .number("1"), "rel": .string("x")])
        XCTAssertNil(IpcEvent.parse(method: Ops.eventPublish, params: bad))
        let badEnsure: JSONValue = .object(["request_id": .number("2")])
        XCTAssertNil(IpcEvent.parse(method: Ops.eventEnsureDir, params: badEnsure))
    }
}

/// 录制型 fdpass 通道(单测桩):记帧,回固定 ack。
final class RecordingChannel: FdPassAnswering, @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: [FdPassFrame] = []

    var frames: [FdPassFrame] { lock.withLock { recorded } }

    func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
        lock.withLock { recorded.append(FdPassFrame(kind: kind, payload: payload)) }
        let ack: [String: Any] = ["ok": true, "request_id": 0]
        return FdPassFrame(
            kind: FdPassKind.ack,
            payload: try JSONSerialization.data(withJSONObject: ack, options: [.sortedKeys]),
        )
    }
}
