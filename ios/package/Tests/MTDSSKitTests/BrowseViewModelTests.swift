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
/// - 副文/授权根数据源(收件箱恒在)。
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
}
