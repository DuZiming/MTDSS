/// 浏览 List 化触点面 UI 测试(R6 #3 + 评审轮 1/5;sim 合成触点 = XCUITest
/// 真事件,普通冒烟脚本无触点驱动先案的官方对应面):
/// - 原生 swipeActions:行左滑 → 动作钮现 → 点「删除」→ 居中确认弹窗现
///   → 取消收口(不动真文件;触达面 = 滑动→钮→确认全链);钮序帧断言
///   (删除贴缘);**目录行(空动作集)左滑无钮**(M-1 无条件挂载的无示能钉);
/// - 左缘右滑返回:谓词命中 → 弹回首页(照片图库固定行在场 = 首页标记);
/// - **左缘触点透传**(评审 I-2):返回箭头点按 / 目录行内展开钮点按 /
///   行首近缘点按导航——带/手势不得吞 x<30 的点按;
/// - **选择翻转滚动保持**(评审 I-1 钉):滚到底 → 长按进选择 → 确认退出
///   → 尾注仍可命中(整页重挂则必回顶);
/// - 远端行滑序(M-4):[下载,转发] 下载贴缘(对端 C:\mtdss-t6a\inbox;
///   对端/目录不可用 = 跳过不判失败)。
/// 前置:引擎 home 已登录(scheme 注入 UITEST_MTDSS_HOME);收件箱内有
/// IMG_* 文件(冒烟/历史 E2E 产物,无则相关测跳过为通过)。

import XCTest

final class BrowseListUITests: XCTestCase {

    /// 启动并经 DEBUG 钩子推入收件箱浏览页;返回 app,文件行已就绪。
    @MainActor
    private func launchAndPushInbox() throws -> (XCUIApplication, XCUIElement) {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        app.launchEnvironment["MTDSS_DEBUG_AUTOPUSH"] = "inbox"
        app.launch()
        // 等浏览页任意文件行(行 a11y = 文件名;收件箱 IMG_* 为历史产物)。
        // List 行的可访问性暴露层型不保证是 button(List/UICollectionView
        // 可扁平化为 cell/other/staticText)——按任意元素型宽匹配。
        let fileRow = Self.firstRow(app: app, labelPrefix: "IMG_")
        guard fileRow.waitForExistence(timeout: 30) else {
            // 收件箱空(裸 home)= 无触点面可测:跳过(不判失败)
            throw XCTSkip("收件箱无 IMG_* 文件行(裸 home),List 触点面无可测对象")
        }
        return (app, fileRow)
    }

    /// 启动并推入**远端**浏览页直开对端 inbox(有文件行才有滑面;对端 =
    /// DESKTOP-VG14605 的 C:\mtdss-t6a\inbox)。对端离线/目录不可用 = 跳过。
    @MainActor
    private func launchAndPushRemoteInbox() throws -> (XCUIApplication, XCUIElement) {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        app.launchEnvironment["MTDSS_DEBUG_AUTOPUSH"] = "device:windows"
        app.launchEnvironment["MTDSS_DEBUG_REMOTE_OPEN"] = "C:\\mtdss-t6a\\inbox"
        app.launch()
        let fileRow = Self.firstRow(app: app, labelPrefix: "a.txt")
        guard fileRow.waitForExistence(timeout: 60) else {
            throw XCTSkip("对端 C:\\mtdss-t6a\\inbox 文件行未现(对端离线/目录变更)")
        }
        return (app, fileRow)
    }

    /// 模拟器引擎 home(= build.sh 缺省 ~/.mtdss-ios-sim)。**不可用
    /// NSHomeDirectory()/getpwuid**:sim 进程内二者皆被定向到 App 沙箱容器
    /// (实测 boot 日志钉:home=<容器>/.mtdss-ios-sim,路径超长,sun_path 104
    /// 上限 → 引擎 UDS bind 失败 helloTimeout)。sim 进程与 Mac 用户同 uid,
    /// ``NSUserName()`` 恒回宿主登录名,按标准布局拼 /Users/<name>。
    /// UITEST_MTDSS_HOME 仍可覆盖。
    private static var simEngineHome: String {
        "/Users/" + NSUserName() + "/.mtdss-ios-sim"
    }

