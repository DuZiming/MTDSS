/// BrowseViewModel — 浏览页视图模型(spec §4.1 浏览页模板:收件箱/远端浏览
/// 共用;镜像 android BrowseViewModel 的 seq 守卫语义,补 T5 面的排序/搜索/
/// 隐藏过滤与浏览器式历史):
/// - **seq 守卫**:快速连续导航时过期响应丢弃(seq 单调,旧响应不得覆盖新
///   位置;android 同款);
/// - **浏览器式历史**:返回/前进双箭头(spec §4.1 顶栏);open 截断前进
///   分支追加历史,back/forward 只移动游标;
/// - **导航即时反馈**(R12 ②,用户报告「标题先变内容后变,无反馈;返回
///   同」):open/goBack/goForward 标题先行 + **列表即刻清空进 loading**
///   (旧目录条目不得在拉取窗内残留,远端冷握手 25-35s);原地 reload
///   不清(下拉刷新/「···」刷新/删除后重载保内容语义不变);
/// - 排序:文件夹恒前(所有字段),默认 名称 升序;日期 最新在前、大小
///   大者在前、种类 按扩展名(对齐 iOS 文件 App 惯例);
/// - 隐藏项(dotfiles)默认过滤——「显示隐藏项」开关归 T10 设置,VM 只挂
///   ``showHidden`` 输入;搜索框按名称过滤当前列表;
/// - 副文:「日期 - N 项」(目录)/「日期 - 大小」(文件);list 契约
///   children 子项数已接(v2-P5 T5b,目录项携直接子项数;文件项/stat 单查
///   缺省 → 纯日期);
/// - **行内展开**(R3 真机轮 #8):文件夹行箭头 = 行内展开(逐级缩进,
///   ``visibleRows`` 树投影);行点击 = 进整页(历史导航不变);一次展开
///   一次拉取(缓存,收起释放),搜索起手收起全部,导航换页重置,选择
///   跨层级(全选/收集含展开子级)。
///
/// 每个仓库调用均经 ``RpcGuard/withTimeout(seconds:_:)`` 限时(T3 缓办
/// 携带:无应答调用不得永久挂起)。宿主可测:纯 MTDSSKit,无 SwiftUI。

import Combine
import Foundation

/// 排序字段(「···」菜单单选;文件夹恒前由比较器统一保证)。
public enum BrowseSort: String, CaseIterable, Identifiable, Sendable {
    case name
    case date
    case size
    case kind

    public var id: String { rawValue }
}

/// 授权目录条目(第一标签「快捷目录」根列表数据源;2026-11-01 Ruling N:
/// 首页唯一展示项 = 收件箱。授权集 = 收件箱(默认)+ 信任目录(白名单),
/// 信任目录只进引擎授权集(engine-roots.conf,对端可浏览+发送授权)与设置
/// 页管理列表,**不出现在首页**)。
public struct AuthorizedRoot: Equatable, Hashable, Sendable, Identifiable {
    public let name: String
    public let path: String
    public let isInbox: Bool

    public init(name: String, path: String, isInbox: Bool) {
        self.name = name
        self.path = path
        self.isInbox = isInbox
    }

    public var id: String { path }

    /// 首页根列表 = 收件箱单枚(recv_dir;status 未含则为空,不编造)。
    /// Ruling N:信任目录不再投影首页(2026-10-31「授权目录列表」合成撤销;
    /// 管理/展示面只在设置 → 信任目录)。
    public static func roots(recvDir: String?) -> [AuthorizedRoot] {
        guard let recvDir, !recvDir.isEmpty else { return [] }
        return [AuthorizedRoot(name: "收件箱", path: recvDir, isInbox: true)]
    }
}

/// 浏览行副文/计数格式化(纯函数;双端口径镜像 android ui/common/Format.kt,
/// 日期段按 spec §4.1 mockup 用 yyyy-MM-dd):
/// - size:0 → "0 B";1024 进制、一位小数不剪尾("2.0 MB"),字节段整数,
///   超出表尾单位(PB)封顶;
/// - subtitle:非空段用「 - 」连接——目录「日期 - N 项」(数未知退化纯日期)、
///   文件「日期 - 大小」。
public enum BrowseFormat {

