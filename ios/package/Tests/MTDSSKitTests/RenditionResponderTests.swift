/// 转码桥应答器宿主单测(V4-1 T14;daemon 侧 jobs.rs RenditionBridge /
/// fdpass 0x0C 契约为真源,Android RenditionResponderTest 同构):`fs
/// .rendition_open {path, request_id, start_at}` → 源解析(照片/文件两臂)
/// → 会话目录 → **回执先行** → 注入伪管线产段落盘 + 0x0C `{index, ok, path,
/// request_id}` 段回告;`fs.rendition_cancel` → 取消旗 + 清段目录 +
/// `{ok, request_id}`(未知会话幂等)。拒形矩阵、畸形帧(解析面收口)、
/// 回执未受理(不开管线)、管线失败回告、挂载清扫。
import Foundation
@testable import MTDSSKit
import XCTest

/// 有界等待闸(管线进场对齐;零长睡,fire 后即回)。
final class SignalGate: @unchecked Sendable {
    private let lock = NSLock()
    private var fired = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    var isFired: Bool {
        lock.lock(); defer { lock.unlock() }
        return fired
    }

    func fire() {
        lock.lock()
        fired = true
        let ws = waiters
        waiters = []
        lock.unlock()
        for w in ws { w.resume() }
    }

    func wait() async {
        lock.lock()
        if fired {
            lock.unlock()
            return
        }
        lock.unlock()
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            lock.lock()
            if fired {
                lock.unlock()
                c.resume()
                return
            }
            waiters.append(c)
            lock.unlock()
        }
    }
}

final class RenditionResponderTests: XCTestCase {

    // MARK: 伪服务(状态可编排;ThumbResponderTests 同构)

    private final class FakeRenditionPhotoService: PhotoLibraryService, @unchecked Sendable {
        private let lock = NSLock()
        var status: PhotoLibraryPermission = .notDetermined
        var albumsResult: [PhotoAlbum] = []
        var assetsByAlbum: [String: [PhotoAsset]] = [:]
        var openErrors: [String: Error] = [:]
        var openFiles: [String: URL] = [:]
        private(set) var openCalls: [String] = []

        func permissionStatus() async -> PhotoLibraryPermission {
            lock.lock(); defer { lock.unlock() }
            return status
        }

        func requestAccess() async -> PhotoLibraryPermission {
            lock.lock(); status = .authorized; defer { lock.unlock() }
            return status
        }

        func permissionChanges() -> AsyncStream<PhotoLibraryPermission> {
            AsyncStream { _ in }
        }

        func albums() async throws -> [PhotoAlbum] {
            lock.lock(); defer { lock.unlock() }
            return albumsResult
        }

        func assets(albumId: String?) async throws -> [PhotoAsset] {
            lock.lock(); defer { lock.unlock() }
            return assetsByAlbum[albumId ?? ""] ?? []
        }

        func exportAssets(ids: [String], to directory: URL) async throws -> [URL] {
            throw SeekOpenError.unavailable("RenditionResponderTests 伪服务不导出")
        }

        func openSeekable(photoId: String) async throws -> Int32 {
            lock.lock()
            openCalls.append(photoId)
            let failure = openErrors[photoId]
            let url = openFiles[photoId]
            lock.unlock()
            if let failure { throw failure }
            guard let url else {
                throw SeekOpenError.unavailable("无交付文件: \(photoId)")
            }
            let fd = Darwin.open(url.path, O_RDONLY)
            guard fd >= 0 else {
                throw SeekOpenError.unavailable("交付文件不可开: \(photoId)")
            }
            return fd
        }
    }

    // MARK: 伪转码管线(Produce/Hang/Fail 三模;Android FakeFactory 同构)

    final class FakeTranscoder: RenditionTranscoding, @unchecked Sendable {
        enum Mode { case produce, hang, fail }

        struct Call {
            let fd: Int32
            let displayName: String
            let startAtMs: Int64
            let sessionDir: String
            let isCancelled: @Sendable () -> Bool
        }

        private let lock = NSLock()
        private(set) var calls: [Call] = []
        let mode: Mode
        /// 管线进场闸(测试对齐管线受理面)。
        let started = SignalGate()
        private var finishContinuation: CheckedContinuation<Void, Never>?

        init(mode: Mode) {
            self.mode = mode
        }

        var callCount: Int {
            lock.lock(); defer { lock.unlock() }
            return calls.count
        }

        func call(at index: Int) -> Call {
            lock.lock(); defer { lock.unlock() }
            return calls[index]
        }

