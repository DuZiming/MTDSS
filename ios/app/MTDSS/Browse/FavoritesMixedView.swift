/// 常用目录混合页(V3-3 T16,spec §6「混合页」;Windows mixedview.ts 同引擎
/// 语义的 iOS 消费面,T17 根列表行推入):
/// - **VM 单口**(T15 ``MixedViewModel``):reload 渐进扇出(行渐进出现,
///   不等齐)、复合键选择(device\0path,同名跨源两行两 key 不合并)、逐组
///   动作(下载/发送/删除按来源设备分组);本视图只装,零第二份逻辑。
/// - **显示方式**(spec §5.2 裁量 #4):「···」菜单 混合|分组,@AppStorage
///   "mtdss.fav.org" 全局记,缺省 mixed。混合 = 一张列表统一排序;分组 =
///   节头行(色点+设备名+路径末段;offline/failed 灰显+「离线」/「无法访问」
///   标注,pending「加载中…」)+节内行,节序 = states 序(成员添加序),节内
///   排序与混合同源(``visibleRows`` 全局排定后按来源过滤——比较器不出 Kit,
///   同值并列序与混合逐行一致)。
/// - **行** = ``BrowseEntryRow`` 复用 + 行首来源色点(capsule 6pt,色 =
///   ``FavoritesPalette`` device_id 排序位次映射,hex→Color 渲染侧拼装);
///   同名跨源两行并存,靠色点区分(spec §4)。行内展开无(钻取替代)。
/// - **钻取**:目录行点按 = 推 ``FavDrillTarget``(device + 子目录路径,
///   页内子 NavigationStack 值路由,单层;更深的页内历史归 BrowseViewModel)
///   ——device 参数取**真实 id**(引擎 is_self 认真实 id,本机成员短路同口,
///   T13b 修#4;T16 实现裁定:统一 RemoteBrowseTarget 同形态的值路由、不
///   复刻 FavoritesRoute.sub,但目的地须携子路径而 RemoteBrowseTarget 仅
///   deviceId/name/platform,故本页自有薄包装),``BrowseDestination`` 复用
///   (本任务起 +isRemote/+deviceName 两缺省参,既有 .root/.sub 语义不变)。
/// - **选择模式**(Ruling O 骨架):菜单「选择」/行长按进入,顶栏换
///   全选/「N 项」/确认(确认只退出),底部动作条 = [下载, 发送]+删除?
///   ——删除门与 VM ``MixedViewModel/deleteSelected`` 整批门对齐:全本机
///   选择恒出,含远端行须「允许删除远端文件」开。VM 动作面读 vm.selection
///   (活选),故动作先执行后退选——Browse 侧「先捕获后退选」在 VM 读活选
///   形态下不可前置(设计偏差,报告在案)。
/// - **favorite 跟随**:store $favorites 订阅 → 名变更即标题更新(@State
///   镜像驱动 body 重估,T15 concern② 收口)、members 变更 VM 内部硬重扇出、
///   常用被删 → 弹回首页。
/// - **dotfiles**:MixedViewModel 无 showHidden 面(T15 裁定记档),行面
///   直出 entries;T17/T19 若要再补,本页不做。

import Combine
import MTDSSKit
import SwiftUI

struct FavoritesMixedView: View {
    let fav: Favorite
    let repo: MtdssRepository
    let selfDeviceId: String?
    /// 本页返回 = 弹回常用首页(推入页;FavoritesView navPath 收口)。
    let onPopToRoot: () -> Void

    @Environment(AppModel.self) private var model
    @StateObject private var vm: MixedViewModel
    /// 钻取子栈(页内 NavigationStack;单层推入,更深 = 钻取页内历史)。
    @State private var drillPath: [FavDrillTarget] = []
    /// 显示方式(spec §5.2 裁量 #4:全局偏好,非每常用记忆;缺省混合)。
    @AppStorage("mtdss.fav.org") private var org = "mixed"
    /// 设备快照(节头设备名 + 色板 join 数据源,spec §3.1「不入库,渲染时
    /// join」)。
    @State private var devices: [DirectoryDevice] = []
    /// 确认后待发送的混合选择行(non-nil → 选设备弹层)。
    @State private var mixedPicked: MixedPicked?
    /// 待确认删除请求(选择删除;居中 alert R5b #9 同款)。
    @State private var deleteRequest: DeleteRequest?
    /// 动作失败提示(VM 动作返 Bool 聚合,无明细面)。
    @State private var actionError: String?
    /// 动作发起反馈胶囊(R3 #3 同语言)。
    @State private var toast: String?
    /// 常用名镜像(T15 concern②:favorite 非 @Published,名变更须经 @State
    /// 写入驱动 body 重估,标题才即时刷新)。
    @State private var favName: String
    /// DEBUG 钩子恰一次闸(.task 生命周期重入防,FavoritesRootList 同款)。
    @State private var debugHookFired = false

