/// 传输列表页(spec §4.5 传输列表页,mockup transfer-list):
/// - 顶栏:标题绝对居中,根页无箭头,「···」= 清除已完成(spec §4.5);
/// - 分段控件 全部/接收/发送/调度(iOS 原生 segmented 样式);
/// - 行 = 类型徽章(接收蓝/发送绿/调度紫灰)+ 任务名 + 方向设备
///   (「公司台式机 → 本机」)+ 状态——进行中 = 细进度条+速度+百分比
///   (接收蓝/发送绿随徽章),中断/失败 = 红字原因(error 空兜底「已中断」),
///   完成 = 绿勾+时间(仅现场观测到 done 的行,契约无时间戳不编造)+
///   跳过段(R3 #3/#6:skipped>0 →「跳过 N 项(同名已存在)」);
/// - 最新在最上(VM 面保证);左滑:进行中→取消 / 中断·失败→重新发起+清除 /
///   完成·已取消→清除(VM TransfersFormat.actions 按状态出集;R9 #1 起
///   容器 = ``BrowseList``(List)+ 原生 `.swipeActions`,与浏览页同组件
///   同形态——自绘 SwipeableRow 的拖拽行随迁撤除;动作集空集行不示能);
/// - 「N 项」尾注 = List 普通末行(BrowseList 沉底取舍,见组件头)。
/// 事件收敛:event.transfer 增量 + event.devices 目录名(方向设备文案解析);
/// 新行(事件帧不带 name/src/dst)即刻补拉对账;前台回归重拉(引擎对后台
/// 掉线的非终态任务标「中断」,回前台即刻反映)。

import MTDSSKit
import SwiftUI

struct TransfersView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let repo = model.repository {
            TransfersRootView(repo: repo)
        } else {
            // 引擎 boot 未就绪(bootError 横幅由 AppTabView 顶层承担)
            VStack(spacing: 0) {
                BrowseNavBar(title: "传输列表", showsArrows: false) { EmptyView() }
                Spacer()
                ProgressView()
                Spacer()
            }
            .background(Color(uiColor: .systemBackground))
            .toolbar(.hidden, for: .navigationBar)
        }
    }
}

