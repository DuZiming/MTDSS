/// ThumbResponder 单测(V4-1 T11;伪 PhotoLibraryService + 记录通道 + 伪
/// 编码器注入——macOS 宿主不可触 AVAssetImageGenerator/UIImage 真解码,
/// 解码正确性归实机冒烟):
/// - 事件解析:fs.request_thumb → 类型化事件(形状钉);畸形帧丢弃;
/// - 照片臂交付:相册/名 → 批内唯一名重放(0x06 同序)→ openSeekable →
///   编码 → 0x0B `{ok, path, request_id}` 字母序字节钉(落盘 thumb-serve,
///   **不删**——引擎读毕 unlink,挂载清扫兜底);
/// - w 钳制:0 → 1、巨值 → ``thumbMaxWidth``(与 daemon THUMB_MAX_WIDTH
///   同值);
/// - iCloud 未卸载错误形:openSeekable 抛 .icloudNotLocal → 拒词恒
///   `icloud_not_local`(稳定词透传,引擎落既有回退);
/// - 拒形矩阵:根/相册段/无此相册/名缺席/编码失败(nil/空)/权限门外/
///   服务错透传;
/// - 文件臂:真实路径交付/缺席拒/目录拒(DefaultSeekFiles fail-closed);
/// - 挂载清扫:thumb-serve 整目录清空(残件自愈;与引擎 thumb-tmp 分立,
///   目录本体保留)。
import Foundation
@testable import MTDSSKit
import XCTest

final class ThumbResponderTests: XCTestCase {

    // MARK: 伪服务(状态可编排;SeekResponderTests 同构)

    private final class FakeThumbPhotoService: PhotoLibraryService, @unchecked Sendable {
        private let lock = NSLock()
        var status: PhotoLibraryPermission = .notDetermined
        var albumsResult: [PhotoAlbum] = []
        var assetsByAlbum: [String: [PhotoAsset]] = [:]
        /// photoId → 打开错误(注入 .icloudNotLocal 等;nil = 正常开)。
        var openErrors: [String: Error] = [:]
        /// photoId → 交付文件 URL(正常开面)。
        var openFiles: [String: URL] = [:]
        /// photoId → 系统缩略图字节(field r26g #1 快径注入面;nil/空 =
        /// 不命中,落全量路——协议缺省同语义)。
        var thumbDataBy: [String: Data] = [:]
        private(set) var openCalls: [String] = []
        private(set) var thumbCalls: [String] = []

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
            throw SeekOpenError.unavailable("ThumbResponderTests 伪服务不导出")
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

        func thumbData(photoId: String, w: Int) async -> Data? {
            lock.lock()
            thumbCalls.append(photoId)
            let hit = thumbDataBy[photoId]
            lock.unlock()
            return hit
        }
    }

    // MARK: 伪编码器(记录调用参数 + 定额产物)

    private final class FakeThumbEncoder: ThumbEncoder, @unchecked Sendable {
        /// 恒回产物(nil/空 = 拒答面注入)。
        let result: Data?
        private let lock = NSLock()
        private(set) var calls: [(fd: Int32, displayName: String, w: Int)] = []

        init(result: Data?) {
            self.result = result
        }

        func jpeg(fd: Int32, displayName: String, w: Int) async -> Data? {
            lock.lock()
            calls.append((fd: fd, displayName: displayName, w: w))
            lock.unlock()
            return result
        }

        var callCount: Int {
            lock.lock(); defer { lock.unlock() }
            return calls.count
        }

        func call(at index: Int) -> (fd: Int32, displayName: String, w: Int) {
            lock.lock(); defer { lock.unlock() }
            return calls[index]
        }
    }

    // MARK: 记录通道(帧面断言;SeekResponderTests RecordingFdChannel 同构
    // 去 fd 面——thumb 通道 0x0B 恒无 fd)

    private final class RecordingChannel: FdPassAnswering, @unchecked Sendable {
        struct Sent {
            let kind: UInt8
            /// 原始 payload 字节(字母序字节钉面)。
            let payload: Data
            let json: [String: Any]
        }

