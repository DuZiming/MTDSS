import XCTest
import MTDSSKit

/// T6 单测(brief Step 1,先红后绿):
/// - **SendService 请求帧组装逐字段对 golden**(crates/ipc/contract 单一真源):
///   kind/files/to/skip/overwrite/cut 等与 golden req 逐字段相等;dir 为
///   App 产品语义(发送/转发 = 收件箱哨兵 "inbox" 发送免选目录,spec §3;
///   下载 = 本机 recv_dir 绝对路径,desktop-cfg 引擎口径),不取 golden 值
///   ——golden 的 dir 是 golden 场景自设的绝对路径。
/// - **0 选确认 = 退出选择模式**(spec §4.2:蓝圈确认 0 选即退出);
/// - 选择集收集/全选/行切换;
/// - **传输活动面**(spec §5.5):任一非终态任务 → 亮屏,终态复位
///   (终态集 = done/failed/cancelled/interrupted,android TasksReducer 同口径)。
@MainActor
final class SendServiceTests: XCTestCase {

    // MARK: golden 常量(与 crates/ipc/contract/*.req.json 逐字段同源)

    /// transfer.start.upload.req.json 的 params 字段(golden 场景值)。
    private nonisolated static let uploadGolden = (
        files: ["contract-fixture/契约样本.bin"],
        to: "dev-peer-fixed",
        dir: "/golden/peer-inbox",
    )
    /// transfer.start.remote_copy.req.json 的 params 字段。
    private nonisolated static let remoteCopyGolden = (
        from: "dev-peer-fixed",
        paths: ["/远端/直传 目录"],
        to: "dev-peer2-fixed",
        dir: "/golden/peer2-inbox",
    )
    /// transfer.start.download.req.json 的 params 字段。
    private nonisolated static let downloadGolden = (
        device: "dev-peer-fixed",
        paths: ["/远端/契约 文档.pdf"],
    )

    // MARK: 助手

    /// 伪 daemon + 客户端 + 仓库 + 服务。onLine 按 method 路由:
    /// status → golden status.resp.json;其余 → okScript(method) 应答。
    private func makeService(
        okScript: @escaping @Sendable (String) -> JSONValue,
    ) async throws -> (FakeDaemon.ServerSide, SendService) {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .object(let obj) = frame,
                  case .number(let idRaw)? = obj["id"],
                  case .string(let method)? = obj["method"] else { return }
            let result: JSONValue
            if method == "status" {
                guard let golden = try? JSONParser.parse(Goldens.text("status.resp.json")),
                      case .object(let gobj) = golden,
                      case .object(let res)? = gobj["result"] else { return }
                result = .object(res)
            } else {
                result = okScript(method)
            }
            conn.sendLine(Data((JSONValue.object([
                "id": .number(idRaw),
                "result": result,
            ]).canonical + "\n").utf8))
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        let side = try XCTUnwrap(fake.connections.first)
        return (side, SendService(repo: MtdssRepository(client: client)))
    }

    /// 捕获到的指定方法首个请求 params。
    private func capturedParams(_ side: FakeDaemon.ServerSide, method: String) throws -> [String: JSONValue] {
        for line in side.received {
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .object(let obj) = frame,
                  case .string(let m)? = obj["method"], m == method,
                  case .object(let params)? = obj["params"] else { continue }
            return params
        }
        XCTFail("未捕获到 \(method) 请求帧")
        return [:]
    }

    private func str(_ params: [String: JSONValue], _ key: String) throws -> String {
        try XCTUnwrap(params[key], "\(key) 键缺失").asString()
    }

    private func bool(_ params: [String: JSONValue], _ key: String) throws -> Bool {
        try XCTUnwrap(params[key], "\(key) 键缺失").asBool()
    }

    private func assertStringArray(_ params: [String: JSONValue], _ key: String, equals expected: [String]) throws {
        let got = try XCTUnwrap(params[key], "\(key) 键缺失").asArray().map { try $0.asString() }
        XCTAssertEqual(got, expected, "\(key) 字段漂移")
    }

