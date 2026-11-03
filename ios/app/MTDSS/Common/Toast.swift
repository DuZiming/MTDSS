/// 动作发起反馈胶囊(R3 真机轮 #3/#6:下载/发送/分享/转发发起即反馈,
/// 回答「点了之后发生什么」;进度与终态仍归传输列表行)。App 内此前无
/// toast 组件——样式沿用 AppTab 顶栏横幅同语言(白字胶囊深底),贴底
/// 浮现,2s 自动消隐;单消息槽(后到覆盖先到,计时重置)。

import SwiftUI

/// 贴底提示胶囊(单条;文案面 = 调用方,组件只管呈现与消隐)。
private struct ToastCapsule: View {
    let message: String

    var body: some View {
        Text(message)
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(.white)
            .lineLimit(2)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Color.black.opacity(0.75), in: Capsule())
            .padding(.bottom, 72) // 悬浮于底部动作条/标签栏之上
            .accessibilityLabel(message)
    }
}

/// `.toast($message)` 修饰器:message 非 nil 显示,duration 后自动置 nil。
struct ToastModifier: ViewModifier {
    @Binding var message: String?
    var duration: TimeInterval = 2

    @State private var dismissTask: Task<Void, Never>?

    func body(content: Content) -> some View {
        content
            .overlay(alignment: .bottom) {
                if let message {
                    ToastCapsule(message: message)
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
            }
            .animation(.easeInOut(duration: 0.2), value: message)
            .onChange(of: message) { _, newValue in
                dismissTask?.cancel()
                guard newValue != nil else { return }
                dismissTask = Task {
                    try? await Task.sleep(nanoseconds: UInt64(duration * 1_000_000_000))
                    if !Task.isCancelled {
                        message = nil
                    }
                }
            }
    }
}

extension View {
    /// 动作发起反馈(toast 单槽;置 nil 即隐,赋新值即换)。
    func toast(_ message: Binding<String?>, duration: TimeInterval = 2) -> some View {
        modifier(ToastModifier(message: message, duration: duration))
    }
}
