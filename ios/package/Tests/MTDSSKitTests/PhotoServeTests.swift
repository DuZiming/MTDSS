/// PhotoResponder 单测(R7;伪 PhotoLibraryService + 记录通道注入):
/// - 0x06 列表:根 = 相册目录项(children = 照片数);相册名 = 照片文件项
///   (name/size/mtime/photo_id);未知相册拒答(ok:false);
/// - 0x07 导出:原始字节落逐请求子目录 → 回绝对路径(文件实存);
/// - 权限门:authorized/limited 应答;notDetermined/denied 一律 ok:false
///   (0x06/0x07 两帧同门,引擎按桥不可用收口 → 虚拟根隐藏);
/// - 清扫:run 挂载即清空 serve 根(崩溃残件自愈);
/// - 事件解析:fs.photo_list/fs.photo_export → 类型化事件(形状钉);
/// - 条目编解码(JSON 形状钉,与 Rust 解析面逐字段对齐)。
import Foundation
@testable import MTDSSKit
import XCTest

final class PhotoServeTests: XCTestCase {

    // MARK: 伪服务(状态可编排)

    private final class FakePhotoService: PhotoLibraryService, @unchecked Sendable {
        private let lock = NSLock()
        var status: PhotoLibraryPermission = .notDetermined
        var albumsResult: [PhotoAlbum] = []
        var albumsError: Error?
        var assetsByAlbum: [String: [PhotoAsset]] = [:]
        var assetsError: Error?
        var exportError: Error?
        /// 资产元数据(导出命名消费;seed 注入)。
        var assetMeta: [String: (filename: String?, ms: Int64)] = [:]
        private(set) var exportCalls: [([String], String)] = [] // (ids, dirPath)

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
            if let albumsError { throw albumsError }
            return albumsResult
        }

        func assets(albumId: String?) async throws -> [PhotoAsset] {
            lock.lock(); defer { lock.unlock() }
            if let assetsError { throw assetsError }
            return assetsByAlbum[albumId ?? ""] ?? []
        }

        func exportAssets(ids: [String], to directory: URL) async throws -> [URL] {
            lock.lock()
            exportCalls.append((ids, directory.path))
            let failure = exportError
            defer { lock.unlock() }
            if let failure { throw failure }
            return try ids.enumerated().map { index, id in
                let meta = assetMeta[id]
                let name = PhotoAssetNaming.displayName(
                    filename: meta?.filename, creationDateMs: meta?.ms ?? 0, index: index + 1)
                let url = directory.appendingPathComponent(name)
                try Data("PHOTO-\(id)".utf8).write(to: url)
                return url
            }
        }