    private static let units = ["B", "KB", "MB", "GB", "TB", "PB"]

    private static let dateFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd"
        f.locale = Locale(identifier: "en_US_POSIX")
        return f
    }()

    /// mtime → "yyyy-MM-dd"(本地时区);0 → 空(daemon 侧无 mtime 语义)。
    public static func date(_ mtimeMs: Int64) -> String {
        guard mtimeMs != 0 else { return "" }
        return dateFormatter.string(from: Date(timeIntervalSince1970: Double(mtimeMs) / 1000))
    }

    /// 字节数 → 人类可读(android Format.size 同口径;目录 size 恒 0,
    /// 目录行不进此函数)。
    public static func size(_ bytes: Int64) -> String {
        if bytes == 0 { return "0 B" }
        var value = Double(bytes)
        var i = 0
        while i < units.count - 1 && value >= 1024 {
            value /= 1024
            i += 1
        }
        // 字节段恒整数(daemon 的 size 是 u64;<1024 才停在本段)
        return i == 0 ? "\(bytes) \(units[0])" : String(format: "%.1f %@", value, units[i])
    }

    /// 行副文:「日期 - N 项」/「日期 - 大小」;空段剔除(mtime 0 等)。
    public static func subtitle(_ entry: DirEntry, childCount: Int? = nil) -> String {
        var parts: [String] = []
        let day = date(entry.mtimeMs)
        if !day.isEmpty { parts.append(day) }
        if entry.isDir {
            if let childCount { parts.append("\(childCount) 项") }
        } else {
            parts.append(size(entry.size))
        }
        return parts.joined(separator: " - ")
    }

    /// 路径显示名 = 最后一段(正反斜杠都切;远端 Windows 路径同形)。
    public static func displayName(ofPath path: String) -> String {
        let comps = path.split(whereSeparator: { $0 == "/" || $0 == "\\" }).map(String.init)
        return comps.last ?? path
    }
}

/// 左缘右滑返回手势判定(2026-11-02 R2 真机轮;纯几何谓词,宿主可测):
/// 浏览推入页顶栏自绘 + 系统 navigationBar 隐藏(`.toolbar(.hidden, for:
/// .navigationBar)` 令系统 interactivePopGesture 不生效——T13b 已证
/// 「系统返回不可用」),边缘右滑由本谓词 + BrowseView 的 DragGesture
/// (simultaneousGesture,与列表滚动并存)补位:
/// - 起手在左缘带(≤``BrowseGesture/edgeZone``);
/// - 位移水平向右达 ``BrowseGesture/minSwipe`` 且横纵比 2:1 以上——列表
///   纵向滚动与斜向漂移不误触;
/// - 选择态由调用面先行忽略(选择模式下滑动 = 滚动列表,不退选——取
///   「最不意外」口径)。
public enum BrowseGesture {
    /// 左缘起手判定带(pt,自容器左缘起算)。
    public static let edgeZone: CGFloat = 30
    /// 触发所需最小水平位移(pt)。
    public static let minSwipe: CGFloat = 70

    /// 谓词:起手横坐标与位移量 → 是否为「右滑返回」。
    public static func isBack(startX: CGFloat, dx: CGFloat, dy: CGFloat) -> Bool {
        guard startX <= edgeZone, dx >= minSwipe else { return false }
        return dx > abs(dy) * 2
    }
}

/// 行内展开投影行(R3 真机轮 #8):根可见行 + 展开目录子树的扁平序列;
/// ``depth`` = 缩进层级(根级 0,子级 = 父 +1),视图按 depth ×
/// ``BrowseViewModel/indentPerLevel`` 缩进。id = entry.path(树内唯一)。
public struct BrowseRow: Equatable, Sendable, Identifiable {
    public let entry: DirEntry
    public let depth: Int

    public var id: String { entry.path }

    public init(entry: DirEntry, depth: Int) {
        self.entry = entry
        self.depth = depth
    }
}

