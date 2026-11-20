import XCTest
import MTDSSKit

/// 预览回环句柄池单测(V4-1 T24;镜像 android PreviewHandlePoolTest 面):
/// 在场引用计数复用 / 归零才 close / open 失败降级不落槽 / close 失败静默 /
/// 离场竞态闭合(取消态不入账,安卓 ensureActive 同款)/ releaseAll 全量收口。
final class PreviewHandlePoolTests: XCTestCase {

    /// 开/关计数(@Sendable 闭包可触;锁保护)。
    private final class Counter: @unchecked Sendable {
        private let lock = NSLock()
        private var openCountStorage = 0
        private var closeCountStorage = 0
        private var closeHandles: [String] = []
        var openCount: Int { lock.withLock { openCountStorage } }
        var closeCount: Int { lock.withLock { closeCountStorage } }
        var closedHandles: [String] { lock.withLock { closeHandles } }
        func bumpOpen() { lock.withLock { openCountStorage += 1 } }
        func bumpClose(_ handle: String) { lock.withLock { closeCountStorage += 1; closeHandles.append(handle) } }
    }

    private func entry(_ path: String) -> DirEntry {
        DirEntry(name: path, path: path, size: 10, mtimeMs: 100, isDir: false)
    }

    private func fixedOpen(_ counter: Counter) -> @Sendable (DirEntry) async throws -> PreviewOpenResult {
        { entry in
            counter.bumpOpen()
            return PreviewOpenResult(handle: "h-\(entry.path)", port: 39801, token: "tok-\(entry.path)")
        }
    }

    // MARK: 在场复用 + 归零才 close

    func testOpenReusesSameHandleWithinPresence() async {
        let counter = Counter()
        let pool = PreviewHandlePool(open: fixedOpen(counter), close: { _ in counter.bumpClose("x") })
        let e = entry("a.mp4")
        let url1 = await pool.urlFor(e)
        let url2 = await pool.urlFor(e)
        XCTAssertEqual(url1, "http://127.0.0.1:39801/f/h-a.mp4/thumb?w=256&t=tok-a.mp4")
        XCTAssertEqual(url2, url1, "在场复用同 handle 同 URL")
        XCTAssertEqual(counter.openCount, 1, "引用计数 +1 复用,不重开")
        await pool.release(e)
        XCTAssertEqual(counter.closeCount, 0, "计数未归零不 close")
        let live1 = await pool.liveCount
        XCTAssertEqual(live1, 1)
        await pool.release(e)
        XCTAssertEqual(counter.closeCount, 1, "归零才 close")
        let live = await pool.liveCount
        XCTAssertEqual(live, 0)
    }

    func testUnknownPathReleaseIsIdempotentNoop() async {
        let counter = Counter()
        let pool = PreviewHandlePool(open: fixedOpen(counter), close: { _ in counter.bumpClose("x") })
        await pool.release(entry("never-opened.mp4"))
        XCTAssertEqual(counter.closeCount, 0)
        let live = await pool.liveCount
        XCTAssertEqual(live, 0)
    }

    func testDifferentPathsOpenDistinctHandles() async {
        let counter = Counter()
        let pool = PreviewHandlePool(open: fixedOpen(counter), close: { _ in counter.bumpClose("x") })
        let a = await pool.urlFor(entry("a.mp4"))
        let b = await pool.urlFor(entry("b.mp4"))
        XCTAssertEqual(counter.openCount, 2)
        XCTAssertNotEqual(a, b, "不同路径各自开槽")
        XCTAssertTrue(a?.contains("t=tok-a.mp4") ?? false)
    }

    // MARK: open 失败 = 降级不落槽(缩略图缺位只是降级面)

    func testOpenFailureYieldsNilWithoutSlotAndNoClose() async {
        let counter = Counter()
        let pool = PreviewHandlePool(
            open: { _ in
                counter.bumpOpen()
                throw RpcError.transport("源打开失败")
            },
            close: { _ in counter.bumpClose("x") })
        let url = await pool.urlFor(entry("a.mp4"))
        XCTAssertNil(url)
        let live = await pool.liveCount
        XCTAssertEqual(live, 0, "失败不落槽")
        XCTAssertEqual(counter.closeCount, 0)
    }

    // MARK: close 失败静默(daemon 60s 空闲回收为终极兜底)

    func testCloseFailureIsSilentAndSlotStillRemoved() async {
        let counter = Counter()
        let pool = PreviewHandlePool(
            open: fixedOpen(counter),
            close: { _ in throw RpcError.transport("管道已关闭") })
        _ = await pool.urlFor(entry("a.mp4"))
        await pool.release(entry("a.mp4")) // close 抛错 = 静默,不炸调用面
        let live = await pool.liveCount
        XCTAssertEqual(live, 0, "槽仍摘除")
    }

    // MARK: 离场竞态闭合:open 在途遇取消 = 不入账半开槽不留

    func testCancelledOpenDoesNotBookkeep() async {
        let counter = Counter()
        let pool = PreviewHandlePool(
            open: { _ in
                counter.bumpOpen()
                // 无视取消继续完成(模拟 IPC 已回;取消由池面守卫识别)
                while !Task.isCancelled { await Task.yield() }
                return PreviewOpenResult(handle: "h1", port: 1, token: "t1")
            },
            close: { _ in counter.bumpClose("x") })
        let task = Task { await pool.urlFor(entry("a.mp4")) }
        let deadline = Date().addingTimeInterval(5)
        while counter.openCount < 1 && Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
        task.cancel()
        let url = await task.value
        XCTAssertNil(url, "取消态不入账直返")
        let live = await pool.liveCount
        XCTAssertEqual(live, 0, "半开槽不留")
        XCTAssertEqual(counter.closeCount, 0, "未入账槽无从 close")
    }

    // MARK: releaseAll = 屏面卸载全量收口

    func testReleaseAllClosesEverything() async {
        let counter = Counter()
        let pool = PreviewHandlePool(open: fixedOpen(counter), close: { counter.bumpClose($0) })
        _ = await pool.urlFor(entry("a.mp4"))
        _ = await pool.urlFor(entry("b.jpg"))
        let live2 = await pool.liveCount
        XCTAssertEqual(live2, 2)
        await pool.releaseAll()
        let live = await pool.liveCount
        XCTAssertEqual(live, 0)
        XCTAssertEqual(counter.closeCount, 2)
        XCTAssertEqual(Set(counter.closedHandles), ["h-a.mp4", "h-b.jpg"], "逐槽各 close 一次,handle 与槽对应")
        XCTAssertEqual(counter.closedHandles.count, 2)
    }
}
