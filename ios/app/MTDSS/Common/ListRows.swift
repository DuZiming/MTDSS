/// 浏览列表的 List 化装配件(R6 #3,20261102.4 真机轮;R9 #1 传输页同迁):
/// - ``BrowseList``:列表行容器自 ScrollView+LazyVStack(SinkingFooterList)
///   迁 List——自绘 SwipeableRow 的 DragGesture 与滚动视图 pan 争用,
///   真机恒败(用户报告:收藏/设备浏览行左滑无响应);原生 `.swipeActions`
///   仅 List 生效,Files App 同源。形态对齐:.plain 通栏 + 行背景透明 +
///   系统分隔线隐藏(FullBleedRow 自绘 0.5 细线带行首内缩)+ 行 insets
///   归零。**尾注 = 普通末行**(「不足一屏沉底」在 List 无平价实现,有意
///   取舍:原生左滑 > 沉底尾注;长列表两者无差)。R9 #1 起传输列表同用
///   本容器(全 App 列表单口,自绘 SwipeableRow 撤除)。
/// - ``NativeSwipeActions``:行动作挂接(无条件挂载,评审 M-1:if/else 空
///   集分支=行结构同一性随选择翻转重挂;空集=空动作面无示能,UITest 钉);
///   allowsFullSwipe 关(单条目动作皆破坏面/出发面,逐钮确认)。
/// 左缘右滑返回:R6 曾设 ``EdgeBackStrip`` 左缘带(List 化后恐页级手势不
/// 达)——sim 合成触点实证**带吞 x<30 点按**(overlay 赢 hit-test,拖拽
/// 手势不认点按也不转发下层:返回箭头/展开钮/近缘行点按三测全挂)→
/// 回 R2 页级 `simultaneousGesture`(与滚动/点按并行,不占 hit-test);
/// List 上可达性由 UITest `testEdgeBackPopsToHome` 钉(iOS 26 手势桥并存
/// 实测达)。

import MTDSSKit
import SwiftUI

/// 左滑动作钮规格(SF 图标/底色/无障碍标签;动作语义归调用方)。原 SwipeableRow
/// 伴随规格(R5b 通用化),R9 #1 自绘行撤除后由 ``NativeSwipeActions`` 承袭
/// 同一语义(浏览/传输页 swipeSpec 出口不变)。
struct SwipeActionSpec {
    let symbol: String
    let tint: Color
    let label: String
}

/// 列表行 List 容器(BrowseView/首页根列表/传输列表共用,R9 #1 起三处同
/// 容器;设备页仍 SinkingFooterList——无左滑面不迁,见 DevicesView)。
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

/// 行左滑动作挂接(动作语义归调用方;R6 起浏览行、R9 #1 起传输行同此收口)。
struct NativeSwipeActions<Action: Equatable>: ViewModifier {
    /// 动作集(空 = 无滑面;选择态翻空集即收)。
    let actions: [Action]
    /// 动作 → 钮规格(SwipeActionSpec:图标/色/标签)。
    var spec: (Action) -> SwipeActionSpec
    let onAction: (Action) -> Void

    /// 无条件挂载(R6 评审 M-1:原 if/else 空集分支=行结构同一性随选择
    /// 翻转重挂;空集=空动作面,实测无滑动示能——UITest 钉目录行左滑
    /// 无删除钮)。声明序保持 reversed(破坏面钮贴缘)。
    func body(content: Content) -> some View {
        content
            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                ForEach(Array(actions.enumerated().reversed()), id: \.offset) { _, action in
                    button(action)
                }
            }
    }

    /// reversed 声明:原生 trailing 动作**首声明者最贴右缘**(iOS 26 实测,
    /// UITest 帧断言钉:delete.frame.maxX > send.frame.maxX)——翻转载入
    /// [删除]贴缘,动作集末位恒在滑出簇最右(本地 [发送|删除] / 远端
    /// [下载|转发(删除)] / 传输 [清除|…] 同形)。
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
