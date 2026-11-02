/// 第一标签「常用目录」根页(2026-11-01 Ruling N:唯一展示项 = 收件箱。
/// 授权集 = 收件箱(默认)+ 信任目录(白名单)——信任目录只进引擎授权集
/// (engine-roots.conf)与设置页管理列表,不投影首页)。数据源 =
/// AuthorizedRoot.roots(recvDir:)。根页无返回/前进箭头,仅三点
/// (spec §4.1);点根条目推入浏览页(4.1 模板)。
/// **根行行内展开**(R4 真机轮 #1):箭头 = 行内展开子级(缩进逐级、懒
/// 加载、同 VM 机制),行点按 = 进整页浏览;子行点目录 = 推入该子目录的
/// 整页浏览(带 initialPath)。

import MTDSSKit
import SwiftUI

struct FavoritesView: View {
    @Environment(AppModel.self) private var model
    /// 值路由(R4 #1 起):根页推入 = 整页浏览根(.root)或子目录(.sub,
    /// 子行点按进整页)。
    @State private var navPath: [FavoritesRoute] = []

    /// 首页根列表(Ruling N:收件箱单枚;信任目录不入首页)。
    private var roots: [AuthorizedRoot] {
        AuthorizedRoot.roots(recvDir: model.recvDir)
    }

    var body: some View {
        NavigationStack(path: $navPath) {
            rootPage
                .navigationDestination(for: FavoritesRoute.self) { route in
                    destination(route)
                }
        }
        .task {
            await model.refreshStatus()
            await pushInboxIfDebugHooked()
        }
    }

    // MARK: 根页

