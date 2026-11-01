/// ShareQueueTests — 分享扩展队列与设备缓存(T11,spec §4.7/§5.4):
/// - 队列写入→读出等价(begin/copy/finish → scan 元数据一致);
/// - 两步拷贝半成品(临时名 .part / 无 meta)被消费端判「拷贝中断」失败态,
///   宽限窗内(扩展可能仍在拷)不动;
/// - meta schema 蛇形键逐字段钉死(扩展与主 App 两端 Codable 不漂移);
/// - 消费面:校验不符拒发、成功路 = 暂存收件箱 + 发送 + 清队列、发送失败
///   不静默丢(文件留暂存 + failed 态可重试)、staged 断点续消;
/// - 设备缓存:写读等价、>5min 过期置灰、缺失/损坏回 nil。

import XCTest
@testable import MTDSSKit

final class ShareQueueTests: XCTestCase {

    // MARK: 脚手架(每个用例独立临时根,App Group 路径可注入)

    private let fm = FileManager.default

    private func makeRoot() throws -> URL {
        let dir = fm.temporaryDirectory
            .appendingPathComponent("mtdss-t11-\(UUID().uuidString)")
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func write(_ data: Data, to url: URL) throws {
        try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url)
    }

    private func bytes(_ s: String) -> Data { Data(s.utf8) }

    private func makeStore() throws -> ShareQueueStore {
        ShareQueueStore(root: try makeRoot())
    }

    /// 两文件就绪项(IMG + pdf,内容各异;返回 id 与源内容)。
    @discardableResult
    private func seedReadyItem(
        _ store: ShareQueueStore, target: String = "peer-1",
        at date: Date = Date(timeIntervalSince1970: 1_760_000_000),
    ) throws -> (id: String, contents: [String]) {
        let files = [
            ShareQueueFile(name: "IMG_0421.HEIC", size: 6),
            ShareQueueFile(name: "年度报告.pdf", size: 4),
        ]
        let contents = ["photo1", "doc2"]
        let id = "item-1"
        try store.beginItem(id: id, targetDeviceId: target, files: files, now: date)
        for (file, content) in zip(files, contents) {
            let src = store.queueDir.appendingPathComponent("src-\(file.name)")
            try write(bytes(content), to: src)
            try store.copyFile(item: id, file: file, from: src)
        }
        try store.finishItem(id: id, now: date)
        return (id, contents)
    }

    private func makeConsumer(
        store: ShareQueueStore, inbox: String,
        send: @escaping @Sendable ([String], String) async throws -> String,
        now: @escaping @Sendable () -> Date = { Date(timeIntervalSince1970: 1_760_000_100) },
    ) -> ShareQueueConsumer {
        ShareQueueConsumer(queue: store, inboxPath: inbox, send: send, now: now)
    }

    // MARK: 队列写入→读出等价

    func testEnqueueReadRoundTrip() throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        let seeded = try seedReadyItem(store, target: "dev-abc", at: date)