    // MARK: 发送(本机 → 对端收件箱;upload 路径形态;全平台哨兵口径)

    /// golden 场景对端(契约 devices 平台语言;平台由调用方随 DirectoryDevice
    /// 携带——golden 的 dir 是 golden 场景自设绝对路径,App 语义 dir 走解析)。
    private nonisolated static func peer(_ id: String, platform: String?) -> DirectoryDevice {
        DirectoryDevice(deviceId: id, name: id, platform: platform, online: true)
    }

    func testSendAssemblesUploadRequestFieldExactAgainstGolden() async throws {
        let (side, service) = try await makeService { _ in .object(["job_id": .string("7")]) }
        let jobId = try await service.send(
            paths: Self.uploadGolden.files,
            to: Self.peer(Self.uploadGolden.to, platform: "windows"),
        )
        XCTAssertEqual(jobId, "7")

        let params = try capturedParams(side, method: "transfer.start")
        // 与 golden transfer.start.upload.req.json 逐字段对齐(dir 除外:App
        // 语义 = 收件箱哨兵,发送免选目录,spec §3;Windows 对端哨兵合法——
        // 对端引擎回退默认收件目录,task-6 E2E 实证)
        XCTAssertEqual(try str(params, "kind"), "upload")
        try assertStringArray(params, "files", equals: Self.uploadGolden.files)
        XCTAssertEqual(try str(params, "to"), Self.uploadGolden.to)
        XCTAssertEqual(try str(params, "dir"), SendService.inboxSentinel)
        try assertStringArray(params, "skip", equals: [])
        try assertStringArray(params, "overwrite", equals: [])
        XCTAssertEqual(try bool(params, "cut"), false)
    }

    // MARK: 全平台哨兵(R12:Ruling I v1 拒发退役——T5b Ruling J 引擎侧
    //        哨兵统一已落地,unix 收端 dir="inbox" 回退默认收件目录,与
    //        Windows 同语义;用户 20261103.3 设备轮「列表不一致」= App 列表
    //        全平台可选,拒发立场不再有消费者。旧四钉[平台分档/拒发两向]
    //        撤,统一钉 = 已知平台全通发 + 哨兵 dir)

    /// 已知平台(android/ios/windows/linux/macos)与未知(nil/其他)全部
    /// 通发,dir 恒 = 收件箱哨兵——拒发退役的钉面:任何平台不得抛、不得
    /// 短路少发帧。
    func testSendToAllKnownPlatformsUsesSentinel() async throws {
        let (side, service) = try await makeService { _ in .object(["job_id": .string("10")]) }
        let platforms: [String?] = ["android", "ios", "windows", "linux", "macos", nil, "freebsd"]
        for platform in platforms {
            _ = try await service.send(paths: ["/root/a.txt"], to: Self.peer("dev-x", platform: platform))
        }
        let uploads = side.received.compactMap { line -> [String: JSONValue]? in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .string("transfer.start")? = frame["method"],
                  case .object(let params)? = frame["params"] else { return nil }
            return params
        }
        XCTAssertEqual(uploads.count, platforms.count, "每平台恰一帧 transfer.start(拒发短路已退役)")
        for params in uploads {
            XCTAssertEqual(try str(params, "dir"), SendService.inboxSentinel, "\(params) dir 应为哨兵")
        }
    }

