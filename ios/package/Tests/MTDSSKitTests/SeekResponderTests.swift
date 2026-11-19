/// SeekResponder 单测(V4-1 T10;伪 PhotoLibraryService + 记录 fd 通道注入):
/// - 事件解析:fs.request_seek → 类型化事件(形状钉);畸形帧丢弃;
/// - 照片臂交付:相册/名 → 批内唯一名重放(0x06 同序)→ openSeekable →
///   0x0D `{request_id}` + 1 fd(经 dup 读回字节验证);
/// - iCloud 未卸载错误形:openSeekable 抛 .icloudNotLocal → 拒开词面恒
///   `icloud_not_local`(稳定词,T25 提示语义);
/// - 拒形矩阵:根/相册段/无此相册/名缺席/权限门外(0x06 同门);
/// - 文件臂:真实路径 FileHandle 直开(交付/缺席拒);
/// - PhotoVpath 拆解(daemon photo_vpath 同口径形状钉)。
import Foundation
@testable import MTDSSKit
import XCTest

final class SeekResponderTests: XCTestCase {

    // MARK: 伪服务(状态可编排)

    private final class FakeSeekPhotoService: PhotoLibraryService, @unchecked Sendable {
        private let lock = NSLock()
        var status: PhotoLibraryPermission = .notDetermined
        var albumsResult: [PhotoAlbum] = []
        var assetsByAlbum: [String: [PhotoAsset]] = [:]
        /// photoId → 打开错误(注入 .icloudNotLocal 等;nil = 正常开)。
        var openErrors: [String: Error] = [:]
        /// photoId → 交付文件 URL(正常开面)。
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
            throw SeekOpenError.unavailable("SeekResponderTests 伪服务不导出")
        }

