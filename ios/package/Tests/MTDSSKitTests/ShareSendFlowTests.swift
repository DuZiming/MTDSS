/// ``ShareSendFlow`` 直发状态机钉测(R10;用户裁定:点设备 → 表内进度条 →
/// 完成,无排队无拉起无「好」钮)。状态机面宿主可测(Ruling E 同款):
/// - busy(staging+boot)→ sending(受理,进度推进)→ done/cancelled/failed;
/// - 降级触发(无会话同步/超帽/boot 失败/受理前失败)→ queued(队列注记);
/// - 批次帽边界 50/51 件、200MB/201MB(ShareDirectSendLimits);
/// - 逐文件进度推导(filesDone/bytesDone → 每文件完成/在途字节/待发)。

import XCTest
@testable import MTDSSKit

final class ShareSendFlowTests: XCTestCase {

    // MARK: 批次帽(R10:件数 ≤50 且总量 ≤200MB 直发,超帽回落队列)

    func testEligibleBoundaries() {
        // 恰在帽内:50 件 / 200MB 整
        XCTAssertTrue(ShareDirectSendLimits.eligible(count: 1, totalBytes: 1))
        XCTAssertTrue(ShareDirectSendLimits.eligible(count: 50, totalBytes: 1))
        XCTAssertTrue(
            ShareDirectSendLimits.eligible(
                count: 1, totalBytes: ShareDirectSendLimits.maxTotalBytes))
        XCTAssertTrue(
            ShareDirectSendLimits.eligible(
                count: 50, totalBytes: ShareDirectSendLimits.maxTotalBytes))
        // 越帽:51 件 / 201MB
        XCTAssertFalse(ShareDirectSendLimits.eligible(count: 51, totalBytes: 1))
        XCTAssertFalse(
            ShareDirectSendLimits.eligible(
                count: 1, totalBytes: ShareDirectSendLimits.maxTotalBytes + 1))
        // 空单不直发(无意义;面板侧另有空单拒发)
        XCTAssertFalse(ShareDirectSendLimits.eligible(count: 0, totalBytes: 0))
    }

    // MARK: pick 状态流转

    func testPickFromIdleEntersBusy() {
        var flow = ShareSendFlow()
        XCTAssertTrue(flow.pick(deviceId: "d1"))
        XCTAssertEqual(flow.phase, .busy(deviceId: "d1"))
    }