        private let lock = NSLock()
        private(set) var frames: [Sent] = []
        /// 注入的发送错误(send 抛错面)。
        var sendError: Error?

        func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
            lock.lock()
            if let sendError {
                lock.unlock()
                throw sendError
            }
            let json = (try? JSONSerialization.jsonObject(with: payload) as? [String: Any]) ?? [:]
            frames.append(Sent(kind: kind, payload: payload, json: json))
            lock.unlock()
            return FdPassFrame(
                kind: FdPassKind.ack,
                payload: Data("{\"ok\":true,\"request_id\":0}".utf8))
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

    // MARK: 装配

    private func tempDir() -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("mtdss-thumb-test-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.path
    }

    private func makeResponder(
        _ service: FakeThumbPhotoService,
        channel: RecordingChannel,
        encoder: FakeThumbEncoder,
        thumbDir: String,
    ) -> ThumbResponder {
        ThumbResponder(
            service: service,
            fdpass: channel,
            thumbDir: thumbDir,
            encoder: encoder)
    }

    private func authorizedService() -> FakeThumbPhotoService {
        let service = FakeThumbPhotoService()
        service.status = .authorized
        return service
    }

    /// 预置相册与单照片资产(交付文件落临时目录;openSeekable 真 open)。
    @discardableResult
    private func seedPhoto(
        _ service: FakeThumbPhotoService,
        id: String, filename: String, album: String, content: Data,
        recents: Bool = false,
    ) -> URL {
        let dir = tempDir()
        let url = URL(fileURLWithPath: dir + "/\(id).bin")
        try? content.write(to: url)
        service.albumsResult = [
            PhotoAlbum(id: recents ? "recents" : "alb-\(album)", name: album, count: 1,
                       isRecents: recents),
        ]
        service.assetsByAlbum[recents ? "" : "alb-\(album)"] = [
            PhotoAsset(
                id: id, filename: filename, creationDateMs: 0,
                resourceSize: Int64(content.count)),
        ]
        service.openFiles[id] = url
        return url
    }

    private static let jpegFixture = Data("JPEG-BYTES-0123456789".utf8)

    // MARK: 事件解析(形状钉)

    func testEventParsingThumbFrame() {
        let ev = IpcEvent.parse(
            method: "fs.request_thumb",
            params: .object([
                "request_id": .uint(11), "path": .string("/收件箱/图.jpg"), "w": .uint(256),
            ]),
        )
        XCTAssertEqual(ev, .requestThumb(requestId: 11, path: "/收件箱/图.jpg", w: 256))
        // 畸形参数 = 帧丢弃(前向兼容面;Kotlin handle 缺键拒答/丢弃同语义,
        // 类型化事件在解析面收口)
        XCTAssertNil(IpcEvent.parse(method: "fs.request_thumb", params: .object([:])))
        XCTAssertNil(IpcEvent.parse(method: "fs.request_thumb", params: .object(["request_id": .uint(1)])))
        XCTAssertNil(IpcEvent.parse(
            method: "fs.request_thumb",
            params: .object(["request_id": .uint(1), "path": .string("/x")])))
        // 非 thumb 事件不误投
        XCTAssertNil(IpcEvent.parse(method: "fs.request_seek", params: .object([:])))
    }

    // MARK: 照片臂交付

    func testPhotoArmDeliversInlineJpeg() async throws {
        let service = authorizedService()
        seedPhoto(service, id: "pid-1", filename: "IMG_0001.HEIC", album: "旅行",
                  content: Data("RAW-ORIGINAL".utf8))
        let thumbDir = tempDir()
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: thumbDir)

        await responder.handle(.requestThumb(requestId: 7, path: "/照片图库/旅行/IMG_0001.HEIC", w: 256))