    init(
        fav: Favorite,
        repo: MtdssRepository,
        selfDeviceId: String?,
        onPopToRoot: @escaping () -> Void,
    ) {
        self.fav = fav
        self.repo = repo
        self.selfDeviceId = selfDeviceId
        self.onPopToRoot = onPopToRoot
        _vm = StateObject(wrappedValue: MixedViewModel(
            repo: repo, favorite: fav, selfDeviceId: selfDeviceId))
        _favName = State(initialValue: fav.name)
    }

    var body: some View {
        NavigationStack(path: $drillPath) {
            VStack(spacing: 0) {
                topBar
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
            // 钻取目的地挂**恒存容器**(R5c 同课:挂条件分支视图上,分支互换
            // 会重挂 destination 修饰符并重置导航栈)
            .navigationDestination(for: FavDrillTarget.self) { target in
                drillDestination(target)
            }
        }
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
        // 文案点明整树语义,DeleteRequest 形态沿 FavoritesView)
        .alert(
            deleteRequest?.alertTitle ?? "删除 0 项?",
            isPresented: Binding(
                get: { deleteRequest != nil },
                set: { if !$0 { deleteRequest = nil } },
            ),
        ) {
            Button("删除", role: .destructive) {
                if deleteRequest != nil {
                    Task { await performDeleteSelection() }
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
            // 设备快照(名/色点 join)与混合扇出并行起跑,互不阻塞;失败各
            // 自保旧值(名退化 id、点灰),下拉刷新收口
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
                canBack: false, // 推入页无页内历史:返回 = 弹回常用首页
                onBack: { onPopToRoot() },
                menu: { overflowMenu },
            )
        }
    }

    /// 「···」菜单 = 选择 + 显示方式(混合|分组,全局偏好)+ 排序(与浏览
    /// 页同面;文件夹恒前)。
    @ViewBuilder private var overflowMenu: some View {
        Button("选择") {
            vm.enterSelection()
        }
        Picker("显示方式", selection: $org) {
            Text("混合").tag("mixed")
            Text("分组").tag("grouped")
        }
        Picker("排序", selection: $vm.sort) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
    }

    // MARK: 列表区(空态/首扇出窗/列表 + 沉底尾注)

    @ViewBuilder private var listArea: some View {
        if vm.favorite.members.isEmpty {
            // 空常用(0 成员)= 空态引导(spec §5.2)
            ContentUnavailableView(
                "暂无成员",
                systemImage: "folder.badge.plus",
                description: Text("到设备浏览中添加目录"),
            )
        } else if vm.loading && mixedRows.isEmpty {
            // 首扇出种子窗(行渐进出现前的空窗; 任一应答落地即让位列表)
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
            if org == "grouped" {
                groupedRows
            } else {
                ForEach(mixedRows) { row in
                    favRow(row)
                }
            }
            ItemCountFooter(count: mixedRows.count, note: vm.truncated ? "已截断" : nil)
            if org == "mixed" {
                unavailableFooter
            }
        }
        // 选择态进/出驱动选圈列宽与圈滑入/滑出动画(R4 #3 同拍 0.2s)
        .animation(.easeInOut(duration: 0.2), value: vm.isSelecting)
        // 下拉刷新:设备快照 + 混合扇出;非结构化 Task 承载解绑 SwiftUI 取消
        // (R5a #4,三面同款)
        .refreshable {
            await Task {
                await loadDevices()
                await vm.reload()
            }.value
        }
    }

    /// 混合形态尾注「N 个成员离线或不可用」(spec §5.2;分组形态标注在节头,
    /// 不重复)。
    @ViewBuilder private var unavailableFooter: some View {
        let n = vm.states.filter {
            $0.status == .offline || $0.status == .failed
        }.count
        if n > 0 {
            Text("\(n) 个成员离线或不可用")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(maxWidth: .infinity)
                .padding(.top, 2)
                .padding(.bottom, 14)
        }
    }

    // MARK: 行投影(混合 = VM 单口;分组 = 同投影按来源过滤)

    /// 混合排序投影(VM 单口;混合与分组共用数据面)。
    private var mixedRows: [MixedViewModel.MixedRow] {
        vm.visibleRows()
    }

    /// 分组形态(spec §5.2):节 = 成员,节序 = states 序(成员添加序);节内
    /// = 当前排序(visibleRows 全局排定后按来源过滤,排序语义单源);离线/
    /// 失败/在途节 = 节头单行无条目。
    @ViewBuilder private var groupedRows: some View {
        ForEach(vm.states) { st in
            sectionHeader(st)
            if st.status == .ok {
                ForEach(sectionRows(st)) { row in
                    favRow(row)
                }
            }
        }
    }

    /// 节内行 = 混合投影按来源过滤。
    private func sectionRows(_ st: MixedViewModel.MemberState) -> [MixedViewModel.MixedRow] {
        mixedRows.filter { $0.device == st.member.device }
    }

    /// 节头行:色点 + 设备名 + 路径末段;offline/failed → 灰显 +「离线」/
    /// 「无法访问」标注,pending →「加载中…」(spec §5.2 用户确认面)。
    private func sectionHeader(_ st: MixedViewModel.MemberState) -> some View {
        let dim = st.status == .offline || st.status == .failed
        return FullBleedRow(height: 44) {
            HStack(spacing: 8) {
                Circle()
                    .fill(sourceDot(st.member.device))
                    .frame(width: 6, height: 6)
                Text(deviceName(st.member.device))
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(dim ? Color(uiColor: .systemGray) : Color.primary)
                    .lineLimit(1)
                Text(BrowseFormat.displayName(ofPath: st.member.path))
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

    /// 混合/节内行(BrowseEntryRow 复用 + 行首来源色点):选择态点按 = 切换
    /// 选中(复合键);常态目录行点按 = 钻取(推 RemoteBrowseTarget,真实
    /// device id);文件行无点按面(同浏览页);行内展开无(钻取替代);
    /// 长按任意行 = 选择入口其二(Browse 骨架同款 0.4s)。
    private func favRow(_ row: MixedViewModel.MixedRow) -> some View {
        BrowseEntryRow(
            entry: row.entry,
            selecting: vm.isSelecting,
            selected: vm.selection.contains(row.key),
            sourceDot: sourceDot(row.device),
            onOpen: {
                if vm.isSelecting {
                    vm.toggle(row.key)
                } else if row.entry.isDir {
                    drillPath.append(FavDrillTarget(
                        deviceId: row.device,
                        path: row.entry.path,
                    ))
                }
            },
        )
        .simultaneousGesture(
            LongPressGesture(minimumDuration: 0.4).onEnded { _ in
                if !vm.isSelecting { vm.enterSelection() }
            },
        )
    }

    // MARK: 钻取目的地(BrowseDestination 复用;真实 device id 统一形态)

    /// 钻取 = VM 以子目录为浏览根(进页 canGoBack=false → 顶栏返回/边缘
    /// 右滑 = 弹回本页,无中间页);远端档动作集(下载/转发+删除门)+
    /// 「设备名 | 目录名」标题,本机档 = 本地动作集单段标题。
    @ViewBuilder private func drillDestination(_ target: FavDrillTarget) -> some View {
        let isRemote = target.deviceId != model.selfDeviceId
        let tail = BrowseFormat.displayName(ofPath: target.path)
        if let repo = model.repository {
            BrowseDestination(
                root: AuthorizedRoot(name: tail, path: target.path, isInbox: false),
                repo: repo,
                deviceId: target.deviceId,
                isRemote: isRemote,
                deviceName: isRemote ? deviceName(target.deviceId) : nil,
                onPopToRoot: { drillPath = [] },
            )
        } else {
            bootPlaceholder(title: tail)
        }
    }

    /// 引擎未就绪窗兜底(与 FavoritesView destination 兜底同形;repository
    /// 一般随页存活,引擎重启窗口期防御)。
    private func bootPlaceholder(title: String) -> some View {
        VStack(spacing: 0) {
            BrowseNavBar(
                title: title,
                showsArrows: true,
                canBack: false,
                canForward: false,
                onBack: { drillPath = [] },
                menu: { EmptyView() },
            )
            ContentUnavailableView(
                "引擎未就绪",
                systemImage: "cpu",
                description: Text("引擎启动完成后即可浏览"),
            )
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
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

    /// 底栏下载 = VM downloadSelected(逐组 transfer.start download,dir =
    /// 收件箱哨兵,本机组跳过)→ 退选 → 成功 toast(计数 = 真发起组行数)/
    /// 失败 alert。VM 动作面读 vm.selection,故先动作后退选。
    private func downloadSelection() {
        let rows = selectedRows
        guard !rows.isEmpty else { return }
        let sendable = rows.filter { $0.device != model.selfDeviceId }
        Task {
            let ok = await vm.downloadSelected()
            vm.exitSelection()
            guard ok else {
                actionError = "下载发起失败,请重试。"
                return
            }
            if !sendable.isEmpty {
                toast = "已开始 \(sendable.count) 项 · 进度见传输列表"
            }
        }
    }

    /// 底栏发送 = 捕获行进选设备弹层(选择保持至动作执行,理由同下载)。
    private func openSendPicker() {
        let rows = selectedRows
        guard !rows.isEmpty else { return }
        mixedPicked = MixedPicked(rows: rows)
    }

    /// 发起发送(选设备弹层点选后;弹层已自 dismiss):VM sendSelected 逐组
    /// upload(dir = 收件箱哨兵,from==to 组跳过)→ 退选 → 成功 toast(计数
    /// = 真发起组行数)/失败 alert。
    private func startSend(_ rows: [MixedViewModel.MixedRow], to device: DirectoryDevice) {
        let sendable = rows.filter { $0.device != device.deviceId }
        Task {
            let ok = await vm.sendSelected(to: device)
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
        deleteRequest = DeleteRequest(
            paths: rows.map(\.entry.path),
            folders: rows.filter { $0.entry.isDir }.count,
        )
    }

    /// 删除执行(确认后):VM deleteSelected(allowRemoteDelete:)(逐组
    /// ctrl delete force;含远端门关 = 整批拒零外发返 false——UI 出钮条件
    /// 已对齐,落此处 = 真失败/门态竞窗,同文案可重试)。VM 只执行不退选
    /// 不重载(T15 头注),收口在此:全成 = 退选 + 原地重载静默;失败 =
    /// 统一提示(VM 返聚合 Bool,无明细面)。
    private func performDeleteSelection() async {
        let ok = await vm.deleteSelected(allowRemoteDelete: model.allowRemoteDelete)
        guard ok else {
            actionError = "删除未全部成功,请重试。"
            return
        }
        vm.exitSelection()
        await vm.reload()
    }

    // MARK: 数据跟随(设备快照 / favorite 整单)

    /// 设备快照(节头设备名 + 色板 join 数据源):失败保旧值(名退化 id、
    /// 点灰),下拉刷新/重进页收口。
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
    /// (updateFavorite 世代守卫)、常用被删 → 弹回首页(解除链接不触碰
    /// 文件,弹离即收)。
    private func followFavoriteChange(_ favs: [Favorite]) {
        guard let latest = favs.first(where: { $0.id == vm.favorite.id }) else {
            onPopToRoot()
            return
        }
        favName = latest.name
        vm.updateFavorite(latest)
    }

    // MARK: 色点与名称(设备快照 join)

    private var colorMap: [String: String] {
        FavoritesPalette.colorMap(devices: devices)
    }

    /// 行首/节头来源色点(色 = device_id 排序位次映射;快照未及 = 灰点,
    /// 刷新后归位)。
    private func sourceDot(_ device: String) -> Color {
        if let hex = colorMap[device] {
            return Color(paletteHex: hex)
        }
        return Color(uiColor: .systemGray3)
    }

    /// 设备显示名(快照 join;未及退化 id)。
    private func deviceName(_ device: String) -> String {
        devices.first { $0.deviceId == device }?.name ?? device
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;真机/产线不编译)

    #if DEBUG
    /// MTDSS_DEBUG_FAVMIXED=1:等条目就位后停留(截图面;推入驱动归 T17
    /// 根列表行/手动轮)。=grouped:先切分组形态再等。等待口径 =
    /// FavoritesView debugExpandIfHooked 同款(只看条目投影,不看 loading
    /// ——loading 翻转先于应答落地,看 loading 会提前出循环)。
    private func debugWaitIfHooked() async {
        let raw = ProcessInfo.processInfo.environment["MTDSS_DEBUG_FAVMIXED"] ?? ""
        guard raw == "1" || raw == "grouped" else { return }
        if raw == "grouped" { org = "grouped" }
        for _ in 0..<100 where mixedRows.isEmpty {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
    }
    #endif
}

/// 钻取目标(页内 NavigationStack 值路由;device_id + 子目录路径)。形态
/// 同 ``RemoteBrowseTarget``(DevicesView 的值路由薄包装),但钻取须携子
/// 路径而其无 path 面,故本页自有(Hashable 供 NavigationStack 携带)。
private struct FavDrillTarget: Hashable {
    let deviceId: String
    let path: String
}

/// 已捕获待发送的混合选择行(发送流载荷;须携来源设备——from==to 组被 VM
/// 跳过,toast 计数按真实可发起行数。``PickedSelection`` 仅携 DirEntry,
/// 不够用)。
private struct MixedPicked: Identifiable {
    let id = UUID()
    let rows: [MixedViewModel.MixedRow]
}
