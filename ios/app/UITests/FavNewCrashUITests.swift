/// 新建快捷点入崩溃回归(§14.6 崩溃二轮修复回归锚,2026-11-06 S7):
/// iOS 26 ``NavigationStack`` 值路径混入**异型路径元素**时,SwiftUI 转场
/// 记账 `AnyNavigationPath` 类型比较 try! 即炸(`comparisonTypeMismatch`
/// 断言,SIGTRAP,NavigationColumnState.swift:684;.ips+console 双证在案)
/// ——混合页页内子 NavigationStack(FavDrillTarget)与外层栈
/// (FavoritesRoute)异型,**任意快捷推入首拍必崩**(新/旧条目、0-600ms
/// 点入延迟全崩,复现矩阵钉死;与 R9 §13.7 转场窗重入同址断言不同根,
/// 两修互补,NavPathGate 不涉本崩)。二轮修复 = **结构根修**:嵌套子栈
/// 撤销,钻取并入外层同型值路由(``FavoritesRoute/drill``),区域单一路径
/// 类型,崩面结构性消除(spec §14.6;候选「闸改转场观测」取证后证伪回退,
/// 勿据此找闸代码)。
///
/// 本测 = S7 复现驱动(MTDSS_DEBUG_FAVNEW=rapid)同拍回归钉:「新建快捷
/// 目录→点入→秒退→再点」×8 轮(0/100/300/600ms 点入延迟循环),修复前
/// 首推必崩(≈7s 进程死,.ips 在案),修复后全程存活——**任意快捷首推
/// 存活锚**(不限新建;新旧两因子隔离钉 = 钩子 old0 模式,harness 跑)。
///
/// **轮转可观测断言**(R9 marker 同款,防空转通过):驱动收尾把完成轮数
/// 写 `{MTDSS_HOME}/favnew-marker`;本测断言标记在场且 ≥8。驱动含登录
/// (dev 回显码 rig),**前置**:Mac 上先起本地 signaling rig
/// (`signaling-server --plain-http 127.0.0.1:8787 --dev-echo-code` +
/// `mtdss-admin user add repro@test.local`,见 artifacts/v3p3-task9-
/// crashrepro/s7-harness.sh),launchEnvironment 经
/// MTDSS_DEBUG_FAVNEW_SERVER/MTDSS_DEBUG_FAVNEW_EMAIL 注入。

import XCTest

final class FavNewCrashUITests: XCTestCase {

    /// 模拟器引擎 home(= build.sh 缺省;FavTapCrashUITests 同源口径)。
    private static var simEngineHome: String {
        "/Users/" + NSUserName() + "/.mtdss-ios-sim"
    }

    /// rapid 轮数(与 FavoritesView 驱动缺省一致)。
    private static let rounds = 8

    @MainActor
    func testFavNewRapidTapCycleSurvives() throws {
        let home = ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        let markerURL = URL(fileURLWithPath: home).appendingPathComponent("favnew-marker")
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] = home
        app.launchEnvironment["MTDSS_DEBUG_FAVNEW"] = "rapid"
        app.launchEnvironment["MTDSS_DEBUG_FAVNEW_ROUNDS"] = String(Self.rounds)
        if let server = ProcessInfo.processInfo.environment["UITEST_FAVNEW_SERVER"] {
            app.launchEnvironment["MTDSS_DEBUG_FAVNEW_SERVER"] = server
        }
        if let email = ProcessInfo.processInfo.environment["UITEST_FAVNEW_EMAIL"] {
            app.launchEnvironment["MTDSS_DEBUG_FAVNEW_EMAIL"] = email
        }
        // 跨轮残留清零:标记必须产自本轮 app(上轮旧标记会假绿)
        try? FileManager.default.removeItem(at: markerURL)
        app.launch()

        // 驱动耗时:登录(≤5s)+ 8 轮 × (create RPC + 0~0.6s 点入延迟 +
        // 0.3/0.15/1.2/0.8s 节拍)≈ 30s;给 60s 窗看进程。
        // 崩溃形态 = 进程死(app.state 不再是 runningForeground)。
        sleep(60)
        XCTAssertEqual(
            app.state, .runningForeground,
            "新建快捷点入轮转后应用进程死亡(§14.6 NavigationStack 转场重入崩溃回归)")

        // 轮转发生断言(防空转通过):驱动 marker 在场且轮数足额。
        guard let raw = try? String(contentsOf: markerURL, encoding: .utf8) else {
            XCTFail("轮转标记缺席(favnew-marker)——驱动未跑,回归锚空转")
            return
        }
        let parts = raw.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: ":")
        XCTAssertGreaterThanOrEqual(
            parts.count >= 2 ? Int(parts[1]) ?? 0 : 0, Self.rounds,
            "完成轮数不足(marker=\(raw))——驱动提前退出,回归锚空转")
    }
}