        let entries = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(entries.count, 1)
        let entry = try XCTUnwrap(entries.first)
        XCTAssertEqual(entry.id, seeded.id)
        XCTAssertEqual(entry.meta.targetDeviceId, "dev-abc")
        XCTAssertEqual(entry.meta.sourceName, "IMG_0421.HEIC")
        XCTAssertEqual(entry.meta.size, 10)
        XCTAssertEqual(entry.meta.enqueuedAt, date)
        XCTAssertEqual(entry.meta.status, .ready)
        XCTAssertEqual(entry.meta.files, [
            ShareQueueFile(name: "IMG_0421.HEIC", size: 6),
            ShareQueueFile(name: "年度报告.pdf", size: 4),
        ])
        XCTAssertNil(entry.meta.error)
        // 落盘字节齐全(两步拷贝以最终名在场为准;无 .part 残留)
        for (file, content) in zip(entry.meta.files, seeded.contents) {
            let url = store.filesDir(id: entry.id).appendingPathComponent(file.name)
            XCTAssertEqual(try Data(contentsOf: url), bytes(content))
            XCTAssertFalse(fm.fileExists(atPath: url.appendingPathExtension("part").path))
        }
    }

    // MARK: meta schema 蛇形键 round-trip(扩展/主 App 两端不漂移)

    func testMetaSchemaRoundTrip() throws {
        let meta = ShareQueueMeta(
            targetDeviceId: "dev-1", sourceName: "a.txt", size: 7,
            enqueuedAt: Date(timeIntervalSince1970: 1_760_000_123),
            status: .ready,
            files: [ShareQueueFile(name: "a.txt", size: 7)],
            stagedPaths: ["/inbox/a.txt"], error: "设备离线",
        )
        let data = try ShareQueueStore.metaEncoder.encode(meta)
        let obj = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        // 键面钉死(简报 schema:{target_device_id, source_name, size, enqueued_at, status})
        XCTAssertEqual(Set(obj.keys), [
            "target_device_id", "source_name", "size", "enqueued_at", "status",
            "files", "staged_paths", "error",
        ])
        XCTAssertEqual(obj["target_device_id"] as? String, "dev-1")
        XCTAssertEqual(obj["source_name"] as? String, "a.txt")
        XCTAssertEqual(obj["size"] as? Int, 7)
        XCTAssertEqual(obj["enqueued_at"] as? Double, 1_760_000_123)
        XCTAssertEqual(obj["status"] as? String, "ready")
        let files = try XCTUnwrap(obj["files"] as? [[String: Any]])
        XCTAssertEqual(files[0]["name"] as? String, "a.txt")
        XCTAssertEqual(files[0]["size"] as? Int, 7)
        XCTAssertEqual(obj["staged_paths"] as? [String], ["/inbox/a.txt"])
        XCTAssertEqual(obj["error"] as? String, "设备离线")

        // 字面 JSON 反解(蛇形键逐字段;可选键缺席合法)
        let literal = """
        {"target_device_id":"dev-2","source_name":"b.bin","size":3,\
        "enqueued_at":1760000123.5,"status":"copying",\
        "files":[{"name":"b.bin","size":3}]}
        """
        let decoded = try ShareQueueStore.metaDecoder.decode(
            ShareQueueMeta.self, from: Data(literal.utf8))
        XCTAssertEqual(decoded, ShareQueueMeta(
            targetDeviceId: "dev-2", sourceName: "b.bin", size: 3,
            enqueuedAt: Date(timeIntervalSince1970: 1_760_000_123.5),
            status: .copying,
            files: [ShareQueueFile(name: "b.bin", size: 3)],
            stagedPaths: nil, error: nil))
    }

    func testItemNameSlashSanitized() {
        XCTAssertEqual(ShareQueueStore.sanitizedName("a/b:c.txt"), "b_c.txt") // 目录分量剥除+冒号净化
        XCTAssertEqual(ShareQueueStore.sanitizedName("ok.txt"), "ok.txt")
    }

    // MARK: 两步拷贝半成品 → 消费端判「拷贝中断」(宽限窗外)

    func testPartialTempRejectedAsFailed() throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        let files = [
            ShareQueueFile(name: "done.bin", size: 4),
            ShareQueueFile(name: "killed.bin", size: 9),
        ]
        try store.beginItem(id: "kill-1", targetDeviceId: "dev-1", files: files, now: date)
        // 文件一完整(拷完 rename);文件二只落了 .part 临时名(拷一半被杀)
        try write(bytes("abcd"), to: store.queueDir.appendingPathComponent("src-done.bin"))
        try store.copyFile(item: "kill-1", file: files[0], from: store.queueDir.appendingPathComponent("src-done.bin"))
        try write(bytes("12345"), to: store.filesDir(id: "kill-1").appendingPathComponent("killed.bin.part"))

        // 宽限窗内:扩展可能仍在拷(或等待 meta 翻 ready),不动
        let young = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(young.count, 1)
        XCTAssertEqual(young[0].meta.status, .copying)

        // 宽限窗外:扩展已死(主 App 下次启动)→ failed 态,不静默丢
        let stale = try store.scan(now: date.addingTimeInterval(ShareQueueStore.defaultCopyGrace + 1))
        XCTAssertEqual(stale.count, 1)
        XCTAssertEqual(stale[0].meta.status, .failed)
        XCTAssertEqual(stale[0].meta.error, ShareQueueConsumer.errorCopyInterrupted)
    }

    func testMetaMissingSynthesizedAsFailed() throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        // 目录在场但 meta 未写(极端:beginItem 前被杀)——孤儿目录也按失败
        // 收口,不静默丢
        try fm.createDirectory(
            at: store.itemDir(id: "orphan").appendingPathComponent("files"),
            withIntermediateDirectories: true)
        try write(bytes("x"), to: store.filesDir(id: "orphan").appendingPathComponent("stray.bin"))

        let entries = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0].id, "orphan")
        XCTAssertEqual(entries[0].meta.status, .failed)
        XCTAssertEqual(entries[0].meta.error, ShareQueueConsumer.errorCopyInterrupted)
    }

    // MARK: 消费面

    func testConsumerHappyPathStagesIntoInboxAndSends() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        try seedReadyItem(store, target: "dev-9", at: date)
        let inbox = try makeRoot().appendingPathComponent("Documents").path

        final class Sender: @unchecked Sendable {
            var paths: [[String]] = []
            var targets: [String] = []
            let lock = NSLock()
            func record(_ p: [String], _ t: String) {
                lock.lock(); defer { lock.unlock() }
                paths.append(p); targets.append(t)
            }
            func snapshot() -> ([[String]], [String]) {
                lock.lock(); defer { lock.unlock() }
                return (paths, targets)
            }
        }
        let sender = Sender()
        let consumer = makeConsumer(store: store, inbox: inbox) { paths, target in
            sender.record(paths, target)
            return "job-1"
        }

        let outcome = await consumer.consumeOnce()
        XCTAssertEqual(outcome.consumed, 1)
        let (sentBatches, targets) = sender.snapshot()
        XCTAssertEqual(targets, ["dev-9"])
        XCTAssertEqual(sentBatches.count, 1)
        // 发送路径 = 收件箱内绝对路径,内容逐字节一致
        let sentPaths = sentBatches[0]
        XCTAssertEqual(sentPaths.count, 2)
        XCTAssertTrue(sentPaths.allSatisfy { $0.hasPrefix(inbox + "/") })
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: sentPaths[0])), bytes("photo1"))
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: sentPaths[1])), bytes("doc2"))
        // 队列项已清;无失败行
        XCTAssertTrue(try store.scan(now: date.addingTimeInterval(1)).isEmpty)
    }

    func testConsumerSizeMismatchRefusesSendAndMarksFailed() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        let files = [ShareQueueFile(name: "bad.bin", size: 10)]
        try store.beginItem(id: "m-1", targetDeviceId: "dev-1", files: files, now: date)
        try write(bytes("0123456789"), to: store.queueDir.appendingPathComponent("src-bad.bin"))
        try store.copyFile(item: "m-1", file: files[0], from: store.queueDir.appendingPathComponent("src-bad.bin"))
        try store.finishItem(id: "m-1", now: date)
        // 完成后盘上字节被截(盘损/篡改形态)——消费端逐文件复验须拦下
        try bytes("12345").write(to: store.filesDir(id: "m-1").appendingPathComponent("bad.bin"))
        let inbox = try makeRoot().appendingPathComponent("Documents").path

        var sendCalled = false
        let consumer = makeConsumer(store: store, inbox: inbox) { _, _ in
            sendCalled = true
            return "job-x"
        }
        let outcome = await consumer.consumeOnce()
        XCTAssertEqual(outcome.consumed, 0)
        XCTAssertFalse(sendCalled) // 校验不过不发(坏字节不上线)
        let entries = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0].meta.status, .failed)
        XCTAssertEqual(entries[0].meta.error, ShareQueueConsumer.errorSizeMismatch)
    }

    func testConsumerMissingFileMarksFailed() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        let files = [ShareQueueFile(name: "gone.bin", size: 3)]
        try store.beginItem(id: "g-1", targetDeviceId: "dev-1", files: files, now: date)
        try store.finishItem(id: "g-1", now: date) // 声明了文件但从未落盘
        let inbox = try makeRoot().appendingPathComponent("Documents").path

        var sendCalled = false
        let consumer = makeConsumer(store: store, inbox: inbox) { _, _ in
            sendCalled = true
            return "job-x"
        }
        let outcome = await consumer.consumeOnce()
        XCTAssertEqual(outcome.consumed, 0)
        XCTAssertFalse(sendCalled)
        let entries = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(entries[0].meta.status, .failed)
        XCTAssertEqual(entries[0].meta.error, ShareQueueConsumer.errorSizeMismatch)
    }

    func testConsumerSendFailureKeepsFilesAndSurfacesFailed() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        try seedReadyItem(store, target: "dev-1", at: date)
        let inbox = try makeRoot().appendingPathComponent("Documents").path

        let consumer = makeConsumer(store: store, inbox: inbox) { _, _ in
            throw RpcError(code: 1, message: "设备离线")
        }
        let outcome = await consumer.consumeOnce()
        XCTAssertEqual(outcome.consumed, 0)
        let entries = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(entries.count, 1)
        XCTAssertEqual(entries[0].meta.status, .failed)
        XCTAssertEqual(entries[0].meta.error, "设备离线")
        // 暂存文件留在收件箱(数据不丢,重试不重拷)
        XCTAssertEqual(entries[0].meta.stagedPaths?.count, 2)
        for p in entries[0].meta.stagedPaths ?? [] {
            XCTAssertTrue(fm.fileExists(atPath: p))
        }
    }

    func testConsumerResumesStagedItemAfterRestart() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        let inboxURL = try makeRoot().appendingPathComponent("Documents")
        // 断点形态:暂存已完成(staged)但发送未及(进程被杀)——重启续消
        let stagedA = inboxURL.appendingPathComponent("a.bin")
        try write(bytes("AAA"), to: stagedA)
        try store.beginItem(
            id: "s-1", targetDeviceId: "dev-2",
            files: [ShareQueueFile(name: "a.bin", size: 3)], now: date)
        try store.markStaged(id: "s-1", stagedPaths: [stagedA.path], now: date)

        final class Got: @unchecked Sendable {
            var paths: [String] = []
            var target: String?
        }
        let got = Got()
        let consumer = makeConsumer(store: store, inbox: inboxURL.path) { paths, target in
            got.paths = paths
            got.target = target
            return "job-2"
        }
        let outcome = await consumer.consumeOnce()
        XCTAssertEqual(outcome.consumed, 1)
        XCTAssertEqual(got.target, "dev-2")
        XCTAssertEqual(got.paths, [stagedA.path])
        XCTAssertTrue(try store.scan(now: date.addingTimeInterval(1)).isEmpty)
    }

    func testConsumerStagedFileMissingMarksFailed() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        let inboxURL = try makeRoot().appendingPathComponent("Documents")
        try store.beginItem(
            id: "s-2", targetDeviceId: "dev-2",
            files: [ShareQueueFile(name: "a.bin", size: 3)], now: date)
        try store.markStaged(
            id: "s-2",
            stagedPaths: [inboxURL.appendingPathComponent("vanished.bin").path],
            now: date)

        var sendCalled = false
        let consumer = makeConsumer(store: store, inbox: inboxURL.path) { _, _ in
            sendCalled = true
            return "job-x"
        }
        let outcome = await consumer.consumeOnce()
        XCTAssertEqual(outcome.consumed, 0)
        XCTAssertFalse(sendCalled)
        let entries = try store.scan(now: date.addingTimeInterval(1))
        XCTAssertEqual(entries[0].meta.status, .failed)
    }

    func testConsumerRetryAfterFailedSendsAgain() async throws {
        let store = try makeStore()
        let date = Date(timeIntervalSince1970: 1_760_000_000)
        try seedReadyItem(store, target: "dev-1", at: date)
        let inbox = try makeRoot().appendingPathComponent("Documents").path

        // 第一轮:发送失败(设备瞬断)→ failed 态 + 文件留暂存;常规消费
        // 不自动重发(重试是用户动作)
        _ = await makeConsumer(store: store, inbox: inbox) { _, _ in
            throw RpcError(code: 1, message: "设备离线")
        }.consumeOnce()
        XCTAssertEqual(try store.scan(now: date.addingTimeInterval(1)).count, 1)

        // 重试(重选目标后同一入口):staged 断点重发 + 目标覆盖,成功即清
        final class Retry: @unchecked Sendable {
            var target: String?
            var pathCounts: [Int] = []
        }
        let retry = Retry()
        let consumer = makeConsumer(store: store, inbox: inbox) { paths, target in
            retry.target = target
            retry.pathCounts.append(paths.count)
            return "job-3"
        }
        let sent = await consumer.retryOnce(id: "item-1", overrideTargetDeviceId: "dev-2")
        XCTAssertTrue(sent)
        XCTAssertEqual(retry.target, "dev-2")
        XCTAssertEqual(retry.pathCounts, [2])
        XCTAssertTrue(try store.scan(now: date.addingTimeInterval(1)).isEmpty)
    }

    func testStageCollisionSuffixesName() throws {
        let inboxURL = try makeRoot().appendingPathComponent("Documents")
        try write(bytes("old"), to: inboxURL.appendingPathComponent("dup.txt"))
        let incoming = inboxURL.appendingPathComponent("incoming-dup.txt")
        try write(bytes("new"), to: incoming)
        let staged = try ShareQueueStore.moveIntoInbox(
            fileURL: incoming, name: "dup.txt", inbox: inboxURL.path)
        XCTAssertEqual((staged as NSString).lastPathComponent, "dup (1).txt")
        XCTAssertEqual(try Data(contentsOf: URL(fileURLWithPath: staged)), bytes("new"))
        XCTAssertEqual(try Data(contentsOf: inboxURL.appendingPathComponent("dup.txt")), bytes("old"))
    }

    // MARK: 设备缓存(>5min 过期置灰)

    private func makeCache() throws -> SharedDeviceCache {
        SharedDeviceCache(root: try makeRoot())
    }

    func testDeviceCacheWriteReadEquivalence() throws {
        let cache = try makeCache()
        let devices = [
            SharedDeviceEntry(deviceId: "a", name: "公司台式机", platform: "windows", online: true),
            SharedDeviceEntry(deviceId: "b", name: "家里的 Linux", platform: "linux", online: false),
        ]
        let t0 = Date(timeIntervalSince1970: 1_760_000_000)
        try cache.write(devices: devices, now: t0)
        let snap = try XCTUnwrap(cache.read())
        XCTAssertEqual(snap.devices, devices)
        XCTAssertEqual(snap.refreshedAt, t0)

        let (read, stale) = try XCTUnwrap(
            cache.readWithStaleness(now: t0.addingTimeInterval(10)))
        XCTAssertEqual(read, devices)
        XCTAssertFalse(stale)
    }

    func testDeviceCacheStaleAfter5Minutes() throws {
        let cache = try makeCache()
        let t0 = Date(timeIntervalSince1970: 1_760_000_000)
        try cache.write(devices: [
            SharedDeviceEntry(deviceId: "a", name: nil, platform: nil, online: true),
        ], now: t0)
        // 300s 整未过期,301s 过期(>5min 置灰口径)
        XCTAssertEqual(cache.readWithStaleness(now: t0.addingTimeInterval(300))?.stale, false)
        XCTAssertEqual(cache.readWithStaleness(now: t0.addingTimeInterval(301))?.stale, true)
    }

    func testDeviceCacheMissingOrCorruptReadsNil() throws {
        let cache = try makeCache()
        XCTAssertNil(cache.read()) // 缺失:扩展侧全部置灰 + 提示打开 App
        try write(bytes("{not json"), to: cache.fileURL)
        XCTAssertNil(cache.read()) // 损坏:同缺失兜底
    }

    // MARK: 字节文案(扩展项目清单/失败行共面)

    func testShareFormatBytes() {
        XCTAssertEqual(ShareFormat.bytes(0), "0 B")
        XCTAssertEqual(ShareFormat.bytes(512), "512 B")
        XCTAssertEqual(ShareFormat.bytes(1024), "1.0 KB")
        XCTAssertEqual(ShareFormat.bytes(1536), "1.5 KB")
        XCTAssertEqual(ShareFormat.bytes(3_214_572), "3.1 MB")
        XCTAssertEqual(ShareFormat.bytes(2_147_483_648), "2.0 GB")
    }

    // MARK: 可取消回调等待(评审 I-1:系统 completionHandler 永不回调形态,
    // 取消必须能真正收口——竞窗/任务组退出不再被无感续体挂死)

    /// 取消 → 立即 false 收口(挂死形态钉面:续体若不感知取消,本例的
    /// await task.value 永不返回)。
    func testCallbackFallbackResumesFalseOnCancel() async {
        let task = Task {
            await withCallbackFallback { _ in
                // 永不回调(open() 静默无回的形态)
            }
        }
        try? await Task.sleep(nanoseconds: 50_000_000)
        task.cancel()
        let value = await task.value
        XCTAssertFalse(value)
    }

    /// 回调先到 → 原值;迟到取消不再改写(恰一次续)。
    func testCallbackFallbackReturnsCallbackValueAndIgnoresLateCancel() async {
        let value = await withCallbackFallback { onDone in
            Task {
                try? await Task.sleep(nanoseconds: 20_000_000)
                onDone(true)
            }
        }
        XCTAssertTrue(value)
    }

    /// 已取消任务进入 → 回退值即时收口(attempt 仍发起,迟到回调被吞)。
    func testCallbackFallbackPreCancelledEntersWithFallback() async {
        let task = Task {
            await withCallbackFallback { onDone in
                Task {
                    try? await Task.sleep(nanoseconds: 100_000_000)
                    onDone(true) // 迟到回调,须被恰一次守卫吞掉(不崩不重续)
                }
            }
        }
        task.cancel()
        let value = await task.value
        XCTAssertFalse(value)
    }

    /// 竞窗:超时回 nil 且**不挂组**(败方经取消感知面即时收口——挂死
    /// 形态的端到端钉面:若挂,本测试超 XCT 默认执行时限)。
    func testFirstOrTimeoutReturnsNilAndDoesNotHang() async {
        let start = Date()
        let result = await firstOrTimeout(seconds: 0.2) {
            await withCallbackFallback { _ in } // 永不回调的竞窗败方
        }
        XCTAssertNil(result)
        XCTAssertLessThan(Date().timeIntervalSince(start), 5)
    }

    /// 竞窗:先回者胜(run 0.05s 胜 3s 定时器)。
    func testFirstOrTimeoutReturnsFirstResult() async {
        let result = await firstOrTimeout(seconds: 3) {
            try? await Task.sleep(nanoseconds: 50_000_000)
            return "winner"
        }
        XCTAssertEqual(result, "winner")
    }
}