    /// 按标签前缀宽匹配行(任意元素型;List 扁平化下 button/staticText/
    /// other 皆可能承载行 a11y)。
    @MainActor
    private static func firstRow(app: XCUIApplication, labelPrefix: String) -> XCUIElement {
        app.descendants(matching: .any).matching(
            NSPredicate(format: "label BEGINSWITH %@", labelPrefix)
        ).firstMatch
    }

    /// 按标签精确匹配任意元素型。
    @MainActor
    private static func labeled(_ app: XCUIApplication, _ label: String) -> XCUIElement {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@", label)).firstMatch
    }

    /// 左滑 → 首动作钮现 → 帧断言 edge 钮贴缘(R6 评审 M-4:本地/远端双钉)。
    @MainActor
    private func assertSwipeOrder(
        app: XCUIApplication, row: XCUIElement, edge: String, inner: String,
    ) throws {
        row.swipeLeft()
        let edgeBtn = Self.labeled(app, edge)
        XCTAssertTrue(
            edgeBtn.waitForExistence(timeout: 5),
            "左滑后「\(edge)」钮未现(原生 swipeActions 未生效)")
        let innerBtn = Self.labeled(app, inner)
        guard innerBtn.exists else {
            XCTFail("左滑后「\(inner)」钮未现(动作簇不全)")
            return
        }
        XCTAssertGreaterThan(
            edgeBtn.frame.maxX, innerBtn.frame.maxX,
            "「\(edge)」钮应贴滑出簇最右(decl 序/贴缘关系不符)")
    }

    /// 原生左滑(本地文件行 [发送,删除]):滑动 → 删除钮现+贴缘 → 确认弹窗
    /// 现 → 取消(全链触达,零副作用)。
    @MainActor
    func testNativeSwipeRevealsActionsAndConfirmDialog() throws {
        let (app, fileRow) = try launchAndPushInbox()
        try assertSwipeOrder(app: app, row: fileRow, edge: "删除", inner: "发送")
        let delete = Self.labeled(app, "删除")
        delete.tap()
        // 居中确认弹窗(R5b #9 同款文案面);出现即证动作链通,取消收口
        let cancel = app.alerts.firstMatch.buttons["取消"]
        XCTAssertTrue(
            cancel.waitForExistence(timeout: 5),
            "删除确认弹窗未现(滑动动作未达 handleSwipe)")
        cancel.tap()
    }

    /// 目录行(空动作集)左滑无示能(R6 评审 M-1 钉:无条件挂载下空集=
    /// 空动作面,不得出删除钮)。
    @MainActor
    func testDirectoryRowSwipeHasNoActions() throws {
        let (app, _) = try launchAndPushInbox()
        let dirRow = Self.firstRow(app: app, labelPrefix: "Codex")
        XCTAssertTrue(dirRow.waitForExistence(timeout: 10), "Codex 目录行未现")
        dirRow.swipeLeft()
        let delete = Self.labeled(app, "删除")
        XCTAssertFalse(
            delete.waitForExistence(timeout: 2),
            "目录行(空动作集)左滑出现删除钮——空集挂载面回归")
    }

