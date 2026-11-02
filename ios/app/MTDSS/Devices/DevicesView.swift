/// 设备页(spec §4.4 设备列表根, mockup device-pages ② 设备列表根):
/// 行 = 设备图标(显示器/手机/终端) + 名称 + 「平台 · 在线/离线」 +
/// 右侧状态点(绿/灰)——与 DevicePickerRow 同语言(spec §4.3「设备行同设备页
/// 语言」,组件复用);离线整行置灰不可进;点在线设备推入远端浏览(4.1 模板,
/// 标题 = 设备名|目录名(R4 #5)。根页无返回/前进箭头,「···」= 刷新(spec
/// §4.1:设备列表根页为「刷新」)。
/// 注:spec 副文「平台 · 在线/最近时间」的最近时间契约未携带(devices 仅
/// device_id/name/platform/online),渲染「平台 · 在线/离线」(同 picker,
/// 不编造数据;契约面见 task-7 报告 NEEDS_CONTEXT)。

import MTDSSKit
import SwiftUI

struct DevicesView: View {
    @Environment(AppModel.self) private var model
    @State private var navPath: [RemoteBrowseTarget] = []

    var body: some View {
        NavigationStack(path: $navPath) {
            if let repo = model.repository {
                DevicesRootView(repo: repo, selfDeviceId: model.selfDeviceId) { target in
                    navPath.append(target)
                }
                .navigationDestination(for: RemoteBrowseTarget.self) { target in
                    RemoteBrowseDestination(target: target, repo: repo) {
                        navPath = []
                    }
                }
            } else {
                // 引擎 boot 未就绪(bootError 横幅由 AppTabView 顶层承担)
                VStack(spacing: 0) {
                    BrowseNavBar(title: "设备", showsArrows: false) { EmptyView() }
                    Spacer()
                    ProgressView()
                    Spacer()
                }
                .background(Color(uiColor: .systemBackground))
                .toolbar(.hidden, for: .navigationBar)
            }
        }
        .task {
            await model.refreshStatus()
            await pushIfDebugHooked()
        }
    }