/// 选择底栏动作集(R9 #3;与行左滑同一远端删除门——两表面单源出集,
/// 宿主可测)。本地 = [分享, 删除](自有文件,不设门);远端 = [下载,
/// 转发](门关)/ [下载, 转发, 删除](门开,R5b #8 的删除自本版起受门)。
public enum BrowseSelectionAction: Equatable, Sendable {
    case share
    case delete
    case download
    case forward
    /// V3-3 T18:纯目录多选追加「快捷」(spec §5.3/§6 添加成员入口——底栏
    /// → FavoritePickerSheet;既有 ``actions`` 签名与出集**零改**,本 case
    /// 只经 ``dirSelectionActions`` 出集)。
    case favorite

    /// 动作集按浏览档 + 远端删除门出集(``BrowseSwipeAction/actions`` 的
    /// 选择态同源面;顺序即底栏自左向右,破坏面恒在簇末)。
    public static func actions(isRemote: Bool, allowRemoteDelete: Bool) -> [BrowseSelectionAction] {
        if isRemote {
            return allowRemoteDelete ? [.download, .forward, .delete] : [.download, .forward]
        }
        return [.share, .delete]
    }

    /// 纯目录多选动作集(V3-3 T18,spec §5.3「多选(纯目录)同项可用,含
    /// 文件或混合选择时禁用」的出集收口 = allDirs 才示「快捷」):=
    /// ``actions`` 出集 + (allDirs ? [.favorite] : []),favorite 恒簇末。
    /// 既有 ``actions`` 签名与出集零改——非 allDirs 输入与旧集逐字同
    /// (零回归锚测试 testDirSelectionActionsAppendsFavoriteOnlyWhenAllDirs)。
    public static func dirSelectionActions(
        isRemote: Bool, allDirs: Bool, allowRemoteDelete: Bool = false,
    ) -> [BrowseSelectionAction] {
        var set = actions(isRemote: isRemote, allowRemoteDelete: allowRemoteDelete)
        if allDirs { set.append(.favorite) }
        return set
    }
}

/// 行左滑动作(R5b 真机轮 #7;上下文式——动作集按浏览档出集;R11 ③ 起
/// **目录行同文件行同集**——用户裁定「文件夹要支持左滑,设备里面的和快捷
/// 目录的」:发送/下载/转发/删除引擎侧目录语义俱全[send/download/forward
/// = collect_items 递归展开整树,delete = remove_dir_all 整树删,均为引擎
/// 既有面,R11 引擎核查在案],文件夹整树 = 单路径入帧,零新契约):
/// 本地行 = [发送, 删除];远端行 = [下载, 转发],门开追加 删除(R9 #3:
/// 删除=破坏面,App 侧设置门默认关;本地删除不设门)。下载直达本机收件
/// 箱,发送/转发进选设备弹层;删除入居中确认(文件夹形文案点明含内容,
/// R11 ③)。动作语义与选择模式同流(同一发送/删除/下载收口,只差条目数)。
/// 收件箱**根行不在此函数面**(根行非 entry 不入 visibleRows,视图侧自绘
/// 行也不挂滑面——结构面,钉测 testHomeRootRowNeverSwipeable)。
public enum BrowseSwipeAction: Equatable, Sendable {
    /// 本地行:发送(选设备弹层 → SendService.send;目录 = collect_items
    /// 整树展开,rel = 目录名/内部相对路径)。
    case send
    /// 本地行:删除(确认后 deletePaths;目录 = remove_dir_all 整树删)。
    case delete
    /// 远端行:下载(直达本机收件箱;目录 = 源端 collect_items 整树)。
    case download
    /// 远端行:转发(选设备弹层 → forward,来源 = 浏览的设备;目录 =
    /// remote_copy from 端 upload 驱动整树)。
    case forward
    /// V3-3 T18:目录行追加「快捷」(spec §5.3/§6 添加成员入口——目录行
    /// 左滑 → FavoritePickerSheet;既有 ``actions`` 签名与出集**零改**,
    /// 本 case 只经 ``dirActions`` 出集)。
    case favorite

    /// 行动集(R11 ③:目录与文件同集,`isRemote` 分浏览档;远端删除受
    /// ``allowRemoteDelete`` 门,缺省关 = 今日集——R9 #3,调用方为本地
    /// 档时传 false 与缺省同义)。
    public static func actions(
        entry: DirEntry, isRemote: Bool, allowRemoteDelete: Bool = false,
    ) -> [BrowseSwipeAction] {
        if isRemote {
            return allowRemoteDelete ? [.download, .forward, .delete] : [.download, .forward]
        }
        return [.send, .delete]
    }

