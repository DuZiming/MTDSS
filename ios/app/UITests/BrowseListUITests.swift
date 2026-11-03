/// 浏览 List 化触点面 UI 测试(R6 #3 + 评审轮 1/5;sim 合成触点 = XCUITest
/// 真事件,普通冒烟脚本无触点驱动先案的官方对应面):
/// - 原生 swipeActions:行左滑 → 动作钮现 → 点「删除」→ 居中确认弹窗现
///   → 取消收口(不动真文件;触达面 = 滑动→钮→确认全链);钮序帧断言
///   (删除贴缘);**目录行左滑 = 文件夹动作集 +「删除文件夹及内容?」
///   文案**(R11 ③,原 R6 M-1 空集钉随启用翻转);
/// - 左缘右滑返回:谓词命中 → 弹回首页(收件箱根行在场 = 首页标记);
/// - **左缘触点透传**(评审 I-2):返回箭头点按 / 目录行内展开钮点按 /
///   行首近缘点按导航——带/手势不得吞 x<30 的点按;
/// - **选择翻转滚动保持**(评审 I-1 钉):滚到底 → 长按进选择 → 确认退出
///   → 尾注仍可命中(整页重挂则必回顶);
/// - 远端行滑序(M-4,R9 钉正):[下载,转发] **转发贴缘**(动作集末位=
///   簇最右,与本地删除贴缘同律;M-4 原期望「下载贴缘」系笔误且对端不可达
///   恒 skip 从未实证,对端 44d97f96 可达时钉);
/// - 远端删除门(R9 #3):门开左滑 删除 贴缘 / 门关回今日集(对端可达时;
///   收尾置回门关,保 M-4 前置)。
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
    /// Windows 构建盒 dev daemon(home C:\mtdss-t6a,`MTDSS_HOME=C:\mtdss-t6a
    /// mtdssd` 即起,信令产线;id 自 session.json,R9 重钉——原 c87f79e0
    /// 的 home 已不存在,R6 起 M-4 环境skip 根因之一)。inbox = {home}/inbox
    /// 默认授权,a.txt 为历史 E2E 实档)。对端离线/目录不可用 = 跳过。
    /// `gate` = MTDSS_DEBUG_ALLOWREMOTEDELETE 注入值(R9 #3 门两态钉;
    /// App init 直写持久开关,「0」显式清防跨用例/跨轮残留)。
    @MainActor
    private func launchAndPushRemoteInbox(gate: String? = nil) throws -> (XCUIApplication, XCUIElement) {
        let app = XCUIApplication()
        app.launchEnvironment["MTDSS_HOME"] =
            ProcessInfo.processInfo.environment["UITEST_MTDSS_HOME"] ?? Self.simEngineHome
        // 设备标签必须激活:AUTOPUSH device 钩子挂 DevicesView .task(标签
        // 根),TabView 惰性挂载下非本标签不 appear 钩子不跑——R6 起 M-4
        // 远端滑序「环境 skip」的另一半根因(此前只疑对端可达性)。
        app.launchEnvironment["MTDSS_DEBUG_TAB"] = "devices"
        app.launchEnvironment["MTDSS_DEBUG_AUTOPUSH"] = "device:id=44d97f9625e525de79f35c73bfed8981"
        app.launchEnvironment["MTDSS_DEBUG_REMOTE_OPEN"] = "C:\\mtdss-t6a\\inbox"
        if let gate {
            app.launchEnvironment["MTDSS_DEBUG_ALLOWREMOTEDELETE"] = gate
        }
        app.launch()
        let fileRow = Self.firstRow(app: app, labelPrefix: "a.txt")
        // 90s:冷对端 punch 建连 25-35s 先案(R5c)+ 列表往返
        guard fileRow.waitForExistence(timeout: 90) else {
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

    /// 目录行左滑(R11 ③,用户裁定「文件夹要支持左滑」;原 R6 M-1 空集
    /// 钉随目录行动作集启用翻转):左滑 → [发送,删除] 钮现(删除贴缘)
    /// → 点删除 → **文件夹形确认弹窗**「删除文件夹及内容?」现(整树文案
    /// 面)→ 取消收口(零副作用)。
    @MainActor
    func testDirectoryRowSwipeRevealsFolderActions() throws {
        let (app, _) = try launchAndPushInbox()
        let dirRow = Self.firstRow(app: app, labelPrefix: "Codex")
        XCTAssertTrue(dirRow.waitForExistence(timeout: 10), "Codex 目录行未现")
        try assertSwipeOrder(app: app, row: dirRow, edge: "删除", inner: "发送")
        let delete = Self.labeled(app, "删除")
        delete.tap()
        let folderTitle = app.alerts.firstMatch.staticTexts["删除文件夹及内容?"]
        XCTAssertTrue(
            folderTitle.waitForExistence(timeout: 5),
            "文件夹形删除确认弹窗未现(目录行滑删文案面回归)")
        let cancel = app.alerts.firstMatch.buttons["取消"]
        XCTAssertTrue(cancel.exists, "取消钮未现")
        cancel.tap()
    }

    /// 左缘右滑返回:起手 x≈4pt 右拖 ~170pt → 谓词命中 → 弹回首页。
    @MainActor
    func testEdgeBackPopsToHome() throws {
        let (app, _) = try launchAndPushInbox()
        let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: end)
        // 首页标记:收件箱根行(R7 撤照片图库固定行后首页恒显收件箱;
        // 浏览推入页无;宽匹配同上)
        let inboxRow = Self.firstRow(app: app, labelPrefix: "收件箱")
        XCTAssertTrue(
            inboxRow.waitForExistence(timeout: 8),
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
        let inboxRow = Self.firstRow(app: app, labelPrefix: "收件箱")
        XCTAssertTrue(
            inboxRow.waitForExistence(timeout: 8),
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

    /// (c) 左缘触点交付图谱(x=35 展开 / x=60 行导航):
    /// **iOS 26 系统前缘识别器保留带 ~0-30pt**——隐藏导航栏下
    /// interactivePop 的边缘识别器仍占前缘:带内合成点按不达(x15/x25
    /// 实测,frame (0,284,440,58) 落点精确仍在带内不达;带外 34/60 达)。
    /// 非本波引入(R2 前同形态),带内无任何可交互内容(选圈/箭头/图标
    /// 均 x≥20,实靶中心 ≥28),人手指点按实靶不受影响;**左缘拖拽仍返回**
    /// (我方谓词与系统 pop 在浏览根同归宿,用户可见行为不变)。真机轮
    /// 复核带宽与手感。
    @MainActor
    func testLeadingEdgeTouchDelivery() throws {
        let (app, _) = try launchAndPushInbox()
        let dirRow = Self.firstRow(app: app, labelPrefix: "Obsidian Vault")
        XCTAssertTrue(dirRow.waitForExistence(timeout: 10), "Obsidian Vault 目录行未现")
        let y = dirRow.frame.midY
        let appFrame = app.frame
        func tapAt(_ x: CGFloat) {
            app.coordinate(withNormalizedOffset: CGVector(
                dx: x / appFrame.width, dy: y / appFrame.height)).tap()
        }
        // ① x=35(带外,展开钮右半):交付到行内展开钮 → 收起钮现
        tapAt(35)
        let collapse = Self.labeled(app, "收起")
        XCTAssertTrue(
            collapse.waitForExistence(timeout: 5),
            "x=35 点按未交付到展开钮(带比预期宽)")
        // ② 收起复位
        collapse.tap()
        // ③ x=60(文件夹图标区):交付到行 → 导航进目录 → 尾注在场
        tapAt(60)
        let footer = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == 'item-count-footer'")).firstMatch
        XCTAssertTrue(
            footer.waitForExistence(timeout: 5),
            "x=60 行点按未导航(交付面回归)")
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
        // XCUITest 对不可命中元素会自动滚动到位,滚动保持钉会被自己破坏。
        // 滚动代理用**屏内行**(页中部,不受确认后标签栏回归的底部遮挡面):
        // 选择进/出后同一行仍可命中 = 未整页重挂;重挂 → 滚动归零 → 该行
        // 沉到屏外不可命中。
        let rows = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label BEGINSWITH 'IMG_'")).allElementsBoundByIndex
        guard let visibleRow = rows.first(where: { $0.isHittable }) else {
            throw XCTSkip("滚动后无可命中文件行")
        }
        let pinnedLabel = visibleRow.label
        visibleRow.press(forDuration: 0.6)
        let selectAll = Self.labeled(app, "全选")
        XCTAssertTrue(
            selectAll.waitForExistence(timeout: 5),
            "长按后未进选择态")
        Self.labeled(app, "确认").tap()
        let pinned = Self.firstRow(app: app, labelPrefix: pinnedLabel)
        XCTAssertTrue(
            pinned.waitForExistence(timeout: 5) && pinned.isHittable,
            "选择进/出后原可见行(\(pinnedLabel))不可命中——滚动位置丢失(整页重挂回归)")
    }

    /// 远端文件行 [下载,转发] 滑序(M-4,R9 复测钉正):**转发贴缘**——
    /// 动作集末位恒在滑出簇最右(NativeSwipeActions reversed 声明=首声明
    /// 者贴缘,即数组末位;本地 [发送,删除] 删除贴缘同律)。M-4 原期望
    /// 「下载贴缘」系笔误且 R6 起对端不可达恒 skip 从未实证,R9 首次对
    /// 活对端跑出即纠正。
    @MainActor
    func testRemoteRowSwipeOrder() throws {
        let (app, fileRow) = try launchAndPushRemoteInbox()
        try assertSwipeOrder(app: app, row: fileRow, edge: "转发", inner: "下载")
    }

    /// 远端删除门(R9 #3;对端不可达 = 跳过,同上):门开左滑出「删除」
    /// 且贴缘(动作集 [下载,转发,删除],Kit 纯函数钉同语言);**门开臂在
    /// 前、门关臂在后收尾**——门钩子直写持久开关,收尾置回 0 保
    /// testRemoteRowSwipeOrder(M-4,断言 转发贴缘)的前置假设。
    @MainActor
    func testRemoteRowSwipeDeleteGate() throws {
        // 门开臂:删除现且贴缘(簇末位,今日集的「转发贴缘」被取代)
        let (appOn, rowOn) = try launchAndPushRemoteInbox(gate: "1")
        try assertSwipeOrder(app: appOn, row: rowOn, edge: "删除", inner: "转发")
        // 门关臂:回到今日集(转发贴缘,删除不现)
        let (appOff, rowOff) = try launchAndPushRemoteInbox(gate: "0")
        try assertSwipeOrder(app: appOff, row: rowOff, edge: "转发", inner: "下载")
        let deleteOff = Self.labeled(appOff, "删除")
        XCTAssertFalse(
            deleteOff.waitForExistence(timeout: 2),
            "门关左滑出现「删除」钮(远端删除门失效)")
    }
}
