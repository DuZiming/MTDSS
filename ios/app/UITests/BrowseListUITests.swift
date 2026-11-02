/// 浏览 List 化触点面 UI 测试(R6 #3;sim 合成触点 = XCUITest 真事件,
/// 普通冒烟脚本无触点驱动先案的官方对应面):
/// - 原生 swipeActions:行左滑 → 动作钮现 → 点「删除」→ 居中确认弹窗现
///   → 取消收口(不动真文件;触达面 = 滑动→钮→确认全链);
/// - 左缘右滑返回:``EdgeBackStrip`` 谓词命中 → 弹回首页(照片图库固定行
///   在场 = 首页标记;浏览推入页无此元素)。
/// 前置:引擎 home 已登录(UITEST_MTDSS_HOME,缺省 ~/.mtdss-ios-sim;
/// 收件箱内有 IMG_* 文件——冒烟/历史 E2E 产物,无则两测都跳过为通过)。

import XCTest

final class BrowseListUITests: XCTestCase {

    /// 启动并经 DEBUG 钩子推入收件箱浏览页;返回 app,文件行已就绪。
    @MainActor
    private func launchAndPushInbox() throws -> (XCUIApplication, XCUIElement) {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"]
                ?? NSHomeDirectory() + "/.mtdss-ios-sim"
        app.launchEnvironment["MTDSS_DEBUG_AUTOPUSH"] = "inbox"
        app.launch()
        // 等浏览页任意文件行(行 a11y = 文件名;收件箱 IMG_* 为历史产物)
        let fileRow = app.buttons.matching(
            NSPredicate(format: "label BEGINSWITH 'IMG_'")
        ).firstMatch
        guard fileRow.waitForExistence(timeout: 30) else {
            // 收件箱空(裸 home)= 无触点面可测:跳过(不判失败)
            throw XCTSkip("收件箱无 IMG_* 文件行(裸 home),List 触点面无可测对象")
        }
        return (app, fileRow)
    }

    /// 原生左滑:滑动 → 删除钮现 → 确认弹窗现 → 取消(全链触达,零副作用)。
    @MainActor
    func testNativeSwipeRevealsActionsAndConfirmDialog() throws {
        let (app, fileRow) = try launchAndPushInbox()
        fileRow.swipeLeft()
        let delete = app.buttons["删除"]
        XCTAssertTrue(
            delete.waitForExistence(timeout: 5),
            "左滑后删除钮未现(原生 swipeActions 未生效——List 迁移面回退)")
        delete.tap()
        // 居中确认弹窗(R5b #9 同款文案面);出现即证动作链通,取消收口
        let cancel = app.alerts.firstMatch.buttons["取消"]
        XCTAssertTrue(
            cancel.waitForExistence(timeout: 5),
            "删除确认弹窗未现(滑动动作未达 handleSwipe)")
        cancel.tap()
    }

    /// 左缘右滑返回:起手 x≈4pt 右拖 ~170pt → EdgeBackStrip 命中 → 弹回首页。
    @MainActor
    func testEdgeBackPopsToHome() throws {
        let (app, _) = try launchAndPushInbox()
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: end)
        // 首页标记:照片图库固定行(R5c 第二固定行;浏览推入页无)
        let photoRow = app.buttons["照片图库"]
        XCTAssertTrue(
            photoRow.waitForExistence(timeout: 8),
            "左缘右滑后未弹回首页(EdgeBackStrip 未触发)")
    }
}
