/// 浏览页顶栏模板(spec §4.1):左 返回/前进双箭头(SF chevron 粗圆头,
/// 返回蓝/前进浅灰禁用——前进=返回后可再进入的浏览器式导航),标题绝对
/// 居中(整页 50% 定位,两侧预留防挤压),右上「···」圆钮。标签根页无
/// 箭头(仅三点);推入页显示返回(mockup browse-final .navrow 像素参照)。

import SwiftUI

struct BrowseNavBar<MenuContent: View>: View {
    let title: String
    var showsArrows: Bool
    var canBack: Bool = false
    var canForward: Bool = false
    var onBack: (() -> Void)? = nil
    var onForward: (() -> Void)? = nil
    @ViewBuilder var menu: () -> MenuContent

    var body: some View {
        ZStack {
            HStack(spacing: 16) {
                if showsArrows {
                    Button(action: { onBack?() }) {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 16, weight: .bold))
                            .foregroundStyle(Color.accentColor)
                            .frame(width: 20, height: 30)
                            .contentShape(Rectangle())
                    }
                    .disabled(onBack == nil)
                    .opacity(onBack == nil ? 0.4 : 1)

                    Button(action: { onForward?() }) {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 16, weight: .bold))
                            .foregroundStyle(canForward ? Color.accentColor : Color(uiColor: .systemGray3))
                            .frame(width: 20, height: 30)
                            .contentShape(Rectangle())
                    }
                    .disabled(!canForward || onForward == nil)
                }
                Spacer()
                dotsMenu
            }
            Text(title)
                .font(.system(size: 17, weight: .semibold))
                .lineLimit(1)
                // R4 真机轮 #5:远端「设备名 | 目录名」可能长——中间截断
                // (头尾都可见,设备与目录同现)
                .truncationMode(.middle)
                .padding(.horizontal, 100) // 绝对居中:两侧各留出箭头+菜单的净空
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }

    /// 「···」圆钮:34pt 圆形触区,三枚 4pt 蓝点(mockup .dots 同形)。
    private var dotsMenu: some View {
        Menu {
            menu()
        } label: {
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { _ in
                    Circle().frame(width: 4, height: 4)
                }
            }
            .foregroundStyle(Color.accentColor)
            .frame(width: 34, height: 34)
            .contentShape(Circle())
        }
    }
}