    // MARK: DEBUG 冒烟钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    /// MTDSS_DEBUG_AUTOPUSH="device[:platform]|device:id=<device_id>":等设备
    /// 列表就绪后推入首台在线设备(platform 前缀过滤,如 "device:windows";
    /// id= 全匹配钉单台,多台同平台账号的确定性 E2E 用)。
    private func pushIfDebugHooked() async {
        #if DEBUG
        let spec = ProcessInfo.processInfo.environment["MTDSS_DEBUG_AUTOPUSH"] ?? ""
        guard spec.hasPrefix("device") else { return }
        let arg = spec.split(separator: ":", maxSplits: 1).dropFirst().first.map(String.init) ?? ""
        let idPin = arg.hasPrefix("id=") ? arg.dropFirst(3) : nil
        let wanted = idPin == nil ? arg : ""
        for _ in 0..<100 {
            if let repo = model.repository, navPath.isEmpty {
                let vm = DevicesViewModel(repo: repo, selfDeviceId: model.selfDeviceId)
                await vm.load()
                let hit = vm.devices.first(where: { device in
                    guard device.online else { return false }
                    if let idPin { return device.deviceId == idPin }
                    return device.platform?.lowercased().hasPrefix(wanted) == true
                })
                if let device = hit {
                    navPath = [RemoteBrowseTarget(device: device)]
                    return
                }
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        #endif
    }
}

/// 远端浏览导航目标(NavigationStack 值路由;DirectoryDevice 无 Hashable,
/// App 侧薄包装不外溢)。
struct RemoteBrowseTarget: Hashable {
    let deviceId: String
    let name: String?
    let platform: String?

    init(device: DirectoryDevice) {
        deviceId = device.deviceId
        name = device.name
        platform = device.platform
    }
}

/// 设备列表根页(VM 随 repo 就绪创建,StateObject 随页生命周期)。
private struct DevicesRootView: View {
    @Environment(AppModel.self) private var model
    let repo: MtdssRepository
    let selfDeviceId: String?
    let onOpen: (RemoteBrowseTarget) -> Void

    @StateObject private var vm: DevicesViewModel

    init(repo: MtdssRepository, selfDeviceId: String?, onOpen: @escaping (RemoteBrowseTarget) -> Void) {
        self.repo = repo
        self.selfDeviceId = selfDeviceId
        self.onOpen = onOpen
        _vm = StateObject(wrappedValue: DevicesViewModel(repo: repo, selfDeviceId: selfDeviceId))
    }

    var body: some View {
        VStack(spacing: 0) {
            BrowseNavBar(title: "设备", showsArrows: false) {
                Button("刷新") {
                    Task { await vm.load() }
                }
            }
            deviceList
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        .task {
            // 首进现拉;status 投影晚到经 onChange 重滤(selfDeviceId 实时面)
            if vm.rawDevices.isEmpty && vm.errorText == nil && !vm.loading {
                vm.selfDeviceId = model.selfDeviceId
                await vm.load()
            }
        }
        .onChange(of: model.selfDeviceId) { _, id in
            vm.selfDeviceId = id
        }
        .task {
            // event.devices 秒级收敛(T6 缓办携带:在线/离线翻转随目录事件跟线)
            guard let events = model.subscribeEvents() else { return }
            for await event in events {
                if case .devices(let list) = event {
                    vm.apply(list)
                }
            }
        }
    }

    @ViewBuilder private var deviceList: some View {
        if let error = vm.errorText {
            ContentUnavailableView {
                Label("无法加载设备", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await vm.load() } }
            }
        } else if vm.loading && vm.devices.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else if vm.devices.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ContentUnavailableView(
                    "暂无其他设备",
                    systemImage: "display",
                    description: Text("其他设备登录同一账号后出现在这里"),
                )
                Spacer()
            }
        } else {
            SinkingFooterList {
                ForEach(vm.devices, id: \.deviceId) { device in
                    DevicePickerRow(device: device) {
                        guard device.online else { return }
                        onOpen(RemoteBrowseTarget(device: device))
                    }
                }
            } footer: {
                // 「N 项」沉底;daemon 回退缓存时注记(T3 devices stale 语义)
                ItemCountFooter(count: vm.devices.count, note: vm.stale ? "缓存" : nil)
            }
        }
    }
}

/// 远端浏览推入页(4.1 模板远端档:标题 = 设备名|目录名,R4 #5;浏览树根 =
/// 卷列表,契约 list path 空串语义;android BrowseScreen 同口径)。
/// T13b 修#5:点行即推入(导航同步,不待任何网络面),首拉在飞期间
/// BrowseView listArea 呈 ProgressView(远端冷握手秒级~35s 全程有 loading)。
private struct RemoteBrowseDestination: View {
    let target: RemoteBrowseTarget
    let repo: MtdssRepository
    let onPopToRoot: () -> Void

    @StateObject private var vm: BrowseViewModel

    init(target: RemoteBrowseTarget, repo: MtdssRepository, onPopToRoot: @escaping () -> Void) {
        self.target = target
        self.repo = repo
        self.onPopToRoot = onPopToRoot
        _vm = StateObject(wrappedValue: BrowseViewModel(
            repo: repo,
            deviceId: target.deviceId,
            rootPath: "", // 契约 list path 空串 = 根卷列表(daemon ctrl 面)
            rootTitle: target.name ?? target.deviceId,
            deviceName: target.name ?? target.deviceId, // R4 #5:标题 = 设备名|目录名
        ))
    }

    var body: some View {
        BrowseView(
            vm: vm,
            showsArrows: true,
            onPopToRoot: onPopToRoot, // 浏览根按返回 → 弹回设备列表(自定义顶栏,系统返回不可用)
            sendService: SendService(repo: repo),
            isRemote: true,
        )
        .toolbar(.hidden, for: .navigationBar)
    }
}
