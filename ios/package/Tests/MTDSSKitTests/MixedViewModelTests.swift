import XCTest
import MTDSSKit

/// MixedViewModel 行为测试(V3-3 Task 15,Windows favfetch.test.ts/mixedview
/// 场景的 Swift 镜像,伪 daemon 脚本直驱,BrowseViewModelTests 脚本手法同源):
/// ①快照分桶:离线成员初始即 offline 桶且 **list 不发**(Review Focus #3);
/// ②resolve 并入/reject→failed;③世代守卫:reload 中途再 reload,旧应答
/// 整包弃;④updateFavorite members 变更硬重扇出/仅名变不重扇出;⑤
/// visibleRows 文件夹恒前排序 + 同路径跨源两行两 key(Review Focus #2);
/// ⑥复合键选择/全选回翻;⑦downloadSelected 逐组分组(跳过本机组,dir=
/// 收件箱哨兵,anyFail→false);⑧deleteSelected 门(全本机恒可删/含远端
/// 门关整批拒零外发);⑨sendSelected 跳过 from==to 组。
@MainActor
final class MixedViewModelTests: XCTestCase {

    // MARK: 条目/帧助手(BrowseViewModelTests 同源)

    private nonisolated static func dir(_ name: String, path: String, mtime: Int64 = 0) -> DirEntry {
        DirEntry(name: name, path: path, size: 0, mtimeMs: mtime, isDir: true, children: nil)
    }

    private nonisolated static func file(_ name: String, path: String, size: Int64, mtime: Int64 = 0) -> DirEntry {
        DirEntry(name: name, path: path, size: size, mtimeMs: mtime, isDir: false, children: nil)
    }

    // MARK: 脚本(KvScript 手法镜像:内存仓 + 调用留痕,NSLock 收口)

    private struct ListReq: Equatable { let device: String; let path: String }
    private struct DownloadReq: Equatable { let device: String; let paths: [String]; let dir: String }
    private struct UploadReq: Equatable { let files: [String]; let to: String; let dir: String }
    private struct DeleteReq: Equatable { let device: String; let paths: [String]; let force: Bool }

    private final class Script: @unchecked Sendable {
        private let lock = NSLock()
        private var devices: [DirectoryDevice] = []
        private var routes: [String: [DirEntry]] = [:]
        private var failPaths: Set<String> = []
        private var delayStorage: TimeInterval = 0
        private var listLog: [ListReq] = []
        private var deviceCallCount = 0
        private var downloadLog: [DownloadReq] = []
        private var uploadLog: [UploadReq] = []
        private var deleteLog: [DeleteReq] = []
        private var downloadShouldFail = false
        private var uploadShouldFail = false

        // 帧助手
        private nonisolated static func entryJSON(_ e: DirEntry) -> JSONValue {
            .object([
                "name": .string(e.name),
                "path": .string(e.path),
                "size": .number(String(e.size)),
                "mtime_ms": .number(String(e.mtimeMs)),
                "is_dir": .bool(e.isDir),
            ])
        }

        private nonisolated static func deviceJSON(_ d: DirectoryDevice) -> JSONValue {
            .object([
                "device_id": .string(d.deviceId),
                "name": d.name.map { .string($0) } ?? .null,
                "platform": d.platform.map { .string($0) } ?? .null,
                "online": .bool(d.online),
            ])
        }

        private nonisolated static func result(id: JSONValue, _ result: [String: JSONValue]) -> String {
            JSONValue.object(["id": id, "result": .object(result)]).canonical
        }

        private nonisolated static func error(id: JSONValue, message: String) -> String {
            JSONValue.object(["id": id, "error": .object([
                "code": .number("1"),
                "message": .string(message),
            ])]).canonical
        }

