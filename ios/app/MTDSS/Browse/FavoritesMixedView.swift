/// 快捷目录混合页(V3-3 T16;返工波 4 §16.1 分类恒定):
/// - **分类恒定**(§16.1):「混合|分组」切换整体移除(@AppStorage
///   "mtdss.fav.org" 随之删,混合平铺消费废止)——恒按成员分节:节头 =
///   「设备名 - 目录名」(§16.1 词面;设备名 = devices 快照 join,未及退化
///   id;目录名 = 路径显示名),节内 = 该成员条目(排序与 ``visibleRows()``
///   同管线,排序语义单源),节序 = states 序(成员添加序),成员到达 =
///   整节追加。**来源色点全量移除**(§16.2,行与节头零消费)。
/// - **节收起**(§13.5 延续):节头 chevron 可收起(默认全展;收起 = 节头
///   单行,标注保留;会话内 state 不持久;收起不修剪选择集)。
/// - **SWR 缓存**(§16.4):VM 内建(Windows favCache 同象)——重进同快捷
///   缓存种子瞬时整表,在线成员后台静默再校验差异落位;members 变更/
///   登出失效(VM 侧实现,本组件零感知)。
/// - **行** = ``BrowseEntryRow`` 复用;行内展开无(钻取替代)。
/// - **搜索框**(§15.6):浏览页同款搜索框——VM ``MixedViewModel/searchText``
///   单口按名滤行(大小写不敏感子串),全选/动作分组与渲染行同面;零命中
///   节 = 节头单行。
/// - **行左滑**(§15.6):[下载, 转发] + 删除?(删除门镜像底栏:本机行
///   恒可删;远端行须「允许删除远端文件」开)。动作与选择底栏同一函数面:
///   单行 = 单行集(``MixedViewModel/downloadRows`` 等 rows 入参变体,逐组
///   语义零差)。
/// - **钻取**:目录行点按 = 推 ``FavoritesRoute/drill``(device + 子目录
///   路径,**外层栈统一值路由**;更深的页内历史归 BrowseViewModel)
///   ——device 参数取**真实 id**(引擎 is_self 认真实 id,本机成员短路同口,
///   T13b 修#4),``BrowseDestination`` 复用(+isRemote/+deviceName)。
///   **§14.6 二轮根修(2026-11-06)改道**:原为页内子 NavigationStack 值
///   路径(FavDrillTarget 薄包装)——sim 取证钉死 iOS 26 两栈路径元素
///   **异型**(FavoritesRoute vs FavDrillTarget)在转场记账合流时触发
///   `AnyNavigationPath.comparisonTypeMismatch` 断言(SIGTRAP,用户报告
///   「新建快捷点入即崩」;.ips+console 双证在案)——嵌套栈撤销,钻取并入
///   外层 FavoritesRoute 同型路径,崩面结构性消除。
/// - **选择模式**(Ruling O 骨架):菜单「选择」/行长按进入,顶栏换
///   全选/「N 项」/确认(确认只退出),底部动作条 = [下载, 发送]+删除?
///   ——删除门与 VM ``MixedViewModel/deleteSelected`` 整批门对齐:全本机
///   选择恒出,含远端行须「允许删除远端文件」开。VM 动作面读 vm.selection
///   (活选),故动作先执行后退选——Browse 侧「先捕获后退选」在 VM 读活选
///   形态下不可前置(设计偏差,报告在案)。
/// - **favorite 跟随**:store $favorites 订阅 → 名变更即标题更新(@State
///   镜像驱动 body 重估,T15 concern② 收口)、members 变更 VM 内部硬重扇出、
///   快捷被删 → 弹回首页。
/// - **dotfiles**:MixedViewModel 无 showHidden 面(T15 裁定记档),行面
///   直出 entries;T17/T19 若要再补,本页不做。

import Combine
import MTDSSKit
import SwiftUI

struct FavoritesMixedView: View {
    let fav: Favorite
    let repo: MtdssRepository
    let selfDeviceId: String?
    /// 本页返回 = 弹回快捷首页(推入页;FavoritesView navPath 收口)。
    let onPopToRoot: () -> Void
    /// 钻取(目录行点按):外层栈统一值路由(§14.6 二轮根修,见类型头注
    /// 「钻取」bullet)。deviceName = 本页快照现值(时点定格,注释钉)。
    let onDrill: (_ deviceId: String, _ deviceName: String?, _ path: String) -> Void

