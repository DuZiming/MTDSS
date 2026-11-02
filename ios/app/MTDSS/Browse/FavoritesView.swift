/// 第一标签「常用目录」根页(2026-11-01 Ruling N:唯一展示项 = 收件箱。
/// 授权集 = 收件箱(默认)+ 信任目录(白名单)——信任目录只进引擎授权集
/// (engine-roots.conf)与设置页管理列表,不投影首页)。数据源 =
/// AuthorizedRoot.roots(recvDir:)。根页无返回/前进箭头,仅三点
/// (spec §4.1);点根条目推入浏览页(4.1 模板)。
/// **根行行内展开**(R4 真机轮 #1):箭头 = 行内展开子级(缩进逐级、懒
/// 加载、同 VM 机制),行点按 = 进整页浏览;子行点目录 = 推入该子目录的
/// 整页浏览(带 initialPath)。
/// **选择模式**(R5b #1,spec §4.2 同一骨架):「···」菜单「选择」或
/// 长按任意子行(depth≥1)进入——顶栏换 全选/「N 项」/确认(Ruling O
/// 确认只退出),子行首选圈,**根行(收件箱)不可选(无圈)**,标签栏
/// 隐藏,底部动作条 = 分享/删除;确认/动作语义与浏览页同一 VM 面
/// (``BrowseViewModel`` isSelecting/selectedEntries 复用,零重复)。
/// **照片图库第二固定行**(R5c 方案 A:照片图库 = 收件箱并列的特殊来源,
/// 非信任目录/文件系统目录)——收件箱行下恒显「照片图库」行,点按推入
/// 照片图库页(权限门→相册→网格,``PhotoLibraryView``);选择模式同根行
/// 口径不可选(固定容器行非条目,点按无操作)。

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

    @ViewBuilder private var rootPage: some View {
        if let bootError = model.bootError {
            // 引擎未就绪:占位页(无选择面;菜单空——boot 前无可操作项)
            VStack(spacing: 0) {
                BrowseNavBar(title: "常用目录", showsArrows: false) { EmptyView() }
                ContentUnavailableView {
                    Label("引擎未就绪", systemImage: "cpu")
                } description: {
                    Text(bootError)
                }
            }
            .background(Color(uiColor: .systemBackground))
            .toolbar(.hidden, for: .navigationBar)
        } else if roots.isEmpty || model.repository == nil {
            // status 未投影 / 引擎重启窗口:转圈兜底(信任目录变更面)
            VStack(spacing: 0) {
                BrowseNavBar(title: "常用目录", showsArrows: false) { EmptyView() }
                VStack(spacing: 8) {
                    Spacer()
                    ProgressView()
                    Spacer()
                }
            }
            .background(Color(uiColor: .systemBackground))
            .toolbar(.hidden, for: .navigationBar)
        } else if let repo = model.repository {
            // 首页列表(R4 #1:根行可展开;R5b #1:选择模式。VM =
            // BrowseViewModel 复用,顶栏/选择顶栏随 VM 态在列表组件内收口;
            // R5c:照片图库第二固定行推入 PhotoLibraryView)
            FavoritesRootList(
                repo: repo,
                selfDeviceId: model.selfDeviceId,
                inbox: roots[0],
                showHidden: model.showHidden,
                onOpenRoot: { navPath = [.root($0)] },
                onOpenPath: { navPath = [.sub(roots[0], path: $0)] },
                onOpenPhotoLibrary: { navPath = [.photoLibrary] },
            )
        }
    }

    // MARK: 推入页(浏览模板)

    @ViewBuilder private func destination(_ route: FavoritesRoute) -> some View {
        if let repo = model.repository {
            if route.isPhotoLibrary {
                // 照片图库(R5c 方案 A;权限门/相册/网格页内自持)
                PhotoLibraryView(repo: repo)
            } else {
                // 本地目录浏览不设登录门(T13b 修#4:daemon 本机短路不要求
                // 会话;未登录 device 参数走保留别名 "self")。
                BrowseDestination(
                    root: route.root,
                    repo: repo,
                    deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: model.selfDeviceId),
                    initialPath: route.initialPath,
                    onPopToRoot: { navPath = [] },
                )
            }
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
                    menu: { EmptyView() },
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
        // MTDSS_DEBUG_PHOTO=1:推入照片图库页(R5c 截图面;相册就绪由
        // PhotoLibraryView 内层钩子接管)
        if ProcessInfo.processInfo.environment["MTDSS_DEBUG_PHOTO"] == "1"
            && navPath.isEmpty {
            for _ in 0..<50 where model.repository == nil {
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
            navPath = [.photoLibrary]
            return
        }
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

/// 首页导航目标:根页推入浏览根、子行点按直入子目录整页(R4 #1),或推入
/// 照片图库(R5c 方案 A 第二固定行)。
/// **子目录档单目的直入**(2026-11-02 R4 用户裁定回炉):子页 VM 以子
/// 目录为浏览根(进页 canGoBack=false → 顶栏返回/边缘右滑 = onPopToRoot
/// 弹回首页,无中间收件箱页);页内再进 = 正常页内历史。
enum FavoritesRoute: Hashable {
    case root(AuthorizedRoot)
    case sub(AuthorizedRoot, path: String)
    /// 照片图库(R5c:特殊来源,非授权根——root 档不适用)。
    case photoLibrary

    /// 照片图库档(浏览根档不适用;boot 未就绪占位面用)。
    var isPhotoLibrary: Bool {
        if case .photoLibrary = self { return true }
        return false
    }

    var root: AuthorizedRoot {
        switch self {
        case .root(let root): root
        case .sub(let root, _): root
        case .photoLibrary: AuthorizedRoot(name: "照片图库", path: "photos://", isInbox: false)
        }
    }

    /// 子目录整页直入路径(.root = nil,开页走 loadRoot)。
    var initialPath: String? {
        switch self {
        case .root: nil
        case .sub(_, let path): path
        case .photoLibrary: nil
        }
    }
}

/// 浏览推入页:每根一枚 VM(StateObject 随页生命周期);showHidden 以
/// 设置开关为源(App 侧,T10),开页取值 + 开关变化活体跟随。
/// initialPath(R4 #1 用户裁定回炉)= 子目录直入单目的:**VM 以子目录为
/// 浏览根**(history = [子目录],进页 canGoBack=false → 顶栏返回/边缘
/// 右滑 = onPopToRoot 弹回首页,无中间收件箱页;标题 = 子目录名,与原
/// 深级口径一致);nil = 常规开页(浏览根 = root.path)。
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
            rootPath: initialPath ?? root.path,
            rootTitle: initialPath.map { BrowseFormat.displayName(ofPath: $0) } ?? root.name,
        ))
    }

    var body: some View {
        BrowseView(
            vm: vm,
            showsArrows: true,
            onPopToRoot: onPopToRoot,
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
/// 子级行内呈现,行点按 = 进整页浏览;R5b #1:选择模式,顶栏/底栏随 VM
/// 态收口在本组件——菜单「选择」与行长按两入口同落 ``enterSelection``)。
///
/// **复用裁定(记录)**:不另起 FavoritesViewModel——首页 VM 就是
/// ``BrowseViewModel``(浏览根 = 收件箱路径,根列表 = ``entries``,展开
/// 机制全量单源:懒加载缓存/世代守卫/缩进/排序管线与浏览页同一实现);
/// 根行本体不是 entry,其展开态 = 页面侧一个 bool——数据早已随根拉取
/// 就位(进页 ``loadRoot()`` 一次),展开/收起 = 状态翻转零拉取;收起走
/// ``collapseAll()``(世代 +1 作废在飞子项拉取、子树缓存释放——R3 评审
/// I-1 同款收口,收起不得留迟到应答复活面)。**根行不可选**(R5b #1):
/// 收件箱根行不在 ``visibleRows`` 内(非 entry),选择圈/全选/收集天然
/// 不及根——选择面只覆盖子行。
private struct FavoritesRootList: View {
    let repo: MtdssRepository
    let selfDeviceId: String?
    let inbox: AuthorizedRoot
    var showHidden: Bool
    let onOpenRoot: (AuthorizedRoot) -> Void
    /// 子目录行点按 = 直入该子目录整页(R4 #1 用户裁定回炉:单目的,
    /// 子页 VM 以子目录为浏览根 → 顶栏返回/边缘右滑弹回首页,无中间
    /// 收件箱页;文件行无点按面,同浏览页)。
    let onOpenPath: (String) -> Void
    /// 照片图库行点按 = 推入照片图库页(R5c 方案 A 第二固定行)。
    let onOpenPhotoLibrary: () -> Void

    @StateObject private var vm: BrowseViewModel
    @State private var inboxExpanded = false
    @Environment(AppModel.self) private var model
    /// 确认后待发送的选择集(non-nil → 选设备弹层)。
    @State private var pickedForSend: PickedSelection?
    /// 待确认删除请求(选择删除;确认居中弹窗 R5b #9 同款)。
    @State private var deleteRequest: DeleteRequest?
    /// 删除结果提示(部分失败明细;纯成功静默)。
    @State private var deleteNotice: String?
    @State private var sending = false
    /// 动作发起反馈胶囊(R3 #3 同语言)。
    @State private var toast: String?

    init(
        repo: MtdssRepository,
        selfDeviceId: String?,
        inbox: AuthorizedRoot,
        showHidden: Bool,
        onOpenRoot: @escaping (AuthorizedRoot) -> Void,
        onOpenPath: @escaping (String) -> Void,
        onOpenPhotoLibrary: @escaping () -> Void,
    ) {
        self.repo = repo
        self.selfDeviceId = selfDeviceId
        self.inbox = inbox
        self.showHidden = showHidden
        self.onOpenRoot = onOpenRoot
        self.onOpenPath = onOpenPath
        self.onOpenPhotoLibrary = onOpenPhotoLibrary
        _vm = StateObject(wrappedValue: BrowseViewModel(
            repo: repo,
            deviceId: BrowseViewModel.localBrowseDeviceId(selfDeviceId: selfDeviceId),
            rootPath: inbox.path,
            rootTitle: inbox.name,
        ))
    }

    var body: some View {
        VStack(spacing: 0) {
            if vm.isSelecting {
                SelectionNavBar(
                    count: vm.selection.count,
                    onSelectAll: { vm.selectAll() },
                    onConfirm: { vm.confirmSelection() },
                )
            } else {
                BrowseNavBar(title: "常用目录", showsArrows: false) {
                    overflowMenu
                }
            }
            rootList
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        // 选择模式隐藏标签栏(spec §4.2;BrowseView 推入页同款口径——首页
        // 是 NavigationStack 根内容,系统同按页内可见性口径执行,R5b 截图钉)
        .toolbar(vm.isSelecting ? Visibility.hidden : .visible, for: .tabBar)
        // 动作发起反馈(首页分享发起;浏览页同语言)
        .toast($toast)
        .safeAreaInset(edge: .bottom) {
            if vm.isSelecting {
                SelectionActionBar(actions: [.share, .delete]) { action in
                    switch action {
                    case .share: shareSelection()
                    case .delete: confirmDeleteSelection()
                    default: break
                    }
                }
            }
        }
        .sheet(item: $pickedForSend) { picked in
            DevicePickerSheet(
                title: "发送 \(picked.items.count) 项到…",
                repo: repo,
                selfDeviceId: model.selfDeviceId,
            ) { device in
                startSend(picked.items, to: device)
            }
        }
        // 删除确认(居中 alert,spec §4.2/R5b #9 同款文案)
        .alert(
            "删除 \(deleteRequest?.paths.count ?? 0) 项?",
            isPresented: Binding(
                get: { deleteRequest != nil },
                set: { if !$0 { deleteRequest = nil } },
            ),
        ) {
            Button("删除", role: .destructive) {
                if let request = deleteRequest {
                    Task { await performDelete(request.paths) }
                }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("所选项目将被永久删除,不可恢复。")
        }
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { vm.lastActionError != nil },
                set: { if !$0 { vm.lastActionError = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(vm.lastActionError ?? "")
        }
        .alert(
            "删除完成",
            isPresented: Binding(
                get: { deleteNotice != nil },
                set: { if !$0 { deleteNotice = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(deleteNotice ?? "")
        }
    }

    // MARK: 列表(根行 + 展开子级;沉底尾注)

    private var rootList: some View {
        SinkingFooterList {
            inboxRow
            photoLibraryRow
            if inboxExpanded {
                childrenArea
            }
        } footer: {
            // 首页计数 = 固定行数(Ruling N 收件箱单枚 + R5c 照片图库行;
            // 展开子级不计数——与浏览页「N 项 = 当前目录计数」同口径)
            ItemCountFooter(count: 2)
        }
        // 根行展开/收起与子级行内展开各驱动各的插拔/旋转动画(值 scoped,
        // 与 BrowseView 同拍 0.2s);选择态进/出驱动选圈列宽动画(同 R4 #3)
        .animation(.easeInOut(duration: 0.2), value: inboxExpanded)
        .animation(.easeInOut(duration: 0.2), value: vm.expandedPaths)
        .animation(.easeInOut(duration: 0.2), value: vm.isSelecting)
        // 下拉刷新(R4 真机轮 #4;R5a 修#4):status 重投影(根数据源)+
        // 收件箱原地重载(不动历史/展开态保持,子缓存逐个现拉刷新);
        // 非结构化 Task 承载解绑 SwiftUI 取消(R5a #4,三面同款)
        .refreshable {
            await Task {
                await model.refreshStatus()
                await vm.reload()
            }.value
        }
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

    // MARK: 「···」菜单(选择 + 排序单选勾;选择入口其一)

    @ViewBuilder private var overflowMenu: some View {
        Button("选择") {
            vm.enterSelection()
        }
        // 根页 = 收件箱单枚(非文件浏览),排序无意义但保留形(T5)
        Picker("排序", selection: .constant(BrowseSort.name)) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
        .disabled(true)
    }

    // MARK: 选择流(spec §4.2 + Ruling O:先捕获后退出;动作仅自底栏)

    /// 底栏分享 = 捕获选择集 → 退出选择模式 → 选设备弹层(浏览页同款次序)。
    private func shareSelection() {
        let items = vm.selectedEntries
        guard !items.isEmpty else { return }
        vm.exitSelection()
        pickedForSend = PickedSelection(items: items)
    }

    /// 底栏删除 = 捕获路径入确认弹窗(确认后才执行;R5b #9)。
    private func confirmDeleteSelection() {
        let paths = vm.selectedEntries.map(\.path)
        guard !paths.isEmpty else { return }
        deleteRequest = DeleteRequest(paths: paths)
    }

    /// 删除执行(确认后;R5b #9):选择档走 ``deleteSelected``(退选+原地
    /// 重载收口在 VM),滑删档走 ``deletePaths``+原地重载;部分失败给明细,
    /// 全成静默。
    private func performDelete(_ paths: [String]) async {
        if vm.isSelecting {
            guard let result = await vm.deleteSelected() else { return } // 失败走 lastActionError
            guard !result.failed.isEmpty else { return } // 全成静默
            deleteNotice = "已删除 \(result.deleted) 项,\(result.failed.count) 项失败:\(result.failed[0].message)"
        } else {
            // 行滑删(R5b #7):单条目,device = 本机(home VM 浏览目标)
            guard let result = await vm.deletePaths(paths) else { return } // 失败走 lastActionError
            await vm.reload()
            guard !result.failed.isEmpty else { return } // 全成静默
            deleteNotice = "已删除 \(result.deleted) 项,\(result.failed.count) 项失败:\(result.failed[0].message)"
        }
    }

    /// 发送到设备(选设备弹层点选后;选择态已在动作面收)。发起成功
    /// toast(R3 #3);失败提示,dir 由 SendService 按对端平台解析(Ruling I)。
    /// files = 收件箱内绝对路径——引擎上传源守卫按授权根收口,收件箱恒在
    /// 授权集(spec §5.2),与本地浏览页发送同形。
    private func startSend(_ items: [DirEntry], to device: DirectoryDevice) {
        let send = SendService(repo: repo)
        guard !sending else { return }
        sending = true
        let paths = items.map(\.path)
        Task {
            defer { sending = false }
            do {
                _ = try await send.send(paths: paths, to: device)
                toast = "已开始 \(paths.count) 项 · 进度见传输列表"
            } catch {
                vm.lastActionError = BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    // MARK: 根行(收件箱:箭头 = 展开,行点按 = 进整页;选择态不可选)

    private var inboxRow: some View {
        Button {
            // 选择模式根行不可选也不导航(R5b #1:点按 = 无操作,不误出页)
            guard !vm.isSelecting else { return }
            onOpenRoot(inbox)
        } label: {
            FullBleedRow {
                HStack(spacing: 12) {
                    // 箭头 = 行内展开钮(R4 #1):› 展开态旋转 90° 成 ∨;
                    // 触区外扩 28pt 方 + 负 padding 还原占位(BrowseEntryRow
                    // 同手法,视觉像素不变);选择态收起钮同禁(展开面照旧
                    // 可用与否无碍选择——保留展开便于子行勾选,R5b 语义)
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
        // 根行无选圈(R5b #1:收件箱根 = 容器行,非文件条目;不进选择集,
        // 不给圈——选择面只覆盖子行)
        .accessibilityLabel(Text(inbox.name))
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

    // MARK: 照片图库行(R5c 方案 A:收件箱行下第二固定行;特殊来源非
    //        授权根,无展开/无选择圈,选择模式点按无操作)

    private var photoLibraryRow: some View {
        Button {
            guard !vm.isSelecting else { return } // 选择态固定行不可选不导航
            onOpenPhotoLibrary()
        } label: {
            FullBleedRow {
                HStack(spacing: 12) {
                    Image(systemName: "photo.stack")
                        .font(.system(size: 20))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 28)
                    Text("照片图库")
                        .font(.system(size: 17))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(PressableRowStyle())
        .accessibilityLabel(Text("照片图库"))
    }

    // MARK: 展开子级(depth = 行序 +1,根行占 depth 0;BrowseEntryRow 复用;
    //        R5b #1 选择模式:子行可选,长按任意子行 = 选择入口其二)

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
                swipeableChildRow(row)
            }
        }
    }

    // MARK: 子行左滑(R5b #7:本地文件行 = [发送, 删除];目录行不可滑)

    /// 子行包 ``SwipeableRow``(浏览页同款组件):文件行才有滑面;动作与
    /// 选择底栏同流——发送进选设备弹层,删除入确认弹窗。
    @ViewBuilder private func swipeableChildRow(_ row: BrowseRow) -> some View {
        let swipes = vm.isSelecting ? [] : BrowseSwipeAction.actions(entry: row.entry, isRemote: false)
        SwipeableRow(
            actions: swipes,
            spec: { $0.swipeSpec },
            onAction: { action in
                switch action {
                case .send:
                    pickedForSend = PickedSelection(items: [row.entry])
                case .delete:
                    deleteRequest = DeleteRequest(paths: [row.entry.path])
                default: break // 首页仅本地集([发送,删除])
                }
            },
        ) {
            BrowseEntryRow(
                entry: row.entry,
                depth: row.depth + 1,
                expanded: vm.expandedPaths.contains(row.entry.path),
                selecting: vm.isSelecting,
                selected: vm.selection.contains(row.entry.path),
                onOpen: {
                    // 选择态点按 = 切换选中(不开目录,spec §4.2);
                    // 常态行点按 = 进整页(目录;文件行无点按面,同浏览页)
                    if vm.isSelecting {
                        vm.toggle(row.entry.path)
                    } else if row.entry.isDir {
                        onOpenPath(row.entry.path)
                    }
                },
                onToggleExpand: row.entry.isDir
                    ? { Task { await vm.toggleExpand(row.entry.path) } }
                    : nil,
            )
        }
        // 长按任意子行进选择(R5b #1 入口其二;BrowseView 同款 0.4s)
        .simultaneousGesture(
            LongPressGesture(minimumDuration: 0.4).onEnded { _ in
                if !vm.isSelecting { vm.enterSelection() }
            },
        )
    }

    // MARK: DEBUG 截图钩子(模拟器无触点驱动;真机/产线不编译)

    #if DEBUG
    /// MTDSS_DEBUG_FAVTREE=1:展开收件箱根行 + 其首个子目录(两级行内
    /// 展开截图面)。MTDSS_DEBUG_FAVSELECT=1:同上再进选择态并勾 3 个
    /// 子行(R5b #1 首页选择截图面:选圈列/选择顶栏/底部动作条/标签栏
    /// 隐藏)。等待口径 = BrowseView ``waitForEntries`` 同款(只看
    /// entries/error,不看 loading——loading 翻转先于应答落地,看 loading
    /// 会提前出循环)。
    private func debugExpandIfHooked() async {
        let env = ProcessInfo.processInfo.environment
        let wantSelect = env["MTDSS_DEBUG_FAVSELECT"] == "1"
        guard env["MTDSS_DEBUG_FAVTREE"] == "1" || wantSelect else { return }
        for _ in 0..<100 where vm.entries.isEmpty && vm.errorText == nil {
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
        if wantSelect {
            vm.enterSelection()
            vm.visibleRows.prefix(3).forEach { vm.toggle($0.entry.path) }
        }
    }
    #endif
}
