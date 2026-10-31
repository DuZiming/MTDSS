/// 文件 App 式通栏行样式(spec §4.1:行高 58,细分割线从行首内缩 20pt 起、
/// 通栏到右缘)。浏览/设备/传输各列表复用的行容器(mockup browse-final
/// .frow 像素参照)。

import SwiftUI

struct FullBleedRow<Content: View>: View {
    var height: CGFloat = 58
    var separatorLeadingInset: CGFloat = 20
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .frame(maxWidth: .infinity, minHeight: height, alignment: .leading)
            .overlay(alignment: .bottom) {
                Rectangle()
                    .fill(Color(uiColor: .separator).opacity(0.6))
                    .frame(height: 0.5)
                    .padding(.leading, separatorLeadingInset)
            }
    }
}

/// 列表尾注「N 项」(spec §4.1:沉底——列表不满屏时贴底,满屏随滚动;
/// mockup .cnt margin-top:auto 语义:Spacer 顶开 + 视口 minHeight)。
struct ItemCountFooter: View {
    let count: Int
    var note: String? = nil

    var body: some View {
        Text(note.map { "\(count) 项(\($0))" } ?? "\(count) 项")
            .font(.system(size: 13))
            .foregroundStyle(Color(uiColor: .systemGray))
            .frame(maxWidth: .infinity)
            .padding(.top, 10)
            .padding(.bottom, 14)
    }
}

/// 「列表 + 沉底尾注」滚动容器:内容不足一屏时尾注贴底,超一屏随列表滚动。
struct SinkingFooterList<Rows: View, Footer: View>: View {
    @ViewBuilder var rows: () -> Rows
    @ViewBuilder var footer: () -> Footer

    var body: some View {
        GeometryReader { geo in
            ScrollView {
                VStack(spacing: 0) {
                    LazyVStack(spacing: 0, content: rows)
                    Spacer(minLength: 0)
                    footer()
                }
                .frame(minHeight: geo.size.height)
            }
        }
    }
}
