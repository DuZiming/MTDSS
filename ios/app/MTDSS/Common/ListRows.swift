/// 浏览列表的 List 化装配件(R6 #3,20261102.4 真机轮):
/// - ``BrowseList``:浏览行容器自 ScrollView+LazyVStack(SinkingFooterList)
///   迁 List——自绘 ``SwipeableRow`` 的 DragGesture 与滚动视图 pan 争用,
///   真机恒败(用户报告:收藏/设备浏览行左滑无响应);原生 `.swipeActions`
///   仅 List 生效,Files App 同源。形态对齐:.plain 通栏 + 行背景透明 +
///   系统分隔线隐藏(FullBleedRow 自绘 0.5 细线带行首内缩)+ 行 insets
///   归零。**尾注 = 普通末行**(「不足一屏沉底」在 List 无平价实现,有意
///   取舍:原生左滑 > 沉底尾注;长列表两者无差)。
/// - ``NativeSwipeActions``:行动作挂接(动作集空 = 不挂,选择态翻空集即
///   收——与 SwipeableRow 语义同形);allowsFullSwipe 关(单条目动作皆
///   破坏面/出发面,逐钮确认)。
/// - ``EdgeBackStrip``:左缘右滑返回带。List 为 UICollectionView 背书,页级
///   `simultaneousGesture(DragGesture)` 不再保证收到被滚动视图争用的拖拽;
///   返回带独立于列表(无滚动争用,DragGesture 恒达),几何谓词不变
///   (``BrowseGesture.isBack``)。代价:左缘带内起手的纵向滚动不达列表
///   (系统边缘返回手势同形态,可接受)。

import MTDSSKit
import SwiftUI

/// 浏览行 List 容器(BrowseView/首页根列表共用;设备/传输页不迁,各无
/// 左滑面/自绘面维持原容器)。
struct BrowseList<Rows: View>: View {
    @ViewBuilder var rows: () -> Rows

    var body: some View {
        List {
            // 行级修饰挂 Group(Group 透明,修饰符落到每行)——挂 List 层
            // 不下传(listRowInsets/listRowSeparator 系行级环境,sim 截图
            // 钉:容器层挂法双分隔线 + 默认行内缩漏出)
            Group {
                rows()
            }
            .listRowInsets(EdgeInsets())
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color(uiColor: .systemBackground))
    }
}

/// 浏览行原生左滑动作(动作语义归调用方,与 SwipeableRow 同一收口面)。
struct NativeSwipeActions<Action: Equatable>: ViewModifier {
    /// 动作集(空 = 无滑面;选择态翻空集即收)。
    let actions: [Action]
    /// 动作 → 钮规格(SwipeActionSpec 同 SwipeableRow 语义:图标/色/标签)。
    var spec: (Action) -> SwipeActionSpec
    let onAction: (Action) -> Void

    func body(content: Content) -> some View {
        if actions.isEmpty {
            content
        } else {
            content
                .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                    ForEach(Array(actions.enumerated().reversed()), id: \.offset) { _, action in
                        button(action)
                    }
                }
        }
    }

    /// reversed 声明:原生 trailing 动作**首声明者最贴右缘**(iOS 26 实测,
    /// UITest 帧断言钉:delete.frame.maxX > send.frame.maxX)——翻转载入
    /// [删除]贴缘,与自绘 SwipeableRow 的 HStack 顺序([发送][删除])视觉
    /// 同形(删除恒在滑出簇最右)。
    private func button(_ action: Action) -> some View {
        let s = spec(action)
        return Button {
            onAction(action)
        } label: {
            Image(systemName: s.symbol)
                .font(.system(size: 18, weight: .medium))
        }
        .tint(s.tint)
        .accessibilityLabel(Text(s.label))
    }
}

/// 左缘右滑返回带(R6 #3;BrowseView 页面级挂接,替换原页级
/// simultaneousGesture——List 化后该手势不达)。DEBUG 面打印几何入参,
/// 真机轮排障用(谓词不触发的边缘拖拽也可据此判形)。
struct EdgeBackStrip: View {
    /// 谓词命中后的返回动作(历史内 goBack / 浏览根 popToRoot,调用方收口)。
    let onBack: () -> Void

    var body: some View {
        Color.clear
            .contentShape(Rectangle())
            .frame(width: BrowseGesture.edgeZone)
            .frame(maxHeight: .infinity)
            .gesture(
                DragGesture(minimumDistance: 10).onEnded { value in
                    #if DEBUG
                    print("[R6-EDGE] drag start=\(value.startLocation.x) dx=\(value.translation.width) dy=\(value.translation.height)")
                    #endif
                    guard BrowseGesture.isBack(
                        startX: value.startLocation.x,
                        dx: value.translation.width,
                        dy: value.translation.height,
                    ) else { return }
                    onBack()
                },
            )
            .accessibilityHidden(true)
    }
}
