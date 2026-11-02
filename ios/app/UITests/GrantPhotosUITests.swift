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
        // iOS 17+ 「允许访问所有照片」/「允许完全访问」(中文 sim)/旧英文文案
        let allow = springboard.buttons.matching(
            NSPredicate(format: "label IN {'Allow Access to All Photos', 'Allow Full Access', '允许访问所有照片', '允许完全访问'}"),
        ).firstMatch
        if allow.waitForExistence(timeout: 30) {
            allow.tap()
            // 弹窗必须真正收口(授权行落库)——仍在场 = 点按未生效,判失败
            let gone = allow.waitForNonExistence(timeout: 15)
            XCTAssertTrue(gone, "授权弹窗点按后仍未消失(tap 未生效)")
        }
        // 幂等:无弹窗(已授权)不失败
    }
}
