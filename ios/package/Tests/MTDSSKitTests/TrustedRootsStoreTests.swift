import MTDSSKit
import XCTest

/// 信任目录账本单测(T10 brief Step 1,先红后绿):书签 encode→resolve **真
/// 往返**(macOS 宿主 Foundation 书签全可用;security-scoped startAccessing
/// 是 iOS 面,协议化注入 ``BookmarkCodec``),账本持久化/stale 剔除/引擎
/// conf 文件格式(与 Rust 桥 parse 同口径跨语言锚定)。
@MainActor
final class TrustedRootsStoreTests: XCTestCase {

    // MARK: 夹具

    private nonisolated static func newTmp() -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("t10-trust-\(UUID().uuidString)", isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func makeDir(under root: URL, _ name: String) throws -> URL {
        let url = root.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func makeStore(root: URL) -> TrustedRootsStore {
        TrustedRootsStore(ledgerURL: root.appendingPathComponent("trust-roots.json"))
    }

    // MARK: 书签 encode → resolve 真往返(同路径)

    func testBookmarkRoundTripSamePath() throws {
        let root = Self.newTmp()
        defer { try? FileManager.default.removeItem(at: root) }
        let dir = try makeDir(under: root, "照片备份")
        let codec = FoundationBookmarkCodec()

        let data = try codec.bookmarkData(for: dir)
        XCTAssertFalse(data.isEmpty)
        let resolved = try codec.resolve(data)

        XCTAssertEqual(
            resolved.url.standardizedFileURL.path,
            dir.standardizedFileURL.path,
            "encode→resolve 往返应落同一目录",
        )
        XCTAssertFalse(resolved.stale, "目标存在时不应报 stale")
    }

    // MARK: 账本持久化(add/remove 跨 store 重建)

    func testAddPersistsAcrossStoreReinit() throws {
        let root = Self.newTmp()
        defer { try? FileManager.default.removeItem(at: root) }
        let dir = try makeDir(under: root, "docs")

        let store = makeStore(root: root)
        store.load()
        let added = try store.add(url: dir)
        XCTAssertEqual(added.path, dir.standardizedFileURL.path)
        XCTAssertEqual(added.name, "docs")
        XCTAssertEqual(store.roots.count, 1)

        // 新 store 同账本重开:持久化可见(杀 App 重启同象)
        let reopened = makeStore(root: root)
        reopened.load()
        XCTAssertEqual(reopened.roots.count, 1)
        XCTAssertEqual(reopened.roots.first?.name, "docs")
        XCTAssertEqual(reopened.roots.first?.path, added.path)
    }

    func testRemovePersistsAcrossStoreReinit() throws {
        let root = Self.newTmp()
        defer { try? FileManager.default.removeItem(at: root) }
        let a = try makeDir(under: root, "a")
        let b = try makeDir(under: root, "b")

        let store = makeStore(root: root)
        store.load()
        _ = try store.add(url: a)
        let second = try store.add(url: b)
        XCTAssertEqual(store.roots.map(\.name), ["a", "b"], "插入序保持")

        store.remove(second)
        XCTAssertEqual(store.roots.map(\.name), ["a"])

        let reopened = makeStore(root: root)
        reopened.load()
        XCTAssertEqual(reopened.roots.map(\.name), ["a"], "移除持久化")
    }

    // MARK: resolveAll:解析路径 + stale(容器迁移/目录删除)剔除并回写账本

    func testResolveAllReturnsResolvedPaths() throws {
        let root = Self.newTmp()
        defer { try? FileManager.default.removeItem(at: root) }
        let dir = try makeDir(under: root, "media")

        let store = makeStore(root: root)
        store.load()
        _ = try store.add(url: dir)

        let resolved = store.resolveAll()
        XCTAssertEqual(resolved.count, 1)
        XCTAssertEqual(resolved.first?.path, dir.standardizedFileURL.path)
        XCTAssertEqual(resolved.first?.root.name, "media")
    }

    func testResolveAllPrunesStaleAndRewritesLedger() throws {
        let root = Self.newTmp()
        defer { try? FileManager.default.removeItem(at: root) }
        let dir = try makeDir(under: root, "gone")

        let store = makeStore(root: root)
        store.load()
        _ = try store.add(url: dir)
        XCTAssertEqual(store.resolveAll().count, 1)

        // 目录消失(容器迁移后旧书签的同型失效):解析剔除 + 账本回写
        try FileManager.default.removeItem(at: dir)
        let after = store.resolveAll()
        XCTAssertTrue(after.isEmpty, "失效书签应被剔除(fail-closed:坏根缩小授权集)")

        let reopened = makeStore(root: root)
        reopened.load()
        XCTAssertTrue(reopened.roots.isEmpty, "剔除应回写账本(重启后不再出现)")
    }

    // MARK: 引擎 conf(与 Rust 桥 parse 同口径:一行一路径/# 注释/空行)

    func testEngineRootsConfSerializeFormat() {
        XCTAssertEqual(EngineRootsConf.serialize(paths: []), "")
        XCTAssertEqual(
            EngineRootsConf.serialize(paths: ["/a", "/b b"]),
            "/a\n/b b\n",
            "一行一路径 + 尾换行(Rust 侧 lines() 解析)",
        )
    }

    func testEngineRootsConfWritePlacesFileUnderHome() throws {
        let root = Self.newTmp()
        defer { try? FileManager.default.removeItem(at: root) }
        try EngineRootsConf.write(home: root.path, paths: ["/x/y", "/z"])

        let conf = root.appendingPathComponent("engine-roots.conf")
        let text = try String(contentsOf: conf, encoding: .utf8)
        XCTAssertEqual(text, "/x/y\n/z\n")
    }

    func testEngineRootsConfMergeInboxFirstAndDedups() {
        let merged = EngineRootsConf.merge(inbox: "/inbox", trusted: ["/inbox", "/t1", "/t2", "/t1"])
        XCTAssertEqual(merged, ["/inbox", "/t1", "/t2"], "收件箱恒首位;重复根去重保序")

        XCTAssertEqual(EngineRootsConf.merge(inbox: nil, trusted: ["/t1"]), ["/t1"], "recv_dir 未就绪不编造")
        XCTAssertEqual(EngineRootsConf.merge(inbox: "/inbox", trusted: []), ["/inbox"])
    }
}
