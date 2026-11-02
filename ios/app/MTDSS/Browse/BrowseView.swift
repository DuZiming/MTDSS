/// 浏览页模板(spec §4.1,收件箱/信任目录/远端浏览共用):
/// 顶栏 → 搜索框(灰底圆角+放大镜,按名称过滤当前列表)→ 通栏行
/// (文件夹=左蓝›+蓝文件夹图标;文件=无箭头+问号文档图标;副文
/// 「日期 - N 项」/「日期 - 大小」)→「N 项」沉底。「···」菜单 =
/// 选择 + 排序(名称/日期/大小/种类,单选勾)。
/// **选择模式**(spec §4.2,T6 + Ruling O 2026-11-02):长按任意行或菜单
/// 「选择」进入——顶栏换 全选/「N 项」/蓝圈白勾确认,行首选圈(选中=蓝底
/// 白勾+行浅灰底),搜索框保留,**标签栏隐藏**,底部动作条(本地=分享/
/// 删除);**确认=退出选择模式(无论选否,无副作用)**;操作仅自底栏
/// 发起:分享/转发 → 选设备弹层,下载/删除 → 直发。
/// 浏览状态全在 ``BrowseViewModel``(MTDSSKit,宿主可测);本视图只装。

import MTDSSKit
import SwiftUI

struct BrowseView: View {
    @ObservedObject var vm: BrowseViewModel
    /// 推入页显示箭头;标签根页无箭头(仅三点)。
    var showsArrows: Bool
    /// 已在浏览根按返回 → 弹回标签根(常用目录/设备页)。
    var onPopToRoot: (() -> Void)? = nil
    /// 本机浏览必给(分享/删除动作);远端浏览 T7 换远端动作集(挂接点)。
    var sendService: SendService? = nil
    /// 远端档(T7,设备页浏览):底部动作 = 下载/转发(spec §4.2 远端集),
    /// 动作进「转发 N 项到…」弹层;本地档 = 分享/删除 →「发送 N 项到…」。
    var isRemote: Bool = false

    @Environment(AppModel.self) private var model
    /// 确认后待发送的选择集(non-nil → 选设备弹层)。
    @State private var pickedForSend: PickedSelection?
    @State private var confirmingDelete = false
    /// 删除结果提示(部分失败明细;纯成功静默)。
    @State private var deleteNotice: String?
    @State private var sending = false
    /// 动作发起反馈胶囊(R3 真机轮 #3/#6:发起即 toast,进度归传输列表)。
    @State private var toast: String?