    /// 目录行动作集(V3-3 T18,spec §6「目录行滑删动作集 +『快捷』(仅
    /// 目录行)」):= ``actions`` 出集 + (entry.isDir ? [.favorite] : []),
    /// favorite 恒簇末(原生 swipe reversed 声明 = 贴缘,首个揭示位)。
    /// 既有 ``actions`` 签名与出集零改——文件行/两档非目录输入与旧集逐字
    /// 同(零回归锚测试 testDirActionsAppendsFavoriteOnlyForDirs)。
    public static func dirActions(
        entry: DirEntry, isRemote: Bool, allowRemoteDelete: Bool = false,
    ) -> [BrowseSwipeAction] {
        var set = actions(entry: entry, isRemote: isRemote, allowRemoteDelete: allowRemoteDelete)
        if entry.isDir { set.append(.favorite) }
        return set
    }
}

/// 浏览页视图模型(单实例单目录树;收件箱/信任目录各起一枚,远端 T7 复用)。
@MainActor
public final class BrowseViewModel: ObservableObject {

    /// 单次 list/stat 调用限时(秒)。远端浏览(T7)首拉要建对端连接
    /// (信令 punch/中继握手,秒级起)——本地进程内 UDS 本可短时限,统一
    /// 取握手余量口径(与 SendService.callTimeout 同值);RpcGuard 兜底
    /// 不让等待占住界面。
    static let callTimeout: TimeInterval = 30

    // MARK: 只读状态(UI 订阅)

    /// 当前目录路径(初始即 rootPath,标题先行就位)。
    @Published public private(set) var path: String
    /// daemon 原始列表(未滤;可见面走 ``visibleEntries``)。
    @Published public private(set) var entries: [DirEntry] = []
    /// 目录超出引擎上限(10000 项)被截断。
    @Published public private(set) var truncated = false
    @Published public private(set) var loading = false
    @Published public private(set) var errorText: String?

    // MARK: UI 输入

    /// 搜索框:按名称过滤当前列表(空 = 不过滤)。搜索起手 = 收起全部
    /// 行内展开(R3 #8 裁定;清空不恢复)——无条件收口:即使展开集暂空,
    /// 也要作废在飞子项拉取(R3 评审 I-1:防迟到应答复活展开)。
    @Published public var searchText = "" {
        didSet {
            if searchText != oldValue, !searchText.isEmpty {
                collapseAll()
            }
        }
    }
    /// 排序(默认 名称 升序;文件夹恒前)。
    @Published public var sort: BrowseSort = .name
    /// 隐藏项显示开关(默认过滤 dotfiles;T10 设置页接线)。
    @Published public var showHidden = false

    // MARK: 依赖与历史

    private let repo: MtdssRepository
    /// 本机 device_id(本机浏览;远端浏览 = 对端 id,T7)。
    public let deviceId: String
    /// 浏览树根(收件箱 = recv_dir)。
    public let rootPath: String
    /// 根目录标题(收件箱等;深于根取路径末段)。
    public let rootTitle: String
    /// 对端设备名(R4 真机轮 #5:远端浏览标题 = 「设备名 | 目录名」;本地
    /// 浏览为 nil,标题保持单段口径)。
    public let deviceName: String?

    /// 本地浏览的 device 参数(2026-11-01 T13b 修#4):已登录用真实
    /// device_id(语义与既有本地短路一致);未登录(None)用保留别名
    /// `"self"`(daemon 侧本机短路不要求会话,``Contract/selfDevice``)。
    public static func localBrowseDeviceId(selfDeviceId: String?) -> String {
        selfDeviceId ?? Contract.selfDevice
    }

    /// 浏览器式历史(游标制;open 截断前进分支)。
    private var history: [String]
    private var historyIndex: Int
    private var seq = 0

    public init(
        repo: MtdssRepository,
        deviceId: String,
        rootPath: String,
        rootTitle: String,
        deviceName: String? = nil,
    ) {
        self.repo = repo
        self.deviceId = deviceId
        self.rootPath = rootPath
        self.rootTitle = rootTitle
        self.deviceName = deviceName
        self.history = [rootPath]
        self.historyIndex = 0
        self.path = rootPath
    }