    @Environment(AppModel.self) private var model
    @StateObject private var vm: MixedViewModel
    /// 设备快照(节头设备名 join 数据源,spec §3.1「不入库,渲染时 join」)。
    @State private var devices: [DirectoryDevice] = []
    /// 节收起集(§13.5:键 = 成员身份 device\0path;空集 = 全展,节头整行
    /// 点按切换。会话内 state 不持久;收起仅藏节内行,选择集不修剪)。
    @State private var collapsedKeys: Set<String> = []
    /// 确认后待发送的行集(non-nil → 选设备弹层;选择底栏 = 选中行,行左滑
    /// 转发 = 单行,同一收口)。
    @State private var mixedPicked: MixedPicked?
    /// 待确认删除请求(选择删除/行滑删同一收口;居中 alert R5b #9 同款)。
    @State private var deleteRequest: DeleteRequest?
    /// 待删除行集(行滑删档;确认后走 ``performDeleteRows``——非选择态,
    /// VM 活选为空,单行 = 单行集显式入参)。
    @State private var deleteRowsPending: [MixedViewModel.MixedRow] = []
    /// 动作失败提示(VM 动作返 Bool 聚合,无明细面)。
    @State private var actionError: String?
    /// 动作发起反馈胶囊(R3 #3 同语言)。
    @State private var toast: String?
    /// 快捷名镜像(T15 concern②:favorite 非 @Published,名变更须经 @State
    /// 写入驱动 body 重估,标题才即时刷新)。
    @State private var favName: String
    /// DEBUG 钩子恰一次闸(.task 生命周期重入防,FavoritesRootList 同款)。
    @State private var debugHookFired = false

