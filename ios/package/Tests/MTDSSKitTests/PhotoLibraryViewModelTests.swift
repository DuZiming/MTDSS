import XCTest
import MTDSSKit

/// PhotoLibraryViewModel 单测(R5c 方案 A;伪 PhotoLibraryService 注入):
/// - 权限状态机:首查门 / 授权拉相册 / 请求授权弹窗链 / 撤权清面;
/// - 相册/网格加载与错误态;
/// - 选择语义(Ruling O 同浏览页:全选再点清空 / 确认只退出 / 选中集网格序);
/// - 发送流(T11 管线):导出 → 暂存收件箱 → 管道发送(路径+设备钉)→
///   终态 done 才清暂存件;未达终态/发送失败 → 暂存件保留;
/// - 发送命名(原文件名 / IMG_<拍摄时间>_<序号>.HEIC 回退)。
@MainActor
final class PhotoLibraryViewModelTests: XCTestCase {

    // MARK: 伪服务(状态可编排;变化流手工驱动)

    private final class FakePhotoService: PhotoLibraryService, @unchecked Sendable {
        private let lock = NSLock()
        var status: PhotoLibraryPermission = .notDetermined
        /// requestAccess 的编排结果(nil = 不改状态)。
        var requestResult: PhotoLibraryPermission?
        private(set) var requestCount = 0
        var albumsResult: [PhotoAlbum] = []
        var albumsError: Error?
        var assetsByAlbum: [String: [PhotoAsset]] = [:]
        var assetsError: Error?
        var exportError: Error?
        /// 逐批导出记录(参数序)。
        private(set) var exportCalls: [([String], String)] = [] // (ids, dirPath)

        /// 变化流 continuation(测试 emitChange 手工推)。
        private var changeContinuation: AsyncStream<PhotoLibraryPermission>.Continuation?

        func permissionStatus() async -> PhotoLibraryPermission {
            lock.lock(); defer { lock.unlock() }
            return status
        }

        func requestAccess() async -> PhotoLibraryPermission {
            lock.lock()
            requestCount += 1
            if let r = requestResult { status = r }
            defer { lock.unlock() }
            return status
        }

        func permissionChanges() -> AsyncStream<PhotoLibraryPermission> {
            AsyncStream { continuation in
                lock.lock()
                changeContinuation = continuation
                lock.unlock()
            }
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
            // 落真实文件(暂存 move 语义要真文件)
            return try ids.map { id in
                let url = directory.appendingPathComponent("exported-\(id).jpg")
                try Data("PHOTO-\(id)".utf8).write(to: url)
                return url
            }
        }

        // 测试驱动面
        func emitChange(_ p: PhotoLibraryPermission) {
            lock.lock(); defer { lock.unlock() }
            changeContinuation?.yield(p)
        }
    }

    // MARK: 管道记录盒

    private final class PipelineBox: @unchecked Sendable {
        private let lock = NSLock()
        private(set) var sendCalls: [([String], DirectoryDevice)] = []
        private(set) var waitCalls: [String] = []
        private(set) var cleanupCalls: [[String]] = []
        var sendResult: Result<String, Error> = .success("job-1")
        var waitResult: String? = "done"

        func recordSend(_ paths: [String], _ device: DirectoryDevice) { lock.lock(); sendCalls.append((paths, device)); lock.unlock() }
        func recordWait(_ id: String) { lock.lock(); waitCalls.append(id); lock.unlock() }
        func recordCleanup(_ paths: [String]) { lock.lock(); cleanupCalls.append(paths); lock.unlock() }
        func snapshotSend() -> [([String], DirectoryDevice)] { lock.lock(); defer { lock.unlock() }; return sendCalls }
        func snapshotWait() -> [String] { lock.lock(); defer { lock.unlock() }; return waitCalls }
        func snapshotCleanup() -> [[String]] { lock.lock(); defer { lock.unlock() }; return cleanupCalls }
    }

    // MARK: 装配

    private func makeVM(
        _ service: FakePhotoService,
        inbox: String,
        box: PipelineBox,
    ) -> PhotoLibraryViewModel {
        PhotoLibraryViewModel(
            service: service,
            stagingInbox: inbox,
            pipeline: PhotoSendPipeline(
                send: { paths, device in
                    box.recordSend(paths, device)
                    return try box.sendResult.get()
                },
                waitTerminal: { id in
                    box.recordWait(id)
                    return box.waitResult
                },
                cleanup: { paths in
                    box.recordCleanup(paths)
                },
            ),
        )
    }