    /// 左缘右滑返回:起手 x≈4pt 右拖 ~170pt → 谓词命中 → 弹回首页。
    @MainActor
    func testEdgeBackPopsToHome() throws {
        let (app, _) = try launchAndPushInbox()
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: end)
        // 首页标记:照片图库固定行(R5c 第二固定行;浏览推入页无;宽匹配同上)
        let photoRow = Self.firstRow(app: app, labelPrefix: "照片图库")
        XCTAssertTrue(
            photoRow.waitForExistence(timeout: 8),
            "左缘右滑后未弹回首页(边缘返回手势未触发)")
    }

    // MARK: 左缘触点透传(R6 评审 I-2:带/手势不得吞 x<30 点按)

    /// (a) 顶栏返回箭头(x≈18-38,带重叠)点按 → 弹回首页。
    @MainActor
    func testBackArrowTapNavigatesHome() throws {
        let (app, _) = try launchAndPushInbox()
        let back = Self.labeled(app, "返回")
        XCTAssertTrue(back.waitForExistence(timeout: 5), "返回钮未现")
        back.tap()
        let photoRow = Self.firstRow(app: app, labelPrefix: "照片图库")
        XCTAssertTrue(
            photoRow.waitForExistence(timeout: 8),
            "点按返回箭头未弹回首页(x<30 点按被左缘带吞)")
    }

    /// (b) 目录行内展开钮(x≈20-48,带重叠)点按 → 展开(收起钮现)。
    @MainActor
    func testDirectoryChevronTapExpands() throws {
        let (app, _) = try launchAndPushInbox()
        let chevron = Self.labeled(app, "展开")
        XCTAssertTrue(chevron.waitForExistence(timeout: 10), "展开钮未现")
        chevron.tap()
        let collapse = Self.labeled(app, "收起")
        XCTAssertTrue(
            collapse.waitForExistence(timeout: 5),
            "点按展开钮未展开(x<30 点按被左缘带吞)")
    }

    /// (c) 行首近缘点按(x≈15)导航 → 进该目录整页(尾注计数翻转为目录项数)。
    @MainActor
    func testRowTapNearLeadingEdgeNavigates() throws {
        let (app, _) = try launchAndPushInbox()
        let dirRow = Self.firstRow(app: app, labelPrefix: "Obsidian Vault")
        XCTAssertTrue(dirRow.waitForExistence(timeout: 10), "Obsidian Vault 目录行未现")
        let width = dirRow.frame.width
        dirRow.coordinate(withNormalizedOffset: CGVector(dx: 15.0 / width, dy: 0.5)).tap()
        // 进页后浏览根 = 该目录(VM init 定根)→ 尾注 = 目录项数(3 项;
        // 若点按被吞仍停在收件箱 = 13 项)
        let footer = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == 'item-count-footer'")).firstMatch
        XCTAssertTrue(footer.waitForExistence(timeout: 5), "尾注未现")
        XCTAssertEqual(
            footer.label, "3 项",
            "x≈15 行点按未导航进目录(点按被左缘带吞;尾注仍为收件箱计数:\(footer.label))")
    }

    // MARK: 选择翻转滚动保持(R6 评审 I-1 钉:整页重挂则必回顶)

    @MainActor
    func testSelectionTogglePreservesScroll() throws {
        let (app, _) = try launchAndPushInbox()
        app.swipeUp()
        app.swipeUp()
        app.swipeUp()
        let footer = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == 'item-count-footer'")).firstMatch
        XCTAssertTrue(footer.waitForExistence(timeout: 5), "尾注未现")
        if !footer.isHittable {
            throw XCTSkip("收件箱行数不足一屏(滚动无位差),滚动保持无可测对象")
        }
        // 长按任意**可见**行进选择(0.4s 阈;合成按 0.6s)。必须取屏内行——
        // XCUITest 对不可命中元素会自动滚动到位,滚动保持钉会被自己破坏
        let rows = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH 'IMG_'")).allElementsBoundByIndex
        guard let visibleRow = rows.first(where: { $0.isHittable }) else {
            throw XCTSkip("滚动后无可命中文件行")
        }
        visibleRow.press(forDuration: 0.6)
        let selectAll = Self.labeled(app, "全选")
        XCTAssertTrue(
            selectAll.waitForExistence(timeout: 5),
            "长按后未进选择态")
        Self.labeled(app, "确认").tap()
        XCTAssertTrue(
            footer.waitForExistence(timeout: 5) && footer.isHittable,
            "选择进/出后尾注不可命中——滚动位置丢失(整页重挂回归)")
    }

    /// 远端文件行 [下载,转发] 滑序(M-4):下载贴缘(R5b 远端动作集序)。
    @MainActor
    func testRemoteRowSwipeOrder() throws {
        let (app, fileRow) = try launchAndPushRemoteInbox()
        try assertSwipeOrder(app: app, row: fileRow, edge: "下载", inner: "转发")
    }
}