    func testPickWhileBusyAndSendingRejected() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        XCTAssertFalse(flow.pick(deviceId: "d2"))
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        XCTAssertFalse(flow.pick(deviceId: "d3"))
        XCTAssertEqual(flow.phase, .sending(deviceId: "d1", jobId: "7"))
    }

    /// failed 后可换设备重发(R10:failed = 重试[重选目标]/转队列双钮)
    func testRepickAfterFailed() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "failed", jobId: "7", error: "对端不可达")
        XCTAssertEqual(flow.phase, .failed(deviceId: "d1", message: "对端不可达"))
        XCTAssertTrue(flow.pick(deviceId: "d2"))
        XCTAssertEqual(flow.phase, .busy(deviceId: "d2"))
    }

    /// queued(降级)后不再重选——队列已接管,直发流收口(视图自动完成)
    func testRepickAfterQueuedRejected() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.fallbackQueued(deviceId: "d1")
        XCTAssertFalse(flow.pick(deviceId: "d2"))
        XCTAssertEqual(flow.phase, .queued(deviceId: "d1"))
    }

    // MARK: 受理与进度推进

    func testSendAcceptedSeedsProgressAndGoesSending() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a.bin", "b.bin"], sizes: [100, 200])
        XCTAssertEqual(flow.phase, .sending(deviceId: "d1", jobId: "7"))
        let p = try! XCTUnwrap(flow.progress)
        XCTAssertEqual(p.filesTotal, 2)
        XCTAssertEqual(p.bytesTotal, 300)
        XCTAssertEqual(p.overallPercent, 0)
        XCTAssertEqual(p.files.map(\.bytesDone), [0, 0])
    }

    /// 逐文件推导:filesDone=1、bytesDone=120 → 首文件完成、次文件在途
    /// 120-100=20B、末文件待发;总百分比 120/300。
    func testProgressTickDerivesPerFile() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a", "b", "c"], sizes: [100, 200, 50])
        flow.apply(filesDone: 1, bytesDone: 120, bytesTotal: 350)
        let p = try! XCTUnwrap(flow.progress)
        XCTAssertEqual(p.files.map(\.isDone), [true, false, false])
        XCTAssertEqual(p.files.map(\.bytesDone), [100, 20, 0])
        XCTAssertEqual(p.filesDone, 1)
        XCTAssertEqual(p.overallPercent, Int(120 * 100 / 350))
    }

    /// 全部完成形:filesDone=3 → 每文件 done,百分比 100。
    func testProgressTickAllDone() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a", "b"], sizes: [100, 200])
        flow.apply(filesDone: 2, bytesDone: 300, bytesTotal: 300)
        let p = try! XCTUnwrap(flow.progress)
        XCTAssertEqual(p.files.map(\.isDone), [true, true])
        XCTAssertEqual(p.overallPercent, 100)
    }

    /// 引擎字节数超前(filesDone 落后于字节数)时在途字节按文件尺寸钳制,
    /// 不出负/超尺寸。
    func testProgressTickBytesAheadOfFilesClamped() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a", "b"], sizes: [100, 200])
        flow.apply(filesDone: 0, bytesDone: 250, bytesTotal: 300)
        let p = try! XCTUnwrap(flow.progress)
        XCTAssertEqual(p.files.map(\.bytesDone), [100, 150])
    }

    /// apply 只在 sending 相位生效(busy/终态迟到帧不翻转)。
    func testApplyIgnoredOutsideSending() {
        var flow = ShareSendFlow()
        flow.apply(filesDone: 1, bytesDone: 1, bytesTotal: 1)
        XCTAssertNil(flow.progress)
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "done", jobId: "7", error: nil)
        flow.apply(filesDone: 9, bytesDone: 9, bytesTotal: 9)
        XCTAssertEqual(flow.phase, .done(deviceId: "d1"))
    }

    // MARK: 终态

    func testTerminalDone() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "done", jobId: "7", error: nil)
        XCTAssertEqual(flow.phase, .done(deviceId: "d1"))
    }

    func testTerminalCancelled() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "cancelled", jobId: "7", error: nil)
        XCTAssertEqual(flow.phase, .cancelled(deviceId: "d1"))
    }

    func testTerminalFailedCarriesError() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "interrupted", jobId: "7", error: "源文件消失")
        XCTAssertEqual(flow.phase, .failed(deviceId: "d1", message: "源文件消失"))
    }

    /// 终态无错误文案 → 兜底「传输失败(state)」。
    func testTerminalFailedWithoutErrorFallbackText() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "failed", jobId: "7", error: nil)
        XCTAssertEqual(flow.phase, .failed(deviceId: "d1", message: "传输失败(failed)"))
    }

    /// 他作业/未知作业的终态帧不翻转(过滤错误 jobId)。
    func testTerminalForUnknownJobIgnored() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        XCTAssertFalse(flow.terminal(state: "done", jobId: "99", error: nil))
        XCTAssertEqual(flow.phase, .sending(deviceId: "d1", jobId: "7"))
    }

    // MARK: 降级(队列)

    /// fallbackQueued 只在 busy 相位生效(sending 中不接受降级——发送已受理,
    /// 终态面接管)。
    func testFallbackQueuedFromBusyOnly() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.fallbackQueued(deviceId: "d1")
        XCTAssertEqual(flow.phase, .queued(deviceId: "d1"))
        var sending = ShareSendFlow()
        sending.pick(deviceId: "d1")
        sending.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        sending.fallbackQueued(deviceId: "d1")
        XCTAssertEqual(sending.phase, .sending(deviceId: "d1", jobId: "7"))
    }

    /// 降级收编失败(队列写入抛错形)→ failed(带原因,可重试)。
    func testFallbackFailedFromBusy() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.fallbackFailed(deviceId: "d1", message: "容器满")
        XCTAssertEqual(flow.phase, .failed(deviceId: "d1", message: "容器满"))
    }

    // MARK: 先订后发(评审 R10-1:hub 不重放,晚订丢终态帧)

    /// 伪 daemon:transfer.start 应答后**同连接立即**推终态事件帧
    /// (瞬时小传输形——受理即完成,终态帧先于调用方恢复执行)。
    /// 扩展编排修后形态 = 先 `hub.subscribe()` 再发受理调用:本钉测复刻
    /// 该序,终态事件必达(若先发后订,该帧已广播无人接、流恒卡 sending)。
    func testEarlySubscribeDeliversTerminalFrameEmittedAtAcceptance() async throws {
        let fake = FakeDaemon()
        fake.onLine = { conn, line in
            guard let frame = try? JSONParser.parse(String(decoding: line, as: UTF8.self)),
                  case .object(let obj) = frame,
                  case .number(let idRaw)? = obj["id"],
                  case .string("transfer.start")? = obj["method"] else { return }
            conn.sendLine(Data((JSONValue.object([
                "id": .number(idRaw),
                "result": .object(["job_id": .string("job-1")]),
            ]).canonical + "\n").utf8))
            conn.sendLine(Data((JSONValue.object([
                "method": .string("event.transfer"),
                "params": .object([
                    "id": .string("job-1"), "kind": .string("upload"),
                    "state": .string("done"),
                    "files_done": .number("1"), "files_total": .number("1"),
                    "bytes_done": .number("10"), "bytes_total": .number("10"),
                    "speed_bps": .number("0"), "skipped": .number("0"),
                    "relay": .bool(false),
                ]),
            ]).canonical + "\n").utf8))
        }
        let client = IpcClient(endpoint: fake)
        try await client.connect()
        let hub = EventHub(client: client)
        defer { hub.close() }
        // 先订(修后形态;DirectSendEngine.run 同序)
        let events = hub.subscribe()
        let repo = MtdssRepository(client: client)
        let peer = DirectoryDevice(deviceId: "dev-1", name: nil, platform: "windows", online: true)
        let jobId = try await SendService(repo: repo).send(paths: ["/tmp/a.bin"], to: peer)
        XCTAssertEqual(jobId, "job-1")
        // 泵:首个 job-1 帧即受理窗内推的终态帧(带 5s 闸防挂)
        let state = try await RpcGuard.withTimeout(seconds: 5) {
            for await event in events {
                if case .transfer(let p) = event, p.id == "job-1" {
                    return p.state
                }
            }
            return "stream-ended"
        }
        XCTAssertEqual(state, "done", "受理窗内推的终态帧必须被先订的泵收到")
    }

    /// 状态机消费受理窗前的终态帧:sendAccepted 之后 terminal(done) 翻转
    /// done——编排把「先到的终态帧」喂流即正常收口(与上钉合成闭环)。
    func testTerminalFrameFromAcceptanceWindowSettlesFlow() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "job-1", names: ["a"], sizes: [10])
        flow.apply(filesDone: 1, bytesDone: 10, bytesTotal: 10)
        flow.terminal(state: "done", jobId: "job-1", error: nil)
        XCTAssertEqual(flow.phase, .done(deviceId: "d1"))
    }

    /// failed 相位的「转队列」用户动作 → queued(转队列自 failed 也合法;
    /// deviceId 不符的迟到面不翻转)。
    func testQueueFallbackFromFailedPhase() {
        var flow = ShareSendFlow()
        flow.pick(deviceId: "d1")
        flow.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        flow.terminal(state: "failed", jobId: "7", error: "对端不可达")
        flow.fallbackQueued(deviceId: "d1")
        XCTAssertEqual(flow.phase, .queued(deviceId: "d1"))
        var other = ShareSendFlow()
        other.pick(deviceId: "d1")
        other.sendAccepted(jobId: "7", names: ["a"], sizes: [1])
        other.terminal(state: "failed", jobId: "7", error: nil)
        other.fallbackQueued(deviceId: "d2")
        guard case .failed = other.phase else {
            return XCTFail("deviceId 不符的降级不得翻转")
        }
    }
}