    // MARK: 派生面

    /// 顶栏标题(R4 真机轮 #5 组合口径,见 ``composeTitle``)。
    public var title: String {
        Self.composeTitle(rootTitle: rootTitle, path: path, rootPath: rootPath, deviceName: deviceName)
    }

    /// 标题组合(纯函数,宿主可测):
    /// - 浏览根 = rootTitle 单段(远端 = 设备名,本地 = 收件箱等);
    /// - 深于根:远端(deviceName 在场)= 「设备名 | 目录名」(R4 #5——
    ///   浏览在哪台设备的哪个目录一眼可辨;R5a #5 分隔留白:裸 `|` 两段
    ///   粘连难读,前后各一空格,系统标题栏通用呼吸感);本地(deviceName
    ///   nil)保持原单段口径 = 目录名(收件箱页标题不动)。
    public static func composeTitle(
        rootTitle: String,
        path: String,
        rootPath: String,
        deviceName: String?,
    ) -> String {
        guard path != rootPath else { return rootTitle }
        let folder = BrowseFormat.displayName(ofPath: path)
        guard let deviceName, !deviceName.isEmpty else { return folder }
        return "\(deviceName) | \(folder)"
    }

    /// 可见条目 = 隐藏过滤 → 搜索过滤 → 排序(文件夹恒前)。
    public var visibleEntries: [DirEntry] {
        var list = entries
        if !showHidden {
            list = list.filter { !$0.name.hasPrefix(".") }
        }
        if !searchText.isEmpty {
            list = list.filter { $0.name.localizedCaseInsensitiveContains(searchText) }
        }
        return Self.sorted(list, by: sort)
    }

    /// 行内展开投影 = 根可见行 + 逐层展开子树(每层走同一 隐藏过滤 →
    /// 排序 管线,depth = 父 +1;搜索过滤只在根级——搜索已收起全部展开,
    /// 子级不投影,根级与 ``visibleEntries`` 同口径)。
    public var visibleRows: [BrowseRow] {
        var rows: [BrowseRow] = []
        let search = searchText
        func walk(_ list: [DirEntry], depth: Int) {
            var leveled = Self.levelEntries(list, showHidden: showHidden, sort: sort)
            if depth == 0, !search.isEmpty {
                leveled = leveled.filter { $0.name.localizedCaseInsensitiveContains(search) }
            }
            for entry in leveled {
                rows.append(BrowseRow(entry: entry, depth: depth))
                if entry.isDir, search.isEmpty, expandedPaths.contains(entry.path) {
                    walk(childrenCache[entry.path] ?? [], depth: depth + 1)
                }
            }
        }
        walk(entries, depth: 0)
        return rows
    }

    /// 单层可见化(展开子级同管线;搜索只在根级滤——子级仅在无搜索时投影)。
    private static func levelEntries(_ list: [DirEntry], showHidden: Bool, sort: BrowseSort) -> [DirEntry] {
        var l = list
        if !showHidden {
            l = l.filter { !$0.name.hasPrefix(".") }
        }
        return sorted(l, by: sort)
    }

    /// 底部计数「N 项」(可见面)。
    public var itemCount: Int { visibleEntries.count }

    public var canGoBack: Bool { historyIndex > 0 }
    public var canGoForward: Bool { historyIndex + 1 < history.count }

    // MARK: 选择模式(spec §4.2;Ruling O 2026-11-02)

    /// 选择模式开关(进入时顶栏换 全选/N 项/蓝圈确认,标签栏隐藏,底部动作条)。
    @Published public private(set) var isSelecting = false
    /// 选中路径集(行渲染选中态;键 = entry.path)。
    @Published public private(set) var selection: Set<String> = []

    /// 进入选择模式(···菜单「选择」/ 行长按两入口同落此)。
    public func enterSelection() {
        isSelecting = true
        selection = []
    }

    /// 退出选择模式(取消/完成后的收尾;选择集清空)。
    public func exitSelection() {
        isSelecting = false
        selection = []
    }

    /// 行点按切换选中态(选择模式下点按不再打开目录)。
    public func toggle(_ path: String) {
        if selection.contains(path) {
            selection.remove(path)
        } else {
            selection.insert(path)
        }
    }