        // 配置(测试主线程调用)
        func setDevices(_ list: [DirectoryDevice]) { lock.withLock { devices = list } }
        func setRoute(_ path: String, _ entries: [DirEntry]) { lock.withLock { routes[path] = entries } }
        func setFail(_ path: String) { lock.withLock { failPaths.insert(path) } }
        func setDownloadFail() { lock.withLock { downloadShouldFail = true } }
        func setUploadFail() { lock.withLock { uploadShouldFail = true } }

        /// 每个应答前的延迟秒(世代/在途窗口测试用)。
        var listDelay: TimeInterval {
            get { lock.withLock { delayStorage } }
            set { lock.withLock { delayStorage = newValue } }
        }

        // 观测
        var listRequests: [ListReq] { lock.withLock { listLog } }
        var deviceCalls: Int { lock.withLock { deviceCallCount } }
        var downloads: [DownloadReq] { lock.withLock { downloadLog } }
        var uploads: [UploadReq] { lock.withLock { uploadLog } }
        var deletes: [DeleteReq] { lock.withLock { deleteLog } }

        // 锁内读写收口(handler 服务线程调用)
        fileprivate func recordDeviceCall() { lock.withLock { deviceCallCount += 1 } }
        fileprivate func recordList(device: String, path: String) { lock.withLock { listLog.append(ListReq(device: device, path: path)) } }
        fileprivate func snapshotDevices() -> [DirectoryDevice] { lock.withLock { devices } }
        fileprivate func route(_ path: String) -> [DirEntry]? { lock.withLock { routes[path] } }
        fileprivate func isFailed(_ path: String) -> Bool { lock.withLock { failPaths.contains(path) } }
        fileprivate func delayIfNeeded() { let d = lock.withLock { delayStorage }; if d > 0 { Thread.sleep(forTimeInterval: d) } }
        fileprivate func recordDownload(_ req: DownloadReq) -> Bool { lock.withLock { downloadLog.append(req); return downloadShouldFail } }
        fileprivate func recordUpload(_ req: UploadReq) -> Bool { lock.withLock { uploadLog.append(req); return uploadShouldFail } }
        fileprivate func recordDelete(_ req: DeleteReq) { lock.withLock { deleteLog.append(req) } }

