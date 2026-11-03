/// 直发暂存(``SendStaging``)+ 降级收编(``ShareQueueStore/adoptFiles``)
/// 钉测(R10):直发批两步拷入 App Group(``send-staging/<批>/``,尺寸守卫
/// 同队列两步拷);直发不可行(超帽/boot 失败)时整批 **rename 收编**为队列
/// 项(同卷零拷贝,主 App 下次打开照常消费)。

import XCTest
@testable import MTDSSKit

final class SendStagingTests: XCTestCase {

    private var root: URL!

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("sendstaging-\(UUID().uuidString)", isDirectory: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func makeSource(_ name: String, bytes: [UInt8]) throws -> URL {
        let url = root.appendingPathComponent("src-\(name)")
        try Data(bytes).write(to: url)
        return url
    }

    // MARK: 派生

    func testAppGroupRootDerivation() throws {
        let group = URL(fileURLWithPath: "/group", isDirectory: true)
        let staging = SendStaging.appGroupRoot(groupRoot: group)
        XCTAssertEqual(staging.root.path, "/group/send-staging")
        XCTAssertEqual(SendStaging.dirName, "send-staging")
    }

    // MARK: 两步暂存

    func testStageFileCopiesWithSizeGuardAndNoTempLeftover() throws {
        let staging = SendStaging(root: root.appendingPathComponent("send-staging"))
        let batch = try staging.newBatch(id: "b1")
        let source = try makeSource("a.bin", bytes: [1, 2, 3, 4])
        try staging.stageFile(batch: batch, file: ShareQueueFile(name: "a.bin", size: 4), from: source)
        let dest = batch.appendingPathComponent("a.bin")
        XCTAssertEqual(try Data(contentsOf: dest), Data([1, 2, 3, 4]))
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: batch.path)
            .filter { $0.hasSuffix(".part") }
        XCTAssertTrue(leftovers.isEmpty)
        // 尺寸不符 → 拒拷(终名不在场 = 不入坏件)
        let bad = try makeSource("b.bin", bytes: [9, 9])
        XCTAssertThrowsError(
            try staging.stageFile(batch: batch, file: ShareQueueFile(name: "b.bin", size: 5), from: bad))
        XCTAssertFalse(FileManager.default.fileExists(atPath: batch.appendingPathComponent("b.bin").path))
    }

    func testRemoveBatchAndPurgeOrphans() throws {
        let staging = SendStaging(root: root.appendingPathComponent("send-staging"))
        let old = try staging.newBatch(id: "old")
        try Data([1]).write(to: old.appendingPathComponent("x"))
        // 老批:目录改动时间回拨 2 小时(过 1 小时线)
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(-7200)], ofItemAtPath: old.path)
        let keep = try staging.newBatch(id: "keep")
        // 老批(2 小时前)清,当前批留
        try staging.purgeOrphans(keeping: keep, olderThan: 3600, now: Date())
        XCTAssertFalse(FileManager.default.fileExists(atPath: old.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: keep.path))
        // 显式移除当前批
        staging.removeBatch(keep)
        XCTAssertFalse(FileManager.default.fileExists(atPath: keep.path))
    }

    // MARK: 降级收编(整批 rename 进队列)

    func testAdoptFilesMovesBatchIntoReadyQueueItem() throws {
        let group: URL = root
        let staging = SendStaging(root: group.appendingPathComponent("send-staging"))
        let batch = try staging.newBatch(id: "b1")
        let files = [ShareQueueFile(name: "a.bin", size: 3), ShareQueueFile(name: "b.txt", size: 2)]
        try Data([1, 2, 3]).write(to: batch.appendingPathComponent("a.bin"))
        try Data([4, 5]).write(to: batch.appendingPathComponent("b.txt"))

        let store = ShareQueueStore(root: group)
        try store.adoptFiles(
            item: "q1", targetDeviceId: "dev-1", files: files, fromDirectory: batch, now: Date())
        // 队列项 ready、目标设备在场、字节随行
        let meta = try XCTUnwrap(store.readMeta(id: "q1"))
        XCTAssertEqual(meta.status, .ready)
        XCTAssertEqual(meta.targetDeviceId, "dev-1")
        XCTAssertEqual(meta.files, files)
        XCTAssertEqual(meta.size, 5)
        // 收编 = move:暂存批内原文件不在场,队列 files/ 内在场
        XCTAssertFalse(FileManager.default.fileExists(atPath: batch.appendingPathComponent("a.bin").path))
        XCTAssertEqual(
            try Data(contentsOf: store.filesDir(id: "q1").appendingPathComponent("a.bin")),
            Data([1, 2, 3]))
    }

    /// 收编中尺寸不符 → 整单拒(队列项不留坏件;批内文件原样保留)。
    func testAdoptFilesSizeMismatchLeavesNoBadItem() throws {
        let group: URL = root
        let staging = SendStaging(root: group.appendingPathComponent("send-staging"))
        let batch = try staging.newBatch(id: "b1")
        try Data([1, 2]).write(to: batch.appendingPathComponent("a.bin"))
        let store = ShareQueueStore(root: group)
        XCTAssertThrowsError(
            try store.adoptFiles(
                item: "q1", targetDeviceId: "dev-1",
                files: [ShareQueueFile(name: "a.bin", size: 3)], fromDirectory: batch, now: Date()))
        XCTAssertNil(store.readMeta(id: "q1"), "收编失败不得留半套队列项")
        XCTAssertTrue(FileManager.default.fileExists(atPath: batch.appendingPathComponent("a.bin").path))
    }

    // MARK: 未暂存面整单入队(不可行/超帽降级路)

    func testEnqueueFilesCopiesSourcesIntoReadyItem() throws {
        let group: URL = root
        let source = try makeSource("c.bin", bytes: [7, 8, 9])
        let store = ShareQueueStore(root: group)
        try store.enqueueFiles(
            id: "q2", targetDeviceId: "dev-2",
            files: [ShareQueueFile(name: "c.bin", size: 3)],
            sources: [source], now: Date())
        let meta = try XCTUnwrap(store.readMeta(id: "q2"))
        XCTAssertEqual(meta.status, .ready)
        XCTAssertEqual(meta.targetDeviceId, "dev-2")
        XCTAssertEqual(
            try Data(contentsOf: store.filesDir(id: "q2").appendingPathComponent("c.bin")),
            Data([7, 8, 9]))
        // 半途失败(第二文件尺寸不符)→ 整项移除,不落坏项
        let other = try makeSource("d.bin", bytes: [1])
        XCTAssertThrowsError(
            try store.enqueueFiles(
                id: "q3", targetDeviceId: "dev-2",
                files: [ShareQueueFile(name: "c.bin", size: 3), ShareQueueFile(name: "d.bin", size: 2)],
                sources: [source, other], now: Date()))
        XCTAssertNil(store.readMeta(id: "q3"))
    }
}