        func openSeekable(photoId: String) async throws -> FileHandle {
            lock.lock()
            openCalls.append(photoId)
            let failure = openErrors[photoId]
            let url = openFiles[photoId]
            lock.unlock()
            if let failure { throw failure }
            guard let url else {
                throw SeekOpenError.unavailable("无交付文件: \(photoId)")
            }
            return try FileHandle(forReadingFrom: url)
        }
    }

    // MARK: 记录 fd 通道(帧面 + fd 面断言)

    private final class RecordingFdChannel: FdPassFdAnswering, @unchecked Sendable {
        struct Sent {
            let kind: UInt8
            let json: [String: Any]
            /// 随帧 fd(dup 存证——应答器交付即关本侧,存证读面用副本)。
            let fd: Int32?
        }

        private let lock = NSLock()
        private(set) var frames: [Sent] = []
        /// 注入的发送错误(send 抛错面)。
        var sendError: Error?

        func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
            try await send(kind: kind, payload: payload, fileDescriptor: nil)
        }

        func send(kind: UInt8, payload: Data, fileDescriptor: Int32?) async throws -> FdPassFrame {
            lock.lock()
            if let sendError {
                lock.unlock()
                throw sendError
            }
            let json = (try? JSONSerialization.jsonObject(with: payload) as? [String: Any]) ?? [:]
            frames.append(Sent(kind: kind, json: json, fd: fileDescriptor.map { Darwin.dup($0) }))
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

        /// 读取已收 fd 的内容(存证 dup;从当前偏移 0 读到底)。
        func readFdContent(at index: Int) -> Data? {
            lock.lock(); defer { lock.unlock() }
            guard let fd = frames[index].fd else { return nil }
            var out = Data()
            var buf = [UInt8](repeating: 0, count: 4096)
            while true {
                let n = buf.withUnsafeMutableBytes { raw in
                    read(fd, raw.baseAddress!, 4096)
                }
                if n <= 0 { break }
                out.append(contentsOf: buf[0..<n])
            }
            Darwin.close(fd)
            return out
        }
    }

    // MARK: 装配

    private func tempDir() -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("mtdss-seek-test-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.path
    }

    private func makeResponder(
        _ service: FakeSeekPhotoService,
        channel: RecordingFdChannel,
    ) -> SeekResponder {
        SeekResponder(service: service, fdpass: channel)
    }

    private func authorizedService() -> FakeSeekPhotoService {
        let service = FakeSeekPhotoService()
        service.status = .authorized
        return service
    }

    /// 预置相册与单照片资产(交付文件落临时目录)。
    @discardableResult
    private func seedPhoto(
        _ service: FakeSeekPhotoService,
        id: String, filename: String, album: String, content: Data,
    ) -> URL {
        let dir = tempDir()
        let url = URL(fileURLWithPath: dir + "/\(id).bin")
        try? content.write(to: url)
        service.albumsResult = [PhotoAlbum(id: "alb-\(album)", name: album, count: 1)]
        service.assetsByAlbum["alb-\(album)"] = [
            PhotoAsset(id: id, filename: filename, creationDateMs: 0),
        ]
        service.openFiles[id] = url
        return url
    }

    // MARK: 事件解析(形状钉)

    func testEventParsingSeekFrame() {
        let ev = IpcEvent.parse(
            method: "fs.request_seek",
            params: .object(["request_id": .uint(11), "path": .string("/照片图库/相册/x.HEIC")]),
        )
        XCTAssertEqual(ev, .requestSeek(requestId: 11, path: "/照片图库/相册/x.HEIC"))
        // 畸形参数 = 帧丢弃(前向兼容面)
        XCTAssertNil(IpcEvent.parse(method: "fs.request_seek", params: .object([:])))
        XCTAssertNil(IpcEvent.parse(method: "fs.request_seek", params: .object(["request_id": .uint(1)])))
        // 非 seek 事件不误投
        XCTAssertNil(IpcEvent.parse(method: "fs.photo_list", params: .object([:])))
    }

    // MARK: 照片臂交付

    func testPhotoArmDeliversSeekableFd() async throws {
        let service = authorizedService()
        let content = Data("JPEG-BYTES-0123456789".utf8)
        seedPhoto(service, id: "pid-1", filename: "IMG_0001.HEIC", album: "旅行", content: content)
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)

        await responder.handle(.requestSeek(requestId: 7, path: "/照片图库/旅行/IMG_0001.HEIC"))

        XCTAssertEqual(channel.count, 1, "恰一帧应答")
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.seekFD, "seek 通道 0x0D")
        XCTAssertEqual(frame.json["request_id"] as? Int, 7, "交付形 = {request_id}")
        XCTAssertNil(frame.json["error"], "交付形无 error")
        XCTAssertNil(frame.json["ok"], "交付形无 ok(Kotlin seekFdPayload 同形)")
        XCTAssertNotNil(frame.fd, "交付形随帧 1 fd")
        XCTAssertEqual(channel.readFdContent(at: 0), content, "fd 读回字节与源一致")
        XCTAssertEqual(service.openCalls, ["pid-1"], "openSeekable 凭 photoId")
    }

    /// 批内唯一名重放(PhotoResponder 同序:原始名优先、撞名 ` (n)` 顺延)
    /// ——daemon 侧树名即此处名,命中即开第二枚资产。
    func testPhotoArmUniqueNameReplay() async throws {
        let service = authorizedService()
        let dir = tempDir()
        let urlA = URL(fileURLWithPath: dir + "/a.bin")
        let urlB = URL(fileURLWithPath: dir + "/b.bin")
        try Data("FIRST".utf8).write(to: urlA)
        try Data("SECOND".utf8).write(to: urlB)
        service.albumsResult = [PhotoAlbum(id: "alb", name: "相册A", count: 2)]
        service.assetsByAlbum["alb"] = [
            PhotoAsset(id: "p1", filename: "DUP.JPG", creationDateMs: 0),
            PhotoAsset(id: "p2", filename: "DUP.JPG", creationDateMs: 0),
        ]
        service.openFiles = ["p1": urlA, "p2": urlB]
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)

        await responder.handle(.requestSeek(requestId: 3, path: "/照片图库/相册A/DUP (1).JPG"))

        XCTAssertEqual(service.openCalls, ["p2"], "重放命中顺延名 → 第二枚资产")
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.seekFD)
        XCTAssertEqual(channel.readFdContent(at: 0), Data("SECOND".utf8))
    }

    /// 最近项目缺省档(isRecents → albumId nil = 全库)。
    func testPhotoArmRecentsUsesFullLibrary() async throws {
        let service = authorizedService()
        let content = Data("RECENTS-CONTENT".utf8)
        let dir = tempDir()
        let url = URL(fileURLWithPath: dir + "/r.bin")
        try content.write(to: url)
        service.albumsResult = [
            PhotoAlbum(id: "recents", name: "最近项目", count: 1, isRecents: true),
        ]
        service.assetsByAlbum[""] = [
            PhotoAsset(id: "p-r", filename: "IMG_R.HEIC", creationDateMs: 0),
        ]
        service.openFiles["p-r"] = url
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)

        await responder.handle(.requestSeek(requestId: 5, path: "/照片图库/最近项目/IMG_R.HEIC"))

        XCTAssertEqual(service.openCalls, ["p-r"])
        XCTAssertEqual(channel.frame(at: 0).kind, FdPassKind.seekFD)
        XCTAssertEqual(channel.readFdContent(at: 0), content)
    }

    // MARK: iCloud 未卸载错误形(brief 钉死)

    func testIcloudNotLocalRefusesWithStableWord() async {
        let service = authorizedService()
        seedPhoto(service, id: "pid-9", filename: "CLOUD.HEIC", album: "旅行", content: Data("x".utf8))
        service.openErrors["pid-9"] = SeekOpenError.icloudNotLocal
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)

        await responder.handle(.requestSeek(requestId: 21, path: "/照片图库/旅行/CLOUD.HEIC"))

        XCTAssertEqual(channel.count, 1)
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.seekFD)
        XCTAssertEqual(frame.json["ok"] as? Bool, false, "拒开形 ok:false")
        XCTAssertEqual(frame.json["error"] as? String, "icloud_not_local", "稳定词钉面(T25 提示语义)")
        XCTAssertNil(frame.fd, "拒开形无 fd")
        XCTAssertNil(frame.json["path"], "拒开形无 path")
    }

    /// SeekOpenError.word 投影(稳定词/透传 detail)。
    func testSeekOpenErrorWordProjection() {
        XCTAssertEqual(SeekOpenError.icloudNotLocal.word, "icloud_not_local")
        XCTAssertEqual(SeekOpenError.unavailable("资产已删除").word, "资产已删除")
    }

    // MARK: 拒形矩阵

    func testPhotoArmRefusalMatrix() async {
        let service = authorizedService()
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)

        // 根("")
        await responder.handle(.requestSeek(requestId: 1, path: "/照片图库"))
        // 根带尾斜杠(拆解归 "" 同拒)
        await responder.handle(.requestSeek(requestId: 2, path: "/照片图库/"))
        // 相册段(无文件名)
        await responder.handle(.requestSeek(requestId: 3, path: "/照片图库/旅行"))
        // 无此相册
        await responder.handle(.requestSeek(requestId: 4, path: "/照片图库/缺相册/x.HEIC"))
        // 相册在场、名缺席
        service.albumsResult = [PhotoAlbum(id: "alb", name: "旅行", count: 0)]
        await responder.handle(.requestSeek(requestId: 5, path: "/照片图库/旅行/缺.HEIC"))

        XCTAssertEqual(channel.count, 5, "五路各一拒帧")
        for i in 0..<5 {
            let frame = channel.frame(at: i)
            XCTAssertEqual(frame.kind, FdPassKind.seekFD)
            XCTAssertEqual(frame.json["ok"] as? Bool, false, "拒开形 ok:false")
            XCTAssertNotNil(frame.json["error"], "拒形必携词面: \(frame.json)")
            XCTAssertNil(frame.fd, "拒形无 fd")
            XCTAssertNil(frame.json["path"], "拒形无 path")
        }
        XCTAssertTrue(service.openCalls.isEmpty, "拒形不得触 openSeekable")
        let words = (0..<5).map { channel.frame(at: $0).json["error"] as? String ?? "" }
        XCTAssertTrue(words[0].contains("根不可 seek"), "根拒词: \(words[0])")
        XCTAssertTrue(words[2].contains("相册不可 seek"), "相册段拒词: \(words[2])")
        XCTAssertTrue(words[3].contains("无此相册"), "缺席相册拒词: \(words[3])")
        XCTAssertTrue(words[4].contains("源已不存在"), "缺席名拒词: \(words[4])")
    }

    /// 权限门(0x06 同门):notDetermined / denied 一律拒,不触 openSeekable。
    func testPhotoArmPermissionGate() async {
        let service = FakeSeekPhotoService() // notDetermined
        seedPhoto(service, id: "p", filename: "A.HEIC", album: "相", content: Data("x".utf8))
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)
        await responder.handle(.requestSeek(requestId: 1, path: "/照片图库/相/A.HEIC"))
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, false)
        XCTAssertEqual(channel.frame(at: 0).json["error"] as? String, "照片权限未授予")
        XCTAssertTrue(service.openCalls.isEmpty)

        service.status = .denied
        await responder.handle(.requestSeek(requestId: 2, path: "/照片图库/相/A.HEIC"))
        XCTAssertEqual(channel.frame(at: 1).json["error"] as? String, "照片权限未授予")
    }

    /// limited = 子集内照常(0x06 同门)。
    func testPhotoArmLimitedIsAuthorized() async throws {
        let service = FakeSeekPhotoService()
        service.status = .limited
        let content = Data("LIMITED".utf8)
        seedPhoto(service, id: "p-l", filename: "L.HEIC", album: "相", content: content)
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)
        await responder.handle(.requestSeek(requestId: 1, path: "/照片图库/相/L.HEIC"))
        XCTAssertEqual(channel.frame(at: 0).kind, FdPassKind.seekFD)
        XCTAssertEqual(channel.readFdContent(at: 0), content)
    }

    /// 服务面抛错(相册拉取失败)→ 拒开词面透传。
    func testPhotoArmServiceErrorRefuses() async {
        let service = authorizedService()
        let channel = RecordingFdChannel()
        // 伪服务 albums() 抛错:以不存在的交付文件注(走 openSeekable 失败臂
        // 同拒面)——这里直接注 assets 缺席即拒,另以 openErrors 注 RpcError
        // 形错误验证透传。
        seedPhoto(service, id: "p-e", filename: "E.HEIC", album: "相", content: Data("x".utf8))
        service.openErrors["p-e"] = RpcError(code: 1, message: "boom")
        let responder = makeResponder(service, channel: channel)
        await responder.handle(.requestSeek(requestId: 1, path: "/照片图库/相/E.HEIC"))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNotNil(frame.json["error"])
    }

    // MARK: 文件臂(授权根沙箱真实路径)

    func testFileArmDeliversFdForRealPath() async throws {
        let service = authorizedService()
        let content = Data("PLAIN-FILE-BYTES".utf8)
        let dir = tempDir()
        let path = dir + "/doc.bin"
        try content.write(to: URL(fileURLWithPath: path))
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)

        // 非照片前缀 = 文件臂(真实沙箱路径;iOS 授权根面)
        await responder.handle(.requestSeek(requestId: 9, path: path))

        XCTAssertEqual(service.openCalls.isEmpty, true, "非照片 vpath 不触照片臂")
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.seekFD)
        XCTAssertEqual(frame.json["request_id"] as? Int, 9)
        XCTAssertEqual(channel.readFdContent(at: 0), content, "文件臂 fd 读回字节一致")
    }

    func testFileArmRefusesMissingPath() async {
        let service = authorizedService()
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)
        await responder.handle(.requestSeek(requestId: 2, path: "/收件箱/缺.bin"))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNil(frame.fd)
        XCTAssertTrue(frame.json["error"] != nil)
    }

    func testFileArmRefusesDirectory() async throws {
        let service = authorizedService()
        let dir = tempDir()
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)
        await responder.handle(.requestSeek(requestId: 3, path: dir))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false, "目录不可 seek(仅文件)")
        XCTAssertNil(frame.fd)
    }

    // MARK: PhotoVpath 拆解(纯词法,Kotlin photoVpathSub 同构形状钉)

    func testPhotoVpathSub() {
        XCTAssertEqual(PhotoVpath.sub("/照片图库"), "")
        XCTAssertEqual(PhotoVpath.sub("/照片图库/"), "")
        XCTAssertEqual(PhotoVpath.sub("/照片图库//最近项目"), "最近项目")
        XCTAssertEqual(PhotoVpath.sub("/照片图库/最近项目/IMG_1.HEIC"), "最近项目/IMG_1.HEIC")
        XCTAssertEqual(PhotoVpath.sub("/照片图库/最近项目/IMG_1.HEIC/"), "最近项目/IMG_1.HEIC")
        XCTAssertNil(PhotoVpath.sub("/照片图库x"))
        XCTAssertNil(PhotoVpath.sub("/照片图库x/y"))
        // 逃逸形不在此拒(纯词法切分):落照片臂按无此相册拒(下方用例钉)
        XCTAssertEqual(PhotoVpath.sub("/照片图库/../etc"), "../etc")
        XCTAssertNil(PhotoVpath.sub("/收件箱/x.bin"))
        XCTAssertNil(PhotoVpath.sub(""))
    }

    /// 逃逸形防御面:`/照片图库/../etc` → 相册 ".." 缺席拒开(fail-closed;
    /// daemon photo_vpath 规范化先行,此形态不会到 App——纯纵深)。
    func testPhotoVpathEscapeFormRefusesAtAlbumLookup() async {
        let service = authorizedService()
        let channel = RecordingFdChannel()
        let responder = makeResponder(service, channel: channel)
        await responder.handle(.requestSeek(requestId: 31, path: "/照片图库/../etc"))
        XCTAssertEqual(channel.count, 1)
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertEqual(frame.json["error"] as? String, "无此相册: ..")
        XCTAssertTrue(service.openCalls.isEmpty)
    }
}