    /// unix 桌面对端(linux B)转发同哨兵(remote_copy;拒发退役后 B 端
    /// 落点 = 对端默认收件目录,引擎侧回退收口,Ruling J)。
    func testForwardToUnixDesktopPeerUsesSentinel() async throws {
        let (side, service) = try await makeService { _ in .object(["job_id": .string("12")]) }
        let jobId = try await service.forward(
            from: Self.remoteCopyGolden.from,
            paths: Self.remoteCopyGolden.paths,
            to: Self.peer("dev-linux-b", platform: "linux"),
        )
        XCTAssertEqual(jobId, "12")
        let params = try capturedParams(side, method: "transfer.start")
        XCTAssertEqual(try str(params, "kind"), "remote_copy")
        XCTAssertEqual(try str(params, "to"), "dev-linux-b")
        XCTAssertEqual(try str(params, "dir"), SendService.inboxSentinel, "unix 桌面 B 端 dir 应为哨兵(拒发退役)")
    }

    // MARK: 下载(远端 → 本机收件箱;dir = recv_dir)

    func testDownloadTargetsInboxResolvedFromStatusRecvDir() async throws {
        let (side, service) = try await makeService { _ in .object(["job_id": .string("8")]) }
        let jobId = try await service.download(device: Self.downloadGolden.device, paths: Self.downloadGolden.paths)
        XCTAssertEqual(jobId, "8")

        // dir = 本机收件箱(status.recv_dir;golden status.resp = /mtdss/contract-inbox)
        let params = try capturedParams(side, method: "transfer.start")
        XCTAssertEqual(try str(params, "kind"), "download")
        XCTAssertEqual(try str(params, "device"), Self.downloadGolden.device)
        try assertStringArray(params, "paths", equals: Self.downloadGolden.paths)
        XCTAssertEqual(try str(params, "dir"), "/mtdss/contract-inbox")
        try assertStringArray(params, "skip", equals: [])
        try assertStringArray(params, "overwrite", equals: [])
        XCTAssertEqual(try bool(params, "cut"), false)
    }

    // MARK: 转发(调度 A→B;remote_copy;落 B 收件箱)

    func testForwardAssemblesRemoteCopyRequestFieldExactAgainstGolden() async throws {
        let (side, service) = try await makeService { _ in .object(["job_id": .string("9")]) }
        let jobId = try await service.forward(
            from: Self.remoteCopyGolden.from,
            paths: Self.remoteCopyGolden.paths,
            to: Self.peer(Self.remoteCopyGolden.to, platform: "windows"),
        )
        XCTAssertEqual(jobId, "9")

        let params = try capturedParams(side, method: "transfer.start")
        // 与 golden transfer.start.remote_copy.req.json 逐字段对齐(dir 除外:
        // App 语义 = B 端收件箱哨兵,全平台同口径[Windows B 为例])
        XCTAssertEqual(try str(params, "kind"), "remote_copy")
        XCTAssertEqual(try str(params, "from"), Self.remoteCopyGolden.from)
        try assertStringArray(params, "paths", equals: Self.remoteCopyGolden.paths)
        XCTAssertEqual(try str(params, "to"), Self.remoteCopyGolden.to)
        XCTAssertEqual(try str(params, "dir"), SendService.inboxSentinel)
        try assertStringArray(params, "skip", equals: [])
        try assertStringArray(params, "overwrite", equals: [])
        XCTAssertEqual(try bool(params, "cut"), false)
    }

    // MARK: 本地删除(delete {device,paths,force})

    func testDeleteSendsDeleteFrameAndParsesResult() async throws {
        let (side, service) = try await makeService { _ in
            .object([
                "deleted": .number("2"),
                "recycled": .bool(false),
                "failed": .array([
                    .object(["path": .string("/root/忙.txt"), "code": .string("NOT_FOUND"), "message": .string("不存在")]),
                ]),
            ])
        }
        let result = try await service.deleteLocal(device: "dev-self", paths: ["/root/a.txt", "/root/忙.txt"])
        XCTAssertEqual(result.deleted, 2)
        XCTAssertFalse(result.recycled)
        XCTAssertEqual(result.failed.count, 1)
        XCTAssertEqual(result.failed[0].path, "/root/忙.txt")
        XCTAssertEqual(result.failed[0].code, "NOT_FOUND")
        XCTAssertEqual(result.failed[0].message, "不存在")

        let params = try capturedParams(side, method: "delete")
        XCTAssertEqual(try str(params, "device"), "dev-self")
        try assertStringArray(params, "paths", equals: ["/root/a.txt", "/root/忙.txt"])
        // iOS 引擎恒永久删(core fs:android/ios 无回收设施,force 收口 true)
        XCTAssertEqual(try bool(params, "force"), true)
    }