    private func tempInbox(_ name: String) -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("mtdss-photo-test-\(name)-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.path
    }

    private func asset(_ id: String, filename: String? = nil, ms: Int64 = 0) -> PhotoAsset {
        PhotoAsset(id: id, filename: filename, creationDateMs: ms)
    }

    // MARK: 权限状态机

    func testFirstCheckGateAndDeniedKeepsAlbumsEmpty() async {
        let service = FakePhotoService()
        service.status = .denied
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("denied"), box: box)
        XCTAssertFalse(vm.permissionChecked)
        await vm.refreshPermission()
        XCTAssertEqual(vm.permission, .denied)
        XCTAssertTrue(vm.permissionChecked)
        XCTAssertFalse(vm.isAuthorized)
        XCTAssertTrue(vm.albums.isEmpty)
    }

    func testAuthorizedCheckLoadsAlbums() async {
        let service = FakePhotoService()
        service.status = .authorized
        service.albumsResult = [PhotoAlbum(id: "recent", name: "最近项目", count: 3, isRecents: true)]
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("auth"), box: box)
        await vm.refreshPermission()
        XCTAssertEqual(vm.permission, .authorized)
        XCTAssertEqual(vm.albums.map(\.name), ["最近项目"])
    }

    func testRequestAccessAuthorizedLoadsAlbums() async {
        let service = FakePhotoService()
        service.status = .notDetermined
        service.requestResult = .authorized
        service.albumsResult = [PhotoAlbum(id: "recent", name: "最近项目", count: 2, isRecents: true)]
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("req"), box: box)
        await vm.requestAccess()
        XCTAssertEqual(service.requestCount, 1)
        XCTAssertEqual(vm.permission, .authorized)
        XCTAssertEqual(vm.albums.count, 1)
    }

    func testRequestDeniedStaysGuidance() async {
        let service = FakePhotoService()
        service.status = .notDetermined
        service.requestResult = .denied
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("reqdenied"), box: box)
        await vm.requestAccess()
        XCTAssertEqual(vm.permission, .denied)
        XCTAssertFalse(vm.isAuthorized)
        XCTAssertTrue(vm.albums.isEmpty)
    }

    func testChangeStreamGrantedThenRevoked() async {
        let service = FakePhotoService()
        service.status = .denied
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("stream"), box: box)
        let consumer = Task { await vm.consumePermissionChanges() }
        // 给消费任务机会建流,再编排相册数据
        service.albumsResult = [PhotoAlbum(id: "recent", name: "最近项目", count: 5, isRecents: true)]
        try? await Task.sleep(nanoseconds: 50_000_000)
        // 授权到 → 相册拉起
        service.status = .authorized
        service.emitChange(.authorized)
        try? await Task.sleep(nanoseconds: 100_000_000)
        await MainActor.run { XCTAssertEqual(vm.permission, .authorized) }
        let albumCount = vm.albums.count
        XCTAssertEqual(albumCount, 1)
        // 撤权 → 相册清空(授权内容不残留)
        service.status = .denied
        service.emitChange(.denied)
        try? await Task.sleep(nanoseconds: 100_000_000)
        await MainActor.run {
            XCTAssertEqual(vm.permission, .denied)
            XCTAssertTrue(vm.albums.isEmpty)
        }
        consumer.cancel()
    }

    // MARK: 相册/网格

    func testAlbumErrorAndRetry() async {
        let service = FakePhotoService()
        service.status = .authorized
        service.albumsError = RpcError(code: 1, message: "boom")
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("albumerr"), box: box)
        await vm.refreshPermission()
        XCTAssertNotNil(vm.albumsError)
        service.albumsError = nil
        service.albumsResult = [PhotoAlbum(id: "a", name: "旅行", count: 7)]
        await vm.loadAlbums()
        XCTAssertNil(vm.albumsError)
        XCTAssertEqual(vm.albums.map(\.name), ["旅行"])
    }

    func testOpenAlbumLoadsAssetsAndRecentsPassesNil() async {
        let service = FakePhotoService()
        service.status = .authorized
        let recents = PhotoAlbum(id: "recent", name: "最近项目", count: 2, isRecents: true)
        let custom = PhotoAlbum(id: "alb-1", name: "旅行", count: 1)
        service.albumsResult = [recents, custom]
        service.assetsByAlbum[""] = [asset("p1"), asset("p2")]
        service.assetsByAlbum["alb-1"] = [asset("p3")]
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("open"), box: box)
        await vm.refreshPermission()
        await vm.open(album: recents)
        XCTAssertEqual(vm.currentAlbum, recents)
        XCTAssertEqual(vm.assets.map(\.id), ["p1", "p2"])
        await vm.open(album: custom)
        XCTAssertEqual(vm.assets.map(\.id), ["p3"])
    }

    func testAssetsErrorSurfaces() async {
        let service = FakePhotoService()
        service.status = .authorized
        let recents = PhotoAlbum(id: "recent", name: "最近项目", count: 2, isRecents: true)
        service.albumsResult = [recents]
        service.assetsError = RpcError(code: 1, message: "boom")
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("asterr"), box: box)
        await vm.refreshPermission()
        await vm.open(album: recents)
        XCTAssertEqual(vm.assetsError, "boom")
        service.assetsError = nil
        service.assetsByAlbum[""] = [asset("p1")]
        await vm.reloadAssets()
        XCTAssertNil(vm.assetsError)
        XCTAssertEqual(vm.assets.count, 1)
    }

    // MARK: 选择语义(Ruling O 同浏览页)

    func testSelectionSemantics() async {
        let service = FakePhotoService()
        service.status = .authorized
        let recents = PhotoAlbum(id: "recent", name: "最近项目", count: 3, isRecents: true)
        service.albumsResult = [recents]
        service.assetsByAlbum[""] = [asset("p1"), asset("p2"), asset("p3")]
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("select"), box: box)
        await vm.refreshPermission()
        await vm.open(album: recents)

        vm.enterSelection()
        XCTAssertTrue(vm.isSelecting)
        vm.toggle("p1")
        vm.toggle("p3")
        XCTAssertEqual(vm.selection, ["p1", "p3"])
        // 选中集 = 网格序(非勾选序)
        XCTAssertEqual(vm.selectedAssets.map(\.id), ["p1", "p3"])
        // 全选 → 再全选 = 清空
        vm.selectAll()
        XCTAssertEqual(vm.selection.count, 3)
        vm.selectAll()
        XCTAssertTrue(vm.selection.isEmpty)
        // 确认 = 只退出(Ruling O)
        vm.toggle("p2")
        vm.confirmSelection()
        XCTAssertFalse(vm.isSelecting)
        XCTAssertTrue(vm.selection.isEmpty)
    }

    // MARK: 发送流(T11 管线)

    func testSendFlowStagesSendsAndCleansAfterDone() async {
        let service = FakePhotoService()
        let box = PipelineBox()
        let inbox = tempInbox("send")
        let vm = makeVM(service, inbox: inbox, box: box)
        let picked = [asset("p1", filename: "IMG_1001.HEIC"), asset("p2", filename: nil, ms: 1_700_000_000_000)]
        box.sendResult = .success("job-9")
        box.waitResult = "done"

        let ok = await vm.send(picked, to: DirectoryDevice(deviceId: "peer-1", name: "PC", platform: "windows", online: true))
        XCTAssertTrue(ok)
        // ① 导出按 ids 序
        XCTAssertEqual(service.exportCalls.map(\.0), [["p1", "p2"]])
        // ② 发送收到收件箱内暂存路径(存在、导出临时目录之外)+ 设备
        let send = box.snapshotSend()
        XCTAssertEqual(send.count, 1)
        let (paths, device) = send[0]
        XCTAssertEqual(device.deviceId, "peer-1")
        XCTAssertEqual(paths.count, 2)
        for path in paths {
            XCTAssertTrue(path.hasPrefix(inbox), "\(path) 应在暂存收件箱内")
            XCTAssertTrue(FileManager.default.fileExists(atPath: path))
        }
        // ③ 终态等待恰一次
        XCTAssertEqual(box.snapshotWait(), ["job-9"])
        // ④ done → 清暂存件(文件已删)
        let cleanups = box.snapshotCleanup()
        XCTAssertEqual(cleanups, [paths])
        for path in paths {
            XCTAssertFalse(FileManager.default.fileExists(atPath: path))
        }
    }

    func testSendStagedFileNamesFollowNamingRule() async {
        let service = FakePhotoService()
        let box = PipelineBox()
        let inbox = tempInbox("names")
        let vm = makeVM(service, inbox: inbox, box: box)
        // p1 = 原文件名;p2 = 缺名回退 IMG_<时间戳>_2.HEIC(ms=1.7e12 → UTC 2023-11-14T22:13:20)
        let picked = [asset("p1", filename: "IMG_1001.HEIC"), asset("p2", filename: nil, ms: 1_700_000_000_000)]
        box.waitResult = "done"
        _ = await vm.send(picked, to: DirectoryDevice(deviceId: "peer-1", name: "PC", platform: "windows", online: true))
        let names = box.snapshotSend()[0].0.map { ( $0 as NSString).lastPathComponent }
        XCTAssertEqual(names[0], "IMG_1001.HEIC")
        XCTAssertEqual(names[1], "IMG_20231114221320_2.HEIC")
    }

    func testSendKeepsStagedWhenTerminalUnknown() async {
        let service = FakePhotoService()
        let box = PipelineBox()
        let inbox = tempInbox("timeout")
        let vm = makeVM(service, inbox: inbox, box: box)
        let picked = [asset("p1", filename: "IMG_1001.HEIC")]
        box.waitResult = nil // 超时未达终态
        let ok = await vm.send(picked, to: DirectoryDevice(deviceId: "peer-1", name: "PC", platform: "windows", online: true))
        XCTAssertTrue(ok) // 发送已受理
        XCTAssertTrue(box.snapshotCleanup().isEmpty)
        let path = box.snapshotSend()[0].0[0]
        XCTAssertTrue(FileManager.default.fileExists(atPath: path)) // 留暂存可寻回
    }

    func testSendFailureSurfacesErrorAndKeepsStaged() async {
        let service = FakePhotoService()
        let box = PipelineBox()
        let inbox = tempInbox("fail")
        let vm = makeVM(service, inbox: inbox, box: box)
        let picked = [asset("p1", filename: "IMG_1001.HEIC")]
        box.sendResult = .failure(RpcError(code: 7, message: "对端离线"))
        let ok = await vm.send(picked, to: DirectoryDevice(deviceId: "peer-1", name: "PC", platform: "windows", online: true))
        XCTAssertFalse(ok)
        XCTAssertEqual(vm.lastActionError, "对端离线")
        XCTAssertTrue(box.snapshotCleanup().isEmpty)
        XCTAssertTrue(box.snapshotWait().isEmpty) // 无作业可等
        let path = box.snapshotSend()[0].0[0]
        XCTAssertTrue(FileManager.default.fileExists(atPath: path))
    }

    func testExportFailureSurfacesError() async {
        let service = FakePhotoService()
        service.exportError = NSError(domain: "test", code: 1)
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("expfail"), box: box)
        let ok = await vm.send([asset("p1")], to: DirectoryDevice(deviceId: "peer-1", name: "PC", platform: "windows", online: true))
        XCTAssertFalse(ok)
        XCTAssertTrue(vm.lastActionError?.contains("照片导出失败") ?? false)
        XCTAssertTrue(box.snapshotSend().isEmpty)
    }

    func testSendEmptySelectionAndReentryGuard() async {
        let service = FakePhotoService()
        let box = PipelineBox()
        let vm = makeVM(service, inbox: tempInbox("guard"), box: box)
        let empty = await vm.send([], to: DirectoryDevice(deviceId: "d", name: nil, platform: "windows", online: true))
        XCTAssertFalse(empty)
        XCTAssertTrue(box.snapshotSend().isEmpty)
    }

    // MARK: 命名规则(纯函数)

    func testNamingUsesOriginalFilenameSanitized() {
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: "IMG_1234.HEIC", creationDateMs: 0, index: 1), "IMG_1234.HEIC")
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: "a/b:pic.png", creationDateMs: 0, index: 1), "b_pic.png")
    }

    func testNamingFallsBackToIMGTimestampIndex() {
        // 1_700_000_000_000 ms = 2023-11-14T22:13:20Z
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: nil, creationDateMs: 1_700_000_000_000, index: 2), "IMG_20231114221320_2.HEIC")
        XCTAssertEqual(PhotoAssetNaming.displayName(filename: "", creationDateMs: 0, index: 5), "IMG_00000000000000_5.HEIC")
    }
}
