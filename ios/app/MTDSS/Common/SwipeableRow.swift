/// 通栏行左滑动作行(T9 传输列表先例组件,R5b #7 通用化供浏览行接入):
/// SinkingFooterList 是 ScrollView+LazyVStack,系统 swipeActions 仅 List
/// 可用——自绘轻量左滑:内容层拖拽位移,动作钮恒驻背后,过半吸附;
/// 动作集 = 调用方按行态出集(传输行按状态三分:取消/重新发起/清除;
/// 浏览文件行按浏览档:本地[发送,删除]/远端[下载,转发],R5b #7)。
/// **展开态点行 = 只收动作**:内容 disabled 收口——浏览行内容是整行
/// Button,祖先 tap 手势竞争不过子 Button,禁用后点按才落到行面收动作。
/// 动作钮规格经 ``SwipeActionSpec`` 由调用方出(图标/底色/无障碍标签)。

import SwiftUI

/// 左滑动作钮规格(SF 图标/底色/无障碍标签;动作语义归调用方)。
struct SwipeActionSpec {
    let symbol: String
    let tint: Color
    let label: String
}

struct SwipeableRow<Action: Equatable, Content: View>: View {
    /// 动作集(空 = 无滑面,行不挂手势;选择态翻空集即收)。
    let actions: [Action]
    /// 动作 → 钮规格。
    var spec: (Action) -> SwipeActionSpec
    var busy: Bool = false
    /// DEBUG 截图钩子:初始即展开(sim 无滑动手势驱动;真机/产线恒 false)。
    var debugOpen: Bool = false
    var onAction: (Action) -> Void
    @ViewBuilder var content: () -> Content

    @State private var offsetX: CGFloat = 0
    @State private var open = false

    private var revealWidth: CGFloat { CGFloat(actions.count) * 76 }

    var body: some View {
        ZStack(alignment: .trailing) {
            if !actions.isEmpty {
                HStack(spacing: 0) {
                    ForEach(Array(actions.enumerated()), id: \.offset) { _, action in
                        actionButton(action)
                    }
                }
            }
            content()
                .background(Color(uiColor: .systemBackground))
                .offset(x: offsetX)
                // 展开态行内容钮不抢点(整行 Button 的浏览行场景)——点行 = 只收
                .disabled(open)
        }
        .contentShape(Rectangle())
        .onTapGesture { if open { close() } }
        .gesture(actions.isEmpty || busy ? nil : drag)
        .onChange(of: actions) { _ in close() } // 状态翻转动集变化 → 复位
        .onAppear {
            if debugOpen, !actions.isEmpty {
                open = true
                offsetX = -revealWidth
            }
        }
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 24)
            .onChanged { value in
                let base: CGFloat = open ? -revealWidth : 0
                offsetX = min(max(base + value.translation.width, -revealWidth), 0)
            }
            .onEnded { value in
                let shouldOpen = open
                    ? offsetX < -revealWidth / 2
                    : offsetX < -revealWidth * 0.4 || value.predictedEndTranslation.width < -revealWidth
                withAnimation(.spring(duration: 0.25)) {
                    open = shouldOpen
                    offsetX = shouldOpen ? -revealWidth : 0
                }
            }
    }

    private func close() {
        withAnimation(.spring(duration: 0.25)) {
            open = false
            offsetX = 0
        }
    }

    /// 动作钮(mockup:通栏色块+图标;规格由调用方出)。
    private func actionButton(_ action: Action) -> some View {
        let s = spec(action)
        return Button {
            close()
            onAction(action)
        } label: {
            Image(systemName: s.symbol)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(.white)
                .frame(width: 76)
                .frame(maxHeight: .infinity)
                .background(s.tint)
        }
        .opacity(busy ? 0.5 : 1)
        .accessibilityLabel(Text(s.label))
    }
}