    struct PickedSelection: Identifiable {
        let id = UUID()
        let items: [DirEntry]
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
                BrowseNavBar(
                    title: vm.title,
                    showsArrows: showsArrows,
                    canBack: vm.canGoBack,
                    canForward: vm.canGoForward,
                    onBack: {
                        if vm.canGoBack {
                            Task { await vm.goBack() }
                        } else {
                            onPopToRoot?()
                        }
                    },
                    onForward: { Task { await vm.goForward() } },
                    menu: { overflowMenu },
                )
            }
            searchBar
            listArea
        }
        .background(Color(uiColor: .systemBackground))
        // 动作发起反馈(下载/发送/分享/转发;spec §4.2 R3 裁定)
        .toast($toast)
        // 左缘右滑返回(R2 真机轮):本页自绘顶栏 + `.toolbar(.hidden, …)`
        // 已令系统 interactivePopGesture 失效(T13b「系统返回不可用」先案),
        // 边缘返回由自装手势独占补位——无系统手势可冲突、无双跳面:
        // - 历史内(canGoBack)= vm.goBack() 目录级后退;
        // - 已在浏览根 = onPopToRoot 弹回推入前页面(与顶栏返回箭头同语义,
        //   即该页在无系统手势下「自然的 iOS 返回」);
        // - simultaneousGesture 与列表滚动并存,横纵比谓词滤纵向滚动
        //   (``BrowseGesture``);选择态先行忽略(选择中滑动 = 滚动,不退选)。
        .simultaneousGesture(
            DragGesture(minimumDistance: 10).onEnded { value in
                guard !vm.isSelecting else { return }
                guard BrowseGesture.isBack(
                    startX: value.startLocation.x,
                    dx: value.translation.width,
                    dy: value.translation.height,
                ) else { return }
                if vm.canGoBack {
                    Task { await vm.goBack() }
                } else {
                    onPopToRoot?()
                }
            },
        )
        // 选择模式隐藏标签栏(spec §4.2;推入页在 TabView 的 NavigationStack 内,
        // 系统支持的整页工具栏可见性口径)
        .toolbar(vm.isSelecting ? Visibility.hidden : .visible, for: .tabBar)
        .safeAreaInset(edge: .bottom) {
            if vm.isSelecting {
                SelectionActionBar(actions: isRemote ? [.download, .forward] : [.share, .delete]) { action in
                    switch action {
                    case .share: shareSelection()
                    case .delete: confirmingDelete = true
                    case .download: downloadSelection()
                    case .forward: shareSelection() // 转发与发送同形:确认 → 选设备弹层
                    }
                }
            }
        }
        .sheet(item: $pickedForSend) { picked in
            if let repo = model.repository {
                DevicePickerSheet(
                    title: "\(isRemote ? "转发" : "发送") \(picked.items.count) 项到…",
                    repo: repo,
                    selfDeviceId: model.selfDeviceId,
                    excluding: isRemote ? vm.deviceId : nil, // 转发 B ≠ 来源 A(daemon 拒 from==to,前置滤除)
                ) { device in
                    if isRemote {
                        startForward(picked.items, to: device)
                    } else {
                        startSend(picked.items, to: device)
                    }
                }
            }
        }
        .confirmationDialog(
            "删除 \(vm.selection.count) 项?",
            isPresented: $confirmingDelete,
            titleVisibility: .visible,
        ) {
            Button("删除", role: .destructive) { Task { await deleteSelection() } }
            Button("取消", role: .cancel) {}
        } message: {
            Text("所选项目将被永久删除,此操作不可恢复。")
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
        .task {
            // 初次进页拉浏览根(错误态不自动重拉,重试按钮承担)。首页子行
            // 直入(R4 用户裁定)= 子目录即浏览根(VM init 定根),不经本面
            if vm.entries.isEmpty && vm.errorText == nil && !vm.loading {
                await vm.loadRoot()
            }
        }
        #if DEBUG
        .task { await debugE2EIfHooked() }
        #endif
    }

    // MARK: 选择流(spec §4.2 确认语义 + 底部动作;Ruling O:确认只退出,
    //         动作仅自底栏发起——先捕获 selectedEntries 再退出选择模式)

    /// 底栏分享/转发 = 捕获选择集 → 退出选择模式 → 选设备弹层。
    private func shareSelection() {
        let items = vm.selectedEntries
        guard !items.isEmpty else { return }
        vm.exitSelection()
        pickedForSend = PickedSelection(items: items)
    }

    /// 删除选中项(确认弹窗后;iOS 引擎恒永久删)。
    private func deleteSelection() async {
        guard let result = await vm.deleteSelected() else { return } // 失败走 lastActionError
        guard !result.failed.isEmpty else { return } // 全成静默
        deleteNotice = "已删除 \(result.deleted) 项,\(result.failed.count) 项失败:\(result.failed[0].message)"
    }

    /// 发送到设备(选设备弹层点选后;选择态已在动作面收)。发起成功
    /// toast(R3 #3);失败提示,dir 由 SendService 按对端平台解析(Ruling I)。
    private func startSend(_ items: [DirEntry], to device: DirectoryDevice) {
        guard let send = sendService, !sending else { return }
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

    /// 转发选中项到设备 B(远端档;remote_copy 调度,数据不经操作端;B 端
    /// dir 由 SendService 按 B 平台解析)。B 离线/不支持在弹层/动作面拦截,
    /// 不可达错误走 lastActionError 提示。
    private func startForward(_ items: [DirEntry], to device: DirectoryDevice) {
        guard let send = sendService, !sending else { return }
        sending = true
        let paths = items.map(\.path)
        let from = vm.deviceId
        Task {
            defer { sending = false }
            do {
                _ = try await send.forward(from: from, paths: paths, to: device)
                toast = "已开始 \(paths.count) 项 · 进度见传输列表"
            } catch {
                vm.lastActionError = BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    /// 下载选中项到本机收件箱(远端档;dir = 本机 recv_dir,SendService 现拉)。
    private func downloadSelection() {
        guard let send = sendService, !sending else { return }
        let paths = vm.selectedEntries.map(\.path)
        guard !paths.isEmpty else { return }
        vm.exitSelection() // Ruling O:动作发起即退出选择(捕获已在上两行)
        sending = true
        let device = vm.deviceId
        Task {
            defer { sending = false }
            do {
                _ = try await send.download(device: device, paths: paths)
                toast = "已开始 \(paths.count) 项 · 进度见传输列表"
            } catch {
                vm.lastActionError = BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    // MARK: 「···」菜单(选择 + 排序单选勾)

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

    // MARK: 搜索框(mockup .search:高 40,圆角 11,systemGray6 底)

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

    // MARK: 列表区(错误态/加载态/列表+沉底尾注)

    @ViewBuilder private var listArea: some View {
        if let error = vm.errorText, vm.entries.isEmpty {
            ContentUnavailableView {
                Label("无法加载目录", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await vm.reload() } }
            }
        } else if vm.loading && vm.entries.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else {
            SinkingFooterList {
                ForEach(vm.visibleRows) { row in
                    BrowseEntryRow(
                        entry: row.entry,
                        depth: row.depth,
                        expanded: vm.expandedPaths.contains(row.entry.path),
                        selecting: vm.isSelecting,
                        selected: vm.selection.contains(row.entry.path),
                        onOpen: {
                            if vm.isSelecting {
                                vm.toggle(row.entry.path)
                            } else if row.entry.isDir {
                                Task { await vm.open(row.entry.path) }
                            }
                        },
                        // 箭头行内展开(R3 #8);文件行无(箭头仅占位对齐)
                        onToggleExpand: row.entry.isDir
                            ? { Task { await vm.toggleExpand(row.entry.path) } }
                            : nil,
                    )
                    .simultaneousGesture(
                        LongPressGesture(minimumDuration: 0.4).onEnded { _ in
                            if !vm.isSelecting { vm.enterSelection() }
                        },
                    )
                }
            } footer: {
                ItemCountFooter(count: vm.itemCount, note: vm.truncated ? "已截断" : nil)
            }
            // 展开态变化驱动箭头旋转与子行插拔动画(R3 #8);选择态进/出
            // 驱动选圈列宽与圈滑入/滑出动画(R4 #3)——两值各自 scoped
            // `.animation(_:value:)`,互不串扰(展开动画不吃选择翻转面)
            .animation(.easeInOut(duration: 0.2), value: vm.expandedPaths)
            .animation(.easeInOut(duration: 0.2), value: vm.isSelecting)
            // 下拉刷新(R4 真机轮 #4;R5a 修#4):···同口 reload 单口,但经
            // 非结构化 Task 承载——SwiftUI 对 refreshable 任务的所有权可因
            // 视图重建/用户离场中途取消,取消曾落到兜底文案伪装成「无法加载
            // 目录·连接中断」并清空列表(VM 侧另有良性取消收口兜底)。解绑
            // 后下拉语义与「···」刷新完全一致:总跑完,转圈随真实完成收。
            .refreshable { await Task { await vm.reload() }.value }
        }
    }

    // MARK: DEBUG E2E 钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    #if DEBUG
    /// MTDSS_DEBUG_AUTOSEND=<device_id>[|<platform>]:选前 2 个文件→确认→
    /// 直发(vm 选择语义 + SendService.send 全链;platform 缺省 windows);
    /// MTDSS_DEBUG_AUTODL=<device_id>|<远端绝对路径>:SendService.download
    /// (本机收件箱);MTDSS_DEBUG_AUTOFWD=<to_id>|<to_platform>:选前 2 项
    /// →确认→转发(vm.deviceId = 来源 A)。均等终态后 reload。
    /// MTDSS_DEBUG_SELECT=1:进入选择态并选 2 项后停留(截图面);
    /// MTDSS_DEBUG_PICKER=1:同上且确认进选设备弹层后停留(截图面)。
    private func debugE2EIfHooked() async {
        let env = ProcessInfo.processInfo.environment
        guard let send = sendService else { return }
        // MTDSS_DEBUG_REMOTE_OPEN=<绝对路径>:推入页直开子目录(远端浏览
        // 截图/E2E 面;模拟器无触点驱动)
        if let open = env["MTDSS_DEBUG_REMOTE_OPEN"], !open.isEmpty {
            await waitForEntries()
            await vm.open(open)
        }
        if env["MTDSS_DEBUG_SELECT"] == "1" {
            await debugEnterSelection(confirmToPicker: false)
        }
        if env["MTDSS_DEBUG_PICKER"] == "1" {
            await debugEnterSelection(confirmToPicker: true)
        }
        // R3 #3 截图面:直接亮 toast(动作发起反馈胶囊形态)
        if env["MTDSS_DEBUG_TOAST"] == "1" {
            await waitForEntries()
            toast = "已开始 2 项 · 进度见传输列表"
        }
        // R3 #8 截图面:两级行内展开(首个目录 + 其首个子目录)
        if env["MTDSS_DEBUG_TREE"] == "1" {
            await debugExpandTwoLevels()
        }
        // R3 #8 截图面:展开态上进入选择(头三个可见行混合层级勾选)
        if env["MTDSS_DEBUG_TREE_SELECT"] == "1" {
            await debugExpandTwoLevels()
            vm.enterSelection()
            vm.visibleRows.prefix(3).forEach { vm.toggle($0.entry.path) }
        }
        if let spec = env["MTDSS_DEBUG_AUTOSEND"], !spec.isEmpty {
            let parts = spec.split(separator: "|", maxSplits: 1).map(String.init)
            await debugSelectAndSend(
                to: DirectoryDevice(deviceId: parts[0], name: parts[0], platform: parts.count > 1 ? parts[1] : "windows", online: true),
                send: send,
            )
        }
        if let spec = env["MTDSS_DEBUG_AUTODL"], !spec.isEmpty {
            let parts = spec.split(separator: "|", maxSplits: 1).map(String.init)
            if parts.count == 2 {
                await debugTransferWait(device: parts[0], path: parts[1], send: send)
            }
        }
        if let spec = env["MTDSS_DEBUG_AUTOFWD"], !spec.isEmpty {
            let parts = spec.split(separator: "|", maxSplits: 1).map(String.init)
            await debugSelectAndForward(
                to: DirectoryDevice(deviceId: parts[0], name: parts[0], platform: parts.count > 1 ? parts[1] : "windows", online: true),
                send: send,
            )
        }
    }

    private func waitForEntries() async {
        for _ in 0..<100 where vm.entries.isEmpty && vm.errorText == nil {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
    }

    /// R3 #8 截图面:展开首个根级目录,再展开其首个子目录(两级行内展开)。
    private func debugExpandTwoLevels() async {
        await waitForEntries()
        guard let root = vm.visibleRows.first(where: { $0.depth == 0 && $0.entry.isDir }) else {
            print("[R3-E2E] TREE FAIL: 根级无目录")
            return
        }
        await vm.toggleExpand(root.entry.path)
        if let child = vm.visibleRows.first(where: { $0.depth == 1 && $0.entry.isDir }) {
            await vm.toggleExpand(child.entry.path)
        }
    }

    /// 截图钩子:选前 2 个文件;confirmToPicker=true 再进选设备弹层并停留。
    private func debugEnterSelection(confirmToPicker: Bool) async {
        await waitForEntries()
        let files = Array(vm.visibleEntries.filter { !$0.isDir }.prefix(2))
        guard files.count == 2 else {
            print("[T6-E2E] SELECT FAIL: 文件不足 2")
            return
        }
        vm.enterSelection()
        files.forEach { vm.toggle($0.path) }
        if confirmToPicker {
            // Ruling O:确认不再携带选择集——底栏同款「先捕获后退出」次序
            let picked = vm.selectedEntries
            guard !picked.isEmpty else {
                print("[T6-E2E] PICKER FAIL: 选择集空")
                return
            }
            vm.exitSelection()
            pickedForSend = PickedSelection(items: picked)
        }
    }

    private func debugSelectAndSend(to device: DirectoryDevice, send: SendService) async {
        await waitForEntries()
        let files = Array(vm.visibleEntries.filter { !$0.isDir }.prefix(2))
        guard files.count == 2 else {
            print("[T6-E2E] AUTOSEND FAIL: 文件不足 2(共 \(vm.visibleEntries.count) 项)")
            return
        }
        vm.enterSelection()
        files.forEach { vm.toggle($0.path) }
        let picked = vm.selectedEntries
        guard picked.count == 2 else {
            print("[T6-E2E] AUTOSEND FAIL: 选择集不足 2")
            return
        }
        vm.exitSelection() // Ruling O:动作面自捕选择集(确认面已无携带语义)
        do {
            let jobId = try await send.send(paths: picked.map(\.path), to: device)
            print("[T6-E2E] AUTOSENT jobId=\(jobId) files=\(picked.map(\.name).joined(separator: ","))")
            await debugWaitJob(jobId)
        } catch {
            print("[T6-E2E] AUTOSEND FAIL: \(error)")
        }
    }

    /// AUTOFWD:选前 2 项 → 确认 → 转发(来源 = vm.deviceId,目标 = env)。
    /// Ruling I 拒发(unix 桌面对端)同样按 FAIL 打印带文案退出(E2E 断言面)。
    private func debugSelectAndForward(to device: DirectoryDevice, send: SendService) async {
        await waitForEntries()
        let items = Array(vm.visibleEntries.filter { !$0.isDir }.prefix(2))
        guard items.count == 2 else {
            print("[T7-E2E] AUTOFWD FAIL: 文件不足 2(共 \(vm.visibleEntries.count) 项)")
            return
        }
        vm.enterSelection()
        items.forEach { vm.toggle($0.path) }
        let picked = vm.selectedEntries
        guard picked.count == 2 else {
            print("[T7-E2E] AUTOFWD FAIL: 选择集不足 2")
            return
        }
        vm.exitSelection() // Ruling O 同上
        do {
            let jobId = try await send.forward(from: vm.deviceId, paths: picked.map(\.path), to: device)
            print("[T7-E2E] AUTOFWD jobId=\(jobId) files=\(picked.map(\.name).joined(separator: ","))")
            await debugWaitJob(jobId)
        } catch {
            print("[T7-E2E] AUTOFWD FAIL: \(error)")
        }
    }

    private func debugTransferWait(device: String, path: String, send: SendService) async {
        do {
            let jobId = try await send.download(device: device, paths: [path])
            print("[T6-E2E] AUTODL START jobId=\(jobId) path=\(path)")
            await debugWaitJob(jobId)
        } catch {
            print("[T6-E2E] AUTODL FAIL: \(error)")
        }
    }

    /// 等作业终态(transfer.wait 限时 180s;终态后刷新列表)。
    private func debugWaitJob(_ jobId: String) async {
        guard let repo = model.repository else { return }
        if let result = try? await RpcGuard.withTimeout(seconds: 180) { [repo] in
            try await repo.transferWait(id: jobId)
        } {
            print("[T6-E2E] JOB \(jobId) state=\(result.state) error=\(result.error ?? "-")")
        }
        await vm.reload()
    }
    #endif
}

/// 浏览行(mockup .frow:高 58,行首内缩 20;文件夹左侧蓝 ›,文件保留
/// 箭头占位对齐名称列;细分割线自行首起)。行内展开(R3 真机轮 #8):
/// depth = 缩进层级(根 0,子级 +1;每层 ``BrowseViewModel/indentPerLevel``
/// pt,分割线同步内缩——系统 outline 手感),目录行箭头 = 展开钮(展开态
/// › 旋转 90° 成 ∨,触区外扩 28pt 方、布局占位还原),行点按仍 = 进整页。
/// 选择模式(spec §4.2):选圈钉行首固定缘(20pt 内缩处,不随缩进漂移;
/// R4 真机轮 #3),选中=蓝底白勾,选中行浅灰底;点按=切换选中(不开
/// 目录);进/出选择 = 圈左移淡入/出 + 行内容右移让位(全行一致列宽)。
struct BrowseEntryRow: View {
    let entry: DirEntry
    /// 缩进层级(行内展开;根级 0)。
    var depth: Int = 0
    /// 目录行展开态(箭头旋转 ∨)。
    var expanded: Bool = false
    var selecting: Bool = false
    var selected: Bool = false
    var onOpen: () -> Void
    /// 目录行的箭头点按 = 行内展开;nil = 文件行(箭头仅占位)。
    var onToggleExpand: (() -> Void)? = nil

    private var indent: CGFloat {
        CGFloat(depth) * BrowseViewModel.indentPerLevel
    }

    var body: some View {
        Button(action: onOpen) {
            FullBleedRow(separatorLeadingInset: 20 + indent) {
                HStack(spacing: 0) {
                    // 选圈列(R4 真机轮 #3):固定宽列、所有行一致——圈钉在
                    // 行首固定缘(20pt 内缩处),不随树缩进漂移(展开子级间
                    // 无跳面)。非选择态列宽 = 20(恰原行首内缩,布局与
                    // mockup 逐像素同);选择态 = 56(20 内缩 + 24 圈 + 12
                    // 间距),行内容整体右移让位(iOS 文件 App 手感)。圈
                    // 滑入 = 左移淡入 transition,与列宽展开同拍动画。
                    ZStack(alignment: .leading) {
                        if selecting {
                            selectionCircle
                                .padding(.leading, 20)
                                .transition(.opacity.combined(with: .offset(x: -12)))
                        }
                    }
                    .frame(width: selecting ? 56 : 20, alignment: .leading)
                    HStack(spacing: 12) {
                        chevron
                        FileTypeIcon(isDir: entry.isDir)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(entry.name)
                                .font(.system(size: 17))
                                .foregroundStyle(Color.primary)
                                .lineLimit(1)
                            // T5b 接线:契约 children(目录项直接子项数)→ 副文
                            // 「日期 - N 项」;文件项/未知(nil)退化纯日期。
                            let sub = BrowseFormat.subtitle(entry, childCount: entry.children.map { Int($0) })
                            if !sub.isEmpty {
                                Text(sub)
                                    .font(.system(size: 12.5))
                                    .foregroundStyle(Color(uiColor: .systemGray))
                                    .lineLimit(1)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.leading, indent)
                }
                .padding(.trailing, 20)
            }
            .background(selected ? Color(uiColor: .systemGray6) : Color.clear)
            // 整行触区(R3 真机轮 #1/#5:行透明区——Spacer/分割线留白——
            // 不在默认命中面内,真机只有文字/箭头可点;contentShape 补全行
            // 矩形:行点按(Button)与行长按(外挂 simultaneousGesture)
            // 同吃整行;与页级左缘右滑 DragGesture 不冲突面——tap 是点、
            // 边缘返回是拖,几何谓词各判各的)。
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(entry.name + (selected ? ",已选" : "")))
    }

    /// 行首箭头。目录 = 展开钮:› 展开态旋转 90° 成 ∨(蓝,spec §4.1 色
    /// 不变);触区外扩 28pt 方(12pt 字形直点太窄,真机「命中面窄」同课),
    /// 负 padding 还原布局占位 12pt(视觉与 mockup 像素不变)。文件 =
    /// 12pt 占位对齐名称列。
    @ViewBuilder private var chevron: some View {
        if entry.isDir, let onToggleExpand {
            Button(action: onToggleExpand) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Color.accentColor)
                    .frame(width: 12)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                    .padding(8)
                    .contentShape(Rectangle())
                    .padding(.horizontal, -8)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(expanded ? "收起" : "展开"))
        } else {
            Image(systemName: "chevron.right")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Color.accentColor)
                .frame(width: 12)
                .opacity(entry.isDir ? 1 : 0)
        }
    }

    /// 行首选圈(mockup .ck:24pt,1.5 灰描边;选中=蓝底白勾)。
    private var selectionCircle: some View {
        ZStack {
            if selected {
                Circle().fill(Color.accentColor)
                Image(systemName: "checkmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(.white)
            } else {
                Color.clear
                Circle()
                    .strokeBorder(Color(uiColor: .systemGray4), lineWidth: 1.5)
                    .background(Color.white, in: Circle())
            }
        }
        .frame(width: 24, height: 24)
    }
}

/// 排序字段的菜单文案(「···」→ 排序)。
extension BrowseSort {
    var label: String {
        switch self {
        case .name: "名称"
        case .date: "日期"
        case .size: "大小"
        case .kind: "种类"
        }
    }
}
