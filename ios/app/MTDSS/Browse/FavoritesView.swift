/// 第一标签「常用目录」根页 = 授权目录列表(spec §4.6 定案:授权集 =
/// 收件箱(App 初始授权,不可删)+ 信任目录(T10 经设置添加,授权后自动
/// 出现)。v1 数据源 = AuthorizedRoot.roots(recvDir:)(收件箱单枚验收;
/// 列表按授权集建模,T10 只追加条目)。根页无返回/前进箭头,仅三点
/// (spec §4.1);点根条目推入浏览页(4.1 模板)。

import MTDSSKit
import SwiftUI

struct FavoritesView: View {
    @Environment(AppModel.self) private var model
    @State private var navPath: [AuthorizedRoot] = []

    /// 授权目录列表(收件箱恒在;T10 接线点:trust 后追加)。
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
            pushInboxIfDebugHooked()
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
        Button("选择") {
            // 占位:选择模式归 T6
        }
        Picker("排序", selection: .constant(BrowseSort.name)) {
            ForEach(BrowseSort.allCases) { field in
                Text(field.label).tag(field)
            }
        }
        .disabled(true) // 单枚收件箱无排序面;浏览页内排序见 BrowseView
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

    private func pushInboxIfDebugHooked() {
        #if DEBUG
        guard ProcessInfo.processInfo.environment["MTDSS_DEBUG_AUTOPUSH"] == "inbox",
              navPath.isEmpty, let inbox = roots.first(where: \.isInbox) else { return }
        navPath = [inbox]
        #endif
    }
}

/// 浏览推入页:每根一枚 VM(StateObject 随页生命周期)。
private struct BrowseDestination: View {
    let root: AuthorizedRoot
    let repo: MtdssRepository
    let deviceId: String
    let onPopToRoot: () -> Void

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
        BrowseView(vm: vm, showsArrows: true, onPopToRoot: onPopToRoot)
            .toolbar(.hidden, for: .navigationBar)
    }
}
