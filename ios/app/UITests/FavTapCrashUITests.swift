/// 快捷目录导航连点回归(§13.7 崩溃修复回归锚,2026-11-06 R-task9):
/// iOS 26 ``NavigationStack`` 值路径在 pop/push **转场在飞窗**内再次变更 =
/// SwiftUI `NavigationColumnState.boundPathChange` 断言崩溃(SIGTRAP;sim
/// 复现 .ips 在案,用户报告形态「快捷目录点行进不去即崩,成员离线窗+连
/// 点」)。修复 = ``NavPathGate``(同值跳过 + 窗内写入错峰到窗尾套用)。
/// 两枚测试:
/// - **确定性节奏版**(hook 驱动,MTDSS_DEBUG_FAVOPEN=rapid):200ms 一拍
///   pop/push 连发 10 轮——修复前此节奏恒崩(app 进程 3s 内死),修复后
///   经闸错峰全程存活并继续可导航。回归钉的主体。
/// - **真实触点版**(XCUITest 合成触点):推入→点返回→再推入快速轮转,
///   断言进程存活 + 导航意图最终落地(闸只延迟不丢弃的用户可见语义)。
/// 前置:模拟器引擎 home(裸 home 即可;崩溃面纯导航机器,与登录态无关;
/// 收件箱根行随引擎 status 投影出现)。

import XCTest

final class FavTapCrashUITests: XCTestCase {

    /// 模拟器引擎 home(= build.sh 缺省;BrowseListUITests.simEngineHome
    /// 同源口径:sim 进程内 NSHomeDirectory 被定向到容器,按标准布局拼)。
    private static var simEngineHome: String {
        "/Users/" + NSUserName() + "/.mtdss-ios-sim"
    }

    @MainActor
    private static func labeled(_ app: XCUIApplication, _ label: String) -> XCUIElement {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", label)).firstMatch
    }

    @MainActor
    private static func firstRow(app: XCUIApplication, labelPrefix: String) -> XCUIElement {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "label BEGINSWITH %@", labelPrefix)
        ).firstMatch
    }

    /// 确定性节奏版:hook 200ms 一拍 pop/push 轮转(修复前 3s 内必崩的
    /// 节奏),跑完断言进程存活 + 导航机器仍可用(再推一页见「返回」)。
    @MainActor
    func testHookedRapidNavCycleSurvives() throws {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        app.launchEnvironment["MTDSS_DEBUG_FAVOPEN"] = "rapid"
        app.launch()

        // hook 轮转:推入 + 1s + 10×(200+200)ms ≈ 6s;给 15s 窗看进程。
        // 崩溃形态 = 进程死(app.state 不再是 runningForeground)。
        sleep(15)
        XCTAssertEqual(
            app.state, .runningForeground,
            "连点轮转后应用进程死亡(§13.7 NavigationStack 重入崩溃回归)")

        // 导航机器仍可用:点收件箱根行推入页,「返回」现身(闸延迟不吞意图)
        let inboxRow = Self.firstRow(app: app, labelPrefix: "收件箱")
        XCTAssertTrue(inboxRow.waitForExistence(timeout: 10), "收件箱根行未现")
        inboxRow.tap()
        let back = Self.labeled(app, "返回")
        XCTAssertTrue(
            back.waitForExistence(timeout: 8),
            "轮转收尾后推入失败(导航闸吞掉导航意图)")
    }

    /// 真实触点版:推入→点返回→再推入轮转 6 轮(合成触点,含桥延迟的自然
    /// 节奏),断言进程存活且每轮导航意图都落地(推入页「返回」/首页收件箱
    /// 根行按时现身)。
    @MainActor
    func testRapidTapBackAndForthSurvives() throws {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        app.launch()

        let inboxRow = Self.firstRow(app: app, labelPrefix: "收件箱")
        XCTAssertTrue(inboxRow.waitForExistence(timeout: 30), "收件箱根行未现(引擎未就绪)")

        for round in 0..<6 {
            inboxRow.tap()
            let back = Self.labeled(app, "返回")
            XCTAssertTrue(
                back.waitForExistence(timeout: 8),
                "第 \(round) 轮推入未达(导航意图丢失)")
            back.tap()
            XCTAssertTrue(
                inboxRow.waitForExistence(timeout: 8),
                "第 \(round) 轮返回未回首页(弹回意图丢失)")
        }
        XCTAssertEqual(
            app.state, .runningForeground,
            "连点轮转后应用进程死亡(§13.7 NavigationStack 重入崩溃回归)")
    }
}
