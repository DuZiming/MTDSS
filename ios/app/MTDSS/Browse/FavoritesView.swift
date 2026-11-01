/// 第一标签「常用目录」根页(2026-11-01 Ruling N:唯一展示项 = 收件箱。
/// 授权集 = 收件箱(默认)+ 信任目录(白名单)——信任目录只进引擎授权集
/// (engine-roots.conf)与设置页管理列表,不投影首页)。数据源 =
/// AuthorizedRoot.roots(recvDir:)。根页无返回/前进箭头,仅三点
/// (spec §4.1);点根条目推入浏览页(4.1 模板)。

import MTDSSKit
import SwiftUI

struct FavoritesView: View {
    @Environment(AppModel.self) private var model
    @State private var navPath: [AuthorizedRoot] = []

    /// 首页根列表(Ruling N:收件箱单枚;信任目录不入首页)。
    private var roots: [AuthorizedRoot] {
        AuthorizedRoot.roots(recvDir: model.recvDir)
    }

    var body: some View {
        NavigationStack(path: $navPath) {
            rootPage
                .navigationDestination(for: AuthorizedRoot.self) { root in
                    destination(root)
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
        } else {
            SinkingFooterList {
                ForEach(roots) { root in
                    Button {
                        navPath = [root]
                    } label: {
                        FullBleedRow {
                            HStack(spacing: 12) {
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 14, weight: .semibold))
                                    .foregroundStyle(Color.accentColor)
                                    .frame(width: 12)
                                RootFolderIcon()
                                Text(root.name)
                                    .font(.system(size: 17))
                                    .foregroundStyle(Color.primary)
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 20)
                        }
                    }
                    .buttonStyle(.plain)
                }
            } footer: {
                ItemCountFooter(count: roots.count)
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

    @ViewBuilder private func destination(_ root: AuthorizedRoot) -> some View {
        if let repo = model.repository, let deviceId = model.selfDeviceId {
            BrowseDestination(
                root: root,
                repo: repo,
                deviceId: deviceId,
                onPopToRoot: { navPath = [] },
            )
        } else {
            // 引擎未就绪/未登录(status.device 为 null):给返回路的占位
            // (登录门面归 T14;真机登录后重进即可浏览)。
            VStack(spacing: 0) {
                BrowseNavBar(
                    title: root.name,
                    showsArrows: true,
                    canBack: false,
                    canForward: false,
                    onBack: { navPath = [] },
                    menu: { overflowMenu },
                )
                ContentUnavailableView(
                    "未登录",
                    systemImage: "person.crop.circle",
                    description: Text("登录后即可浏览\(root.name)"),
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
                if navPath.isEmpty { navPath = [inbox] }
                return
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        #endif
    }
}

/// 浏览推入页:每根一枚 VM(StateObject 随页生命周期);showHidden 以
/// 设置开关为源(App 侧,T10),开页取值 + 开关变化活体跟随。
private struct BrowseDestination: View {
    let root: AuthorizedRoot
    let repo: MtdssRepository
    let deviceId: String
    let onPopToRoot: () -> Void

    @Environment(AppModel.self) private var model
    @StateObject private var vm: BrowseViewModel

    init(root: AuthorizedRoot, repo: MtdssRepository, deviceId: String, onPopToRoot: @escaping () -> Void) {
        self.root = root
        self.repo = repo
        self.deviceId = deviceId
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
            sendService: SendService(repo: repo),
        )
        .toolbar(.hidden, for: .navigationBar)
        .task { vm.showHidden = model.showHidden }
        .onChange(of: model.showHidden) { _, hidden in
            vm.showHidden = hidden
        }
    }
}
