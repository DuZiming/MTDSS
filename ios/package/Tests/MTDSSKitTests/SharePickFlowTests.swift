/// ``SharePickFlow`` 状态机钉测(R6 #4a):点选即忙 / 拉起成功 / 绕法成功 /
/// 双败降级排队 / 入队失败 / 重选流转。状态机面宿主可测(Ruling E 同款)。

import XCTest
@testable import MTDSSKit

final class SharePickFlowTests: XCTestCase {

    func testPickFromIdleEntersBusy() {
        var flow = SharePickFlow()
        XCTAssertTrue(flow.pick(deviceId: "d1"))
        XCTAssertEqual(flow.phase, .busy(deviceId: "d1"))
    }

    func testPickWhileBusyIsRejected() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        XCTAssertFalse(flow.pick(deviceId: "d2"))
        XCTAssertEqual(flow.phase, .busy(deviceId: "d1"))
    }

    func testOpenSuccessGoesLaunched() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.openSettled(opened: true, fallbackOpened: false)
        XCTAssertEqual(flow.phase, .launched(deviceId: "d1"))
    }

    /// open() 不通 + responder-chain 绕法成功 = launched(R6 #4b 主路径)
    func testFallbackSuccessGoesLaunched() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.openSettled(opened: false, fallbackOpened: true)
        XCTAssertEqual(flow.phase, .launched(deviceId: "d1"))
    }

    /// 双败 = queued(降级注记;「已排队,打开 MTDSS 即自动发送」)
    func testBothOpenPathsFailedGoesQueued() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.openSettled(opened: false, fallbackOpened: false)
        XCTAssertEqual(flow.phase, .queued(deviceId: "d1"))
    }

    /// open 竞窗超时(firstOrTimeout 回 nil = opened false)同双败形
    func testOpenTimeoutTreatedAsFailure() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.openSettled(opened: false, fallbackOpened: false)
        XCTAssertEqual(flow.phase, .queued(deviceId: "d1"))
    }

    func testEnqueueFailureGoesFailedAndAllowsRepick() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.enqueueFailed(message: "容器满")
        XCTAssertEqual(flow.phase, .failed(message: "容器满"))
        XCTAssertTrue(flow.pick(deviceId: "d2"))
        XCTAssertEqual(flow.phase, .busy(deviceId: "d2"))
    }

    /// queued 后可再选设备(换目标 = 新队列项,T11 语义)
    func testRepickAfterQueued() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.openSettled(opened: false, fallbackOpened: false)
        XCTAssertTrue(flow.pick(deviceId: "d2"))
        XCTAssertEqual(flow.phase, .busy(deviceId: "d2"))
    }

    /// settled 只在 busy 相位生效(launched 后迟到回话不翻转)
    func testSettledIgnoredOutsideBusy() {
        var flow = SharePickFlow()
        flow.pick(deviceId: "d1")
        flow.openSettled(opened: false, fallbackOpened: false)
        flow.openSettled(opened: true, fallbackOpened: false)
        XCTAssertEqual(flow.phase, .queued(deviceId: "d1"))
        var idle = SharePickFlow()
        idle.openSettled(opened: true, fallbackOpened: true)
        XCTAssertEqual(idle.phase, .idle)
    }
}
