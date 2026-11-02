/// 照片权限授权 UI 测试(R5c;一次性授权面,sim 冒烟/E2E 前置):
/// simctl privacy grant photos 在 iOS 26 simruntime 不被 tccd 采信(外部
/// 写入的 TCC 行恒按 notDetermined 评估)——唯一可靠授权 = 真系统弹窗被
/// 应答。本测试把 App 带进照片图库页(DEBUG 钩子 MTDSS_DEBUG_PHOTO_PROMPT
/// 触发 requestAccess),等待 springboard 弹窗并点「允许访问所有照片」,
/// tccd 自铸的授权行落库后,常规 simctl launch 即恒已授权。
/// 无弹窗(已授权/已被拒绝)= 幂等通过,不失败。

import XCTest

final class GrantPhotosUITests: XCTestCase {

    func testGrantPhotosPermission() {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] = ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? NSHomeDirectory() + "/.mtdss-ios-sim"
        app.launchEnvironment["MTDSS_DEBUG_PHOTO"] = "1"
        app.launchEnvironment["MTDSS_DEBUG_PHOTO_PROMPT"] = "1"
        app.launch()

        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        // iOS 17+ 「允许访问所有照片」/「允许完全访问」(中文 sim)/旧英文文案;
        // 队列中可能叠多枚历史弹窗(每次未应答的 requestAccess 一枚)——循环
        // 点按直至收口(上界 6 轮),仍在场 = tap 未生效,判失败。
        let allowPredicate = NSPredicate(format: "label IN {'Allow Access to All Photos', 'Allow Full Access', '允许访问所有照片', '允许完全访问'}")
        var attempts = 0
        while attempts < 6 {
            let allow = springboard.buttons.matching(allowPredicate).firstMatch
            guard allow.exists else { break }
            allow.tap()
            attempts += 1
            sleep(2)
        }
        if attempts > 0 {
            let remaining = springboard.buttons.matching(allowPredicate).firstMatch
            XCTAssertTrue(!remaining.exists, "授权弹窗点按 \(attempts) 轮后仍在场(tap 未生效)")
        }
        // 幂等:无弹窗(已授权)不失败

        // 通知授权弹窗顺手收口(E2E 传输完成通知首次触发;精确匹配「允许」,
        // 不误触照片弹窗的「允许完全访问」)
        let notifAllow = springboard.buttons["允许"]
        if notifAllow.waitForExistence(timeout: 3) {
            notifAllow.tap()
            sleep(1)
        }
    }
}
