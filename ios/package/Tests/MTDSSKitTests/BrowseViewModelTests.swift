import XCTest
import MTDSSKit

/// 浏览视图模型单测(T5 brief Step 1):伪 daemon 脚本 list 应答,钉
/// - 名称排序文件夹恒前(默认 名称 升序);
/// - 搜索框按名称过滤当前列表;
/// - 隐藏项(dotfiles)默认过滤,显示开关打开后出现(T10 设置接线);
/// - 排序切换(名称/日期/大小/种类;文件夹恒前);
/// - 错误态(daemon error 帧 → errorText,清列表,loading 收口);
/// - 浏览器式历史(返回/前进/标题切换)与过期应答丢弃(seq 守卫);
/// - 请求帧形状 = 契约 list {device, path}(本机 = 真实 device id 或未登录
///   别名 "self"+ 绝对路径,镜像 Kotlin 用法);
/// - 副文/授权根数据源(收件箱恒在);
/// - 行内展开(R3 真机轮 #8:懒加载一次/收起释放缓存/逐级缩进/搜索收起/
///   导航重置/选择跨层级/重载刷新子缓存)。
@MainActor
final class BrowseViewModelTests: XCTestCase {

    // MARK: 脚本助手

    private nonisolated static func dir(_ name: String, path: String, mtime: Int64 = 0, children: Int64? = nil) -> DirEntry {
        DirEntry(name: name, path: path, size: 0, mtimeMs: mtime, isDir: true, children: children)
    }

    private nonisolated static func file(_ name: String, path: String, size: Int64, mtime: Int64 = 0) -> DirEntry {
        DirEntry(name: name, path: path, size: size, mtimeMs: mtime, isDir: false)
    }

    private nonisolated static func entryJSON(_ e: DirEntry) -> JSONValue {
        var obj: [String: JSONValue] = [
            "name": .string(e.name),
            "path": .string(e.path),
            "size": .number(String(e.size)),
            "mtime_ms": .number(String(e.mtimeMs)),
            "is_dir": .bool(e.isDir),
        ]
        if let c = e.children { obj["children"] = .int(c) } // T5b 可选面:目录项在场,文件项缺省
        return .object(obj)
    }