/// 传输列表根页(VM 随 repo 就绪创建,StateObject 随页生命周期)。
private struct TransfersRootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    let repo: MtdssRepository

    @StateObject private var vm: TransfersViewModel
    /// 重试目标弹层(分享转交失败行的「重试」;重选设备即重发)。
    @State private var retryingShare: ShareCenter.FailureRow?
    @State private var pickerPresented = false

    init(repo: MtdssRepository) {
        self.repo = repo
        _vm = StateObject(wrappedValue: TransfersViewModel(repo: repo))
    }

    var body: some View {
        VStack(spacing: 0) {
            BrowseNavBar(title: "传输列表", showsArrows: false) {
                Button("清除已完成") {
                    Task { await vm.clearCompleted() }
                }
                // 可清面 = 有记录面的终态(T5b:done 已恢复清除入口)
                .disabled(!vm.rows.contains { TransfersFormat.isClearable($0.state) })
            }
            segmentBar
            actionErrorBanner
            listBody
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        .task {
            await initialLoad()
            await debugHookActions()
        }
        .task { await consumeEvents() }
        .task { model.share.refreshRows() }
        .onChange(of: scenePhase) { _, phase in
            // 前台回归重拉:后台掉线任务的中断态/进度对账(AppModel 侧另有
            // transferActive 重对齐,各管各的面)
            if phase == .active {
                Task {
                    await vm.load()
                    model.share.refreshRows()
                }
            }
        }
        .sheet(isPresented: $pickerPresented) {
            // 重试 = 重选目标设备(spec:失败条目可重试,重选即重发)
            DevicePickerSheet(
                title: "重发「\(retryingShare?.name ?? "")」到…",
                repo: repo, selfDeviceId: model.selfDeviceId,
            ) { device in
                guard let row = retryingShare else { return }
                Task {
                    await model.share.retry(
                        id: row.id, targetDeviceId: device.deviceId, repository: repo)
                }
            }
        }
    }

    // MARK: 分段控件

    private var segmentBar: some View {
        Picker("筛选", selection: $vm.filter) {
            ForEach(TransferFilter.allCases) { f in
                Text(f.label).tag(f)
            }
        }
        .pickerStyle(.segmented)
        .padding(.horizontal, 20)
        .padding(.top, 2)
        .padding(.bottom, 10)
    }

    // MARK: 动作失败横幅(一次性;点按清除)

    @ViewBuilder private var actionErrorBanner: some View {
        if let error = vm.lastActionError {
            Button {
                vm.lastActionError = nil
            } label: {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.white)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(.red.opacity(0.85), in: Capsule())
            }
            .padding(.bottom, 6)
        }
    }

    // MARK: 列表体

    /// 分享转交失败行(T11;随分段——出站语义归 全部/发送)。
    private var visibleShareRows: [ShareCenter.FailureRow] {
        model.share.failedRows.filter { _ in vm.filter == .all || vm.filter == .send }
    }

    @ViewBuilder private var listBody: some View {
        if vm.loading && vm.rows.isEmpty && visibleShareRows.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else if let error = vm.errorText, vm.rows.isEmpty, visibleShareRows.isEmpty {
            ContentUnavailableView {
                Label("无法加载传输列表", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await vm.load() } }
            }
        } else if vm.visibleRows.isEmpty && visibleShareRows.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ContentUnavailableView(
                    vm.rows.isEmpty ? "暂无传输" : "该分类暂无任务",
                    systemImage: "arrow.left.arrow.right",
                    description: Text(vm.rows.isEmpty ? "发送/接收的任务会出现在这里" : "换个分段看看"),
                )
                Spacer()
            }
        } else {
            // R9 #1 容器迁 List(``BrowseList``,与浏览页同容器同形态):原生
            // swipeActions 仅 List 生效;尾注 = 普通末行(沉底取舍记录在
            // ``BrowseList`` 头)。传输页原无下拉刷新(前台回归 + 事件增量
            // 承担对账),迁移不加新面。
            BrowseList {
                ForEach(visibleShareRows) { row in
                    ShareFailureRowView(row: row) { action in
                        performShare(action, row)
                    }
                }
                ForEach(vm.visibleRows, id: \.id) { job in
                    TransferRowView(
                        job: job,
                        rows: vm.rows,
                        direction: vm.directionText(for: job),
                        doneTime: vm.completionTime(of: job.id),
                    ) { action in
                        Task { await perform(action, job.id) }
                    }
                }
                ItemCountFooter(count: vm.visibleRows.count + visibleShareRows.count)
            }
        }
    }

    private func perform(_ action: TransferRowAction, _ id: String) async {
        switch action {
        case .cancel: await vm.cancel(id)
        case .reinitiate: await vm.reinitiate(id)
        case .clear: await vm.clear(id)
        }
    }

    /// 分享失败行动作:重试 = 重选目标弹层;清除 = 队列项移除(暂存文件
    /// 留收件箱,用户文件不触碰)。
    private func performShare(_ action: TransferRowAction, _ row: ShareCenter.FailureRow) {
        switch action {
        case .reinitiate:
            retryingShare = row
            pickerPresented = true
        case .clear:
            model.share.clear(id: row.id)
        default:
            break
        }
    }

    // MARK: 装配

    /// 首进现拉 + 设备目录名一次现拉(方向设备文案解析面;后续随
    /// event.devices 增量跟线)。
    private func initialLoad() async {
        if vm.rows.isEmpty && vm.errorText == nil && !vm.loading {
            await vm.load()
        }
        if let devices = try? await RpcGuard.withTimeout(seconds: 10) { [repo] in
            try await repo.devices()
        } {
            vm.deviceNames = Self.names(devices.devices)
        }
    }

    /// 事件消费环(event.transfer 增量 / event.devices 目录名;同源不抢流)。
    private func consumeEvents() async {
        guard let events = model.subscribeEvents() else { return }
        for await event in events {
            switch event {
            case .transfer(let p):
                // 新行 = 事件帧不带 name/src/dst → 补拉对账(显示名/方向就位)
                if vm.apply(p) {
                    Task { await vm.load() }
                }
            case .devices(let list):
                vm.deviceNames = Self.names(list)
            default:
                break
            }
        }
    }

    private static func names(_ devices: [DirectoryDevice]) -> [String: String] {
        Dictionary(devices.map { ($0.deviceId, $0.name ?? $0.deviceId) }, uniquingKeysWith: { _, last in last })
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    /// MTDSS_DEBUG_T9="resume"|"clear":等中断行出现后对其调 reinitiate
    /// (与左滑「重新发起」同一 VM 入口)/ 等完成行出现后调 clearCompleted
    /// (与「···」清除已完成同一入口)——左滑手势/菜单本身无模拟器驱动,
    /// 动作链路经此实跑。
    private func debugHookActions() async {
        #if DEBUG
        // R3 #3 截图面:合成 skipped>0 完成行(经 apply 事件帧同链路;
        // 仅截图驱动,真机/产线不编译)
        if ProcessInfo.processInfo.environment["MTDSS_DEBUG_SKIPROW"] == "1" {
            _ = vm.apply(TransferProgress(
                id: "debug-skip", kind: "upload", state: "done",
                filesDone: 3, filesTotal: 5, bytesDone: 10, bytesTotal: 10,
                speedBps: 0, skipped: 2, relay: false, error: nil,
            ))
            return
        }
        let spec = ProcessInfo.processInfo.environment["MTDSS_DEBUG_T9"] ?? ""
        guard spec == "resume" || spec == "clear" else { return }
        for _ in 0..<600 { // 上界 60s
            if spec == "resume", let job = vm.rows.first(where: { $0.state == "interrupted" }) {
                await vm.reinitiate(job.id)
                return
            }
            if spec == "clear", vm.rows.contains(where: { $0.state == "done" }) {
                await vm.clearCompleted()
                return
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        #endif
    }
}

// MARK: - 行(徽章 + 名 + 方向 + 状态 + 左滑)

private struct TransferRowView: View {
    let job: TransferJob
    let rows: [TransferJob]
    let direction: String
    let doneTime: Date?
    let onAction: (TransferRowAction) -> Void

    private var tint: Color { TransferRowStyle.tint(for: job.kind) }
    private var inFlight: Bool { !TransfersFormat.isTerminal(job.state) }

    var body: some View {
        FullBleedRow {
            HStack(spacing: 12) {
                badge
                VStack(alignment: .leading, spacing: 3) {
                    Text(TransfersFormat.displayName(job))
                        .font(.system(size: 16.5))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    subtitle
                    if inFlight { progressBar }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                trailing
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
        }
        // 原生左滑(R9 #1,与浏览行同组件):动作集按状态(空集行不示能,
        // 如会话行终态/同签名在途抑制);VM busy 防连点收在 control 面
        // (原自绘行的在途钮半透明视觉不迁——原生动作钮无此挂点,防连点
        // 语义不变)。
        .modifier(NativeSwipeActions(
            actions: TransfersFormat.actions(for: job, rows: rows),
            spec: { $0.swipeSpec },
            onAction: onAction,
        ))
    }

    /// 类型徽章(接收蓝/发送绿/调度紫灰;底 = 前景 12% 透明度,深浅色随系统)。
    private var badge: some View {
        Text(TransfersFormat.badgeLabel(for: job.kind))
            .font(.system(size: 11, weight: .bold))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
    }

    /// 副文:方向设备 + 状态段(速度/时间/红因/终态词,按状态分档)。
    private var subtitle: some View {
        Text(subtitleText)
            .font(.system(size: 12.5))
            .foregroundStyle(subtitleColor)
            .lineLimit(1)
    }

    private var subtitleText: String {
        var parts = [direction]
        switch job.state {
        case "running":
            parts.append(TransfersFormat.speedText(job.speedBps))
        case "paused":
            parts.append(TransfersFormat.stateText(job.state, error: job.error))
        case "publishing":
            parts.append(TransfersFormat.stateText(job.state, error: job.error))
        case "interrupted", "failed":
            if let cause = TransfersFormat.causeText(job.state, error: job.error) {
                parts = [cause] // 红因整段承载(spec:中断=红字原因)
            }
        case "done":
            if let doneTime, let text = TransferRowStyle.timeText(doneTime) {
                parts.append(text)
            }
            // 跳过段(R3 真机轮 #3/#6):同名已存在的重复机制直答在副文
            if let skipped = TransfersFormat.skippedText(job.skipped) {
                parts.append(skipped)
            }
        case "cancelled":
            parts.append(TransfersFormat.stateText(job.state, error: job.error))
        default:
            break
        }
        if job.relay { parts.append("中继") }
        return parts.joined(separator: " · ")
    }

    private var subtitleColor: Color {
        (job.state == "interrupted" || job.state == "failed")
            ? Color(uiColor: .systemRed)
            : Color(uiColor: .systemGray)
    }

    /// 细进度条(进行中;总量未知 → 不确定态,总量已知 → 徽章色填充)。
    @ViewBuilder private var progressBar: some View {
        if let ratio = TransfersFormat.progressRatio(bytesDone: job.bytesDone, bytesTotal: job.bytesTotal) {
            Capsule()
                .fill(Color(uiColor: .systemFill))
                .frame(height: 4)
                .overlay(alignment: .leading) {
                    GeometryReader { geo in
                        Capsule()
                            .fill(tint)
                            .frame(width: geo.size.width * ratio)
                    }
                }
                .clipShape(Capsule())
                .animation(.linear(duration: 0.2), value: ratio)
        } else {
            ProgressView()
                .progressViewStyle(.linear)
                .tint(tint)
        }
    }

    /// 右缘:进行中 = 徽章色百分比;完成 = 绿勾(+时间在副文);其余空。
    @ViewBuilder private var trailing: some View {
        switch job.state {
        case "queued", "running", "paused", "publishing":
            if let ratio = TransfersFormat.progressRatio(bytesDone: job.bytesDone, bytesTotal: job.bytesTotal) {
                Text("\(Int((ratio * 100).rounded()))%")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(tint)
            } else {
                Text(TransfersFormat.stateText(job.state, error: job.error))
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color(uiColor: .systemGray))
            }
        case "done":
            Image(systemName: "checkmark")
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(Color(uiColor: .systemGreen))
                .accessibilityLabel("完成")
        default:
            EmptyView()
        }
    }
}

/// 分享转交失败行(T11,spec §4.7:拷贝中断/校验不过不静默丢——失败态
/// 条目上浮传输列表,可重试/清除;渲染复用失败态语言:红因整段 + 红档)。
struct ShareFailureRowView: View {
    let row: ShareCenter.FailureRow
    let onAction: (TransferRowAction) -> Void

    var body: some View {
        FullBleedRow {
            HStack(spacing: 12) {
                badge
                VStack(alignment: .leading, spacing: 3) {
                    Text(displayName)
                        .font(.system(size: 16.5))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)
                    Text(causeText)
                        .font(.system(size: 12.5))
                        .foregroundStyle(Color(uiColor: .systemRed))
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
        }
        .modifier(NativeSwipeActions(
            actions: [.reinitiate, .clear],
            spec: { $0.swipeSpec },
            onAction: onAction,
        ))
    }

    private var displayName: String {
        row.filesTotal > 1 ? "\(row.name) · \(row.filesTotal) 项" : row.name
    }

    /// 红因(spec §4.5「失败=红字原因」;「分享转交失败」总冠 + 明细)。
    private var causeText: String {
        let detail = row.reason.flatMap { $0.isEmpty ? nil : $0 } ?? "未能转交"
        return "分享转交失败 · \(detail)"
    }

    /// 类型徽章(灰档「分享」;与引擎任务的收发/调度徽章区分)。
    private var badge: some View {
        Text("分享")
            .font(.system(size: 11, weight: .bold))
            .foregroundStyle(Color(uiColor: .systemGray))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Color(uiColor: .systemGray).opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
    }
}

/// 行样式常量收口(徽章/进度条着色 + 完成时间格式;mockup transfer-list)。
enum TransferRowStyle {
    /// 徽章/进度条/百分比同色:接收蓝/发送绿/调度紫灰(indigo 近 mockup
    /// #5e548e;未知灰档防御)。
    static func tint(for kind: String) -> Color {
        switch TransfersFormat.badgeLabel(for: kind) {
        case "发送": Color(uiColor: .systemGreen)
        case "接收": .accentColor
        case "调度": .indigo
        default: Color(uiColor: .systemGray)
        }
    }

    private static let formatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        f.locale = Locale(identifier: "en_US_POSIX")
        return f
    }()

    static func timeText(_ date: Date) -> String? {
        formatter.string(from: date)
    }
}

// MARK: - 左滑动作(R9 #1 起原生 swipeActions,组件在 Common/ListRows.swift;
//        传输行只出动作规格:重新发起蓝/清除红/取消灰)。

/// 传输行动作钮规格(mockup 同色;与浏览页 swipeSpec 同一 SwipeActionSpec)。
extension TransferRowAction {
    var swipeSpec: SwipeActionSpec {
        switch self {
        case .cancel: SwipeActionSpec(symbol: "xmark", tint: Color(uiColor: .systemGray), label: "取消")
        case .reinitiate: SwipeActionSpec(symbol: "arrow.clockwise", tint: .accentColor, label: "重新发起")
        case .clear: SwipeActionSpec(symbol: "trash", tint: Color(uiColor: .systemRed), label: "清除")
        }
    }
}