        XCTAssertEqual(channel.count, 1, "恰一帧应答")
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.thumbData, "thumb 通道 0x0B")
        XCTAssertEqual(frame.json["ok"] as? Bool, true, "交付形 ok:true")
        XCTAssertEqual(frame.json["request_id"] as? Int, 7)
        XCTAssertNil(frame.json["error"], "交付形无 error")
        XCTAssertEqual(
            frame.json["jpeg_b64"] as? String, Self.jpegFixture.base64EncodedString(),
            "内联 JPEG = 编码器产物(r26f 修3 撤临时文件三跳)")
        XCTAssertNil(frame.json["path"], "内联形态无 path")
        XCTAssertEqual(service.openCalls, ["pid-1"], "openSeekable 凭 photoId")
        XCTAssertEqual(encoder.callCount, 1)
        XCTAssertEqual(encoder.call(at: 0).displayName, "IMG_0001.HEIC", "分型/命名凭据 = 原始显示名")
        XCTAssertEqual(encoder.call(at: 0).w, 256, "w 原样到达编码器(事件面已带)")
    }

    /// 交付形键序钉:sortedKeys 字母序 `{jpeg_b64,ok,request_id}`(Kotlin
    /// FdWire.exportReplyPayload 同序,daemon parse_thumb_reply 消费面;中
    /// 文/二进制面转义形态 = JSONSerialization 家族既有策略,不钉)。
    func testDeliverPayloadBytesAlphabeticalOrder() async throws {
        let service = authorizedService()
        seedPhoto(service, id: "pid-2", filename: "A.HEIC", album: "相", content: Data("x".utf8))
        let thumbDir = tempDir()
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: thumbDir)

        await responder.handle(.requestThumb(requestId: 3, path: "/照片图库/相/A.HEIC", w: 64))

        let frame = channel.frame(at: 0)
        let body = String(data: frame.payload, encoding: .utf8) ?? ""
        XCTAssertTrue(body.hasPrefix("{\"jpeg_b64\":\""), "交付形首键 jpeg_b64: \(body)")
        XCTAssertTrue(body.hasSuffix(",\"ok\":true,\"request_id\":3}"), "尾键序 ok<request_id: \(body)")
        XCTAssertEqual(frame.json["ok"] as? Bool, true)
        XCTAssertEqual(frame.json["jpeg_b64"] as? String, Self.jpegFixture.base64EncodedString())
    }

    /// 拒答形键序钉:`{"error":...,"ok":false,"request_id":N}`(字母序;error
    /// 词面中文的转义形态属序列化策略,daemon JSON 解析与转义形态无关——只
    /// 钉键序与结构)。
    func testRefusePayloadKeyOrder() async throws {
        let service = FakeThumbPhotoService() // notDetermined → 权限拒
        seedPhoto(service, id: "pid-3", filename: "B.HEIC", album: "相", content: Data("x".utf8))
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 5, path: "/照片图库/相/B.HEIC", w: 64))

        let frame = channel.frame(at: 0)
        let body = String(data: frame.payload, encoding: .utf8) ?? ""
        XCTAssertTrue(body.hasPrefix("{\"error\":"), "拒答形首键 = error: \(body)")
        XCTAssertTrue(body.contains(",\"ok\":false,\"request_id\":5}"), "拒答形键序钉面: \(body)")
    }

    /// 批内唯一名重放(PhotoResponder 同序:原始名优先、撞名 ` (n)` 顺延)
    /// ——daemon 侧树名即此处名,命中即编第二枚资产。
    func testPhotoArmUniqueNameReplay() async throws {
        let service = authorizedService()
        let dir = tempDir()
        let urlA = URL(fileURLWithPath: dir + "/a.bin")
        let urlB = URL(fileURLWithPath: dir + "/b.bin")
        try Data("FIRST".utf8).write(to: urlA)
        try Data("SECOND".utf8).write(to: urlB)
        service.albumsResult = [PhotoAlbum(id: "alb", name: "相册A", count: 2)]
        service.assetsByAlbum["alb"] = [
            PhotoAsset(id: "p1", filename: "DUP.JPG", creationDateMs: 0, resourceSize: 5),
            PhotoAsset(id: "p2", filename: "DUP.JPG", creationDateMs: 0, resourceSize: 6),
        ]
        service.openFiles = ["p1": urlA, "p2": urlB]
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 4, path: "/照片图库/相册A/DUP (1).JPG", w: 64))

        XCTAssertEqual(service.openCalls, ["p2"], "重放命中顺延名 → 第二枚资产")
        XCTAssertEqual(encoder.call(at: 0).displayName, "DUP.JPG", "显示名 = 原始名(Android asset.name 同面)")
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, true)
    }

    /// 最近项目缺省档(isRecents → albumId nil = 全库)。
    func testPhotoArmRecentsUsesFullLibrary() async throws {
        let service = authorizedService()
        seedPhoto(service, id: "p-r", filename: "IMG_R.HEIC", album: "最近项目",
                  content: Data("RECENTS".utf8), recents: true)
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 5, path: "/照片图库/最近项目/IMG_R.HEIC", w: 64))

        XCTAssertEqual(service.openCalls, ["p-r"])
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, true)
    }

    // MARK: w 钳制(daemon THUMB_MAX_WIDTH 同值)

    func testWidthClampedToEncoderBounds() async {
        let service = authorizedService()
        seedPhoto(service, id: "p-w", filename: "W.HEIC", album: "相", content: Data("x".utf8))
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 1, path: "/照片图库/相/W.HEIC", w: 0))
        await responder.handle(.requestThumb(requestId: 2, path: "/照片图库/相/W.HEIC", w: 100_000))
        await responder.handle(.requestThumb(requestId: 3, path: "/照片图库/相/W.HEIC", w: 64))

        XCTAssertEqual(encoder.callCount, 3, "三请求皆达编码器(钳制不丢请求)")
        XCTAssertEqual(encoder.call(at: 0).w, 1, "下界钳 1(0 无意义)")
        XCTAssertEqual(encoder.call(at: 1).w, 1024, "上界钳 1024(daemon THUMB_MAX_WIDTH 同值)")
        XCTAssertEqual(encoder.call(at: 2).w, 64, "界内原样")
        XCTAssertEqual(channel.count, 3, "三请求皆交付")
    }

    // MARK: iCloud 未卸载错误形(稳定词透传)

    func testIcloudNotLocalRefusesWithStableWord() async {
        let service = authorizedService()
        seedPhoto(service, id: "pid-9", filename: "CLOUD.HEIC", album: "旅行", content: Data("x".utf8))
        service.openErrors["pid-9"] = SeekOpenError.icloudNotLocal
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 21, path: "/照片图库/旅行/CLOUD.HEIC", w: 64))

        XCTAssertEqual(channel.count, 1)
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.thumbData)
        XCTAssertEqual(frame.json["ok"] as? Bool, false, "拒答形 ok:false")
        XCTAssertEqual(frame.json["error"] as? String, "icloud_not_local", "稳定词钉面(词面透传)")
        XCTAssertNil(frame.json["path"], "拒答形无 path")
    }

    // MARK: 拒形矩阵(不触 openSeekable/编码器)

    func testPhotoArmRefusalMatrix() async {
        let service = authorizedService()
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: tempDir())

        // 根("")
        await responder.handle(.requestThumb(requestId: 1, path: "/照片图库", w: 64))
        // 根带尾斜杠(拆解归 "" 同拒)
        await responder.handle(.requestThumb(requestId: 2, path: "/照片图库/", w: 64))
        // 相册段(无文件名)
        await responder.handle(.requestThumb(requestId: 3, path: "/照片图库/旅行", w: 64))
        // 无此相册
        await responder.handle(.requestThumb(requestId: 4, path: "/照片图库/缺相册/x.HEIC", w: 64))
        // 相册在场、名缺席
        service.albumsResult = [PhotoAlbum(id: "alb", name: "旅行", count: 0)]
        await responder.handle(.requestThumb(requestId: 5, path: "/照片图库/旅行/缺.HEIC", w: 64))
        // 逃逸形(纯词法切分 → 相册 ".." 缺席拒;daemon photo_vpath 规范化
        // 先行,此形态本就路由不到 App——纯纵深)
        await responder.handle(.requestThumb(requestId: 6, path: "/照片图库/../etc", w: 64))

        XCTAssertEqual(channel.count, 6, "六路各一拒帧")
        for i in 0..<6 {
            let frame = channel.frame(at: i)
            XCTAssertEqual(frame.kind, FdPassKind.thumbData)
            XCTAssertEqual(frame.json["ok"] as? Bool, false, "拒答形 ok:false")
            XCTAssertNotNil(frame.json["error"], "拒形必携词面: \(frame.json)")
            XCTAssertNil(frame.json["path"], "拒形无 path")
        }
        XCTAssertTrue(service.openCalls.isEmpty, "拒形不得触 openSeekable")
        XCTAssertEqual(encoder.callCount, 0, "拒形不得触编码器")
        let words = (0..<6).map { channel.frame(at: $0).json["error"] as? String ?? "" }
        XCTAssertTrue(words[0].contains("根不可出图"), "根拒词: \(words[0])")
        XCTAssertTrue(words[2].contains("相册不可出图"), "相册段拒词: \(words[2])")
        XCTAssertTrue(words[3].contains("无此相册"), "缺席相册拒词: \(words[3])")
        XCTAssertTrue(words[4].contains("源已不存在"), "缺席名拒词: \(words[4])")
        XCTAssertEqual(words[5], "无此相册: ..", "逃逸形 = 相册 .. 缺席拒(SeekResponder 同构)")
    }

    /// 权限门(0x06/0x0D 同门):notDetermined / denied 一律拒,不触
    /// openSeekable/编码器。
    func testPhotoArmPermissionGate() async {
        let service = FakeThumbPhotoService() // notDetermined
        seedPhoto(service, id: "p", filename: "A.HEIC", album: "相", content: Data("x".utf8))
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: tempDir())
        await responder.handle(.requestThumb(requestId: 1, path: "/照片图库/相/A.HEIC", w: 64))
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, false)
        XCTAssertEqual(channel.frame(at: 0).json["error"] as? String, "照片权限未授予")
        XCTAssertTrue(service.openCalls.isEmpty)
        XCTAssertEqual(encoder.callCount, 0)

        service.status = .denied
        await responder.handle(.requestThumb(requestId: 2, path: "/照片图库/相/A.HEIC", w: 64))
        XCTAssertEqual(channel.frame(at: 1).json["error"] as? String, "照片权限未授予")
    }

    /// limited = 子集内照常(0x06 同门)。
    func testPhotoArmLimitedIsAuthorized() async throws {
        let service = FakeThumbPhotoService()
        service.status = .limited
        seedPhoto(service, id: "p-l", filename: "L.HEIC", album: "相", content: Data("x".utf8))
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())
        await responder.handle(.requestThumb(requestId: 1, path: "/照片图库/相/L.HEIC", w: 64))
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, true)
    }

    /// 服务面抛错(openSeekable 非 SeekOpenError 形)→ 拒答词面透传。
    func testPhotoArmServiceErrorRefuses() async {
        let service = authorizedService()
        seedPhoto(service, id: "p-e", filename: "E.HEIC", album: "相", content: Data("x".utf8))
        service.openErrors["p-e"] = RpcError(code: 1, message: "boom")
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())
        await responder.handle(.requestThumb(requestId: 1, path: "/照片图库/相/E.HEIC", w: 64))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNotNil(frame.json["error"])
        XCTAssertNil(frame.json["path"])
    }

    // MARK: 编码失败面(nil/空 = 拒答,引擎落既有回退)

    func testEncoderNilRefuses() async {
        let service = authorizedService()
        seedPhoto(service, id: "p-n", filename: "N.HEIC", album: "相", content: Data("x".utf8))
        let thumbDir = tempDir()
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: nil), thumbDir: thumbDir)

        await responder.handle(.requestThumb(requestId: 1, path: "/照片图库/相/N.HEIC", w: 64))

        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNotNil(frame.json["error"])
        XCTAssertNil(frame.json["path"])
        XCTAssertTrue(service.openCalls.isEmpty == false, "拒前已开源(解码窗即源生命周期)")
    }

    func testEncoderEmptyBytesRefuse() async {
        let service = authorizedService()
        seedPhoto(service, id: "p-z", filename: "Z.HEIC", album: "相", content: Data("x".utf8))
        let thumbDir = tempDir()
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Data()), thumbDir: thumbDir)

        await responder.handle(.requestThumb(requestId: 2, path: "/照片图库/相/Z.HEIC", w: 64))

        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false, "空字节 = 生成失败面(同拒答)")
        XCTAssertNil(frame.json["path"])
    }

    // MARK: 文件臂(授权根沙箱真实路径)

    func testFileArmDeliversForRealPath() async throws {
        let service = authorizedService()
        let thumbDir = tempDir()
        let sourceDir = tempDir()
        let path = sourceDir + "/doc.jpg"
        try Data("PLAIN-FILE".utf8).write(to: URL(fileURLWithPath: path))
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: thumbDir)

        // 非照片前缀 = 文件臂(真实沙箱路径;iOS 授权根面)
        await responder.handle(.requestThumb(requestId: 9, path: path, w: 128))

        XCTAssertTrue(service.openCalls.isEmpty, "非照片 vpath 不触照片臂")
        XCTAssertTrue(service.thumbCalls.isEmpty, "非照片 vpath 不触快径")
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.thumbData)
        XCTAssertEqual(frame.json["ok"] as? Bool, true)
        XCTAssertEqual(encoder.call(at: 0).displayName, "doc.jpg", "分型凭据 = 终段名")
        XCTAssertEqual(encoder.call(at: 0).w, 128)
        XCTAssertEqual(
            frame.json["jpeg_b64"] as? String, Self.jpegFixture.base64EncodedString(),
            "内联 JPEG = 编码器产物")
        XCTAssertNil(frame.json["path"], "内联形态无 path")
    }

    func testFileArmRefusesMissingPath() async {
        let service = authorizedService()
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())
        await responder.handle(.requestThumb(requestId: 2, path: "/收件箱/缺.bin", w: 64))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNotNil(frame.json["error"])
        XCTAssertNil(frame.json["path"])
    }

    func testFileArmRefusesDirectory() async throws {
        let service = authorizedService()
        let dir = tempDir()
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: tempDir())
        await responder.handle(.requestThumb(requestId: 3, path: dir, w: 64))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false, "目录不可出图(DefaultSeekFiles fstat fail-closed)")
        XCTAssertNil(frame.json["path"])
    }

    // MARK: 挂载清扫(thumb-serve 整目录清空;目录本体保留)

    func testSweepClearsThumbServeKeepsDirectory() {
        let thumbDir = tempDir()
        try? Data("STALE-1".utf8).write(to: URL(fileURLWithPath: thumbDir + "/1-stale.jpg"))
        try? Data("STALE-2".utf8).write(to: URL(fileURLWithPath: thumbDir + "/2-stale.jpg"))
        let responder = ThumbResponder(
            service: FakeThumbPhotoService(),
            fdpass: RecordingChannel(),
            thumbDir: thumbDir,
            encoder: FakeThumbEncoder(result: nil))

        responder.sweep()

        let items = try? FileManager.default.contentsOfDirectory(atPath: thumbDir)
        XCTAssertEqual(items, [], "残件自愈(整目录清空): \(items ?? ["<dir-missing>"])")
        XCTAssertTrue(FileManager.default.fileExists(atPath: thumbDir), "目录本体保留")
    }

    /// run 挂载即清扫(PhotoResponder 同纪律)+ 事件消费直通(整合面)。
    func testRunSweepsThenConsumesThumbEvents() async throws {
        let service = authorizedService()
        seedPhoto(service, id: "p-run", filename: "R.HEIC", album: "相", content: Data("x".utf8))
        let thumbDir = tempDir()
        try? Data("STALE".utf8).write(to: URL(fileURLWithPath: thumbDir + "/stale.jpg"))
        let channel = RecordingChannel()
        let responder = makeResponder(
            service, channel: channel, encoder: FakeThumbEncoder(result: Self.jpegFixture),
            thumbDir: thumbDir)

        let stream = AsyncStream<IpcEvent> { cont in
            cont.yield(.requestThumb(requestId: 8, path: "/照片图库/相/R.HEIC", w: 64))
            cont.finish()
        }
        await responder.run(stream)

        let items = try? FileManager.default.contentsOfDirectory(atPath: thumbDir)
        XCTAssertEqual(items, [], "挂载清扫清走残件;内联交付零落盘: \(items ?? [])")
        XCTAssertEqual(channel.count, 1, "事件消费恰一应答")
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, true)
        XCTAssertEqual(
            channel.frame(at: 0).json["jpeg_b64"] as? String,
            Self.jpegFixture.base64EncodedString(), "内联 JPEG = 编码器产物")
    }

    // MARK: 快径前置(field r26g #1 顺序反转)

    /// 照片臂快径先行:thumbData 命中 → 内联交付,**零 openSeekable/零编
    /// 码器**(此前 openThumbSource 无条件先跑 openSeekable——
    /// requestContentEditingInput 全尺寸渲染白付是冷缩略图慢 + 手机卡的最
    /// 大剩余根因)。
    func testFastPathFirstSkipsOpenSeekableAndEncoder() async throws {
        let service = authorizedService()
        seedPhoto(service, id: "pid-f", filename: "F.HEIC", album: "相", content: Data("x".utf8))
        let fast = Data("FAST-SYSTEM-THUMB".utf8)
        service.thumbDataBy["pid-f"] = fast
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 9, path: "/照片图库/相/F.HEIC", w: 256))

        XCTAssertEqual(service.thumbCalls, ["pid-f"], "快径凭 photoId 请求系统缩略图")
        XCTAssertEqual(service.openCalls, [], "快径零 openSeekable(顺序反转钉面)")
        XCTAssertEqual(encoder.callCount, 0, "快径零编码器")
        XCTAssertEqual(channel.count, 1)
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.thumbData)
        XCTAssertEqual(frame.json["ok"] as? Bool, true)
        XCTAssertEqual(frame.json["jpeg_b64"] as? String, fast.base64EncodedString(),
                       "内联 JPEG = 系统缩略图字节")
        XCTAssertNil(frame.json["path"], "内联形态无 path")
    }

    /// 快径不中(thumbData 空)→ 落全量路:openSeekable + 编码器交付。
    func testFastPathMissFallsBackToFullPath() async throws {
        let service = authorizedService()
        seedPhoto(service, id: "pid-m", filename: "M.HEIC", album: "相", content: Data("RAW".utf8))
        service.thumbDataBy["pid-m"] = Data() // 空字节 = 不命中
        let channel = RecordingChannel()
        let encoder = FakeThumbEncoder(result: Self.jpegFixture)
        let responder = makeResponder(service, channel: channel, encoder: encoder, thumbDir: tempDir())

        await responder.handle(.requestThumb(requestId: 10, path: "/照片图库/相/M.HEIC", w: 64))

        XCTAssertEqual(service.thumbCalls, ["pid-m"], "快径已试")
        XCTAssertEqual(service.openCalls, ["pid-m"], "不中 → openSeekable 全量路")
        XCTAssertEqual(encoder.callCount, 1, "不中 → 编码器全量路")
        XCTAssertEqual(channel.count, 1)
        XCTAssertEqual(
            channel.frame(at: 0).json["jpeg_b64"] as? String,
            Self.jpegFixture.base64EncodedString(), "交付 = 编码器产物")
    }
}