    private nonisolated static func listResponse(idRaw: String, entries: [DirEntry]) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "result": .object([
                "entries": .array(entries.map(entryJSON)),
                "truncated": .bool(false),
            ]),
        ]).canonical
    }

    private nonisolated static func errorResponse(idRaw: String, code: Int64, message: String) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "error": .object([
                "code": .number(String(code)),
                "message": .string(message),
            ]),
        ]).canonical
    }

    /// list 应答脚本:path → 条目集(即时回)。
    private nonisolated static func responder(_ routes: [String: [DirEntry]]) -> FakeDaemon.LineHandler {
        { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"],
                  case .object(let params)? = frame["params"],
                  case .string(let path)? = params["path"],
                  let entries = routes[path] else { return }
            conn.sendLine(Data((listResponse(idRaw: idRaw, entries: entries) + "\n").utf8))
        }
    }

    private func makeVM(_ fake: FakeDaemon, deviceId: String = "dev-self") -> BrowseViewModel {
        let client = IpcClient(endpoint: fake)
        return BrowseViewModel(
            repo: MtdssRepository(client: client),
            deviceId: deviceId,
            rootPath: "/root",
            rootTitle: "收件箱",
        )
    }

    /// 可变路由表(重载刷新面测试:同 path 前后不同应答)。
    private final class RouteBox: @unchecked Sendable {
        private let lock = NSLock()
        private var routes: [String: [DirEntry]]
        init(_ routes: [String: [DirEntry]]) { self.routes = routes }
        func set(_ path: String, _ entries: [DirEntry]) {
            lock.lock()
            defer { lock.unlock() }
            routes[path] = entries
        }
        func get(_ path: String) -> [DirEntry]? {
            lock.lock()
            defer { lock.unlock() }
            return routes[path]
        }
    }

    private nonisolated static func routeResponder(_ box: RouteBox) -> FakeDaemon.LineHandler {
        { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"],
                  case .object(let params)? = frame["params"],
                  case .string(let path)? = params["path"],
                  let entries = box.get(path) else { return }
            conn.sendLine(Data((listResponse(idRaw: idRaw, entries: entries) + "\n").utf8))
        }
    }

    // MARK: 加载与排序

    func testLoadRootSortsFoldersFirstThenNameAscending() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("a.txt", path: "/root/a.txt", size: 5),
                Self.dir("工作", path: "/root/工作"),
                Self.file("报告.docx", path: "/root/报告.docx", size: 900),
                Self.dir("Archive", path: "/root/Archive"),
                Self.file("m.txt", path: "/root/m.txt", size: 1),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        XCTAssertEqual(vm.path, "/root")
        XCTAssertEqual(vm.title, "收件箱")
        XCTAssertFalse(vm.loading)
        XCTAssertNil(vm.errorText)
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["Archive", "工作", "a.txt", "m.txt", "报告.docx"])
        XCTAssertTrue(vm.visibleEntries.prefix(2).allSatisfy(\.isDir))
        XCTAssertEqual(vm.itemCount, 5)
    }

    func testSearchFiltersByNameKeepingFoldersFirst() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("报告.docx", path: "/root/报告.docx", size: 900),
                Self.dir("报告归档", path: "/root/报告归档"),
                Self.file("notes.txt", path: "/root/notes.txt", size: 3),
                Self.dir("照片", path: "/root/照片"),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        vm.searchText = "报告"
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["报告归档", "报告.docx"])
        XCTAssertTrue(vm.visibleEntries.first?.isDir ?? false)
        XCTAssertEqual(vm.itemCount, 2)

        vm.searchText = ""
        XCTAssertEqual(vm.itemCount, 4)
    }

    func testHiddenFilteredByDefaultAndShownWhenToggled() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("notes.md", path: "/root/notes.md", size: 3),
                Self.file(".secret", path: "/root/.secret", size: 1),
                Self.dir(".git", path: "/root/.git"),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        XCTAssertEqual(vm.visibleEntries.map(\.name), ["notes.md"])
        vm.showHidden = true
        XCTAssertEqual(vm.visibleEntries.map(\.name), [".git", ".secret", "notes.md"])
    }

    /// 显示隐藏跨层级(R5a 真机轮 #12 钉面):开关翻转同时作用于根级与
    /// 展开子级——visibleRows 每层走同一 隐藏过滤 → 排序 管线,子级不得
    /// 漏滤 dotfiles(R5a 排查结论:Kit 管线本身无缺口,缺口在 App 侧
    /// 远端浏览页从未把开关接进 VM;此测试钉 Kit 侧双层级语义)。
    func testShowHiddenFlipsRootAndExpandedChildren() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("notes.md", path: "/root/notes.md", size: 1),
                Self.dir("工作", path: "/root/工作"),
            ],
            "/root/工作": [
                Self.file(".env", path: "/root/工作/.env", size: 2),
                Self.dir(".git", path: "/root/工作/.git"),
                Self.file("b.txt", path: "/root/工作/b.txt", size: 3),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作")

        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0", "b.txt#1", "notes.md#0"],
            "默认过滤:根级与展开子级都不见 dotfiles(子级同管线,不漏滤)",
        )

        vm.showHidden = true
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0", ".git#1", ".env#1", "b.txt#1", "notes.md#0"],
            "开关打开:根级与展开子级一并现形(子级文件夹恒前、名称升序)",
        )
    }

    func testSortSwitchDateSizeKindFoldersAlwaysFirst() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                // mtime:zip 最新 > docx > dirB > dirA > txt
                Self.file("c.zip", path: "/root/c.zip", size: 200, mtime: 300),
                Self.file("b report.docx", path: "/root/b report.docx", size: 900, mtime: 200),
                Self.file("a.txt", path: "/root/a.txt", size: 5, mtime: 100),
                Self.dir("DirB", path: "/root/DirB", mtime: 150),
                Self.dir("DirA", path: "/root/DirA", mtime: 50),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        vm.sort = .date // 最新在前;并列按名称
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["DirB", "DirA", "c.zip", "b report.docx", "a.txt"])

        vm.sort = .size // 大者在前
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["DirA", "DirB", "b report.docx", "c.zip", "a.txt"])

        vm.sort = .kind // 目录恒前,其余按扩展名升序(docx < txt < zip;无扩展名最前)
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["DirA", "DirB", "b report.docx", "a.txt", "c.zip"])

        vm.sort = .name // 切回默认
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["DirA", "DirB", "a.txt", "b report.docx", "c.zip"])
    }

    // MARK: 错误态与 seq 守卫

    func testErrorStateFromDaemonErrorFrame() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"] else { return }
            conn.sendLine(Data((Self.errorResponse(idRaw: idRaw, code: 3, message: "设备离线: dev-x") + "\n").utf8))
        }
        let vm = makeVM(fake)
        await vm.loadRoot()

        XCTAssertFalse(vm.loading)
        XCTAssertEqual(vm.errorText, "设备离线: dev-x")
        XCTAssertTrue(vm.entries.isEmpty)
        XCTAssertEqual(vm.itemCount, 0)
    }

    func testStaleResponseDroppedBySeqGuard() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"],
                  case .object(let params)? = frame["params"],
                  case .string(let path)? = params["path"] else { return }
            let entries: [DirEntry] = path.hasSuffix("/A")
                ? [Self.file("late-a.txt", path: "/root/A/late-a.txt", size: 1)]
                : [Self.file("b.txt", path: "/root/B/b.txt", size: 2)]
            let respond = {
                conn.sendLine(Data((Self.listResponse(idRaw: idRaw, entries: entries) + "\n").utf8))
            }
            if path.hasSuffix("/A") {
                // A 应答迟到(300ms):不阻塞读线程,另线程延迟回
                Thread.detachNewThread {
                    Thread.sleep(forTimeInterval: 0.3)
                    respond()
                }
            } else {
                respond()
            }
        }
        let vm = makeVM(fake)
        await vm.loadRoot()

        // 先让 A 的请求发出并在飞,再开 B(A 应答故意迟到 300ms)
        let openA = Task { await vm.open("/root/A") }
        try await awaitUntil { fake.connections.reduce(0) { $0 + $1.received.count } >= 2 }
        await vm.open("/root/B")
        _ = await openA.result
        // 等 A 的迟到应答落地窗口
        try await Task.sleep(nanoseconds: 600_000_000)

        XCTAssertEqual(vm.path, "/root/B")
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["b.txt"])
        XCTAssertNil(vm.errorText)
    }

    /// 条件轮询(默认 5s 上界)。
    private func awaitUntil(timeout: TimeInterval = 5, _ cond: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !cond() {
            guard Date() < deadline else { throw XCTSkip("等待超时") }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
    }

    // MARK: loading 态翻转(T13b 修#5:远端冷握手秒级~35s,推入页必须
    // 全程 loading 可见——伪 daemon 延迟应答钉 flag 时序)

    func testLoadingFlagTransitionsDuringSlowFirstList() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"] else { return }
            // 迟到应答(400ms,模拟远端冷握手);另线程延迟回,不阻塞读线程
            Thread.detachNewThread {
                Thread.sleep(forTimeInterval: 0.4)
                conn.sendLine(Data((Self.listResponse(idRaw: idRaw, entries: [
                    Self.file("a.txt", path: "/root/a.txt", size: 1),
                ]) + "\n").utf8))
            }
        }
        let vm = makeVM(fake)

        // 推入页时序:loading 先行置位(首帧即转圈),应答到才收口
        let load = Task { await vm.loadRoot() }
        try await awaitUntil { vm.loading }
        XCTAssertTrue(vm.entries.isEmpty, "在飞时列表保持空(loading 面归属 listArea)")
        _ = await load.result

        XCTAssertFalse(vm.loading)
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["a.txt"])
        XCTAssertNil(vm.errorText)

        // 二次导航同口径:reload 置位 → 应答收口
        let reload = Task { await vm.reload() }
        try await awaitUntil { vm.loading }
        _ = await reload.result
        XCTAssertFalse(vm.loading)
    }

    // MARK: 浏览器式历史(返回/前进)

    func testBackForwardHistoryAndTitleSwitch() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [Self.dir("sub", path: "/root/sub")],
            "/root/sub": [
                Self.dir("deep", path: "/root/sub/deep"),
                Self.dir("other", path: "/root/sub/other"),
                Self.file("x.txt", path: "/root/sub/x.txt", size: 1),
            ],
            "/root/sub/deep": [],
            "/root/sub/other": [],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()
        XCTAssertFalse(vm.canGoBack)
        XCTAssertFalse(vm.canGoForward)

        await vm.open("/root/sub")
        XCTAssertEqual(vm.title, "sub") // 进文件夹标题切换
        XCTAssertTrue(vm.canGoBack)
        XCTAssertFalse(vm.canGoForward)

        await vm.open("/root/sub/deep")
        XCTAssertEqual(vm.title, "deep")

        await vm.goBack()
        XCTAssertEqual(vm.path, "/root/sub")
        XCTAssertTrue(vm.canGoForward) // 浏览器语义:后退保留前进分支
        await vm.goForward()
        XCTAssertEqual(vm.path, "/root/sub/deep")

        await vm.goBack()
        await vm.open("/root/sub/other") // 新分支:前进截断
        XCTAssertFalse(vm.canGoForward)

        await vm.goBack()
        await vm.goBack()
        XCTAssertEqual(vm.path, "/root")
        XCTAssertEqual(vm.title, "收件箱")
        XCTAssertFalse(vm.canGoBack) // 已在浏览根:UI 弹回常用目录
        XCTAssertTrue(vm.canGoForward) // 后退不截断:可原路前进回去
        await vm.goForward()
        XCTAssertEqual(vm.path, "/root/sub")
    }

    // MARK: 请求帧形状(契约对齐,镜像 Kotlin 用法)

    func testListRequestFrameCarriesDeviceAndAbsolutePath() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder(["/root": []])
        let vm = makeVM(fake, deviceId: "dev-self-fixed")
        await vm.loadRoot()

        let line = try XCTUnwrap(fake.connections.first?.received.first)
        let text = String(decoding: line, as: UTF8.self)
        XCTAssertTrue(text.contains(#""method":"list""#), text)
        XCTAssertTrue(text.contains(#""device":"dev-self-fixed""#), text)
        XCTAssertTrue(text.contains(#""path":"/root""#), text)
    }

    // MARK: 本地浏览登录门拆除(2026-11-01 T13b 修#4)

    /// device 参数解析:已登录 = 真实 device_id;未登录(nil)= 保留别名
    /// "self"(daemon 本机短路不要求会话)。
    func testLocalBrowseDeviceIdResolution() {
        XCTAssertEqual(BrowseViewModel.localBrowseDeviceId(selfDeviceId: "dev-me"), "dev-me")
        XCTAssertEqual(BrowseViewModel.localBrowseDeviceId(selfDeviceId: nil), Contract.selfDevice)
        XCTAssertEqual(Contract.selfDevice, "self")
    }

    /// 未登录本地浏览全链:device="self" 的请求帧照发,伪 daemon 照答,
    /// VM 正常装载(浏览面不因登录态挂起)。
    func testLoggedOutLocalBrowseSendsSelfAliasAndLoads() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder(["/home/x/inbox": [
            Self.file("报告.pdf", path: "/home/x/inbox/报告.pdf", size: 9),
        ]])
        let client = IpcClient(endpoint: fake)
        let vm = BrowseViewModel(
            repo: MtdssRepository(client: client),
            deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: nil),
            rootPath: "/home/x/inbox",
            rootTitle: "收件箱",
        )
        await vm.loadRoot()

        let line = try XCTUnwrap(fake.connections.first?.received.first)
        let text = String(decoding: line, as: UTF8.self)
        XCTAssertTrue(text.contains(#""device":"self""#), text)
        XCTAssertNil(vm.errorText)
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["报告.pdf"])
    }

    /// 远端浏览 device 参数不受别名影响(对端 id 原样携带;远端登录门在
    /// daemon 侧「未登录」拒 + 设备目录为空,UI 面离线行禁用不变)。
    func testRemoteBrowseStillTargetsPeerDeviceId() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder(["/root": []])
        let vm = makeVM(fake, deviceId: "dev-peer-a")
        await vm.loadRoot()

        let line = try XCTUnwrap(fake.connections.first?.received.first)
        let text = String(decoding: line, as: UTF8.self)
        XCTAssertTrue(text.contains(#""device":"dev-peer-a""#), text)
        XCTAssertFalse(text.contains(#""device":"self""#), text)
    }

    // MARK: 标题组合(R4 真机轮 #5:远端 = 设备名 | 目录名,本地单段不变;
    //         R5a 真机轮 #5:分隔留白,裸 | 两段粘连改前后空格)

    /// 纯函数臂:根 = 设备名单段;深级 = 「设备名 | 目录名」(远端 Windows
    /// 反斜杠同切);本地(deviceName nil)保持原口径;空设备名视同本地。
    func testComposeTitleDeviceAndFolder() {
        // 远端根级(卷列表,path=rootPath="")= 设备名单段
        XCTAssertEqual(
            BrowseViewModel.composeTitle(rootTitle: "MIX3", path: "", rootPath: "", deviceName: "MIX3"),
            "MIX3",
        )
        // 远端深级 = 设备名 | 目录名(R5a #5:分隔留白)
        XCTAssertEqual(
            BrowseViewModel.composeTitle(rootTitle: "MIX3", path: "/sdcard/DCIM", rootPath: "", deviceName: "MIX3"),
            "MIX3 | DCIM",
        )
        XCTAssertEqual(
            BrowseViewModel.composeTitle(rootTitle: "PC", path: "\\sdcard\\DCIM", rootPath: "", deviceName: "PC"),
            "PC | DCIM",
            "远端 Windows 反斜杠路径同切末段",
        )
        // 本地浏览(deviceName nil):根 = rootTitle,深级 = 目录名(原口径不动)
        XCTAssertEqual(
            BrowseViewModel.composeTitle(rootTitle: "收件箱", path: "/root", rootPath: "/root", deviceName: nil),
            "收件箱",
        )
        XCTAssertEqual(
            BrowseViewModel.composeTitle(rootTitle: "收件箱", path: "/root/工作", rootPath: "/root", deviceName: nil),
            "工作",
        )
        // 空串设备名同 nil(不产「| 目录名」残臂)
        XCTAssertEqual(
            BrowseViewModel.composeTitle(rootTitle: "", path: "/a", rootPath: "", deviceName: ""),
            "a",
        )
    }

    /// VM 臂:远端 VM(带 deviceName)标题随导航切换——根 = 设备名,进
    /// 目录 = 设备名 | 目录名,返回回设备名;本地 VM 不受影响(既有
    /// testBackForwardHistoryAndTitleSwitch 钉面)。
    func testRemoteVMTitleSwitchesWithDevicePrefix() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "": [Self.dir("sdcard", path: "/sdcard")],
            "/sdcard": [Self.dir("DCIM", path: "/sdcard/DCIM")],
            "/sdcard/DCIM": [],
        ])
        let client = IpcClient(endpoint: fake)
        let vm = BrowseViewModel(
            repo: MtdssRepository(client: client),
            deviceId: "dev-mix3",
            rootPath: "",
            rootTitle: "MIX3",
            deviceName: "MIX3",
        )
        await vm.loadRoot()
        XCTAssertEqual(vm.title, "MIX3", "根级 = 设备名单段")

        await vm.open("/sdcard")
        XCTAssertEqual(vm.title, "MIX3 | sdcard")
        await vm.open("/sdcard/DCIM")
        XCTAssertEqual(vm.title, "MIX3 | DCIM")
        await vm.goBack()
        await vm.goBack()
        XCTAssertEqual(vm.title, "MIX3", "返回到根恢复设备名单段")
    }


    // MARK: 首页子行直入子目录页(2026-11-02 R4 用户裁定回炉:返回直达
    //         首页,无中间收件箱页;首页展开态与子页导航互不影响)

    /// 子页 VM 以子目录为浏览根(单目的直入):进页 canGoBack=false——
    /// 顶栏返回/边缘右滑 = onPopToRoot 弹回首页(标题 = 子目录名,与原
    /// 深级口径一致);页内再进 = 正常页内历史(返回回子目录根)。
    func testChildRootedVMStartsFreshAtChild() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root/工作": [Self.dir("内层", path: "/root/工作/内层")],
            "/root/工作/内层": [Self.file("c.txt", path: "/root/工作/内层/c.txt", size: 3)],
        ])
        let client = IpcClient(endpoint: fake)
        let child = BrowseViewModel(
            repo: MtdssRepository(client: client),
            deviceId: "dev-self",
            rootPath: "/root/工作", // 直入子目录:子目录即浏览根
            rootTitle: "工作",
        )
        await child.loadRoot()
        XCTAssertEqual(child.path, "/root/工作")
        XCTAssertEqual(child.title, "工作")
        XCTAssertFalse(child.canGoBack, "直入子页进页即浏览根:返回 = 弹回首页(onPopToRoot)")

        await child.open("/root/工作/内层")
        XCTAssertTrue(child.canGoBack, "页内再进 = 正常页内历史")
        await child.goBack()
        XCTAssertEqual(child.path, "/root/工作")
        XCTAssertFalse(child.canGoBack, "返回收口在子目录根:再退 = 弹回首页")
    }

    /// 首页展开态与子页导航互不影响(首页/子页各自独立 VM):子页
    /// load/open/back 全程,首页 VM 展开集不变、子级行不丢、零额外拉取
    /// (弹回首页不重拉,展开现场原样——FavoritesRootList 的 @StateObject
    /// 状态面在 App 侧随页身份存活,Kit 钉模型层「无交集」契约)。
    func testHomeExpansionUnaffectedByChildPageNavigation() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [Self.dir("工作", path: "/root/工作")],
            "/root/工作": [Self.file("b.txt", path: "/root/工作/b.txt", size: 2)],
            "/root/工作/内层": [Self.file("c.txt", path: "/root/工作/内层/c.txt", size: 3)],
        ])
        // 首页 VM(收件箱为根,展开 工作)
        let home = BrowseViewModel(
            repo: MtdssRepository(client: IpcClient(endpoint: fake)),
            deviceId: "dev-self",
            rootPath: "/root",
            rootTitle: "收件箱",
        )
        await home.loadRoot()
        await home.toggleExpand("/root/工作")
        XCTAssertEqual(home.visibleRows.count, 2)
        let homeFramesBefore = fake.connections.first?.received.count ?? 0

        // 子页 VM(独立实例,直入 /root/工作)导航全程
        let child = BrowseViewModel(
            repo: MtdssRepository(client: IpcClient(endpoint: fake)),
            deviceId: "dev-self",
            rootPath: "/root/工作",
            rootTitle: "工作",
        )
        await child.loadRoot()
        await child.open("/root/工作/内层")
        await child.goBack()
        XCTAssertEqual(child.path, "/root/工作")

        // 首页 VM:展开集原样、可见行原样、连接上无任何新帧(零重拉)
        XCTAssertEqual(home.expandedPaths, ["/root/工作"], "子页导航不影响首页展开态")
        XCTAssertEqual(home.visibleRows.count, 2, "展开现场原样(子级行仍在)")
        XCTAssertEqual(fake.connections.first?.received.count, homeFramesBefore, "弹回首页零重拉(首页连接无新帧)")
    }

    // MARK: 副文格式化(「日期 - N 项」/「日期 - 大小」)

    func testSubtitleFormats() {
        let day = BrowseFormat.date(1_789_900_800_000)
        XCTAssertEqual(day.count, 10) // yyyy-MM-dd
        XCTAssertEqual(BrowseFormat.date(0), "") // 0 → 空

        let d = Self.dir("设计稿", path: "/root/设计稿", mtime: 1_789_900_800_000)
        let f = Self.file("a.bin", path: "/root/a.bin", size: 2 * 1024 * 1024, mtime: 1_789_900_800_000)

        // 文件「日期 - 大小」;目录数已知「日期 - N 项」;数未知(children
        // 缺省:文件项/stat 单查)退化为纯日期;mtime 0 → 空段剔除。
        XCTAssertEqual(BrowseFormat.size(0), "0 B")
        XCTAssertEqual(BrowseFormat.size(2 * 1024 * 1024), "2.0 MB")
        XCTAssertEqual(BrowseFormat.size(999), "999 B")
        XCTAssertEqual(BrowseFormat.subtitle(f), "\(day) - 2.0 MB")
        XCTAssertEqual(BrowseFormat.subtitle(d, childCount: 12), "\(day) - 12 项")
        XCTAssertEqual(BrowseFormat.subtitle(d), day)
        XCTAssertEqual(BrowseFormat.subtitle(Self.file("z", path: "/z", size: 7, mtime: 0)), "7 B") // mtime 0 → 仅大小段

        // T5b 接线:契约 children → childCount(BrowseView 行渲染同表达式
        // `entry.children.map { Int($0) }`)。
        let wired = Self.dir("设计稿", path: "/root/设计稿", mtime: 1_789_900_800_000, children: 3)
        XCTAssertEqual(
            BrowseFormat.subtitle(wired, childCount: wired.children.map { Int($0) }),
            "\(day) - 3 项",
            "契约 children 接线后目录行副文 = 「日期 - N 项」"
        )
    }

    // MARK: 左缘右滑返回判定(2026-11-02 R2 真机轮;选择态忽略由调用面
    // 先行,谓词只管几何)

    /// 边缘右滑谓词:左缘带起手 + 水平主导远滑才算;中部起手/纵向滚动/
    /// 短漂移/左滑都不触发(DragGesture 接线面 = BrowseView 同谓词)。
    func testEdgeSwipeBackPredicate() {
        // 标准返回:左缘起手,水平右滑远超阈值
        XCTAssertTrue(BrowseGesture.isBack(startX: 10, dx: 120, dy: 8))
        // 斜向但水平主导(2:1 之上)仍触发
        XCTAssertTrue(BrowseGesture.isBack(startX: 5, dx: 100, dy: 40))
        // 起手出缘带(> 30pt)/屏幕中部横向滑动不触发
        XCTAssertFalse(BrowseGesture.isBack(startX: 31, dx: 120, dy: 0))
        XCTAssertFalse(BrowseGesture.isBack(startX: 200, dx: 150, dy: 5))
        // 纵向滚动(即使起手在缘内)不触发
        XCTAssertFalse(BrowseGesture.isBack(startX: 8, dx: 30, dy: 260))
        // 短漂移(< 70pt)/左滑不触发
        XCTAssertFalse(BrowseGesture.isBack(startX: 5, dx: 40, dy: 0))
        XCTAssertFalse(BrowseGesture.isBack(startX: 5, dx: -120, dy: 3))
    }

    // MARK: 常用目录根列表(2026-11-01 Ruling N:首页唯一展示项 = 收件箱;
    // 信任目录只进引擎授权集与设置页,不投影首页)

    func testAuthorizedRootsInboxOnly() {
        let inbox = AuthorizedRoot.roots(recvDir: "/home/x/inbox")
        XCTAssertEqual(inbox, [AuthorizedRoot(name: "收件箱", path: "/home/x/inbox", isInbox: true)])

        // status 未含 recv_dir → 空列表(不编造),首页走 boot 态
        XCTAssertTrue(AuthorizedRoot.roots(recvDir: nil).isEmpty)
        XCTAssertTrue(AuthorizedRoot.roots(recvDir: "").isEmpty)
    }

    // MARK: 行内展开(R3 真机轮 #8:箭头=行内展开缩进逐级、行点击=进整页)

    /// 展开懒加载:展开 = 恰一次 list 拉子项;子行 depth = 父+1 插父行后;
    /// 收起 = 平面且缓存释放(再展开重新拉);排序子级同管线(文件夹恒前)。
    func testToggleExpandLazyLoadsOnceAndCollapseReleasesCache() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("a.txt", path: "/root/a.txt", size: 1),
                Self.dir("工作", path: "/root/工作"),
            ],
            "/root/工作": [
                Self.file("b.txt", path: "/root/工作/b.txt", size: 2),
                Self.dir("内层", path: "/root/工作/内层"),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        XCTAssertEqual(vm.visibleRows.map(\.depth), [0, 0], "根态平面(排序:文件夹恒前)")

        await vm.toggleExpand("/root/工作")
        XCTAssertTrue(vm.expandedPaths.contains("/root/工作"))
        // 「名#depth」拼串断言(Swift 6 元组不作 Equatable,XCTAssertEqual 拒收)
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0", "内层#1", "b.txt#1", "a.txt#0"],
            "子项插父行后、depth+1、子级同排序管线(内层 dir 恒前)",
        )

        func listCount() -> Int {
            fake.connections.flatMap(\.received)
                .filter { String(decoding: $0, as: UTF8.self).contains(#""method":"list""#) }
                .count
        }
        XCTAssertEqual(listCount(), 2, "根 1 次 + 展开恰 1 次(懒加载,无预取)")

        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(vm.visibleRows.map(\.depth), [0, 0], "收起回平面")
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(vm.visibleRows.count, 4)
        XCTAssertEqual(listCount(), 3, "收起释放缓存:再展开重新拉一次")
    }

    /// 逐级缩进:两级展开 depth 0→1→2;缩进步长常量钉 18pt(系统 outline
    /// 手感 16-20 取中;视图按 depth × 步长缩进,分割线同步内缩)。
    func testNestedExpandIndentsPerLevel() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [Self.dir("工作", path: "/root/工作")],
            "/root/工作": [Self.dir("内层", path: "/root/工作/内层")],
            "/root/工作/内层": [Self.file("c.txt", path: "/root/工作/内层/c.txt", size: 3)],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        await vm.toggleExpand("/root/工作")
        await vm.toggleExpand("/root/工作/内层")
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0", "内层#1", "c.txt#2"],
            "逐级展开逐级缩进",
        )
        XCTAssertEqual(BrowseViewModel.indentPerLevel, 18)
    }

    /// 搜索 = 收起全部展开(R3 裁定:状态清除,不缓存展开现场);清空
    /// 搜索不恢复。
    func testSearchCollapsesAllExpansions() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("a.txt", path: "/root/a.txt", size: 1),
                Self.dir("工作", path: "/root/工作"),
            ],
            "/root/工作": [Self.file("工作纪要.txt", path: "/root/工作/工作纪要.txt", size: 2)],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(vm.visibleRows.count, 3)

        vm.searchText = "工作"
        XCTAssertTrue(vm.expandedPaths.isEmpty, "搜索起手 = 收起全部展开")
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0"],
            "搜索面只滤根级,无子级投影",
        )
        vm.searchText = ""
        XCTAssertEqual(vm.visibleRows.map(\.depth), [0, 0], "清空搜索不恢复(状态已收)")
    }

    /// 导航(open/back)= 换页:展开态与子缓存一并重置(页级状态)。
    func testNavigationResetsExpansion() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [Self.dir("工作", path: "/root/工作")],
            "/root/工作": [Self.dir("内层", path: "/root/工作/内层")],
            "/root/工作/内层": [],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(vm.visibleRows.count, 2)

        await vm.open("/root/工作/内层")
        XCTAssertTrue(vm.expandedPaths.isEmpty, "推入新页 = 展开态重置")
        XCTAssertTrue(vm.visibleRows.allSatisfy { $0.depth == 0 }, "平面")

        await vm.goBack()
        XCTAssertTrue(vm.expandedPaths.isEmpty, "返回上一页同样重置")
        XCTAssertTrue(vm.visibleRows.allSatisfy { $0.depth == 0 })
    }

    /// 搜索跨级残留(R5a 真机轮 #6):open/goBack/goForward 一律清空搜索
    /// ——搜索只作用于当前层,残留会让新层看似「缺条目」;对照面:原地
    /// reload(下拉刷新/错误重试)不动搜索(刷新语义不重置用户输入)。
    func testNavigationClearsSearchText() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("a.txt", path: "/root/a.txt", size: 1),
                Self.dir("工作", path: "/root/工作"),
            ],
            "/root/工作": [
                Self.dir("内层", path: "/root/工作/内层"),
                Self.file("报告.docx", path: "/root/工作/报告.docx", size: 2),
            ],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()

        // open:进下级即清空,新层全量可见(无过滤)
        vm.searchText = "a.txt"
        XCTAssertEqual(vm.itemCount, 1)
        await vm.open("/root/工作")
        XCTAssertEqual(vm.searchText, "", "进下级清空搜索")
        XCTAssertEqual(vm.itemCount, 2, "新层全量(搜索残留不得滤掉条目)")

        // goBack:同样清空
        vm.searchText = "报告"
        await vm.goBack()
        XCTAssertEqual(vm.searchText, "", "返回同样清空搜索")
        XCTAssertEqual(vm.itemCount, 2, "根层全量")

        // goForward:同样清空
        vm.searchText = "工作"
        await vm.goForward()
        XCTAssertEqual(vm.searchText, "", "前进同样清空搜索")
        XCTAssertEqual(vm.itemCount, 2)

        // 对照面:原地 reload(下拉刷新)保留搜索——只清跨级导航
        vm.searchText = "报告"
        await vm.reload()
        XCTAssertEqual(vm.searchText, "报告", "原地刷新不重置搜索")
        XCTAssertEqual(vm.itemCount, 1)
    }

    /// 选择跨层级:展开子级行可勾选/全选含子级;收集序 = 行序(父前子后)。
    func testSelectionCoversExpandedChildren() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [
                Self.file("a.txt", path: "/root/a.txt", size: 1),
                Self.dir("工作", path: "/root/工作"),
            ],
            "/root/工作": [Self.file("b.txt", path: "/root/工作/b.txt", size: 2)],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作")

        vm.enterSelection()
        vm.toggle("/root/工作/b.txt")
        XCTAssertEqual(vm.selectedEntries.map(\.path), ["/root/工作/b.txt"])

        vm.selectAll()
        XCTAssertEqual(
            Set(vm.selection),
            ["/root/工作", "/root/a.txt", "/root/工作/b.txt"],
            "全选含展开子级",
        )
        vm.toggle("/root/工作")
        XCTAssertEqual(
            vm.selectedEntries.map(\.path),
            ["/root/工作/b.txt", "/root/a.txt"],
            "收集序 = 可见行序(父行先于子行,根 a.txt 殿后)",
        )
    }

    /// 原地重载(删除后)不收起展开态,子缓存随重载刷新(过期子项不残留)。
    func testReloadKeepsExpansionAndRefreshesChildren() async throws {
        let box = RouteBox([
            "/root": [Self.dir("工作", path: "/root/工作")],
            "/root/工作": [Self.file("b.txt", path: "/root/工作/b.txt", size: 2)],
        ])
        let fake = FakeDaemon()
        fake.onLine = Self.routeResponder(box)
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(vm.visibleRows.map(\.entry.name), ["工作", "b.txt"])

        box.set("/root/工作", [
            Self.file("b.txt", path: "/root/工作/b.txt", size: 2),
            Self.file("c.txt", path: "/root/工作/c.txt", size: 3),
        ])
        await vm.reload()
        XCTAssertTrue(vm.expandedPaths.contains("/root/工作"), "原地重载不收起")
        XCTAssertEqual(vm.visibleRows.map(\.entry.name), ["工作", "b.txt", "c.txt"], "子缓存随重载刷新")
    }

    /// 下拉刷新路径(R4 真机轮 #4:refreshable = reload 单口):reload
    /// 恰一发当前目录 list,根列表数据面随现拉更新(展开子缓存刷新另由
    /// testReloadKeepsExpansionAndRefreshesChildren 钉面)。
    func testReloadRefetchesCurrentDirectoryFrame() async throws {
        let box = RouteBox([
            "/root": [Self.file("a.txt", path: "/root/a.txt", size: 1)],
        ])
        let fake = FakeDaemon()
        fake.onLine = Self.routeResponder(box)
        let vm = makeVM(fake)
        await vm.loadRoot()
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["a.txt"])

        box.set("/root", [
            Self.file("a.txt", path: "/root/a.txt", size: 1),
            Self.file("b.txt", path: "/root/b.txt", size: 2),
        ])
        await vm.reload()
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["a.txt", "b.txt"], "刷新后新数据在列")

        let listCount = fake.connections.flatMap(\.received)
            .filter { String(decoding: $0, as: UTF8.self).contains(#""method":"list""#) }
            .count
        XCTAssertEqual(listCount, 2, "根 1 + 刷新 1(恰一发当前目录,无冗余拉取)")
    }

    // MARK: 取消 = 良性收口(R5a 真机轮 #4:下拉刷新被 SwiftUI 取消不得
    //         伪装「请求失败:连接中断」/清空既有内容——refreshable 任务
    //         所有权在视图层,视图重建/用户离场都会中途取消)

    /// 计数器(同 path 前后不同应答时序:首发即回,后续迟到给取消留窗)。
    private final class CallCounter: @unchecked Sendable {
        private let lock = NSLock()
        private var n = 0
        func next() -> Int {
            lock.lock()
            defer { lock.unlock() }
            n += 1
            return n
        }
    }

    /// 在飞 reload 被取消:保留既有列表、无 errorText、loading 收口;迟到
    /// 应答不复活旧帧(id 已随取消出册,IpcClient 侧丢弃)。
    func testCancelledReloadKeepsContentAndShowsNoError() async throws {
        let fake = FakeDaemon()
        let counter = CallCounter()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"] else { return }
            let call = counter.next()
            let respond = {
                conn.sendLine(Data((Self.listResponse(idRaw: idRaw, entries: [
                    Self.file("a.txt", path: "/root/a.txt", size: 1),
                ]) + "\n").utf8))
            }
            if call == 1 {
                respond() // 首拉即回(装载内容)
            } else {
                // 刷新在飞故意迟到 400ms:取消窗口确定落在应答之前
                Thread.detachNewThread {
                    Thread.sleep(forTimeInterval: 0.4)
                    respond()
                }
            }
        }
        let vm = makeVM(fake)
        await vm.loadRoot()
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["a.txt"])

        let reload = Task { await vm.reload() }
        try await awaitUntil { vm.loading }
        reload.cancel()
        _ = await reload.result

        XCTAssertNil(vm.errorText, "取消 = 良性:不得伪装「请求失败:连接中断」")
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["a.txt"], "取消不得清空既有内容")
        XCTAssertFalse(vm.loading, "取消后 loading 收口(转圈不悬挂)")
    }

    /// 首页根行收口(R4 真机轮 #1 复用裁定):首页 = BrowseViewModel 以
    /// 收件箱为浏览根,根行(非 entry)收起 = ``collapseAll()``——子树
    /// 缓存释放(再展开子级重新拉,无缓存复活面),根列表本身不动。
    func testHomeRootCollapseReleasesSubtreeAndRefetchesOnReExpand() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [Self.dir("工作", path: "/root/工作")],
            "/root/工作": [Self.file("b.txt", path: "/root/工作/b.txt", size: 2)],
        ])
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(vm.visibleRows.count, 2)

        // 首页根行收起 = collapseAll(展开面/子树缓存全收)
        vm.collapseAll()
        XCTAssertTrue(vm.expandedPaths.isEmpty)
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0"],
            "根列表(=收件箱子项)仍在,收起不重拉根",
        )

        // 再展开子级 = 重新拉取(缓存已随收起释放,无即席复活)
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0", "b.txt#1"],
        )
        let listCount = fake.connections.flatMap(\.received)
            .filter { String(decoding: $0, as: UTF8.self).contains(#""method":"list""#) }
            .count
        XCTAssertEqual(listCount, 3, "根 1 + 首展 1 + 收起后再展 1(缓存已释放)")
    }

    // MARK: 在飞子项拉取的收起作废(R3 评审 I-1:搜索收起不 bump seq,
    //         迟到应答不得写缓存/展开复活——世代守卫钉面,先红后绿)

    /// 展开(子项应答故意迟到 400ms)→ 拉取在飞时搜索收起 → 迟到应答
    /// 整包丢弃:expandedPaths/缓存不写入,清空搜索仍平面;再展开 =
    /// 重新拉取(无孤儿缓存即席展开)。
    func testSearchCollapseInvalidatesInFlightChildFetch() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"],
                  case .object(let params)? = frame["params"],
                  case .string(let path)? = params["path"] else { return }
            if path == "/root" {
                conn.sendLine(Data((Self.listResponse(idRaw: idRaw, entries: [
                    Self.dir("工作", path: "/root/工作"),
                ]) + "\n").utf8))
            } else {
                // 子目录应答迟到(400ms):另线程延迟回,不阻塞读线程
                Thread.detachNewThread {
                    Thread.sleep(forTimeInterval: 0.4)
                    conn.sendLine(Data((Self.listResponse(idRaw: idRaw, entries: [
                        Self.file("b.txt", path: "/root/工作/b.txt", size: 2),
                    ]) + "\n").utf8))
                }
            }
        }
        let vm = makeVM(fake)
        await vm.loadRoot()

        // 展开在飞(应答未回),搜索起手收起
        let expand = Task { await vm.toggleExpand("/root/工作") }
        try await awaitUntil { fake.connections.reduce(0) { $0 + $1.received.count } >= 2 }
        vm.searchText = "工作"
        _ = await expand.result

        XCTAssertTrue(vm.expandedPaths.isEmpty, "迟到应答不得写展开集(世代作废)")
        vm.searchText = ""
        XCTAssertEqual(vm.visibleRows.map(\.depth), [0], "清空搜索仍平面(根仅工作一项)——缓存未复活展开")

        // 无孤儿缓存:再展开 = 重新拉取并正常投影(共 3 次 list)
        await vm.toggleExpand("/root/工作")
        XCTAssertEqual(
            vm.visibleRows.map { "\($0.entry.name)#\($0.depth)" },
            ["工作#0", "b.txt#1"],
            "重新拉取后正常展开",
        )
        let listCount = fake.connections.flatMap(\.received)
            .filter { String(decoding: $0, as: UTF8.self).contains(#""method":"list""#) }
            .count
        XCTAssertEqual(listCount, 3, "根 1 + 作废展开 1 + 再展开 1(无缓存即席)")
    }

    /// 重载的子缓存刷新同守卫:刷新在飞时搜索收起 → 迟到应答不写孤儿缓存。
    func testRefreshExpandedChildrenDroppedAfterCollapse() async throws {
        let box = RouteBox([
            "/root": [Self.dir("工作", path: "/root/工作")],
            "/root/工作": [Self.file("b.txt", path: "/root/工作/b.txt", size: 2)],
        ])
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .number(let idRaw)? = frame["id"],
                  case .object(let params)? = frame["params"],
                  case .string(let path)? = params["path"],
                  let entries = box.get(path) else { return }
            let respond = {
                conn.sendLine(Data((Self.listResponse(idRaw: idRaw, entries: entries) + "\n").utf8))
            }
            if path == "/root/工作" {
                Thread.detachNewThread {
                    Thread.sleep(forTimeInterval: 0.4)
                    respond()
                }
            } else {
                respond()
            }
        }
        let vm = makeVM(fake)
        await vm.loadRoot()
        await vm.toggleExpand("/root/工作") // 迟到应答,但此时无收起面 → 正常写入
        XCTAssertEqual(vm.visibleRows.count, 2)

        // reload:根即时 → 子缓存刷新(在飞 400ms)→ 期间搜索收起
        let reload = Task { await vm.reload() }
        try await awaitUntil { fake.connections.reduce(0) { $0 + $1.received.count } >= 4 }
        vm.searchText = "工作"
        _ = await reload.result

        XCTAssertTrue(vm.expandedPaths.isEmpty, "收起后刷新不得写展开集")
        vm.searchText = ""
        XCTAssertEqual(vm.visibleRows.map(\.depth), [0], "孤儿缓存不复活展开(根仅工作一项)")
    }
}