        static func handler(_ script: Script) -> FakeDaemon.LineHandler {
            { conn, line in
                guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                      let method = try? frame.req("method").asString(),
                      case .number(let idRaw)? = frame["id"] else {
                    return
                }
                let id = JSONValue.number(idRaw)
                let params = frame["params"] ?? .object([:])
                switch method {
                case Ops.devices:
                    script.recordDeviceCall()
                    let devices = script.snapshotDevices()
                    conn.sendLine(Data((Self.result(id: id, [
                        "devices": .array(devices.map(Self.deviceJSON)),
                        "stale": .bool(false),
                    ]) + "\n").utf8))
                case Ops.list:
                    let device = (try? params.req("device").asString()) ?? ""
                    let path = (try? params.req("path").asString()) ?? ""
                    script.recordList(device: device, path: path) // 请求到达即留痕(延迟前,在途窗口可观测)
                    script.delayIfNeeded()
                    if script.isFailed(path) {
                        conn.sendLine(Data((Self.error(id: id, message: "list 失败(测试注入)") + "\n").utf8))
                        return
                    }
                    if let entries = script.route(path) {
                        conn.sendLine(Data((Self.result(id: id, [
                            "entries": .array(entries.map(Self.entryJSON)),
                            "truncated": .bool(false),
                        ]) + "\n").utf8))
                    }
                    // 无路由 = 不应答(挂死由 RpcGuard 兜底;正常用例全配路由)
                case Ops.transferStart:
                    let kind = (try? params.req("kind").asString()) ?? ""
                    let dir = (try? params.req("dir").asString()) ?? ""
                    // upload 形参键 = files;download/remote_copy = paths
                    let listKey = kind == "upload" ? "files" : "paths"
                    let paths = ((try? params.req(listKey).asArray()) ?? []).compactMap { try? $0.asString() }
                    if kind == "download" {
                        let device = (try? params.req("device").asString()) ?? ""
                        let fail = script.recordDownload(DownloadReq(device: device, paths: paths, dir: dir))
                        if fail {
                            conn.sendLine(Data((Self.error(id: id, message: "下载受理失败(测试注入)") + "\n").utf8))
                        } else {
                            conn.sendLine(Data((Self.result(id: id, ["job_id": .string("job-d\(script.downloads.count)")]) + "\n").utf8))
                        }
                    } else if kind == "upload" {
                        let to = (try? params.req("to").asString()) ?? ""
                        let fail = script.recordUpload(UploadReq(files: paths, to: to, dir: dir))
                        if fail {
                            conn.sendLine(Data((Self.error(id: id, message: "发送受理失败(测试注入)") + "\n").utf8))
                        } else {
                            conn.sendLine(Data((Self.result(id: id, ["job_id": .string("job-u\(script.uploads.count)")]) + "\n").utf8))
                        }
                    } else {
                        conn.sendLine(Data((Self.result(id: id, [:]) + "\n").utf8))
                    }
                case Ops.delete:
                    let device = (try? params.req("device").asString()) ?? ""
                    let paths = ((try? params.req("paths").asArray()) ?? []).compactMap { try? $0.asString() }
                    let force = (try? params.req("force").asBool()) ?? false
                    script.recordDelete(DeleteReq(device: device, paths: paths, force: force))
                    conn.sendLine(Data((Self.result(id: id, [
                        "deleted": .number(String(paths.count)),
                        "recycled": .bool(false),
                        "failed": .array([]),
                    ]) + "\n").utf8))
                default:
                    conn.sendLine(Data((Self.result(id: id, [:]) + "\n").utf8))
                }
            }
        }
    }

    // MARK: 夹具

    private let t0: Int64 = 1_700_000_000_000

    private func fav(_ id: String, _ name: String, members: [FavMember]) -> Favorite {
        Favorite(id: id, name: name, createdAt: t0, members: members)
    }

    private func makeVM(_ fake: FakeDaemon, favorite: Favorite, selfDeviceId: String? = "dev-self") -> MixedViewModel {
        MixedViewModel(repo: MtdssRepository(client: IpcClient(endpoint: fake)), favorite: favorite, selfDeviceId: selfDeviceId)
    }

    private func device(_ id: String, online: Bool = true) -> DirectoryDevice {
        DirectoryDevice(deviceId: id, name: nil, platform: nil, online: online)
    }

    /// 轮询至条件成立(异步扇出/软窗收口;5s 上界,届满放行给断言报差异)。
    private func waitUntil(_ deadline: Date = Date().addingTimeInterval(5), _ cond: () -> Bool) async {
        while !cond() && Date() < deadline {
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    // MARK: ①快照分桶:离线成员初始即 offline 且 list 不发

    func testOfflineSeededImmediatelyWithoutListRequest() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a")])
        script.setRoute("/docs", [Self.file("a.txt", path: "/docs/a.txt", size: 3)])
        // 离线成员也配路由:若错误扇出,应答并入 → 状态断言快速报差异
        script.setRoute("/dark", [Self.file("ghost.txt", path: "/dark/ghost.txt", size: 1)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-a", path: "/docs"),
            FavMember(device: "dev-off", path: "/dark"), // 不在快照 = 离线(TS find ?? false 同象)
        ]))

        await vm.reload()

        XCTAssertEqual(script.listRequests, [ListReq(device: "dev-a", path: "/docs")], "只有在线成员发 list,离线零请求")
        XCTAssertEqual(vm.states.map(\.status), [.ok, .offline], "离线成员初始即 offline 桶")
        XCTAssertEqual(vm.states.map(\.id), ["dev-a\u{0000}/docs", "dev-off\u{0000}/dark"], "成员身份 = device\\0path")
        XCTAssertEqual(vm.states[0].entries.map(\.name), ["a.txt"], "在线成员应答并入")
        XCTAssertEqual(vm.states[1].entries, [])
        XCTAssertFalse(vm.loading)
        XCTAssertFalse(vm.truncated)
    }