        func seed(_ a: PhotoAsset) {
            lock.lock(); defer { lock.unlock() }
            assetMeta[a.id] = (a.filename, a.creationDateMs)
        }
    }

    // MARK: 记录通道(帧面断言)

    private final class RecordingChannel: FdPassAnswering, @unchecked Sendable {
        struct Sent: Equatable {
            let kind: UInt8
            let json: [String: Any]

            static func == (lhs: Sent, rhs: Sent) -> Bool {
                lhs.kind == rhs.kind
                    && (try? JSONSerialization.data(withJSONObject: lhs.json))
                        == (try? JSONSerialization.data(withJSONObject: rhs.json))
            }
        }

        private let lock = NSLock()
        private(set) var frames: [Sent] = []

        func send(kind: UInt8, payload: Data) async throws -> FdPassFrame {
            lock.lock(); defer { lock.unlock() }
            let json = (try? JSONSerialization.jsonObject(with: payload) as? [String: Any]) ?? [:]
            frames.append(Sent(kind: kind, json: json))
            return FdPassFrame(kind: FdPassKind.ack, payload: Data(#"{"ok":true}"#.utf8))
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

    // MARK: 文件面记录盒(清扫/请求目录调用断言)

    private final class RecordingFiles: PhotoServingFilesystem, @unchecked Sendable {
        private let lock = NSLock()
        private(set) var sweeps: [String] = []
        private(set) var requestDirs: [(String, UInt64)] = []
        let underlying = DefaultPhotoServeFilesystem()

        func sweep(directory: String) {
            lock.lock(); sweeps.append(directory); lock.unlock()
            underlying.sweep(directory: directory)
        }

        func makeRequestDirectory(root: String, requestId: UInt64) throws -> String {
            lock.lock(); requestDirs.append((root, requestId)); lock.unlock()
            return try underlying.makeRequestDirectory(root: root, requestId: requestId)
        }
    }

    // MARK: 装配

    private func tempRoot(_ name: String) -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("mtdss-photo-serve-test-\(name)-\(UUID().uuidString)", isDirectory: true)
        return dir.path
    }

    private func makeResponder(
        _ service: FakePhotoService,
        channel: RecordingChannel,
        files: RecordingFiles,
        root: String,
    ) -> PhotoResponder {
        PhotoResponder(service: service, fdpass: channel, serveRoot: root, files: files)
    }

    private func authorizedService() -> FakePhotoService {
        let service = FakePhotoService()
        service.status = .authorized
        return service
    }

    // MARK: 事件解析(形状钉)

    func testEventParsingPhotoFrames() {
        let list = IpcEvent.parse(
            method: "fs.photo_list",
            params: .object(["request_id": .uint(7), "path": .string("最近项目")]),
        )
        XCTAssertEqual(list, .photoList(requestId: 7, path: "最近项目"))
        let export = IpcEvent.parse(
            method: "fs.photo_export",
            params: .object(["request_id": .uint(9), "photo_id": .string("pid-1")]),
        )
        XCTAssertEqual(export, .photoExport(requestId: 9, photoId: "pid-1"))
        // 畸形参数 = 帧丢弃(前向兼容面)
        XCTAssertNil(IpcEvent.parse(method: "fs.photo_list", params: .object([:])))
        XCTAssertNil(IpcEvent.parse(method: "fs.photo_export", params: .object(["request_id": .uint(1)])))
    }

    // MARK: 0x06 列表

    func testListRootReturnsAlbumEntries() async {
        let service = authorizedService()
        service.albumsResult = [
            PhotoAlbum(id: "recent", name: "最近项目", count: 3, isRecents: true),
            PhotoAlbum(id: "alb-1", name: "旅行", count: 7),
        ]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("root"))
        await responder.handle(.photoList(requestId: 5, path: ""))
        XCTAssertEqual(channel.count, 1)
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.photoList)
        XCTAssertEqual(frame.json["ok"] as? Bool, true)
        XCTAssertEqual(frame.json["request_id"] as? Int, 5)
        XCTAssertEqual(frame.json["truncated"] as? Bool, false)
        let entries = frame.json["entries"] as? [[String: Any]] ?? []
        XCTAssertEqual(entries.count, 2)
        XCTAssertEqual(entries[0]["name"] as? String, "最近项目")
        XCTAssertEqual(entries[0]["is_dir"] as? Bool, true)
        XCTAssertEqual(entries[0]["children"] as? Int, 3)
        XCTAssertEqual(entries[1]["children"] as? Int, 7)
    }

    func testListAlbumReturnsPhotoEntries() async {
        let service = authorizedService()
        service.albumsResult = [
            PhotoAlbum(id: "recent", name: "最近项目", count: 2, isRecents: true),
            PhotoAlbum(id: "alb-1", name: "旅行", count: 1),
        ]
        service.assetsByAlbum["alb-1"] = [
            PhotoAsset(id: "pid-9", filename: "IMG_9001.HEIC", creationDateMs: 1_700_000_000_000, resourceSize: 2048),
        ]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("album"))
        await responder.handle(.photoList(requestId: 6, path: "旅行"))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.photoList)
        let entries = frame.json["entries"] as? [[String: Any]] ?? []
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0]["name"] as? String, "IMG_9001.HEIC")
        XCTAssertEqual(entries[0]["size"] as? Int, 2048)
        XCTAssertEqual(entries[0]["mtime_ms"] as? Int, 1_700_000_000_000)
        XCTAssertEqual(entries[0]["photo_id"] as? String, "pid-9")
        XCTAssertEqual(entries[0]["is_dir"] as? Bool, false)
    }

    func testListUnknownAlbumRefuses() async {
        let service = authorizedService()
        service.albumsResult = [PhotoAlbum(id: "recent", name: "最近项目", count: 1, isRecents: true)]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("unknown"))
        await responder.handle(.photoList(requestId: 8, path: "不存在的相册"))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.photoList)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNotNil(frame.json["error"])
    }

    func testListAlbumErrorRefusesWithText() async {
        let service = authorizedService()
        service.albumsError = RpcError(code: 1, message: "boom")
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("albumerr"))
        await responder.handle(.photoList(requestId: 2, path: ""))
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, false)
    }

    /// 单帧条目帽(fdpass 64KiB payload 防超限断连):超出按帽截断 +
    /// truncated=true 上达(照片项新→旧保前缀)。
    func testListingCapsEntriesAndFlagsTruncated() async {
        let service = authorizedService()
        service.albumsResult = [PhotoAlbum(id: "r", name: "最近项目", count: 300, isRecents: true)]
        service.assetsByAlbum[""] = (1...301).map {
            PhotoAsset(id: String($0), filename: "I\($0).HEIC", creationDateMs: 0)
        }
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("cap"))
        await responder.handle(.photoList(requestId: 11, path: "最近项目"))
        let entries = channel.frame(at: 0).json["entries"] as? [[String: Any]] ?? []
        XCTAssertEqual(entries.count, PhotoResponder.listingEntryCap, "按帽截断")
        XCTAssertEqual(channel.frame(at: 0).json["truncated"] as? Bool, true, "截断旗标上达")
        XCTAssertEqual(entries.first?["photo_id"] as? String, "1", "保前缀(新→旧)")
    }

    // MARK: 批内命名收口(R8 终审 I-1)

    /// 同名照片批内唯一名:首现保名,后续 ` (n)` 顺延(InboxPublisher/
    /// ShareQueue 同语言),顺序稳定 = 资产序——0x06 面逐条唯一,engine
    /// rel 面不再因 App 命名撞车硬败(引擎重复 rel 硬败兜底仅对非照片
    /// 形态的真重复可达)。
    func testListAlbumDedupesDuplicatePhotoNames() async {
        let service = authorizedService()
        service.albumsResult = [PhotoAlbum(id: "a", name: "相册A", count: 3)]
        service.assetsByAlbum["a"] = [
            PhotoAsset(id: "p1", filename: "DUP.JPG", creationDateMs: 0),
            PhotoAsset(id: "p2", filename: "DUP.JPG", creationDateMs: 0),
            PhotoAsset(id: "p3", filename: "OTHER.PNG", creationDateMs: 0),
        ]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("dedupe"))
        await responder.handle(.photoList(requestId: 21, path: "相册A"))
        let entries = channel.frame(at: 0).json["entries"] as? [[String: Any]] ?? []
        let names: [String] = entries.compactMap { $0["name"] as? String }
        XCTAssertEqual(names, ["DUP.JPG", "DUP (1).JPG", "OTHER.PNG"], "首现保名+顺延(name (n).ext,发布/队列同语言),序稳定")
        let ids: [String] = entries.compactMap { $0["photo_id"] as? String }
        XCTAssertEqual(ids, ["p1", "p2", "p3"], "photo_id 跟资产走,不因改名漂移")
    }

    /// 回退命名序号 = 批内真实顺位(原恒 1:两张无名同刻照片回退名互撞)。
    func testFallbackNamingUsesBatchIndex() async {
        let service = authorizedService()
        service.albumsResult = [PhotoAlbum(id: "a", name: "相册A", count: 2)]
        service.assetsByAlbum["a"] = [
            PhotoAsset(id: "p1", filename: nil, creationDateMs: 1_700_000_000_000),
            PhotoAsset(id: "p2", filename: nil, creationDateMs: 1_700_000_000_000),
        ]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("batchidx"))
        await responder.handle(.photoList(requestId: 22, path: "相册A"))
        let entries = channel.frame(at: 0).json["entries"] as? [[String: Any]] ?? []
        let names: [String] = entries.compactMap { $0["name"] as? String }
        XCTAssertEqual(names, ["IMG_20231114221320_1.HEIC", "IMG_20231114221320_2.HEIC"], "回退序号 = 批内顺位")
    }

    // MARK: 权限门

    func testPermissionGateRefusesListAndExportWhenNotDetermined() async {
        let service = FakePhotoService() // notDetermined
        service.albumsResult = [PhotoAlbum(id: "r", name: "最近项目", count: 1, isRecents: true)]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("gate1"))
        await responder.handle(.photoList(requestId: 1, path: ""))
        await responder.handle(.photoExport(requestId: 2, photoId: "pid-1"))
        XCTAssertEqual(channel.count, 2, "两帧都应拒答")
        for i in 0..<2 {
            let frame = channel.frame(at: i)
            XCTAssertEqual(frame.json["ok"] as? Bool, false)
            XCTAssertEqual(frame.json["error"] as? String, "照片权限未授予")
        }
        XCTAssertTrue(service.exportCalls.isEmpty, "权限门外不得触导出")
    }

    func testPermissionGateDeniedRefuses() async {
        let service = FakePhotoService()
        service.status = .denied
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("gate2"))
        await responder.handle(.photoList(requestId: 1, path: ""))
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, false)
    }

    func testLimitedIsAuthorized() async {
        let service = FakePhotoService()
        service.status = .limited
        service.albumsResult = [PhotoAlbum(id: "r", name: "最近项目", count: 1, isRecents: true)]
        service.assetsByAlbum[""] = [PhotoAsset(id: "p1", filename: "IMG_1.HEIC", creationDateMs: 0)]
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("limited"))
        await responder.handle(.photoList(requestId: 1, path: ""))
        XCTAssertEqual(channel.frame(at: 0).json["ok"] as? Bool, true, "limited = 子集内照常应答")
    }

    // MARK: 0x07 导出

    func testExportWritesFileAndAnswersPath() async throws {
        let service = authorizedService()
        service.assetMeta["pid-1"] = ("IMG_1001.HEIC", 0)
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let root = tempRoot("export")
        let responder = makeResponder(service, channel: channel, files: files, root: root)
        await responder.handle(.photoExport(requestId: 42, photoId: "pid-1"))
        // 逐请求子目录(root/<request_id>),导出文件实存
        XCTAssertEqual(files.requestDirs.map(\.1), [42])
        XCTAssertEqual(files.requestDirs.map(\.0), [root])
        let dir = root + "/42"
        let exported = service.exportCalls.first
        XCTAssertEqual(exported?.0, ["pid-1"])
        XCTAssertEqual(exported?.1, dir)
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.kind, FdPassKind.photoExport)
        XCTAssertEqual(frame.json["ok"] as? Bool, true)
        let path = try XCTUnwrap(frame.json["path"] as? String)
        XCTAssertTrue(path.hasPrefix(dir), "导出件应在逐请求子目录内: \(path)")
        XCTAssertEqual((try Data(contentsOf: URL(fileURLWithPath: path))), Data("PHOTO-pid-1".utf8))
    }

    func testExportFailureRefuses() async {
        let service = authorizedService()
        service.exportError = NSError(domain: "test", code: 1)
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let responder = makeResponder(service, channel: channel, files: files, root: tempRoot("expfail"))
        await responder.handle(.photoExport(requestId: 3, photoId: "pid-x"))
        let frame = channel.frame(at: 0)
        XCTAssertEqual(frame.json["ok"] as? Bool, false)
        XCTAssertNotNil(frame.json["error"])
    }

    // MARK: 清扫(run 挂载面)

    func testRunSweepsServeRootOnMount() async {
        let service = authorizedService()
        service.albumsResult = []
        let channel = RecordingChannel()
        let files = RecordingFiles()
        let root = tempRoot("sweep")
        // 预置崩溃残件
        try? FileManager.default.createDirectory(atPath: root + "/stale-request", withIntermediateDirectories: true)
        try? Data("stale".utf8).write(to: URL(fileURLWithPath: root + "/stale-request/f.bin"))
        let responder = makeResponder(service, channel: channel, files: files, root: root)
        // 空(已终结)事件流:run 挂载即清扫后自然退
        let (stream, cont) = AsyncStream<IpcEvent>.makeStream()
        cont.finish()
        await responder.run(stream)
        XCTAssertEqual(files.sweeps, [root], "挂载即清扫 serve 根")
        let leftover = try? FileManager.default.contentsOfDirectory(atPath: root)
        XCTAssertEqual(leftover ?? [], [], "清扫后无残件")
    }

    // MARK: 条目编解码(形状钉)

    func testEntryJSONRoundTrip() throws {
        let album = PhotoServeEntry.album(name: "最近项目", count: 12)
        let albumJSON = try XCTUnwrap(album.jsonObject as? [String: Any])
        XCTAssertEqual(albumJSON["is_dir"] as? Bool, true)
        XCTAssertEqual(albumJSON["children"] as? Int, 12)
        XCTAssertEqual(albumJSON["size"] as? Int, 0)
        XCTAssertEqual(try XCTUnwrap(PhotoServeEntry.parse(albumJSON)), album)

        let photo = PhotoServeEntry.photo(name: "IMG_1.HEIC", size: 4_096, mtimeMs: 1_760_000_000_000, photoId: "pid")
        let photoJSON = try XCTUnwrap(photo.jsonObject as? [String: Any])
        XCTAssertEqual(photoJSON["is_dir"] as? Bool, false)
        XCTAssertEqual(photoJSON["size"] as? Int, 4_096)
        XCTAssertEqual(photoJSON["photo_id"] as? String, "pid")
        // Int64 毫秒值经 NSNumber 往返
        XCTAssertEqual(try XCTUnwrap(PhotoServeEntry.parse(photoJSON)), photo)
    }

    // MARK: 命名规则(R5c 钉面迁存:导出命名消费)

    func testNamingUsesOriginalFilenameSanitized() {
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: "IMG_1234.HEIC", creationDateMs: 0, index: 1), "IMG_1234.HEIC")
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: "a/b:pic.png", creationDateMs: 0, index: 1), "b_pic.png")
    }

    func testNamingFallsBackToIMGTimestampIndex() {
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: nil, creationDateMs: 1_700_000_000_000, index: 2), "IMG_20231114221320_2.HEIC")
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: "", creationDateMs: 0, index: 5), "IMG_00000000000000_5.HEIC")
    }
}