        /// 解挂 hang 面(测试收尾)。
        func release() {
            lock.lock()
            let c = finishContinuation
            finishContinuation = nil
            lock.unlock()
            c?.resume()
        }

        func transcode(
            fd: Int32, displayName: String, startAtMs: Int64, sessionDir: String,
            notifier: @escaping @Sendable (Int, String) -> Void,
            isCancelled: @escaping @Sendable () -> Bool,
        ) async throws {
            lock.lock()
            calls.append(Call(fd: fd, displayName: displayName, startAtMs: startAtMs,
                              sessionDir: sessionDir, isCancelled: isCancelled))
            lock.unlock()
            started.fire()
            switch mode {
            case .produce:
                for i in 0...1 {
                    let path = "\(sessionDir)/\(i).m4s"
                    try Data([UInt8(i), 9, 9]).write(to: URL(fileURLWithPath: path))
                    notifier(i, path)
                }
            case .hang:
                // 挂到测试解挂(cancel 测试:旗可见后解挂收尾)
                await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                    lock.lock(); finishContinuation = c; lock.unlock()
                }
            case .fail:
                throw TranscodeFailure("编码器初始化失败")
            }
            // fd 所有权契约:终局必关
            Darwin.close(fd)
        }
    }

    // MARK: 记录通道(帧面断言;ThumbResponderTests RecordingChannel 同构,
    // 0x0C 恒无 fd;ackOk 可编排回执未受理面)

    private final class RecordingChannel: FdPassAnswering, @unchecked Sendable {
        struct Sent {
            let kind: UInt8
            /// 原始 payload 字节(字母序字节钉面)。
            let payload: Data
            let json: [String: Any]
        }

        private let lock = NSLock()
        private(set) var frames: [Sent] = []
        /// ack 的 ok 值(false = daemon 未受理面)。
        var ackOk = true

        func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
            lock.lock()
            let json = (try? JSONSerialization.jsonObject(with: payload) as? [String: Any]) ?? [:]
            frames.append(Sent(kind: kind, payload: payload, json: json))
            let ok = ackOk
            lock.unlock()
            let body = "{\"ok\":\(ok),\"request_id\":0}"
            return FdPassFrame(kind: FdPassKind.ack, payload: Data(body.utf8))
        }

        var count: Int {
            lock.lock(); defer { lock.unlock() }
            return frames.count
        }

        func frame(at index: Int) -> Sent {
            lock.lock(); defer { lock.unlock() }
            return frames[index]
        }
    }

    // MARK: 有界轮询(段回告/管线进场异步面;零长睡,窗内即回)

    private func untilFrames(_ channel: RecordingChannel, _ n: Int) async {
        let deadline = Date().addingTimeInterval(5)
        while channel.count < n {
            XCTAssertTrue(Date() < deadline, "帧未在窗内到齐: \(channel.count)")
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
    }

    // MARK: 装配

    private func tempDir() -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("mtdss-rendition-test-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.path
    }

    private struct Env {
        let service: FakeRenditionPhotoService
        let channel: RecordingChannel
        let transcoder: FakeTranscoder
        let renditionDir: String
        let responder: RenditionResponder
    }

    private func makeEnv(
        service: FakeRenditionPhotoService = FakeRenditionPhotoService(),
        transcoder: FakeTranscoder = FakeTranscoder(mode: .produce),
        renditionDirOverride: String? = nil,
        durationProbe: (any RenditionDurationProbing)? = nil,
    ) -> Env {
        service.status = .authorized
        let renditionDir = renditionDirOverride ?? tempDir()
        let channel = RecordingChannel()
        let responder = RenditionResponder(
            service: service,
            fdpass: channel,
            renditionDir: renditionDir,
            transcoder: transcoder,
            durationProbe: durationProbe)
        return Env(
            service: service, channel: channel, transcoder: transcoder,
            renditionDir: renditionDir, responder: responder)
    }

    /// 预置相册与单视频资产(交付文件落临时目录;openSeekable 真 open)。
    private func seedVideo(
        _ service: FakeRenditionPhotoService,
        id: String, filename: String, album: String, content: Data,
    ) {
        let url = URL(fileURLWithPath: tempDir() + "/\(id).bin")
        try? content.write(to: url)
        service.albumsResult = [PhotoAlbum(id: "alb-\(album)", name: album, count: 1)]
        service.assetsByAlbum["alb-\(album)"] = [
            PhotoAsset(id: id, filename: filename, creationDateMs: 0,
                       resourceSize: Int64(content.count)),
        ]
        service.openFiles[id] = url
    }

    private static func openEvent(_ id: UInt64, path: String, startAt: UInt64) -> IpcEvent {
        .renditionOpen(requestId: id, path: path, startAtMs: startAt)
    }

    // MARK: 事件解析(形状钉)

    func testEventParsingRenditionFrames() {
        XCTAssertEqual(
            IpcEvent.parse(
                method: "fs.rendition_open",
                params: .object([
                    "request_id": .uint(9), "path": .string("/收件箱/视频.mp4"), "start_at": .uint(0),
                ])),
            .renditionOpen(requestId: 9, path: "/收件箱/视频.mp4", startAtMs: 0))
        XCTAssertEqual(
            IpcEvent.parse(
                method: "fs.rendition_cancel",
                params: .object(["request_id": .uint(100), "session": .uint(9)])),
            .renditionCancel(requestId: 100, session: 9))
        // 畸形参数 = 帧丢弃(类型化事件在解析面收口——缺 path/start_at/
        // request_id/session 各形态;Kotlin 缺键拒答/静默同语义)
        XCTAssertNil(IpcEvent.parse(method: "fs.rendition_open", params: .object([:])))
        XCTAssertNil(IpcEvent.parse(
            method: "fs.rendition_open",
            params: .object(["request_id": .uint(1), "path": .string("/x")])))
        XCTAssertNil(IpcEvent.parse(
            method: "fs.rendition_open",
            params: .object(["request_id": .uint(1), "start_at": .uint(0)])))
        XCTAssertNil(IpcEvent.parse(
            method: "fs.rendition_cancel",
            params: .object(["request_id": .uint(1)])))
        XCTAssertNil(IpcEvent.parse(
            method: "fs.rendition_cancel",
            params: .object(["session": .uint(1)])))
    }

    // MARK: open → 回执先行 + 段回告

    func testOpenAckFirstSegmentsInOrderAndSessionDir() async throws {
        let env = makeEnv()
        seedVideo(env.service, id: "vid-1", filename: "视频.mp4", album: "旅行",
                  content: Data("RAW-VIDEO".utf8))

        await env.responder.handle(Self.openEvent(9, path: "/照片图库/旅行/视频.mp4", startAt: 0))
        await untilFrames(env.channel, 4)

        // 帧 0:回执先行({ok, request_id};段回告必随其后)——字母序字节钉
        let ack = env.channel.frame(at: 0)
        XCTAssertEqual(ack.kind, FdPassKind.renditionData, "rendition 通道 0x0C")
        let ackBody = String(data: ack.payload, encoding: .utf8) ?? ""
        XCTAssertTrue(ackBody.hasPrefix("{\"ok\":true,\"request_id\":9}"), "回执先行: \(ackBody)")

        // 帧 1/2:段回告(index 0 = init 段;键序 index<ok<path<request_id;
        // 路径落会话目录)
        for (i, idx) in [(1, 0), (2, 1)] {
            let frame = env.channel.frame(at: i)
            XCTAssertEqual(frame.kind, FdPassKind.renditionData)
            let body = String(data: frame.payload, encoding: .utf8) ?? ""
            XCTAssertTrue(
                body.hasPrefix("{\"index\":\(idx),\"ok\":true,\"path\":\""),
                "键序钉面: \(body)")
            XCTAssertEqual(frame.json["index"] as? Int, idx)
            XCTAssertEqual(frame.json["ok"] as? Bool, true)
            XCTAssertEqual(
                frame.json["path"] as? String, "\(env.renditionDir)/9/\(idx).m4s",
                "路径落会话目录({renditionDir}/{session}/{index}.m4s)")
            XCTAssertEqual(frame.json["request_id"] as? Int, 9)
        }
        // 段文件真落盘(伪管线产物)
        for idx in 0...1 {
            let written = try Data(
                contentsOf: URL(fileURLWithPath: "\(env.renditionDir)/9/\(idx).m4s"))
            XCTAssertEqual(written, Data([UInt8(idx), 9, 9]))
        }
        // 管线调用面:start_at 透传 + 会话目录即会话键目录 + 显示名透传
        XCTAssertEqual(env.transcoder.callCount, 1)
        XCTAssertEqual(env.transcoder.call(at: 0).startAtMs, 0)
        XCTAssertEqual(env.transcoder.call(at: 0).displayName, "视频.mp4")
        XCTAssertEqual(env.transcoder.call(at: 0).sessionDir, "\(env.renditionDir)/9")

        // 帧 3:管线自然完毕通知(field 修 0x0C eof 形;键序 eof<ok<
        // request_id)——引擎记会话完毕,查看端 playlist 封 ENDLIST
        let eof = env.channel.frame(at: 3)
        XCTAssertEqual(eof.kind, FdPassKind.renditionData)
        let eofBody = String(data: eof.payload, encoding: .utf8) ?? ""
        XCTAssertEqual(eofBody, "{\"eof\":true,\"ok\":true,\"request_id\":9}", "eof 形钉面")
    }

    /// r29:探针在场 → 回执携 duration_ms(引擎 playlist VOD 全列);缺省
    /// 探针 = 旧形(键缺省,下方主测的 hasPrefix 已钉)。
    func testOpenAckCarriesDurationWhenProbed() async throws {
        struct Probe: RenditionDurationProbing {
            func durationMs(fd: Int32, displayName: String) async -> UInt64 { 26_000 }
        }
        let env = makeEnv(durationProbe: Probe())
        seedVideo(env.service, id: "vid-d", filename: "d.mp4", album: "旅行",
                  content: Data("RAW".utf8))

        await env.responder.handle(Self.openEvent(11, path: "/照片图库/旅行/d.mp4", startAt: 0))
        await untilFrames(env.channel, 1)

        let ack = env.channel.frame(at: 0)
        let body = String(data: ack.payload, encoding: .utf8) ?? ""
        XCTAssertEqual(
            body, "{\"duration_ms\":26000,\"ok\":true,\"request_id\":11}",
            "探针时长随回执(字母序): \(body)")
    }

    func testOpenStartAtPassthroughNonZero() async throws {
        let env = makeEnv()
        seedVideo(env.service, id: "vid-2", filename: "A.mp4", album: "相", content: Data("x".utf8))

        await env.responder.handle(Self.openEvent(1, path: "/照片图库/相/A.mp4", startAt: 61_000))
        await env.transcoder.started.wait()
        XCTAssertEqual(env.transcoder.call(at: 0).startAtMs, 61_000)
    }

    // MARK: 拒形矩阵

    func testOpenRefusalMatrix() async throws {
        let env = makeEnv()
        seedVideo(env.service, id: "vid-3", filename: "B.mp4", album: "相", content: Data("x".utf8))
        // 文件臂面:未注册根(开失败)/ 缺席文件 / 目录 fail-closed
        let dirPath = tempDir() + "/子目录"
        try? FileManager.default.createDirectory(atPath: dirPath, withIntermediateDirectories: true)

        await env.responder.handle(Self.openEvent(1, path: "/未注册根/x.mp4", startAt: 0))
        await env.responder.handle(Self.openEvent(2, path: tempDir() + "/缺席.mp4", startAt: 0))
        await env.responder.handle(Self.openEvent(3, path: "/照片图库/无此相册/x.mp4", startAt: 0))
        await env.responder.handle(Self.openEvent(4, path: dirPath, startAt: 0))
        await untilFrames(env.channel, 4)

        for i in 0..<4 {
            let frame = env.channel.frame(at: i)
            XCTAssertEqual(frame.kind, FdPassKind.renditionData, "拒形全部 0x0C")
            XCTAssertEqual(
                frame.json["ok"] as? Bool, false,
                "拒形 ok:false(daemon 落 unsupported 回退原流)")
        }
        let words = (0..<4).map { i -> String in
            (env.channel.frame(at: i).json["error"] as? String) ?? ""
        }
        XCTAssertTrue(words[0].contains("源不可开"), "未注册根词面: \(words[0])")
        XCTAssertTrue(words[1].contains("源不可开"), "缺席文件词面: \(words[1])")
        XCTAssertTrue(words[2].contains("无此相册"), "相册缺席词面: \(words[2])")
        XCTAssertTrue(words[3].contains("目录"), "目录词面 fail-closed: \(words[3])")
        // 拒形不开管线、不建会话目录
        XCTAssertEqual(env.transcoder.callCount, 0, "拒形零管线")
        let leftovers = (try? FileManager.default.contentsOfDirectory(atPath: env.renditionDir)) ?? []
        XCTAssertEqual(leftovers.count, 0, "拒形零会话目录")
    }

    // MARK: 拒答形字节序(空 path 退化面;error<ok<request_id 字母序)

    func testRefusePayloadKeyOrder() async throws {
        let env = makeEnv()
        await env.responder.handle(Self.openEvent(9, path: "", startAt: 0))
        await untilFrames(env.channel, 1)

        let frame = env.channel.frame(at: 0)
        let body = String(data: frame.payload, encoding: .utf8) ?? ""
        XCTAssertTrue(body.hasPrefix("{\"error\":"), "拒答形首键 = error: \(body)")
        XCTAssertTrue(body.contains(",\"ok\":false,\"request_id\":9}"), "拒答形键序钉面: \(body)")
        XCTAssertTrue(body.contains("源不可开"), "空 path 走文件臂拒答: \(body)")
        XCTAssertEqual(env.transcoder.callCount, 0, "拒形零管线")
    }

    // MARK: 回执未受理

    func testOpenAckNotAcceptedNoPipeline() async throws {
        let env = makeEnv()
        env.channel.ackOk = false // daemon 未受理(超时/帧丢失)
        seedVideo(env.service, id: "vid-4", filename: "C.mp4", album: "相", content: Data("x".utf8))

        await env.responder.handle(Self.openEvent(5, path: "/照片图库/相/C.mp4", startAt: 0))
        await untilFrames(env.channel, 1)
        try? await Task.sleep(nanoseconds: 50_000_000) // 管线不起(异步面缺席窗)

        XCTAssertEqual(env.channel.count, 1, "仅回执帧,零段回告")
        XCTAssertEqual(env.transcoder.callCount, 0, "不开管线(白烧电防线)")
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: "\(env.renditionDir)/5"),
            "会话目录即清")
    }

    // MARK: 管线失败

    func testPipelineFailureRefuses() async throws {
        let env = makeEnv(transcoder: FakeTranscoder(mode: .fail))
        seedVideo(env.service, id: "vid-5", filename: "D.mp4", album: "相", content: Data("x".utf8))

        await env.responder.handle(Self.openEvent(7, path: "/照片图库/相/D.mp4", startAt: 0))
        await untilFrames(env.channel, 2)

        let ackBody = String(data: env.channel.frame(at: 0).payload, encoding: .utf8) ?? ""
        XCTAssertTrue(ackBody.contains("\"ok\":true,\"request_id\":7"), "回执先行")
        let fail = env.channel.frame(at: 1)
        XCTAssertEqual(fail.kind, FdPassKind.renditionData)
        XCTAssertEqual(fail.json["ok"] as? Bool, false)
        let word = (fail.json["error"] as? String) ?? ""
        XCTAssertTrue(word.contains("编码器初始化失败"), "失败词面透传: \(word)")
    }

    // MARK: cancel

    func testCancelFlagDirCleanAckIdempotent() async throws {
        let env = makeEnv(transcoder: FakeTranscoder(mode: .hang))
        seedVideo(env.service, id: "vid-6", filename: "E.mp4", album: "相", content: Data("x".utf8))

        await env.responder.handle(Self.openEvent(9, path: "/照片图库/相/E.mp4", startAt: 0))
        await env.transcoder.started.wait()
        // cancel:置旗 → 清目录 → 回执
        await env.responder.handle(.renditionCancel(requestId: 100, session: 9))
        XCTAssertTrue(env.transcoder.call(at: 0).isCancelled(), "取消旗对管线可见")
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: "\(env.renditionDir)/9"),
            "段目录即清")
        await untilFrames(env.channel, 2) // hang 管线零段回告:回执×2(open+cancel)
        let cancelAck = env.channel.frame(at: 1)
        let body = String(data: cancelAck.payload, encoding: .utf8) ?? ""
        XCTAssertTrue(body.contains("\"ok\":true,\"request_id\":100"), "cancel 回执: \(body)")
        // 未知会话幂等照成(残件自愈面)
        await env.responder.handle(.renditionCancel(requestId: 101, session: 55))
        await untilFrames(env.channel, 3)
        let idem = env.channel.frame(at: 2)
        XCTAssertTrue(
            (String(data: idem.payload, encoding: .utf8) ?? "").contains("\"request_id\":101"),
            "未知会话 cancel 回执幂等")
        // 解挂管线收尾(取消面静默,无失败回告噪音)
        env.transcoder.release()
        try? await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(env.channel.count, 3, "取消后管线退出零回告")
    }

    // MARK: 挂载清扫

    func testSweepClearsRenditionServeKeepsDirectory() throws {
        let dir = tempDir()
        // 残件:会话目录 + 散件
        try? FileManager.default.createDirectory(atPath: dir + "/9", withIntermediateDirectories: true)
        try? Data([1]).write(to: URL(fileURLWithPath: dir + "/9/0.m4s"))
        try? Data([2]).write(to: URL(fileURLWithPath: dir + "/99"))

        let env = makeEnv(renditionDirOverride: dir)
        env.responder.sweep()

        XCTAssertFalse(FileManager.default.fileExists(atPath: dir + "/9"), "残件会话目录应被清扫")
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir + "/99"), "残件散件应被清扫")
        var isDir: ObjCBool = false
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: dir, isDirectory: &isDir) && isDir.boolValue,
            "目录本体保留")
    }
}
