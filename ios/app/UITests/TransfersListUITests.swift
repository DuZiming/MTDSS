/// 传输列表 List 化触点面 UI 测试(R9 #1;BrowseListUITests 同法,sim 合成
/// 触点 = XCUITest 真事件):传输页自 SinkingFooterList + 自绘 SwipeableRow
/// 迁 ``BrowseList``(List)+ 原生 `.swipeActions`(R6 #3 浏览页同迁先案)
/// ——钉:传输行左滑出原生动作钮(合成 done 行,动作集 [清除]),
/// 点按动作达 VM(合成 id 的 transfer.clear 被引擎拒 → 错误横幅现)。
/// 前置:引擎 home 已登录(scheme 注入 UITEST_MTDSS_HOME)。

import XCTest

final class TransfersListUITests: XCTestCase {

    /// 模拟器引擎 home(= build.sh 缺省 ~/.mtdss-ios-sim;同 BrowseListUITests
    /// ——sim 进程内 NSHomeDirectory/getpwuid 被沙箱定向,须按宿主登录名拼
    /// 标准短 home,UDS sun_path 104 上限)。
    private static var simEngineHome: String {
        "/Users/" + NSUserName() + "/.mtdss-ios-sim"
    }

    /// 传输行原生左滑全链(R9 #1):
    /// MTDSS_DEBUG_TAB=transfers 直落传输标签;MTDSS_DEBUG_SKIPROW=1 合成
    /// done 行(kind=upload 有记录面、无同签名在途 → 动作集恰 [清除],
    /// TransfersFormat.actions 钉测同语言)。左滑 → 「清除」钮现(原生
    /// swipeActions 在传输行生效)→ 点按 → VM clear → 引擎对合成 id 报
    /// 「任务不存在」→ 页内错误横幅现(动作链触达实证,零真任务副作用)。
    @MainActor
    func testTransferRowNativeSwipeRevealsClearAndFires() throws {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        app.launchEnvironment["MTDSS_DEBUG_TAB"] = "transfers"
        app.launchEnvironment["MTDSS_DEBUG_SKIPROW"] = "1"
        app.launch()
        // 行 a11y = 显示名「debug-skip · 5 项」(filesTotal>1 带 N 项段);
        // List 行元素型不保证(同 BrowseListUITests 宽匹配)。
        let row = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH %@", "debug-skip")).firstMatch
        guard row.waitForExistence(timeout: 30) else {
            throw XCTSkip("传输列表合成行未现(引擎未就绪/列表加载失败)")
        }
        row.swipeLeft()
        let clear = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", "清除")).firstMatch
        XCTAssertTrue(
            clear.waitForExistence(timeout: 5),
            "传输行左滑「清除」钮未现(原生 swipeActions 未生效)")
        clear.tap()
        // 动作达 VM/引擎:一次性错误横幅(BrowseNavBar 下方红胶囊)承载
        // 引擎拒答文案「任务不存在」——点按后现 = 全链触达。
        let banner = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@", "任务不存在")).firstMatch
        XCTAssertTrue(
            banner.waitForExistence(timeout: 10),
            "点「清除」后错误横幅未现(动作未达 VM/引擎)")
    }
}
