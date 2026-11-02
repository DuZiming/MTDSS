/// 四标签壳(spec §4 总范式:四标签 + 悬浮玻璃胶囊标签栏;iOS 26 TabView
/// 原生 Liquid Glass 胶囊面,零自绘)。T4 只做空壳:每标签一枚占位视图,
/// 浏览(T5)/设备(T7)/传输(T9)/我的(T14+)由后续任务填充。

import SwiftUI

/// 标签枚举(计划 T4 接口面:四标签壳供后续任务填充)。
enum AppTab: String, CaseIterable, Identifiable {
    case favorites
    case devices
    case transfers
    case profile

    var id: String { rawValue }

    var title: String {
        switch self {
        case .favorites: "常用目录"
        case .devices: "设备"
        case .transfers: "传输列表"
        case .profile: "我的"
        }
    }

    var symbol: String {
        switch self {
        case .favorites: "folder"
        case .devices: "display"
        case .transfers: "arrow.left.arrow.right"
        case .profile: "person"
        }
    }
}

struct AppTabView: View {
    @Environment(AppModel.self) private var model
    @State private var selection: AppTab = .favorites

    var body: some View {
        TabView(selection: $selection) {
            ForEach(AppTab.allCases) { tab in
                content(tab)
                    .tabItem { Label(tab.title, systemImage: tab.symbol) }
                    .tag(tab)
            }
        }
        .overlay(alignment: .top) {
            if let error = model.bootError {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(.red.opacity(0.85), in: Capsule())
                    .padding(.top, 4)
            } else if model.engineRestarting {
                // 信任目录变更的引擎重启面(Ruling L,~1-2s;低频可接受)
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("重连中…")
                        .font(.footnote)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(.blue.opacity(0.85), in: Capsule())
                .padding(.top, 4)
            }
        }
        #if DEBUG
        .task {
            // MTDSS_DEBUG_TAB=<tab>:冒烟/截图钩子切标签(模拟器无触点驱动;
            // 真机/产线不编译,特此报备)
            if let tab = ProcessInfo.processInfo.environment["MTDSS_DEBUG_TAB"],
               let target = AppTab(rawValue: tab) {
                selection = target
            }
        }
        #endif
    }

    /// 各标签内容:常用目录 = 授权目录列表根(T5);设备 = 设备列表根(T7);
    /// 传输 = 传输列表页(T9);我的 = 个人页(T10:登录/设置/信任目录)。
    /// MTDSS_DEBUG_PHOTO_TAB=1(DEBUG 截图面):常用目录标签直render照片
    /// 图库页——外层推入链在 iOS 26 sim 间歇「挂载即弃」(probe 钉:appear
    /// 后紧接 disappear 且 navPath 不变),内层栈直渲染无此面;真机/产线
    /// 不编译。
    @ViewBuilder
    private func content(_ tab: AppTab) -> some View {
        switch tab {
        case .favorites:
            #if DEBUG
            if ProcessInfo.processInfo.environment["MTDSS_DEBUG_PHOTO_TAB"] == "1",
               let repo = model.repository {
                PhotoLibraryView(repo: repo)
            } else {
                FavoritesView()
            }
            #else
            FavoritesView()
            #endif
        case .devices:
            DevicesView()
        case .transfers:
            #if DEBUG
            if ProcessInfo.processInfo.environment["MTDSS_DEBUG_SHARESTATES"] == "1" {
                // 分享扩展状态页(DEBUG 截图面;sim 无真分享面板入口)
                ShareStatesHarness()
            } else {
                TransfersView()
            }
            #else
            TransfersView()
            #endif
        case .profile:
            ProfileView()
        }
    }
}
