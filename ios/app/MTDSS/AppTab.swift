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
        case .favorites: "常用"
        case .devices: "设备"
        case .transfers: "传输"
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
                placeholder(tab)
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
            }
        }
    }

    /// 占位视图:引擎未就绪显提示,就绪后仍为占位(T4 不实装浏览逻辑)。
    @ViewBuilder
    private func placeholder(_ tab: AppTab) -> some View {
        if model.repository == nil {
            ContentUnavailableView(
                tab.title,
                systemImage: tab.symbol,
                description: Text("引擎就绪中…"),
            )
        } else {
            ContentUnavailableView(
                tab.title,
                systemImage: tab.symbol,
                description: Text("后续任务实装"),
            )
        }
    }
}
