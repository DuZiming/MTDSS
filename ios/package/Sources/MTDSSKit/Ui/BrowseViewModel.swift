/// BrowseViewModel — 浏览页视图模型(spec §4.1 浏览页模板:收件箱/远端浏览
/// 共用;镜像 android BrowseViewModel 的 seq 守卫语义,补 T5 面的排序/搜索/
/// 隐藏过滤与浏览器式历史):
/// - **seq 守卫**:快速连续导航时过期响应丢弃(seq 单调,旧响应不得覆盖新
///   位置;android 同款);
/// - **浏览器式历史**:返回/前进双箭头(spec §4.1 顶栏);open 截断前进
///   分支追加历史,back/forward 只移动游标;
/// - 排序:文件夹恒前(所有字段),默认 名称 升序;日期 最新在前、大小
///   大者在前、种类 按扩展名(对齐 iOS 文件 App 惯例);
/// - 隐藏项(dotfiles)默认过滤——「显示隐藏项」开关归 T10 设置,VM 只挂
///   ``showHidden`` 输入;搜索框按名称过滤当前列表;
/// - 副文:「日期 - N 项」(目录)/「日期 - 大小」(文件);list 契约
///   children 子项数已接(v2-P5 T5b,目录项携直接子项数;文件项/stat 单查
///   缺省 → 纯日期)。
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

/// 授权目录条目(第一标签「常用目录」根列表数据源;2026-11-01 Ruling N:
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

    /// 搜索框:按名称过滤当前列表(空 = 不过滤)。
    @Published public var searchText = ""
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

    public init(repo: MtdssRepository, deviceId: String, rootPath: String, rootTitle: String) {
        self.repo = repo
        self.deviceId = deviceId
        self.rootPath = rootPath
        self.rootTitle = rootTitle
        self.history = [rootPath]
        self.historyIndex = 0
        self.path = rootPath
    }

    // MARK: 派生面

    /// 顶栏标题:根 = rootTitle(收件箱),深于根 = 路径末段。
    public var title: String {
        path == rootPath ? rootTitle : BrowseFormat.displayName(ofPath: path)
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

    /// 全选(可见集);已全选再点 = 清空(iOS 文件 App 同惯用)。
    public func selectAll() {
        let all = visibleEntries.map(\.path)
        if all.allSatisfy(selection.contains) && !all.isEmpty {
            selection.removeAll()
        } else {
            selection = Set(all)
        }
    }

    /// 选中条目(可见列表顺序;取消勾选/已不可见项不收)。
    public var selectedEntries: [DirEntry] {
        visibleEntries.filter { selection.contains($0.path) }
    }

    /// 蓝圈确认(Ruling O 2026-11-02 真机轮 #2):**只 = 退出选择模式**,
    /// 无论选中与否、无副作用——分享/删除/下载/转发全部仅自底栏动作发起
    /// (旧 T6「确认携带选择集进选设备弹层」废止)。底栏动作的次序 =
    /// 先读 ``selectedEntries`` 捕获选择集,再调本口退出(退出即清集)。
    public func confirmSelection() {
        exitSelection()
    }

    /// 删除选中项(本地动作;iOS 恒永久删,UI 侧先行确认)。成功后原地重载
    /// 并退出选择模式;返回删除结果供 UI 呈现失败明细。
    public func deleteSelected() async -> DeleteResult? {
        let paths = selectedEntries.map(\.path)
        guard !paths.isEmpty else { return nil }
        do {
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo, deviceId] in
                try await repo.delete(device: deviceId, paths: paths, force: true)
            }
            exitSelection()
            await reload()
            return result
        } catch {
            // 删除失败保持选择模式,错误由 UI 以 ``lastActionError`` 呈现
            lastActionError = Self.userErrorText(for: error)
            return nil
        }
    }

    /// 最近一次动作失败文案(删除/发送等;UI 弹提示后置 nil 清除)。
    @Published public var lastActionError: String?

    // MARK: 导航

    /// 打开目录(导航入口;历史截断前进分支后追加;标题先行——android
    /// open 同象,失败/迟到应答由 seq 收口)。
    public func open(_ target: String) async {
        seq += 1
        let mySeq = seq
        if target != history[historyIndex] {
            if historyIndex + 1 < history.count {
                history.removeSubrange((historyIndex + 1)...)
            }
            history.append(target)
            historyIndex = history.count - 1
        }
        path = target
        await fetch(target, seq: mySeq)
    }

    /// 初次加载浏览根(收件箱等)。
    public func loadRoot() async {
        await open(rootPath)
    }

    /// 返回(游标左移;已在浏览根为空操作——弹回标签根由 UI 层承担)。
    public func goBack() async {
        guard historyIndex > 0 else { return }
        seq += 1
        historyIndex -= 1
        path = history[historyIndex]
        await fetch(path, seq: seq)
    }

    /// 前进(游标右移;返回后可再进入)。
    public func goForward() async {
        guard historyIndex + 1 < history.count else { return }
        seq += 1
        historyIndex += 1
        path = history[historyIndex]
        await fetch(path, seq: seq)
    }

    /// 原地重载当前目录(错误重试;不动历史)。
    public func reload() async {
        seq += 1
        await fetch(history[historyIndex], seq: seq)
    }

    // MARK: 内部

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

    /// 统一比较器:文件夹恒前;字段内主键、名称兜底。
    static func sorted(_ items: [DirEntry], by sort: BrowseSort) -> [DirEntry] {
        items.sorted { a, b in
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
