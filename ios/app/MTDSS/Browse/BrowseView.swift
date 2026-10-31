/// 浏览页模板(spec §4.1,收件箱/信任目录/远端浏览共用):
/// 顶栏 → 搜索框(灰底圆角+放大镜,按名称过滤当前列表)→ 通栏行
/// (文件夹=左蓝›+蓝文件夹图标;文件=无箭头+问号文档图标;副文
/// 「日期 - N 项」/「日期 - 大小」)→「N 项」沉底。「···」菜单 =
/// 选择 + 排序(名称/日期/大小/种类,单选勾)。
/// **选择模式**(spec §4.2,T6):长按任意行或菜单「选择」进入——顶栏
/// 换 全选/「N 项」/蓝圈白勾确认(0 选确认=退出),行首选圈(选中=蓝底
/// 白勾+行浅灰底),搜索框保留,**标签栏隐藏**,底部动作条(本地=分享/
/// 删除);确认(N>0)→ 选设备弹层 → 发送到对端收件箱。
/// 浏览状态全在 ``BrowseViewModel``(MTDSSKit,宿主可测);本视图只装。

import MTDSSKit
import SwiftUI

struct BrowseView: View {
    @ObservedObject var vm: BrowseViewModel
    /// 推入页显示箭头;标签根页无箭头(仅三点)。
    var showsArrows: Bool
    /// 已在浏览根按返回 → 弹回标签根(常用目录)。
    var onPopToRoot: (() -> Void)? = nil
    /// 本机浏览必给(分享/删除动作);远端浏览 T7 换远端动作集(挂接点)。
    var sendService: SendService? = nil

    @Environment(AppModel.self) private var model
    /// 确认后待发送的选择集(non-nil → 选设备弹层)。
    @State private var pickedForSend: PickedSelection?
    @State private var confirmingDelete = false
    /// 删除结果提示(部分失败明细;纯成功静默)。
    @State private var deleteNotice: String?
    @State private var sending = false

    struct PickedSelection: Identifiable {
        let id = UUID()
        let items: [DirEntry]
    }

