import XCTest
import MTDSSKit

/// 浏览视图模型单测(T5 brief Step 1):伪 daemon 脚本 list 应答,钉
/// - 名称排序文件夹恒前(默认 名称 升序);
/// - 搜索框按名称过滤当前列表;
/// - 隐藏项(dotfiles)默认过滤,显示开关打开后出现(T10 设置接线);
/// - 排序切换(名称/日期/大小/种类;文件夹恒前);
/// - 错误态(daemon error 帧 → errorText,清列表,loading 收口);
/// - 浏览器式历史(返回/前进/标题切换)与过期应答丢弃(seq 守卫);
/// - 请求帧形状 = 契约 list {device, path}(本机 = self device id + 绝对
///   路径,镜像 Kotlin 用法);
/// - 副文/授权根数据源(收件箱恒在)。
@MainActor
final class BrowseViewModelTests: XCTestCase {

    // MARK: 脚本助手

    private static func dir(_ name: String, path: String, mtime: Int64 = 0) -> DirEntry {
        DirEntry(name: name, path: path, size: 0, mtimeMs: mtime, isDir: true)
    }

    private static func file(_ name: String, path: String, size: Int64, mtime: Int64 = 0) -> DirEntry {
        DirEntry(name: name, path: path, size: size, mtimeMs: mtime, isDir: false)
    }

    private static func entryJSON(_ e: DirEntry) -> JSONValue {
        .object([
            "name": .string(e.name),
            "path": .string(e.path),
            "size": .number(String(e.size)),
            "mtime_ms": .number(String(e.mtimeMs)),
            "is_dir": .bool(e.isDir),
        ])
    }

    private static func listResponse(idRaw: String, entries: [DirEntry]) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "result": .object([
                "entries": .array(entries.map(entryJSON)),
                "truncated": .bool(false),
            ]),
        ]).canonical
    }

    private static func errorResponse(idRaw: String, code: Int64, message: String) -> String {
        JSONValue.object([
            "id": .number(idRaw),
            "error": .object([
                "code": .number(String(code)),
                "message": .string(message),
            ]),
        ]).canonical
    }

    /// list 应答脚本:path → 条目集(即时回)。
    private static func responder(_ routes: [String: [DirEntry]]) -> FakeDaemon.LineHandler {
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

        let openA = Task { await vm.open("/root/A") }
        await vm.open("/root/B")
        _ = await openA.result
        // 等 A 的迟到应答落地窗口
        try await Task.sleep(nanoseconds: 600_000_000)

        XCTAssertEqual(vm.path, "/root/B")
        XCTAssertEqual(vm.visibleEntries.map(\.name), ["b.txt"])
        XCTAssertNil(vm.errorText)
    }

    // MARK: 浏览器式历史(返回/前进)

    func testBackForwardHistoryAndTitleSwitch() async throws {
        let fake = FakeDaemon()
        fake.onLine = Self.responder([
            "/root": [Self.dir("sub", path: "/root/sub")],
            "/root/sub": [
                Self.dir("deep", path: "/root/sub/deep"),
                Self.file("x.txt", path: "/root/sub/x.txt", size: 1),
            ],
            "/root/sub/deep": [],
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
        XCTAssertTrue(vm.canGoForward) // 返回后可再前进
        await vm.goForward()
        XCTAssertEqual(vm.path, "/root/sub/deep")

        await vm.goBack()
        await vm.goBack()
        XCTAssertEqual(vm.path, "/root")
        XCTAssertEqual(vm.title, "收件箱")
        XCTAssertFalse(vm.canGoBack) // 已在浏览根:UI 弹回常用目录
        XCTAssertFalse(vm.canGoForward) // 回退后前进分支已截断
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

    // MARK: 副文格式化(「日期 - N 项」/「日期 - 大小」)

    func testSubtitleFormats() {
        let day = BrowseFormat.date(1_789_900_800_000)
        XCTAssertEqual(day.count, 10) // yyyy-MM-dd
        XCTAssertEqual(BrowseFormat.date(0), "") // 0 → 空

        let d = Self.dir("设计稿", path: "/root/设计稿", mtime: 1_789_900_800_000)
        let f = Self.file("a.bin", path: "/root/a.bin", size: 2 * 1024 * 1024, mtime: 1_789_900_800_000)

        // 文件「日期 - 大小」;目录数已知「日期 - N 项」;数未知(list 契约
        // 无子项数)退化为纯日期;mtime 0 → 空段剔除。
        XCTAssertEqual(BrowseFormat.size(0), "0 B")
        XCTAssertEqual(BrowseFormat.size(2 * 1024 * 1024), "2.0 MB")
        XCTAssertEqual(BrowseFormat.size(999), "999 B")
        XCTAssertEqual(BrowseFormat.subtitle(f), "\(day) - 2.0 MB")
        XCTAssertEqual(BrowseFormat.subtitle(d, childCount: 12), "\(day) - 12 项")
        XCTAssertEqual(BrowseFormat.subtitle(d), day)
        XCTAssertEqual(BrowseFormat.subtitle(Self.file("z", path: "/z", size: 7, mtime: 0)), "7 B") // mtime 0 → 仅大小段
    }

    // MARK: 常用目录授权根(spec §4.6:v1 = 收件箱单枚,信任目录 T10 追加)

    func testAuthorizedRootsInboxAlwaysPresentTrustedAppended() {
        let inbox = AuthorizedRoot.roots(recvDir: "/home/x/inbox")
        XCTAssertEqual(inbox, [AuthorizedRoot(name: "收件箱", path: "/home/x/inbox", isInbox: true)])

        XCTAssertTrue(AuthorizedRoot.roots(recvDir: nil).isEmpty)

        let trusted = AuthorizedRoot(name: "素材库", path: "/home/x/media", isInbox: false)
        let both = AuthorizedRoot.roots(recvDir: "/home/x/inbox", trusted: [trusted])
        XCTAssertEqual(both.map(\.name), ["收件箱", "素材库"])
    }
}