    /// 全选(可见行集——含展开子级,R3 #8 选择跨层级);已全选再点 = 清空
    /// (iOS 文件 App 同惯用)。
    public func selectAll() {
        let all = visibleRows.map { $0.entry.path }
        if all.allSatisfy(selection.contains) && !all.isEmpty {
            selection.removeAll()
        } else {
            selection = Set(all)
        }
    }

    /// 选中条目(可见行序——含展开子级;取消勾选/已不可见项不收)。
    public var selectedEntries: [DirEntry] {
        visibleRows.map(\.entry).filter { selection.contains($0.path) }
    }

    /// 蓝圈确认(Ruling O 2026-11-02 真机轮 #2):**只 = 退出选择模式**,
    /// 无论选中与否、无副作用——分享/删除/下载/转发全部仅自底栏动作发起
    /// (旧 T6「确认携带选择集进选设备弹层」废止)。底栏动作的次序 =
    /// 先读 ``selectedEntries`` 捕获选择集,再调本口退出(退出即清集)。
    public func confirmSelection() {
        exitSelection()
    }

    /// 删除选中项(本地/远端动作;iOS 恒永久删,UI 侧先行确认)。成功后
    /// 原地重载并退出选择模式;返回删除结果供 UI 呈现失败明细。
    public func deleteSelected() async -> DeleteResult? {
        let paths = selectedEntries.map(\.path)
        guard !paths.isEmpty else { return nil }
        guard let result = await deletePaths(paths) else { return nil } // 失败保持选择,错误走 lastActionError
        exitSelection()
        await reload()
        return result
    }