    init(
        fav: Favorite,
        repo: MtdssRepository,
        selfDeviceId: String?,
        onPopToRoot: @escaping () -> Void,
        onDrill: @escaping (_ deviceId: String, _ deviceName: String?, _ path: String) -> Void,
    ) {
        self.fav = fav
        self.repo = repo
        self.selfDeviceId = selfDeviceId
        self.onPopToRoot = onPopToRoot
        self.onDrill = onDrill
        _vm = StateObject(wrappedValue: MixedViewModel(
            repo: repo, favorite: fav, selfDeviceId: selfDeviceId))
        _favName = State(initialValue: fav.name)
    }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            searchBar
            listArea
        }
        .background(Color(uiColor: .systemBackground))
        // 左缘右滑返回(R2/R6 先案:自绘顶栏令系统 pop 手势失效;本页无
        // 页内历史,边缘右滑恒 = 弹回首页;选择态先行忽略——选择中滑动
        // = 滚动,不误退)
        .simultaneousGesture(
            DragGesture(minimumDistance: 10).onEnded { value in
                guard !vm.isSelecting else { return }
                guard BrowseGesture.isBack(
                    startX: value.startLocation.x,
                    dx: value.translation.width,
                    dy: value.translation.height,
                ) else { return }
                onPopToRoot()
            },
        )
        .toolbar(.hidden, for: .navigationBar)
        .tabBarHiddenWhenSelecting(vm.isSelecting)
        // 动作发起反馈(下载/发送发起;spec §4.2 R3 裁定)
        .toast($toast)
        .safeAreaInset(edge: .bottom) {
            if vm.isSelecting {
                // 动作集 = [下载, 发送] + 删除?(删除门见 ``deleteUnlocked``)
                SelectionActionBar(actions: actionBarActions) { action in
                    switch action {
                    case .download: downloadSelection()
                    case .share: openSendPicker()
                    case .delete: confirmDeleteSelection()
                    default: break
                    }
                }
            }
        }
        .sheet(item: $mixedPicked) { picked in
            // 发送弹层不排除来源设备(T16 简化裁定:引擎拒 from==to 的组 VM
            // 已跳过;全部自环的边界在 ``startSend`` 计数如实)
            DevicePickerSheet(
                title: "发送 \(picked.rows.count) 项到…",
                repo: repo,
                selfDeviceId: model.selfDeviceId,
            ) { device in
                startSend(picked.rows, to: device)
            }
        }
        // 删除确认(居中 alert,spec §4.2/R5b #9 同款文案;R11 ③:文件夹形
        // 文案点明整树语义,DeleteRequest 形态沿 FavoritesView。选择删除 =
        // 活选档(vm.deleteSelected),行滑删 = 单行档(vm.deleteRows),
        // isSelecting 分派)
        .alert(
            deleteRequest?.alertTitle ?? "删除 0 项?",
            isPresented: Binding(
                get: { deleteRequest != nil },
                set: { if !$0 { deleteRequest = nil } },
            ),
        ) {
            Button("删除", role: .destructive) {
                if deleteRequest != nil {
                    if vm.isSelecting {
                        Task { await performDeleteSelection() }
                    } else {
                        Task { await performDeleteRows(deleteRowsPending) }
                    }
                }
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text(deleteRequest?.alertMessage ?? "")
        }
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { actionError != nil },
                set: { if !$0 { actionError = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
        .task {
            // 设备快照(节头设备名 join)与混合扇出并行起跑,互不阻塞;失败各
            // 自保旧值(名退化 id),下拉刷新收口
            Task { await loadDevices() }
            await vm.reload()
        }
        // favorite 跟随(store 整单替换即达:他端编辑经 data_changed 重拉、
        // 本端编辑 put 成才落)
        .onReceive(model.favorites.$favorites) { favs in
            followFavoriteChange(favs)
        }
        #if DEBUG
        .task {
            guard !debugHookFired else { return }
            debugHookFired = true
            await debugWaitIfHooked()
        }
        #endif
    }

    // MARK: 顶栏(选择态换选择骨架;Ruling O:确认只退出)

    @ViewBuilder private var topBar: some View {
        if vm.isSelecting {
            SelectionNavBar(
                count: vm.selection.count,
                onSelectAll: { vm.selectAll() },
                onConfirm: { vm.exitSelection() },
            )
        } else {
            BrowseNavBar(
                title: favName,
                showsArrows: true,
                canBack: false, // 推入页无页内历史:返回 = 弹回快捷首页
                onBack: { onPopToRoot() },
                menu: { overflowMenu },
            )
        }
    }

    /// 「···」菜单 = 选择 + 排序(与浏览页同面;文件夹恒前)。分类恒定后
    /// 显示方式段废止(§16.1:混合模式整体移除)。
    @ViewBuilder private var overflowMenu: some View {
        Button("选择") {
            vm.enterSelection()
        }
        Picker("排序", selection: $vm.sort) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
    }

    // MARK: 搜索框(§15.6;浏览页同款:高 40 圆角 11 systemGray6 底,VM 单口)

    private var searchBar: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14))
                .foregroundStyle(Color(uiColor: .systemGray))
            TextField("搜索", text: $vm.searchText)
                .font(.system(size: 15.5))
                .autocorrectionDisabled()
        }
        .padding(.horizontal, 10)
        .frame(height: 40)
        .background(Color(uiColor: .systemGray6), in: RoundedRectangle(cornerRadius: 11))
        .padding(.horizontal, 20)
        .padding(.top, 4)
        .padding(.bottom, 10)
    }

    // MARK: 列表区(空态/首扇出窗/列表 + 沉底尾注)

    @ViewBuilder private var listArea: some View {
        if vm.favorite.members.isEmpty {
            // 空快捷(0 成员)= 空态引导(spec §5.2)
            ContentUnavailableView(
                "暂无成员",
                systemImage: "folder.badge.plus",
                description: Text("到设备浏览中添加目录"),
            )
        } else if vm.loading && mixedRows.isEmpty {
            // 首扇出种子窗(行渐进出现前的空窗;SWR 缓存命中成员即行,任一
            // 应答落地即让位列表)
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else {
            list
        }
    }

    private var list: some View {
        BrowseList {
            groupedRows
            ItemCountFooter(count: mixedRows.count, note: vm.truncated ? "已截断" : nil)
        }
        // 选择态进/出驱动选圈列宽与圈滑入/滑出动画(R4 #3 同拍 0.2s);节
        // 收起/展开同拍插拔(§13.5)
        .animation(.easeInOut(duration: 0.2), value: vm.isSelecting)
        .animation(.easeInOut(duration: 0.2), value: collapsedKeys)
        // 下拉刷新:设备快照 + 混合扇出(SWR 缓存种子 + 后台再校验同一口);
        // 非结构化 Task 承载解绑 SwiftUI 取消(R5a #4,三面同款)
        .refreshable {
            await Task {
                await loadDevices()
                await vm.reload()
            }.value
        }
    }

    // MARK: 分类恒定渲染(§16.1:恒按成员分节)

    /// 混合排序投影(VM 单口;搜索过滤在 VM——渲染行/全选/动作分组同面)。
    private var mixedRows: [MixedViewModel.MixedRow] {
        vm.visibleRows()
    }

    /// 分节(§16.1):节 = 成员,节序 = states 序(成员添加序);节内 =
    /// 当前排序(visibleRows 全局排定后按来源过滤,排序语义单源);离线/
    /// 失败/在途节 = 节头单行无条目;搜索零命中节同形(§15.6)。
    @ViewBuilder private var groupedRows: some View {
        ForEach(vm.states) { st in
            sectionHeader(st)
            if st.status == .ok, !collapsedKeys.contains(st.id) {
                ForEach(sectionRows(st)) { row in
                    favRow(row)
                }
            }
        }
    }

    /// 节内行 = 混合投影按来源过滤(搜索已在 VM 面滤过)。
    private func sectionRows(_ st: MixedViewModel.MemberState) -> [MixedViewModel.MixedRow] {
        mixedRows.filter { $0.device == st.member.device }
    }

    /// 节头行(§16.1 词面):收起 chevron + 设备名 +「- 目录名」;offline/
    /// failed → 灰显 +「离线」/「无法访问」标注,pending →「加载中…」
    /// (§13.5 延续:整行点按展开/收起,收起仅藏节内行,标注保留)。色点
    /// 已移除(§16.2)。
    private func sectionHeader(_ st: MixedViewModel.MemberState) -> some View {
        let dim = st.status == .offline || st.status == .failed
        let collapsed = collapsedKeys.contains(st.id)
        return Button {
            // 节头整行点按 = 展开/收起(§13.5;选择态照旧——收起面与选择
            // 无涉,R5b 语义)
            if collapsed {
                collapsedKeys.remove(st.id)
            } else {
                collapsedKeys.insert(st.id)
            }
        } label: {
            FullBleedRow(height: 44) {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .frame(width: 12)
                        .rotationEffect(.degrees(collapsed ? 0 : 90))
                    Text(deviceName(st.member.device))
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(dim ? Color(uiColor: .systemGray) : Color.primary)
                        .lineLimit(1)
                    Text(" - " + BrowseFormat.displayName(ofPath: st.member.path))
                        .font(.system(size: 12.5))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if let badge = statusBadge(st.status) {
                        Text(badge)
                            .font(.system(size: 12.5))
                            .foregroundStyle(Color(uiColor: .systemGray))
                    }
                }
                .padding(.horizontal, 20)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(
            (collapsed ? "展开" : "收起") + " " + deviceName(st.member.device)))
    }

    /// 节头状态标注(nil = ok 无标注)。
    private func statusBadge(_ status: MixedViewModel.MemberStatus) -> String? {
        switch status {
        case .pending: "加载中…"
        case .offline: "离线"
        case .failed: "无法访问"
        case .ok: nil
        }
    }

    // MARK: 行(选择/钻取/左滑;色点已移除 §16.2)

    /// 行(BrowseEntryRow 复用):选择态点按 = 切换选中(复合键);常态目录
    /// 行点按 = 钻取(推 RemoteBrowseTarget,真实 device id);文件行无点按
    /// 面(同浏览页);行内展开无(钻取替代);长按任意行 = 选择入口其二
    /// (Browse 骨架同款 0.4s);左滑 = [下载, 转发] + 删除?(§15.6,门见
    /// ``rowSwipes``)。
    private func favRow(_ row: MixedViewModel.MixedRow) -> some View {
        BrowseEntryRow(
            entry: row.entry,
            selecting: vm.isSelecting,
            selected: vm.selection.contains(row.key),
            onOpen: {
                if vm.isSelecting {
                    vm.toggle(row.key)
                } else if row.entry.isDir {
                    onDrill(row.device, deviceName(row.device), row.entry.path)
                }
            },
        )
        .modifier(NativeSwipeActions(
            actions: rowSwipes(row),
            spec: { $0.swipeSpec },
            onAction: { action in
                handleRowSwipe(action, row: row)
            },
        ))
        .simultaneousGesture(
            LongPressGesture(minimumDuration: 0.4).onEnded { _ in
                if !vm.isSelecting { vm.enterSelection() }
            },
        )
    }

    /// 行左滑动作集(§15.6;混排行集 = [下载, 转发] + 删除?——删除门镜像
    /// 底栏:本机行恒可删(全本机恒可删);远端行须「允许删除远端文件」开。
    /// 声明序末位贴缘 = 删除(NativeSwipeActions reversed,浏览集同形)。
    /// 选择态翻空集即收(浏览行同款)。
    private func rowSwipes(_ row: MixedViewModel.MixedRow) -> [BrowseSwipeAction] {
        guard !vm.isSelecting else { return [] }
        let isLocal = row.device == model.selfDeviceId
        var swipes: [BrowseSwipeAction] = [.download, .forward]
        if isLocal || model.allowRemoteDelete {
            swipes.append(.delete)
        }
        return swipes
    }

    /// 行左滑分发(§15.6;动作与选择底栏同一函数面——单行 = 单行集):
    /// 下载直达(远端 download=哨兵 / 本机 local_copy=收件箱,VM 逐组同口);
    /// 转发 = 单行捕获进选设备弹层;删除入确认弹窗(单行档,``deleteRowsPending``)。
    private func handleRowSwipe(_ action: BrowseSwipeAction, row: MixedViewModel.MixedRow) {
        switch action {
        case .download:
            downloadRows([row])
        case .forward:
            openSendPicker(rows: [row])
        case .delete:
            deleteRowsPending = [row]
            deleteRequest = DeleteRequest(
                paths: [row.entry.path],
                folders: row.entry.isDir ? 1 : 0)
        default:
            break // 快捷行集无 send/favorite 面
        }
    }

    // MARK: 选择流(动作仅自底栏;VM 动作面读活选,次序见各法)

    private var selectedRows: [MixedViewModel.MixedRow] {
        vm.visibleRows().filter { vm.selection.contains($0.key) }
    }

    /// 删除钮门(与 VM deleteSelected 整批门对齐):全本机选择恒出;含远端
    /// 行须「允许删除远端文件」开(selfDeviceId 未知 = 保守按远端计)。
    private var deleteUnlocked: Bool {
        if model.allowRemoteDelete { return true }
        let rows = selectedRows
        guard !rows.isEmpty else { return true }
        let selfId = model.selfDeviceId
        return rows.allSatisfy { $0.device == selfId }
    }

    /// 底栏动作集 = [下载, 发送] + 删除?(门见 ``deleteUnlocked``)。
    private var actionBarActions: [SelectionActionBar.ActionKind] {
        var actions: [SelectionActionBar.ActionKind] = [.download, .share]
        if deleteUnlocked {
            actions.append(.delete)
        }
        return actions
    }

    /// 底栏下载 = VM downloadSelected(活选;逐组:远端 download 哨兵/本机
    /// local_copy 收件箱,§14.2 对齐)→ 退选 → 成功 toast(计数 = 全部发起
    /// 行数)/失败 alert。VM 动作面读 vm.selection,故先动作后退选。
    private func downloadSelection() {
        let rows = selectedRows
        guard !rows.isEmpty else { return }
        Task {
            let ok = await vm.downloadSelected(inboxDir: model.recvDir)
            vm.exitSelection()
            guard ok else {
                actionError = "下载发起失败,请重试。"
                return
            }
            toast = "已开始 \(rows.count) 项 · 进度见传输列表"
        }
    }

    /// 下载行集(行左滑档同口;单行 = 单行集;非选择态退选为空操作)。
    private func downloadRows(_ rows: [MixedViewModel.MixedRow]) {
        guard !rows.isEmpty else { return }
        Task {
            let ok = await vm.downloadRows(rows, inboxDir: model.recvDir)
            vm.exitSelection()
            guard ok else {
                actionError = "下载发起失败,请重试。"
                return
            }
            toast = "已开始 \(rows.count) 项 · 进度见传输列表"
        }
    }

    /// 底栏发送 = 捕获行进选设备弹层(选择保持至动作执行,理由同下载)。
    private func openSendPicker() {
        let rows = selectedRows
        guard !rows.isEmpty else { return }
        openSendPicker(rows: rows)
    }

    /// 发送行捕获进选设备弹层(底栏与行左滑转发同一收口)。
    private func openSendPicker(rows: [MixedViewModel.MixedRow]) {
        guard !rows.isEmpty else { return }
        mixedPicked = MixedPicked(rows: rows)
    }

    /// 发起发送(选设备弹层点选后;弹层已自 dismiss):VM sendRows 逐组
    /// upload(dir = 收件箱哨兵,from==to 组跳过)→ 选择态则退选 → 成功
    /// toast(计数 = 真发起行数)/失败 alert。
    private func startSend(_ rows: [MixedViewModel.MixedRow], to device: DirectoryDevice) {
        let sendable = rows.filter { $0.device != device.deviceId }
        Task {
            let ok = await vm.sendRows(rows, to: device)
            vm.exitSelection()
            guard ok else {
                actionError = "发送发起失败,请重试。"
                return
            }
            if !sendable.isEmpty {
                toast = "已开始 \(sendable.count) 项 · 进度见传输列表"
            }
        }
    }

    /// 底栏删除 = 捕获入确认弹窗(确认后才执行;R5b #9。R11 ③:目录计数
    /// 随行——文件夹形文案点明整树语义,DeleteRequest 形态沿 FavoritesView)。
    private func confirmDeleteSelection() {
        let rows = selectedRows
        guard !rows.isEmpty else { return }
        deleteRowsPending = []
        deleteRequest = DeleteRequest(
            paths: rows.map(\.entry.path),
            folders: rows.filter { $0.entry.isDir }.count,
        )
    }

    /// 删除执行·选择档(确认后):VM deleteSelected(allowRemoteDelete:)
    /// (活选逐组 ctrl delete force;含远端门关 = 整批拒零外发返 false——
    /// UI 出钮条件已对齐,落此处 = 真失败/门态竞窗,同文案可重试)。VM 只
    /// 执行不退选不重载(T15 头注),收口在此:全成 = 退选 + 原地重载静默;
    /// 失败 = 统一提示(VM 返聚合 Bool,无明细面)。
    private func performDeleteSelection() async {
        let ok = await vm.deleteSelected(allowRemoteDelete: model.allowRemoteDelete)
        guard ok else {
            actionError = "删除未全部成功,请重试。"
            return
        }
        vm.exitSelection()
        await vm.reload()
    }

    /// 删除执行·行滑档(确认后):VM deleteRows(单行 = 单行集,门口径同
    /// 选择档)→ 原地重载静默;失败统一提示。
    private func performDeleteRows(_ rows: [MixedViewModel.MixedRow]) async {
        let ok = await vm.deleteRows(rows, allowRemoteDelete: model.allowRemoteDelete)
        guard ok else {
            actionError = "删除未全部成功,请重试。"
            return
        }
        await vm.reload()
    }

    // MARK: 数据跟随(设备快照 / favorite 整单)

    /// 设备快照(节头设备名 join 数据源):失败保旧值(名退化 id),下拉
    /// 刷新/重进页收口。
    private func loadDevices() async {
        let directory = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.devices()
        }
        if let directory {
            devices = directory.devices
        }
    }

    /// favorite 整单跟随(store $favorites):名变更即标题更新(@State 镜像
    /// 驱动 body 重估,T15 concern② 收口)、members 变更 VM 内部硬重扇出
    /// (updateFavorite 世代守卫)、快捷被删 → 弹回首页(解除链接不触碰
    /// 文件,弹离即收)。
    private func followFavoriteChange(_ favs: [Favorite]) {
        guard let latest = favs.first(where: { $0.id == vm.favorite.id }) else {
            onPopToRoot()
            return
        }
        favName = latest.name
        vm.updateFavorite(latest)
    }

    /// 设备显示名(快照 join;未及退化 id)。
    private func deviceName(_ device: String) -> String {
        devices.first { $0.deviceId == device }?.name ?? device
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;真机/产线不编译)

    #if DEBUG
    /// MTDSS_DEBUG_FAVMIXED=1:等条目就位后停留(截图面;推入驱动归 T17
    /// 根列表行/手动轮)。等待口径 = FavoritesView debugExpandIfHooked 同款
    /// (只看条目投影,不看 loading——loading 翻转先于应答落地,看 loading
    /// 会提前出循环)。分类恒定后无形态切换(§16.1)。
    private func debugWaitIfHooked() async {
        let raw = ProcessInfo.processInfo.environment["MTDSS_DEBUG_FAVMIXED"] ?? ""
        guard raw == "1" else { return }
        for _ in 0..<100 where mixedRows.isEmpty {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
    }
    #endif
}

/// 已捕获待发送的行集(发送流载荷;须携来源设备——from==to 组被 VM 跳过,
/// toast 计数按真实可发起行数。``PickedSelection`` 仅携 DirEntry,不够用)。
private struct MixedPicked: Identifiable {
    let id = UUID()
    let rows: [MixedViewModel.MixedRow]
}