    // MARK: 限时预算(R5c 控制者派发小修:冷对端 transfer.start 假失败窗)

    /// 预算钉(公开面常量断言):transfer-start 受理调用(send/forward/
    /// download)用 90s 专项预算盖住冷对端 ctrl 握手窗(T7 实测 25–35s,
    /// 30s 默认在对端真收下作业时先到 → UI 假失败而文件照达),其余调用
    /// (status 现拉/本地删除)保持 30s 默认。三受理调用点传专项预算、
    /// 其余点走缺省为接线事实(diff 可查):行为级区分 30/90 需 ≥30s
    /// 真实等待,不进单测;此处钉两常量值与关系。
    func testTimeoutBudgetsTransferStartRaisedOthersDefault() {
        XCTAssertEqual(SendService.callTimeout, 30, "默认预算不动(status 现拉/本地删除)")
        XCTAssertEqual(SendService.transferStartTimeout, 90, "transfer-start 专项预算盖冷握手窗")
        XCTAssertGreaterThan(
            SendService.transferStartTimeout, SendService.callTimeout,
            "专项预算须宽于默认(握手窗 T7 实测 25–35s > 30s 默认)",
        )
    }

    // MARK: 选择模式(spec §4.2:0 选确认=退出;行切换;全选)

    private func makeVM(_ fake: FakeDaemon, entries: [DirEntry]) async -> BrowseViewModel {
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"] else { return }
            let result: JSONValue = .object([
                "entries": .array(entries.map { e in
                    .object([
                        "name": .string(e.name),
                        "path": .string(e.path),
                        "size": .number(String(e.size)),
                        "mtime_ms": .number(String(e.mtimeMs)),
                        "is_dir": .bool(e.isDir),
                    ])
                }),
                "truncated": .bool(false),
            ])
            conn.sendLine(Data((JSONValue.object(["id": .number(idRaw), "result": result]).canonical + "\n").utf8))
        }
        let client = IpcClient(endpoint: fake)
        return BrowseViewModel(
            repo: MtdssRepository(client: client),
            deviceId: "dev-self",
            rootPath: "/root",
            rootTitle: "收件箱",
        )
    }

    func testConfirmWithEmptySelectionExitsSelectionMode() async throws {
        let fake = FakeDaemon()
        let vm = await makeVM(fake, entries: [
            DirEntry(name: "a.txt", path: "/root/a.txt", size: 1, mtimeMs: 0, isDir: false),
        ])
        await vm.loadRoot()

        vm.enterSelection()
        XCTAssertTrue(vm.isSelecting)
        // Ruling O(2026-11-02 真机轮 #2):确认只 = 退出选择模式,无副作用
        vm.confirmSelection()
        XCTAssertFalse(vm.isSelecting)
        XCTAssertTrue(vm.selection.isEmpty)
    }

    /// Ruling O 反转(旧 T6 语义废止):N 选点确认**不携带选择集**,只退出
    /// 选择模式——分享/下载/转发全部仅自底栏发起,底栏动作在退出**前**读
    /// ``selectedEntries`` 捕获(捕获面由既有 toggle/selectedEntries 测试钉)。
    func testConfirmExitsSelectionModeRegardlessOfSelection() async throws {
        let fake = FakeDaemon()
        let vm = await makeVM(fake, entries: [
            DirEntry(name: "a.txt", path: "/root/a.txt", size: 1, mtimeMs: 0, isDir: false),
            DirEntry(name: "b.txt", path: "/root/b.txt", size: 2, mtimeMs: 0, isDir: false),
        ])
        await vm.loadRoot()

        vm.enterSelection()
        vm.toggle("/root/b.txt")
        // 旧 T6 面此处返回 .picked——Ruling O:确认不再承担「进弹层」语义
        vm.confirmSelection()
        XCTAssertFalse(vm.isSelecting)
        XCTAssertTrue(vm.selection.isEmpty, "确认 = 退出选择模式,选择集清空不外携")
    }

    func testToggleSelectAllAndSelectedEntriesOrder() async throws {
        let fake = FakeDaemon()
        let vm = await makeVM(fake, entries: [
            DirEntry(name: "工作", path: "/root/工作", size: 0, mtimeMs: 0, isDir: true),
            DirEntry(name: "a.txt", path: "/root/a.txt", size: 1, mtimeMs: 0, isDir: false),
            DirEntry(name: "b.txt", path: "/root/b.txt", size: 2, mtimeMs: 0, isDir: false),
        ])
        await vm.loadRoot()

        vm.enterSelection()
        vm.toggle("/root/a.txt")
        vm.toggle("/root/a.txt") // 再点 = 取消
        XCTAssertTrue(vm.selection.isEmpty)
        vm.toggle("/root/b.txt")
        XCTAssertEqual(vm.selectedEntries.map(\.path), ["/root/b.txt"])

        vm.selectAll()
        XCTAssertEqual(vm.selection.count, 3)
        // 全选后收集顺序 = 可见列表顺序(文件夹恒前 + 名称升序)
        XCTAssertEqual(vm.selectedEntries.map(\.path), ["/root/工作", "/root/a.txt", "/root/b.txt"])

        vm.selectAll() // 已全选再点全选 = 清空
        XCTAssertTrue(vm.selection.isEmpty)
    }

    // MARK: 传输活动面(spec §5.5:任一非终态 → 亮屏;终态复位)

    func testTransferActivityTrackerTerminalReset() {
        let tracker = TransferActivityTracker()
        XCTAssertFalse(tracker.isActive)

        tracker.apply(TransferProgress(
            id: "1", kind: "upload", state: "running",
            filesDone: 0, filesTotal: 1, bytesDone: 0, bytesTotal: 10,
            speedBps: 5, skipped: 0, relay: false, error: nil,
        ))
        tracker.apply(TransferProgress(
            id: "2", kind: "download", state: "queued",
            filesDone: 0, filesTotal: 1, bytesDone: 0, bytesTotal: 10,
            speedBps: 0, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertTrue(tracker.isActive, "任一非终态任务 → 亮屏")

        tracker.apply(TransferProgress(
            id: "1", kind: "upload", state: "done",
            filesDone: 1, filesTotal: 1, bytesDone: 10, bytesTotal: 10,
            speedBps: 0, skipped: 0, relay: false, error: nil,
        ))
        XCTAssertTrue(tracker.isActive, "仍有非终态任务")

        tracker.apply(TransferProgress(
            id: "2", kind: "download", state: "interrupted",
            filesDone: 0, filesTotal: 1, bytesDone: 0, bytesTotal: 10,
            speedBps: 0, skipped: 0, relay: false, error: "断线",
        ))
        XCTAssertFalse(tracker.isActive, "全部终态(done/interrupted)→ 复位熄屏")
    }

    func testTransferActivityTrackerSeedFromSnapshot() {
        let tracker = TransferActivityTracker()
        let job = { (id: String, state: String) in
            TransferJob(
                id: id, kind: "upload", name: "n", state: state,
                filesDone: 0, filesTotal: 1, bytesDone: 0, bytesTotal: 1,
                speedBps: 0, skipped: 0, relay: false, src: nil, dst: nil, error: nil,
            )
        }
        tracker.seed([job("1", "done"), job("2", "failed")])
        XCTAssertFalse(tracker.isActive)
        tracker.seed([job("3", "running")])
        XCTAssertTrue(tracker.isActive)
    }
}