    /// 删除给定路径集(R5b 真机轮 #7 行滑删;device = VM 浏览目标——本机
    /// 浏览 = 本机 id,远端浏览 = 对端 id,同一 ctrl delete
    /// {device,paths,force} 面,#8 远端选择删除同走此口)。不退选不重载
    /// (调用方定收口);失败置 ``lastActionError`` 返回 nil。
    public func deletePaths(_ paths: [String]) async -> DeleteResult? {
        guard !paths.isEmpty else { return nil }
        do {
            return try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo, deviceId] in
                try await repo.delete(device: deviceId, paths: paths, force: true)
            }
        } catch {
            // 删除失败错误由 UI 以 ``lastActionError`` 呈现
            lastActionError = Self.userErrorText(for: error)
            return nil
        }
    }

    /// 最近一次动作失败文案(删除/发送等;UI 弹提示后置 nil 清除)。
    @Published public var lastActionError: String?

    // MARK: 导航

    /// 打开目录(导航入口;历史截断前进分支后追加;标题先行——android
    /// open 同象,失败/迟到应答由 seq 收口)。换页 = 展开态重置(R3 #8:
    /// 行内展开是页级状态)+ 搜索清空(R5a #6:搜索只属当前层,跨级残留
    /// 会让新层看起来「缺条目」)+ **列表即刻清空进 loading**(R12 ②,
    /// 用户报告「标题先变内容后变,无反馈;返回同」:旧目录条目不得在
    /// 拉取窗内残留——远端冷握手 25-35s,清空 + 转圈 = 即时反馈;原地
    /// reload 不在此列,下拉刷新保内容语义不变)。
    public func open(_ target: String) async {
        seq += 1
        let mySeq = seq
        searchText = ""
        collapseAll()
        if target != history[historyIndex] {
            if historyIndex + 1 < history.count {
                history.removeSubrange((historyIndex + 1)...)
            }
            history.append(target)
            historyIndex = history.count - 1
        }
        path = target
        clearListForNavigation()
        await fetch(target, seq: mySeq)
    }

    /// 初次加载浏览根(收件箱等)。
    public func loadRoot() async {
        await open(rootPath)
    }

    /// 返回(游标左移;已在浏览根为空操作——弹回标签根由 UI 层承担)。
    /// 列表即刻清空进 loading(R12 ②,同 ``open``)。
    public func goBack() async {
        guard historyIndex > 0 else { return }
        seq += 1
        searchText = ""
        collapseAll()
        historyIndex -= 1
        path = history[historyIndex]
        clearListForNavigation()
        await fetch(path, seq: seq)
    }

    /// 前进(游标右移;返回后可再进入)。列表即刻清空进 loading(R12 ②)。
    public func goForward() async {
        guard historyIndex + 1 < history.count else { return }
        seq += 1
        searchText = ""
        collapseAll()
        historyIndex += 1
        path = history[historyIndex]
        clearListForNavigation()
        await fetch(path, seq: seq)
    }

    /// 原地重载当前目录(错误重试;不动历史)。已展开子级缓存随后刷新
    /// (删除等原地重载不收起展开态,过期子项不残留)。**不清列表**
    /// (R12 ②:reload = 原地刷新语义,内容在应答窗内保留——下拉刷新/
    /// 「···」刷新/删除后重载全走此口;清列表只属导航三口)。
    public func reload() async {
        seq += 1
        await fetch(history[historyIndex], seq: seq)
        if errorText == nil {
            await refreshExpandedChildren()
        }
    }

    // MARK: 行内展开(R3 真机轮 #8:箭头=行内展开;行点击=进整页)

    /// 缩进步长(pt/层级;系统 outline 手感 16-20 取 18)。
    public static let indentPerLevel: CGFloat = 18

    /// 展开目录集(键 = 绝对路径;页级状态,导航即重置)。
    @Published public private(set) var expandedPaths: Set<String> = []
    /// 子项缓存(键 = 绝对路径;一次展开一次拉取,页生命周期内复用,
    /// 收起释放;搜索收起/导航重置一并清)。
    private var childrenCache: [String: [DirEntry]] = [:]
    /// 子项拉取在飞(防重复)。
    private var loadingChildren: Set<String> = []
    /// 展开/收起世代(在飞子项拉取的作废面):任何「展开语义重置」
    /// (搜索收起/导航重置)都 +1,迟到应答整包丢弃(R3 评审 I-1;
    /// 与导航 seq 同类守卫,seq 只盖导航)。
    private var expandGeneration = 0

    /// 展开/收起目录行。展开 = list 拉子项一次并缓存(已有缓存即时展开);
    /// 收起 = 出展开集并释放缓存。拉取失败 = 不展开(行仍在,可再点;
    /// 错误不打断浏览主面)。在飞拉取遇导航(seq)/收起(世代)即整包丢弃
    /// ——迟到的应答不得写缓存/展开(R3 评审 I-1)。
    public func toggleExpand(_ path: String) async {
        if expandedPaths.contains(path) {
            expandedPaths.remove(path)
            childrenCache[path] = nil
            return
        }
        if childrenCache[path] != nil {
            expandedPaths.insert(path)
            return
        }
        guard !loadingChildren.contains(path) else { return } // 在飞:本次点按吞掉
        loadingChildren.insert(path)
        defer { loadingChildren.remove(path) }
        let mySeq = seq
        let myGen = expandGeneration
        do {
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo, deviceId] in
                try await repo.list(device: deviceId, path: path)
            }
            guard mySeq == seq, myGen == expandGeneration else { return } // 拉取期间已导航/收起:整包丢弃
            childrenCache[path] = result.entries
            expandedPaths.insert(path)
        } catch {
            guard mySeq == seq, myGen == expandGeneration else { return }
            expandedPaths.remove(path)
        }
    }

    /// 收起全部展开(搜索起手/导航换页同一收口;缓存一并释放)。
    public func collapseAll() {
        // 世代 +1:任何「展开语义重置」都令在飞子项拉取作废(R3 评审 I-1:
        // 仅导航 seq 不够——搜索收起不 bump seq,迟到应答会写缓存复活展开)
        expandGeneration += 1
        expandedPaths = []
        childrenCache = [:]
    }

    /// 重载后刷新已展开目录的子项缓存(逐个现拉;失败该级收起)。导航
    /// (seq)/收起(世代)发生后,在飞刷新整包丢弃(孤儿缓存不写)。
    private func refreshExpandedChildren() async {
        let mySeq = seq
        let myGen = expandGeneration
        for path in expandedPaths.sorted() {
            guard childrenCache[path] != nil, !loadingChildren.contains(path) else { continue }
            do {
                let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo, deviceId] in
                    try await repo.list(device: deviceId, path: path)
                }
                guard mySeq == seq, myGen == expandGeneration else { break }
                childrenCache[path] = result.entries
            } catch {
                guard mySeq == seq, myGen == expandGeneration else { break }
                childrenCache[path] = nil
                expandedPaths.remove(path)
            }
        }
    }

    // MARK: 内部

    /// 导航换页的列表即刻清空(R12 ②):entries/truncated 复位 → 视图
    /// `loading && entries.isEmpty` 分支(转圈)即时接管,「N 项」尾注随
    /// 空列表隐没;错误面不受影响(fetch 失败仍走既有 errorText 收口)。
    /// 只属导航三口(open/goBack/goForward);``reload`` 不调此函数
    /// (原地刷新保内容)。
    private func clearListForNavigation() {
        entries = []
        truncated = false
    }

    /// 拉 listing(限时;seq 不符的迟到应答整包丢弃)。
    private func fetch(_ target: String, seq mySeq: Int) async {
        loading = true
        errorText = nil
        let repo = self.repo
        let deviceId = self.deviceId
        do {
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) {
                try await repo.list(device: deviceId, path: target)
            }
            guard mySeq == seq else { return }
            path = target
            entries = result.entries
            truncated = result.truncated
            loading = false
        } catch {
            guard mySeq == seq else { return }
            // R5a 真机轮 #4:取消 = 良性收口,不是连接故障。refreshable 的
            // 任务所有权在 SwiftUI(视图重建/用户离场即中途取消),取消曾
            // 落到兜底文案伪装成「请求失败:连接中断」并清空既有内容——
            // 现按取消还原 loading,列表与错误面原地不动。
            if error is CancellationError {
                loading = false
                return
            }
            entries = []
            truncated = false
            loading = false
            errorText = Self.userErrorText(for: error)
        }
    }

    /// 错误用户文案(daemon 消息本就面向用户;传输/超时给常识文案)。
    public static func userErrorText(for error: Error) -> String {
        if let rpc = error as? RpcError { return rpc.message }
        if error is RpcGuard.TimeoutError { return "请求超时" }
        return "请求失败:连接中断"
    }

    /// 单条比较器(T15 抽出供混合视图 MixedViewModel 逐行排序同源共用;
    /// ``sorted`` 行为逐字不变):文件夹恒前;字段内主键、名称兜底。
    static func less(_ a: DirEntry, _ b: DirEntry, by sort: BrowseSort) -> Bool {
        if a.isDir != b.isDir { return a.isDir }
        switch sort {
        case .name:
            return nameLess(a.name, b.name)
        case .date:
            if a.mtimeMs != b.mtimeMs { return a.mtimeMs > b.mtimeMs } // 最新在前
            return nameLess(a.name, b.name)
        case .size:
            if a.size != b.size { return a.size > b.size } // 大者在前
            return nameLess(a.name, b.name)
        case .kind:
            let ka = kindKey(a), kb = kindKey(b)
            if ka != kb { return ka < kb }
            return nameLess(a.name, b.name)
        }
    }

    /// 统一排序(文件夹恒前;字段内主键、名称兜底)。R10 起公开:App 侧
    /// 快捷树成员子目录行(V3-3 返工 §13.8)同源复用——排序管线不出 Kit
    /// 单源(与 ``less`` 同一面,既有测试锚行为逐字不变)。
    public static func sorted(_ items: [DirEntry], by sort: BrowseSort) -> [DirEntry] {
        items.sorted { less($0, $1, by: sort) }
    }

    /// 名称兜底比较:本地化大小写不敏感(与系统文件 App 口径一致),同形
    /// 按字节序收口(确定性)。
    private static func nameLess(_ a: String, _ b: String) -> Bool {
        let r = a.compare(b, options: [.caseInsensitive])
        return r == .orderedAscending
    }

    /// 种类键:目录 ""(恒前),文件取小写扩展名(无扩展名 "" 殿后于目录、
    /// 先于有名扩展,确定性成立)。
    private static func kindKey(_ e: DirEntry) -> String {
        guard !e.isDir else { return "" }
        let comps = e.name.split(separator: ".")
        return comps.count > 1 ? comps.last!.lowercased() : ""
    }
}