    // MARK: ⑩快照病态折叠:重复 device_id 行折叠不炸(§13.7 崩溃修复回归锚)

    /// §13.7:devices 快照含重复 device_id 行时 ``MixedViewModel/reload``
    /// 曾用 ``Dictionary(uniqueKeysWithValues:)`` 直接 trap(用户报告形态:
    /// 点快捷行进不去即崩)。修复口径 = 折叠(后行覆盖前行,TS `new Map`
    /// 同象),绝不上抛。现网服务端 devices 表 device_id 为 PRIMARY KEY
    /// 不可产出此形态,本测试钉的是**防御面**:快照透传链任何一环出现
    /// 病态数据,iOS 侧降级而不崩。
    func testReloadFoldsDuplicateDeviceIdSnapshotRowsLastWins() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        // 病态快照:同 id 两行,offline 行在前、online 行在后(末行胜 =
        // 折叠后在线 → 正常扇出)
        script.setDevices([device("dev-a", online: false), device("dev-a", online: true)])
        script.setRoute("/a", [Self.file("a.txt", path: "/a/a.txt", size: 1)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [FavMember(device: "dev-a", path: "/a")]))

        await vm.reload()

        XCTAssertEqual(vm.states.map(\.status), [.ok], "重复行折叠(末行胜)后按在线扇出")
        XCTAssertEqual(script.listRequests.count, 1, "折叠为在线后正常发 list")
        XCTAssertEqual(script.deviceCalls, 1)
        XCTAssertEqual(vm.states[0].entries.map(\.name), ["a.txt"])
        XCTAssertFalse(vm.loading)
    }

    /// 同上防御面的 offline 侧:末行胜 = offline → 初始即 offline 桶零请求
    /// (与 ① 同象,只是上游给了重复行)。
    func testReloadFoldsDuplicateDeviceIdRowsOfflineWins() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a", online: true), device("dev-a", online: false)])
        // 配路由以暴露误扇出(折叠 offline 后不该发)
        script.setRoute("/a", [Self.file("a.txt", path: "/a/a.txt", size: 1)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [FavMember(device: "dev-a", path: "/a")]))

        await vm.reload()

        XCTAssertEqual(vm.states.map(\.status), [.offline], "末行胜 = offline,成员初始即离线桶")
        XCTAssertTrue(script.listRequests.isEmpty, "离线桶零 list 请求")
        XCTAssertFalse(vm.loading)
    }

    // MARK: ②resolve 并入 / reject→failed

    func testResolveMergesAndRejectMarksFailed() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a"), device("dev-b")])
        script.setRoute("/ok", [Self.file("a.txt", path: "/ok/a.txt", size: 3)])
        script.setFail("/bad")
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-a", path: "/ok"),
            FavMember(device: "dev-b", path: "/bad"),
        ]))

        await vm.reload()

        XCTAssertEqual(vm.states.map(\.status), [.ok, .failed], "reject→failed(entries 恒空)")
        XCTAssertEqual(vm.states[0].entries.map(\.name), ["a.txt"])
        XCTAssertTrue(vm.states[1].entries.isEmpty)
        XCTAssertFalse(vm.loading)
        XCTAssertFalse(vm.truncated)
    }

    // MARK: ③世代守卫:reload 中途再 reload,旧应答整包弃

    func testStaleGenerationResponseDroppedOnReload() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a")])
        script.setRoute("/docs", [Self.file("old.txt", path: "/docs/old.txt", size: 1)])
        script.listDelay = 0.3 // 首轮应答稳定在途
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [FavMember(device: "dev-a", path: "/docs")]))

        let first = Task { await vm.reload() }
        await waitUntil { script.listRequests.count >= 1 } // 首轮 list 已发出
        script.setRoute("/docs", [Self.file("new.txt", path: "/docs/new.txt", size: 2)])
        await vm.reload() // 第二次 reload:世代 +1
        await first.value // 旧世代迟到应答此刻才到,须整包弃

        XCTAssertEqual(script.listRequests.count, 2, "两轮各发一次 list")
        XCTAssertEqual(vm.states.map(\.status), [.ok])
        XCTAssertEqual(vm.states[0].entries.map(\.name), ["new.txt"], "旧世代应答不得覆盖新世代状态")
        XCTAssertFalse(vm.loading)
    }

    // MARK: ④updateFavorite:members 变更硬重扇出 / 仅名变不重扇出

    func testUpdateFavoriteMembersChangedRefansOutNameOnlyDoesNot() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a"), device("dev-b")])
        script.setRoute("/a", [Self.file("a1.txt", path: "/a/a1.txt", size: 1)])
        script.setRoute("/b", [Self.file("b1.txt", path: "/b/b1.txt", size: 2)])
        let vm = makeVM(fake, favorite: fav("f1", "旧名", members: [FavMember(device: "dev-a", path: "/a")]))
        await vm.reload()
        XCTAssertEqual(script.listRequests.count, 1)
        XCTAssertEqual(vm.states.count, 1)

        // 仅名变:引用面更新,不重扇出,既有成员状态不动
        vm.updateFavorite(fav("f1", "新名", members: [FavMember(device: "dev-a", path: "/a")]))
        XCTAssertEqual(vm.favorite.name, "新名")
        XCTAssertEqual(script.listRequests.count, 1, "仅名变不重扇出")
        XCTAssertEqual(vm.states.map(\.status), [.ok], "既有状态不动")

        // members 变更:硬重扇出(gen+states 重建,全员补拉)
        vm.updateFavorite(fav("f1", "新名", members: [
            FavMember(device: "dev-a", path: "/a"),
            FavMember(device: "dev-b", path: "/b"),
        ]))
        await waitUntil { vm.states.count == 2 && vm.states.allSatisfy { $0.status == .ok } }

        XCTAssertEqual(script.listRequests.count, 3, "新成员集全员重扇出(a/b 各一次)")
        XCTAssertEqual(vm.states.map(\.member.path), ["/a", "/b"], "states 按新成员序重建")
        XCTAssertEqual(vm.states.map(\.status), [.ok, .ok])
        XCTAssertFalse(vm.loading)
    }

    // MARK: ⑤visibleRows:文件夹恒前排序 + 同路径跨源两行两 key

    func testVisibleRowsSortFoldersFirstAndCrossSourceSamePathTwoKeys() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a"), device("dev-b")])
        script.setRoute("/a", [
            Self.file("a.txt", path: "/shared/a.txt", size: 5),
            Self.dir("工作", path: "/shared/工作"),
            Self.file("报告.docx", path: "/shared/报告.docx", size: 900),
        ])
        script.setRoute("/b", [
            Self.file("a.txt", path: "/shared/a.txt", size: 5), // 同路径跨源(条目同形)
        ])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-a", path: "/a"),
            FavMember(device: "dev-b", path: "/b"),
        ]))
        await vm.reload()

        let rows = vm.visibleRows()
        XCTAssertEqual(
            rows.map(\.entry.name),
            ["工作", "a.txt", "a.txt", "报告.docx"],
            "文件夹恒前 + 名称升序;跨源同名并列保持成员添加序",
        )
        XCTAssertEqual(
            rows.map(\.device),
            ["dev-a", "dev-a", "dev-b", "dev-a"],
            "行设备随排序;跨源同名并列 = 成员添加序(dev-a 行在前)",
        )
        XCTAssertEqual(rows.map(\.key), [
            "dev-a\u{0000}/shared/工作",
            "dev-a\u{0000}/shared/a.txt",
            "dev-b\u{0000}/shared/a.txt", // 同 path 跨源:两行两 key 并存不合并(Review Focus #2)
            "dev-a\u{0000}/shared/报告.docx",
        ])
        XCTAssertEqual(Set(rows.map(\.id)).count, 4, "行身份唯一")
        XCTAssertEqual(rows.first?.id, rows.first?.key, "MixedRow.id = key")

        // 排序切换即时生效(文件夹恒前;并列稳定 = 成员添加序)
        vm.sort = .size
        XCTAssertEqual(
            vm.visibleRows().map { "\($0.device)#\($0.entry.name)" },
            ["dev-a#工作", "dev-a#报告.docx", "dev-a#a.txt", "dev-b#a.txt"],
            "大小降序重排,文件夹恒前",
        )
    }

    // MARK: ⑥选择集复合键 / 全选回翻

    func testSelectionCompositeKeysSelectAllAndToggle() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a"), device("dev-b")])
        script.setRoute("/a", [Self.file("x.txt", path: "/a/x.txt", size: 1)])
        script.setRoute("/b", [Self.file("y.txt", path: "/b/y.txt", size: 2)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-a", path: "/a"),
            FavMember(device: "dev-b", path: "/b"),
        ]))
        await vm.reload()
        XCTAssertFalse(vm.isSelecting)
        XCTAssertTrue(vm.selection.isEmpty)

        vm.enterSelection()
        XCTAssertTrue(vm.isSelecting)
        XCTAssertTrue(vm.selection.isEmpty, "进入选择模式即清空")

        vm.selectAll()
        XCTAssertEqual(vm.selection, ["dev-a\u{0000}/a/x.txt", "dev-b\u{0000}/b/y.txt"], "全选 = 复合键全集")

        vm.selectAll() // 已全选再点 = 清空(BrowseViewModel 同款)
        XCTAssertTrue(vm.selection.isEmpty)

        vm.toggle("dev-a\u{0000}/a/x.txt")
        XCTAssertEqual(vm.selection, ["dev-a\u{0000}/a/x.txt"])
        vm.toggle("dev-a\u{0000}/a/x.txt")
        XCTAssertTrue(vm.selection.isEmpty, "再点同键取消")

        vm.exitSelection()
        XCTAssertFalse(vm.isSelecting)
        XCTAssertTrue(vm.selection.isEmpty)
    }

    // MARK: ⑦downloadSelected:逐组分组(跳本机组,dir=哨兵,anyFail→false)

    func testDownloadSelectedGroupsSkipSelfUseInboxSentinelAndAggregateFailure() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a"), device("dev-self"), device("dev-b")])
        script.setRoute("/a", [
            Self.file("n1.txt", path: "/a/n1.txt", size: 1),
            Self.file("n2.txt", path: "/a/n2.txt", size: 2),
        ])
        script.setRoute("/self", [Self.file("m1.txt", path: "/self/m1.txt", size: 3)])
        script.setRoute("/b", [Self.file("o1.txt", path: "/b/o1.txt", size: 4)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-a", path: "/a"),
            FavMember(device: "dev-self", path: "/self"),
            FavMember(device: "dev-b", path: "/b"),
        ]))
        await vm.reload()
        vm.enterSelection()
        vm.selectAll()

        let ok = await vm.downloadSelected()
        XCTAssertTrue(ok)
        XCTAssertEqual(script.downloads.count, 2, "本机组跳过,仅两远端组")
        XCTAssertEqual(script.downloads.map(\.device), ["dev-a", "dev-b"], "组序 = 渲染序首次出现序")
        XCTAssertEqual(script.downloads[0].paths, ["/a/n1.txt", "/a/n2.txt"], "组内保持渲染序")
        XCTAssertEqual(script.downloads[1].paths, ["/b/o1.txt"])
        XCTAssertTrue(script.downloads.allSatisfy { $0.dir == SendService.inboxSentinel }, "dir = 收件箱哨兵(R12 ① 口径)")

        // 任一组失败 → false(逐组不中止)
        script.setDownloadFail()
        let failed = await vm.downloadSelected()
        XCTAssertFalse(failed, "anyFail→false")
        XCTAssertEqual(script.downloads.count, 4, "逐组继续,不做早期中止")
    }

    // MARK: ⑧deleteSelected 门:全本机恒可删 / 含远端门关整批拒零外发

    func testDeleteSelectedGateLocalAlwaysRemoteNeedsGate() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-self"), device("dev-a")])
        script.setRoute("/self", [Self.file("m1.txt", path: "/self/m1.txt", size: 3)])
        script.setRoute("/a", [Self.file("a1.txt", path: "/a/a1.txt", size: 1)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-self", path: "/self"),
            FavMember(device: "dev-a", path: "/a"),
        ]))
        await vm.reload()
        vm.enterSelection()

        // 全本机组:门关仍可删(force=true,iOS 恒永久删)
        vm.toggle("dev-self\u{0000}/self/m1.txt")
        let localOk = await vm.deleteSelected(allowRemoteDelete: false)
        XCTAssertTrue(localOk, "全本机组恒可删")
        XCTAssertEqual(script.deletes, [DeleteReq(device: "dev-self", paths: ["/self/m1.txt"], force: true)])

        // 含远端组 + 门关:整批拒绝,零外发
        vm.toggle("dev-a\u{0000}/a/a1.txt")
        let gated = await vm.deleteSelected(allowRemoteDelete: false)
        XCTAssertFalse(gated, "含远端组须门开")
        XCTAssertEqual(script.deletes.count, 1, "门关零外发")

        // 门开:两组逐删(组序 = 渲染序首次出现序:a1.txt 渲染在 m1.txt 前)
        let open = await vm.deleteSelected(allowRemoteDelete: true)
        XCTAssertTrue(open)
        XCTAssertEqual(script.deletes.count, 3)
        XCTAssertEqual(script.deletes[1].device, "dev-a")
        XCTAssertEqual(script.deletes[1].paths, ["/a/a1.txt"])
        XCTAssertEqual(script.deletes[2].device, "dev-self")
        XCTAssertEqual(script.deletes[2].paths, ["/self/m1.txt"])
    }

    // MARK: ⑨sendSelected:跳过 from==to 组,upload 同形(哨兵)

    func testSendSelectedSkipsFromEqualsTargetGroup() async throws {
        let fake = FakeDaemon()
        let script = Script()
        fake.onLine = Script.handler(script)
        script.setDevices([device("dev-a"), device("dev-b")])
        script.setRoute("/a", [Self.file("a1.txt", path: "/a/a1.txt", size: 1)])
        script.setRoute("/b", [Self.file("b1.txt", path: "/b/b1.txt", size: 2)])
        let vm = makeVM(fake, favorite: fav("f1", "项目", members: [
            FavMember(device: "dev-a", path: "/a"),
            FavMember(device: "dev-b", path: "/b"),
        ]))
        await vm.reload()
        vm.enterSelection()
        vm.selectAll()

        let ok = await vm.sendSelected(to: DirectoryDevice(deviceId: "dev-a", name: "A", platform: "windows", online: true))
        XCTAssertTrue(ok)
        XCTAssertEqual(script.uploads.count, 1, "from==to 组跳过(远端拒绝自环)")
        XCTAssertEqual(script.uploads[0].files, ["/b/b1.txt"])
        XCTAssertEqual(script.uploads[0].to, "dev-a")
        XCTAssertEqual(script.uploads[0].dir, SendService.inboxSentinel, "SendService.send 同形 = 收件箱哨兵")
    }
}