    private var rootPage: some View {
        VStack(spacing: 0) {
            BrowseNavBar(title: "常用目录", showsArrows: false) {
                overflowMenu
            }
            rootList
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
    }

    @ViewBuilder private var rootList: some View {
        if let bootError = model.bootError {
            ContentUnavailableView {
                Label("引擎未就绪", systemImage: "cpu")
            } description: {
                Text(bootError)
            }
        } else if roots.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else if let repo = model.repository {
            // 首页列表(R4 #1:根行可展开;VM = BrowseViewModel 复用,见下)
            FavoritesRootList(
                repo: repo,
                selfDeviceId: model.selfDeviceId,
                inbox: roots[0],
                showHidden: model.showHidden,
                onOpenRoot: { navPath = [.root($0)] },
                onOpenPath: { navPath = [.root(roots[0]), .sub(roots[0], path: $0)] },
            )
        } else {
            // 引擎重启窗口(repository 短暂缺席,信任目录变更面):转圈兜底
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        }
    }

    @ViewBuilder private var overflowMenu: some View {
        // 根页 = 收件箱单枚(非文件浏览),无选择面;排序无意义但保留形(T5)
        Picker("排序", selection: .constant(BrowseSort.name)) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
        .disabled(true)
    }

    // MARK: 推入页(浏览模板)

    @ViewBuilder private func destination(_ route: FavoritesRoute) -> some View {
        if let repo = model.repository {
            // 本地目录浏览不设登录门(T13b 修#4:daemon 本机短路不要求
            // 会话;未登录 device 参数走保留别名 "self")。
            BrowseDestination(
                root: route.root,
                repo: repo,
                deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: model.selfDeviceId),
                initialPath: route.initialPath,
                onPopToRoot: { navPath = [] },
            )
        } else {
            // 引擎 boot 未就绪(repository 缺;bootError 横幅由 AppTabView
            // 顶层承担):给返回路的占位。
            VStack(spacing: 0) {
                BrowseNavBar(
                    title: route.root.name,
                    showsArrows: true,
                    canBack: false,
                    canForward: false,
                    onBack: { navPath = [] },
                    menu: { overflowMenu },
                )
                ContentUnavailableView(
                    "引擎未就绪",
                    systemImage: "cpu",
                    description: Text("引擎启动完成后即可浏览\(route.root.name)"),
                )
            }
            .background(Color(uiColor: .systemBackground))
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;截图用,真机/产线不编译)

    private func pushInboxIfDebugHooked() async {
        #if DEBUG
        guard ProcessInfo.processInfo.environment["MTDSS_DEBUG_AUTOPUSH"] == "inbox" else { return }
        // 等 status 投影就绪(recv_dir 随引擎 boot/status 异步到达;5s 上界)
        for _ in 0..<50 {
            if let inbox = roots.first(where: \.isInbox) {
                if navPath.isEmpty { navPath = [.root(inbox)] }
                return
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        #endif
    }
}

/// 首页导航目标:根页推入浏览根,或子行点按直入子目录整页(R4 #1)。
enum FavoritesRoute: Hashable {
    case root(AuthorizedRoot)
    case sub(AuthorizedRoot, path: String)

    var root: AuthorizedRoot {
        switch self {
        case .root(let root): root
        case .sub(let root, _): root
        }
    }

    /// 子目录整页直入路径(.root = nil,开页走 loadRoot)。
    var initialPath: String? {
        switch self {
        case .root: nil
        case .sub(_, let path): path
        }
    }
}

/// 浏览推入页:每根一枚 VM(StateObject 随页生命周期);showHidden 以
/// 设置开关为源(App 侧,T10),开页取值 + 开关变化活体跟随。
/// initialPath(R4 #1)= 子目录直入:开页 open 子目录(历史 = [根, 子],
/// 返回/前进同浏览页),home 子行点按与普通浏览点行进整页同语义。
private struct BrowseDestination: View {
    let root: AuthorizedRoot
    let repo: MtdssRepository
    let deviceId: String
    var initialPath: String? = nil
    let onPopToRoot: () -> Void

    @Environment(AppModel.self) private var model
    @StateObject private var vm: BrowseViewModel

    init(root: AuthorizedRoot, repo: MtdssRepository, deviceId: String, initialPath: String? = nil, onPopToRoot: @escaping () -> Void) {
        self.root = root
        self.repo = repo
        self.deviceId = deviceId
        self.initialPath = initialPath
        self.onPopToRoot = onPopToRoot
        _vm = StateObject(wrappedValue: BrowseViewModel(
            repo: repo,
            deviceId: deviceId,
            rootPath: root.path,
            rootTitle: root.name,
        ))
    }

    var body: some View {
        BrowseView(
            vm: vm,
            showsArrows: true,
            onPopToRoot: onPopToRoot,
            openOnAppear: initialPath,
            sendService: SendService(repo: repo),
        )
        .toolbar(.hidden, for: .navigationBar)
        .task { vm.showHidden = model.showHidden }
        .onChange(of: model.showHidden) { _, hidden in
            vm.showHidden = hidden
        }
    }
}

/// 首页根列表(R4 真机轮 #1:根行同浏览行的行内展开语义——箭头 = 展开
/// 子级行内呈现,行点按 = 进整页浏览)。
///
/// **复用裁定(记录)**:不另起 FavoritesViewModel——首页 VM 就是
/// ``BrowseViewModel``(浏览根 = 收件箱路径,根列表 = ``entries``,展开
/// 机制全量单源:懒加载缓存/世代守卫/缩进/排序管线与浏览页同一实现);
/// 根行本体不是 entry,其展开态 = 页面侧一个 bool——数据早已随根拉取
/// 就位(进页 ``loadRoot()`` 一次),展开/收起 = 状态翻转零拉取;收起走
/// ``collapseAll()``(世代 +1 作废在飞子项拉取、子树缓存释放——R3 评审
/// I-1 同款收口,收起不得留迟到应答复活面)。
private struct FavoritesRootList: View {
    let repo: MtdssRepository
    let selfDeviceId: String?
    let inbox: AuthorizedRoot
    var showHidden: Bool
    let onOpenRoot: (AuthorizedRoot) -> Void
    /// 子目录行点按 = 推入该子目录整页(R4 #1;文件行无点按面,同浏览页)。
    let onOpenPath: (String) -> Void

    @StateObject private var vm: BrowseViewModel
    @State private var inboxExpanded = false

    init(
        repo: MtdssRepository,
        selfDeviceId: String?,
        inbox: AuthorizedRoot,
        showHidden: Bool,
        onOpenRoot: @escaping (AuthorizedRoot) -> Void,
        onOpenPath: @escaping (String) -> Void,
    ) {
        self.repo = repo
        self.selfDeviceId = selfDeviceId
        self.inbox = inbox
        self.showHidden = showHidden
        self.onOpenRoot = onOpenRoot
        self.onOpenPath = onOpenPath
        _vm = StateObject(wrappedValue: BrowseViewModel(
            repo: repo,
            deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: selfDeviceId),
            rootPath: inbox.path,
            rootTitle: inbox.name,
        ))
    }

    var body: some View {
        SinkingFooterList {
            inboxRow
            if inboxExpanded {
                childrenArea
            }
        } footer: {
            // 首页计数 = 根数(Ruling N:收件箱单枚;展开子级不计数——与
            // 浏览页「N 项 = 当前目录计数」同口径)
            ItemCountFooter(count: 1)
        }
        // 根行展开/收起与子级行内展开各驱动各的插拔/旋转动画(值 scoped,
        // 与 BrowseView 同拍 0.2s)
        .animation(.easeInOut(duration: 0.2), value: inboxExpanded)
        .animation(.easeInOut(duration: 0.2), value: vm.expandedPaths)
        .task {
            // 根列表(收件箱直接子项)进页即拉一次:根行展开零等待(子级的
            // 子级仍走 toggleExpand 懒加载);失败不阻塞首页(展开后行内重试)
            if vm.entries.isEmpty && vm.errorText == nil && !vm.loading {
                await vm.loadRoot()
            }
        }
        .task { vm.showHidden = showHidden }
        .onChange(of: showHidden) { _, hidden in
            vm.showHidden = hidden
        }
        #if DEBUG
        .task { await debugExpandIfHooked() }
        #endif
    }

    // MARK: 根行(收件箱:箭头 = 展开,行点按 = 进整页)

    private var inboxRow: some View {
        Button {
            onOpenRoot(inbox)
        } label: {
            FullBleedRow {
                HStack(spacing: 12) {
                    // 箭头 = 行内展开钮(R4 #1):› 展开态旋转 90° 成 ∨;
                    // 触区外扩 28pt 方 + 负 padding 还原占位(BrowseEntryRow
                    // 同手法,视觉像素不变)
                    Button {
                        toggleInboxExpand()
                    } label: {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 12)
                            .rotationEffect(.degrees(inboxExpanded ? 90 : 0))
                            .padding(8)
                            .contentShape(Rectangle())
                            .padding(.horizontal, -8)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(Text(inboxExpanded ? "收起" : "展开"))
                    RootFolderIcon()
                    Text(inbox.name)
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
            // 整行触区(R3 真机轮 #1/#5:根行同浏览行,透明区可点)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
    }

    /// 根行箭头点按(R4 #1):数据早已随根拉取就位,展开/收起 = 状态翻转
    /// 零拉取;收起走 ``collapseAll()``(见类型注释)。
    private func toggleInboxExpand() {
        if inboxExpanded {
            vm.collapseAll()
            inboxExpanded = false
        } else {
            inboxExpanded = true
        }
    }

    // MARK: 展开子级(depth = 行序 +1,根行占 depth 0;BrowseEntryRow 复用)

    @ViewBuilder private var childrenArea: some View {
        if let error = vm.errorText, vm.entries.isEmpty {
            // 根拉取失败:行内错误行(重试),不打断首页主面
            HStack(spacing: 8) {
                Text("无法加载目录")
                    .font(.system(size: 13))
                    .foregroundStyle(Color(uiColor: .systemGray))
                Spacer(minLength: 0)
                Button("重试") { Task { await vm.reload() } }
                    .font(.system(size: 13))
            }
            .padding(.leading, 20 + BrowseViewModel.indentPerLevel)
            .padding(.trailing, 20)
            .frame(height: 44)
        } else if vm.loading && vm.entries.isEmpty {
            HStack {
                ProgressView()
                Spacer(minLength: 0)
            }
            .padding(.leading, 20 + BrowseViewModel.indentPerLevel)
            .padding(.trailing, 20)
            .frame(height: 44)
        } else {
            ForEach(vm.visibleRows) { row in
                BrowseEntryRow(
                    entry: row.entry,
                    depth: row.depth + 1,
                    expanded: vm.expandedPaths.contains(row.entry.path),
                    selecting: false, // 首页无选择面(R3 裁定:根页非文件浏览)
                    selected: false,
                    onOpen: {
                        // 行点按 = 进整页(目录;文件行无点按面,同浏览页)
                        if row.entry.isDir {
                            onOpenPath(row.entry.path)
                        }
                    },
                    onToggleExpand: row.entry.isDir
                        ? { Task { await vm.toggleExpand(row.entry.path) } }
                        : nil,
                )
            }
        }
    }

    // MARK: DEBUG 截图钩子(模拟器无触点驱动;真机/产线不编译)

    #if DEBUG
    /// MTDSS_DEBUG_FAVTREE=1:展开收件箱根行 + 其首个子目录(两级行内
    /// 展开截图面)。
    private func debugExpandIfHooked() async {
        guard ProcessInfo.processInfo.environment["MTDSS_DEBUG_FAVTREE"] == "1" else { return }
        for _ in 0..<100 where vm.entries.isEmpty && vm.errorText == nil && !vm.loading {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard !vm.entries.isEmpty else {
            print("[R4-E2E] FAVTREE FAIL: 收件箱列表空(=\(vm.errorText ?? "nil"))")
            return
        }
        toggleInboxExpand()
        if let child = vm.visibleRows.first(where: { $0.entry.isDir }) {
            await vm.toggleExpand(child.entry.path)
        }
    }
    #endif
}
