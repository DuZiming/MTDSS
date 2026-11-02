/// 选择模式骨架(spec §4.2,mockup select-mode-final 像素参照;Ruling O
/// 2026-11-02 确认语义收口):
/// - ``SelectionNavBar``:顶栏 = 全选(左,蓝)/ 居中「N 项」/ 右蓝色圆形
///   白勾确认钮(34pt,投影);**确认 = 退出选择模式(无论选否,无副
///   作用)**——语义收口在 BrowseViewModel.confirmSelection;操作仅自
///   底栏发起。
/// - ``SelectionActionBar``:底部 = 左功能图标组 + 右圆点更多;本地
///   (常用目录/收件箱)= 分享(square.and.arrow.up)/ 删除(trash,红);
///   远端(设备页浏览)= 下载(square.and.arrow.down)/ 转发(square.and.
///   arrow.up)(T7 挂接;R5b #8 远端补删除 trash——两侧统一)。
///   选择模式下标签栏隐藏(BrowseView .toolbar(.hidden, for: .tabBar))。

import MTDSSKit
import SwiftUI

/// 已捕获待发送的选择集(Ruling O:动作面先捕获 ``selectedEntries`` 再
/// 退出选择模式;sheet(item:) 驱动选设备弹层。浏览页/首页同用)。
struct PickedSelection: Identifiable {
    let id = UUID()
    let items: [DirEntry]
}

/// 待确认删除请求(R5b #9:条数入确认文案;选择删除与行滑删同一收口)。
struct DeleteRequest: Identifiable {
    let paths: [String]

    var id: String { paths.joined(separator: "\n") }
}

/// 选择模式顶栏(替换常规浏览顶栏;搜索框保留在下方)。
struct SelectionNavBar: View {
    /// 已选计数(标题「N 项」)。
    let count: Int
    var onSelectAll: () -> Void
    var onConfirm: () -> Void

    var body: some View {
        ZStack {
            HStack {
                Button(action: onSelectAll) {
                    Text("全选")
                        .font(.system(size: 17))
                        .foregroundStyle(Color.accentColor)
                }
                Spacer()
                Button(action: onConfirm) {
                    Image(systemName: "checkmark")
                        .font(.system(size: 15, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(Color.accentColor, in: Circle())
                        .shadow(color: Color.accentColor.opacity(0.35), radius: 6, y: 2)
                }
            }
            Text("\(count) 项")
                .font(.system(size: 17))
                .lineLimit(1)
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }
}

/// 选择模式底部动作条(mockup .actrow:左图标组 26pt、右 38pt 圆底圆点)。
struct SelectionActionBar: View {
    /// 本地动作:分享 / 删除(远端 下载/转发 = T7 挂接点,ActionKind 已留位)。
    enum ActionKind: String, Identifiable {
        case share
        case delete
        case download
        case forward

        var id: String { rawValue }

        var symbol: String {
            switch self {
            case .share, .forward: "square.and.arrow.up"
            case .delete: "trash"
            case .download: "square.and.arrow.down"
            }
        }

        /// 删除 = 红色;其余蓝(mockup)。
        var tint: Color {
            self == .delete ? Color.red : Color.accentColor
        }
    }

    /// 本任务渲染的动作集(本地浏览 = [.share, .delete];远端 = [.download,
    /// .forward])。
    let actions: [ActionKind]
    /// 更多菜单(右圆点;v1 无菜单项 → 禁用灰显一致,T6-minor1 收口)。
    var moreMenu: () -> AnyView = { AnyView(EmptyView()) }
    /// 更多钮是否挂有菜单(无菜单 = 禁用灰显;挂菜单 = 蓝色可点)。
    var hasMore: Bool = false
    var onAction: (ActionKind) -> Void

    var body: some View {
        HStack {
            HStack(spacing: 26) {
                ForEach(actions) { action in
                    Button {
                        onAction(action)
                    } label: {
                        Image(systemName: action.symbol)
                            .font(.system(size: 22, weight: .regular))
                            .foregroundStyle(action.tint)
                            .frame(width: 34, height: 34)
                            .contentShape(Rectangle())
                    }
                    .accessibilityLabel(Text(action.rawValue))
                }
            }
            Spacer()
            moreCircle
        }
        .padding(.horizontal, 24)
        .padding(.top, 6)
        .padding(.bottom, 10)
        .background(.thinMaterial)
    }

    /// 右「圆点更多」钮(38pt 圆底;无菜单禁用且灰显一致——T6-minor1:
    /// 禁用态不再误用主题蓝,浅灰与禁用语义同形)。
    @ViewBuilder private var moreCircle: some View {
        Menu {
            moreMenu()
        } label: {
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { _ in
                    Circle().frame(width: 4.5, height: 4.5)
                }
            }
            .foregroundStyle(hasMore ? Color.accentColor : Color(uiColor: .systemGray3))
            .frame(width: 38, height: 38)
            .background(Color(uiColor: .systemGray6), in: Circle())
        }
        .disabled(!hasMore)
    }
}