    var body: some View {
        VStack(spacing: 0) {
            if vm.isSelecting {
                SelectionNavBar(
                    count: vm.selection.count,
                    visibleCount: vm.itemCount,
                    onSelectAll: { vm.selectAll() },
                    onConfirm: { confirmSelection() },
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
        // 选择模式隐藏标签栏(spec §4.2;推入页在 TabView 的 NavigationStack 内,
        // 系统支持的整页工具栏可见性口径)
        .toolbar(vm.isSelecting ? Visibility.hidden : .visible, for: .tabBar)
        .safeAreaInset(edge: .bottom) {
            if vm.isSelecting {
                SelectionActionBar(actions: [.share, .delete]) { action in
                    switch action {
                    case .share: shareSelection()
                    case .delete: confirmingDelete = true
                    default: break // 远端动作 T7
                    }
                }
            }
        }
        .sheet(item: $pickedForSend) { picked in
            if let repo = model.repository {
                DevicePickerSheet(
                    title: "发送 \(picked.items.count) 项到…",
                    repo: repo,
                    selfDeviceId: model.selfDeviceId,
                ) { device in
                    startSend(picked.items, to: device.deviceId)
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
            // 初次进页拉浏览根(错误态不自动重拉,重试按钮承担)
            if vm.entries.isEmpty && vm.errorText == nil && !vm.loading {
                await vm.loadRoot()
            }
        }
        #if DEBUG
        .task { await debugE2EIfHooked() }
        #endif
    }

    // MARK: 选择流(spec §4.2 确认语义 + 底部动作)

    /// 蓝圈确认:0 选 = 退出选择模式(VM 收口);N 选 = 进选设备弹层。
    private func confirmSelection() {
        switch vm.confirmSelection() {
        case .exited:
            break
        case .picked(let items):
            pickedForSend = PickedSelection(items: items)
        }
    }

    /// 底栏分享 = 与确认同义(发送选中项到所选设备收件箱)。
    private func shareSelection() {
        guard !vm.selection.isEmpty else { return }
        confirmSelection()
    }

    /// 删除选中项(确认弹窗后;iOS 引擎恒永久删)。
    private func deleteSelection() async {
        guard let result = await vm.deleteSelected() else { return } // 失败走 lastActionError
        guard !result.failed.isEmpty else { return } // 全成静默
        deleteNotice = "已删除 \(result.deleted) 项,\(result.failed.count) 项失败:\(result.failed[0].message)"
    }

    /// 发送到设备(选设备弹层点选后;完成静默,失败提示,选择态已收)。
    private func startSend(_ items: [DirEntry], to deviceId: String) {
        guard let send = sendService, !sending else { return }
        sending = true
        let paths = items.map(\.path)
        Task {
            defer { sending = false }
            do {
                _ = try await send.send(paths: paths, to: deviceId)
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
                ForEach(vm.visibleEntries, id: \.path) { entry in
                    BrowseEntryRow(
                        entry: entry,
                        selecting: vm.isSelecting,
                        selected: vm.selection.contains(entry.path),
                        onOpen: {
                            if vm.isSelecting {
                                vm.toggle(entry.path)
                            } else if entry.isDir {
                                Task { await vm.open(entry.path) }
                            }
                        },
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
        }
    }

    // MARK: DEBUG E2E 钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    #if DEBUG
    /// MTDSS_DEBUG_AUTOSEND=<device_id>:选前 2 个文件→确认→直发(vm 选择
    /// 语义 + SendService.send 全链);MTDSS_DEBUG_AUTODL=<device_id>|<远端绝对
    /// 路径>:SendService.download(本机收件箱)。均等终态后 reload。
    /// MTDSS_DEBUG_SELECT=1:进入选择态并选 2 项后停留(截图面);
    /// MTDSS_DEBUG_PICKER=1:同上且确认进选设备弹层后停留(截图面)。
    private func debugE2EIfHooked() async {
        let env = ProcessInfo.processInfo.environment
        guard let send = sendService else { return }
        if env["MTDSS_DEBUG_SELECT"] == "1" {
            await debugEnterSelection(confirmToPicker: false)
        }
        if env["MTDSS_DEBUG_PICKER"] == "1" {
            await debugEnterSelection(confirmToPicker: true)
        }
        if let target = env["MTDSS_DEBUG_AUTOSEND"], !target.isEmpty {
            await debugSelectAndSend(to: target, send: send)
        }
        if let spec = env["MTDSS_DEBUG_AUTODL"], !spec.isEmpty {
            let parts = spec.split(separator: "|", maxSplits: 1).map(String.init)
            if parts.count == 2 {
                await debugTransferWait(device: parts[0], path: parts[1], send: send)
            }
        }
    }

    private func waitForEntries() async {
        for _ in 0..<100 where vm.entries.isEmpty && vm.errorText == nil {
            try? await Task.sleep(nanoseconds: 100_000_000)
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
            guard case .picked(let picked) = vm.confirmSelection() else {
                print("[T6-E2E] PICKER FAIL: 确认未携带选择集")
                return
            }
            pickedForSend = PickedSelection(items: picked)
        }
    }

    private func debugSelectAndSend(to deviceId: String, send: SendService) async {
        await waitForEntries()
        let files = Array(vm.visibleEntries.filter { !$0.isDir }.prefix(2))
        guard files.count == 2 else {
            print("[T6-E2E] AUTOSEND FAIL: 文件不足 2(共 \(vm.visibleEntries.count) 项)")
            return
        }
        vm.enterSelection()
        files.forEach { vm.toggle($0.path) }
        guard case .picked(let picked) = vm.confirmSelection() else {
            print("[T6-E2E] AUTOSEND FAIL: 确认未携带选择集")
            return
        }
        do {
            let jobId = try await send.send(paths: picked.map(\.path), to: deviceId)
            print("[T6-E2E] AUTOSENT jobId=\(jobId) files=\(picked.map(\.name).joined(separator: ","))")
            await debugWaitJob(jobId)
        } catch {
            print("[T6-E2E] AUTOSEND FAIL: \(error)")
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
/// 箭头占位对齐名称列;细分割线自行首起)。选择模式(spec §4.2):行首
/// 24pt 选圈(选中=蓝底白勾),选中行浅灰底;点按=切换选中(不开目录)。
struct BrowseEntryRow: View {
    let entry: DirEntry
    var selecting: Bool = false
    var selected: Bool = false
    var onOpen: () -> Void

    var body: some View {
        Button(action: onOpen) {
            FullBleedRow {
                HStack(spacing: 12) {
                    if selecting {
                        selectionCircle
                    }
                    Image(systemName: "chevron.right")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 12)
                        .opacity(entry.isDir ? 1 : 0)
                    FileTypeIcon(isDir: entry.isDir)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(entry.name)
                            .font(.system(size: 17))
                            .foregroundStyle(Color.primary)
                            .lineLimit(1)
                        let sub = BrowseFormat.subtitle(entry)
                        if !sub.isEmpty {
                            Text(sub)
                                .font(.system(size: 12.5))
                                .foregroundStyle(Color(uiColor: .systemGray))
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 20)
            }
            .background(selected ? Color(uiColor: .systemGray6) : Color.clear)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(entry.name + (selected ? ",已选" : "")))
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
