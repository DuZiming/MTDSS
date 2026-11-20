/// 浏览缩略图槽(V4-1 T24;钦定 mockup mobile-browse/mobile-tiles-v2 iOS 列):
/// 回环 handle URL 就绪即图(URLSession 直连,`?t=` 令牌缝与桌面 previewer
/// 同一面);等比缩放不裁切(确认点 ③ 长图等比);加载中/失败 = 图标打底
/// 可见,成功隐去(防透明图透底);视频时长角标右下角(黑底 60% 白字)。
///
/// 缓存/装载 = ``ThumbImageLoader.shared``(Kit NSCache 按 URL 键,Coil 内存
/// 缓存同语义;daemon /thumb 无缓存头,RFC 7234 下 URLCache 不存储——会话内
/// 同 URL 复进视口零重拉,fix round 1 Important #1)。
///
/// 句柄生命周期:组合在场 = ``PreviewHandlePool/urlFor``(+1);离场 =
/// .task 自取消(池 `Task.isCancelled` 不入账)+ onDisappear detached
/// release(−1,归零 close;detached 不随视图生命周期取消,离场收口必须
/// 走完——安卓 NonCancellable 同款纪律)。

import MTDSSKit
import SwiftUI

struct BrowseThumbSlot<Icon: View>: View {
    let entry: DirEntry
    let pool: PreviewHandlePool
    /// 角标串(调用方经 ``BrowseThumbs/durationBadgeFor`` 预算;空 = 不显)。
    let badge: String
    /// 角标度量(mockup 逐值:列表行 8pt/内缩2/圆角2/横距3,磁贴 9/4/3/4)。
    var badgeFont: CGFloat
    var badgeRadius: CGFloat
    var badgeInset: CGFloat
    var badgePadH: CGFloat
    /// 打底图标(加载中/失败可见;成功隐去)。
    @ViewBuilder let icon: () -> Icon

    @State private var image: CGImage?

    var body: some View {
        ZStack {
            if let image {
                Image(uiImage: UIImage(cgImage: image))
                    .resizable()
                    .scaledToFit() // 长图等比缩放不裁切(mockup 确认点 ③)
            } else {
                icon()
            }
        }
        // 角标钉右下角(overlay:主内容居中不受 ZStack 对齐牵连)
        .overlay(alignment: .bottomTrailing) {
            if !badge.isEmpty {
                Text(badge)
                    .font(.system(size: badgeFont))
                    .foregroundStyle(.white)
                    .lineLimit(1)
                    .padding(.horizontal, badgePadH)
                    .background(Color.black.opacity(0.6), in: RoundedRectangle(cornerRadius: badgeRadius))
                    .padding(badgeInset)
                    .accessibilityHidden(true) // 行/磁贴语义已携条目名,角标免朗读
            }
        }
        .task(id: entry.path) {
            // 句柄换 = URL 换:旧图先撤,新槽图片不串行
            image = nil
            guard let raw = await pool.urlFor(entry), let parsed = URL(string: raw) else { return }
            // 缓存命中零网络;未中 = 回环拉取解码落缓存(失败 = 图标降级,
            // 缩略图缺位只是降级面;取消同走此面,.task 离场自取消在途即弃)
            image = await ThumbImageLoader.shared.image(for: parsed)
        }
        .onDisappear {
            let entry = entry
            let pool = pool
            // detached:屏面卸载时任务树已取消,排队收口必须走完(泄漏底线)。
            Task.detached { await pool.release(entry) }
        }
        .accessibilityHidden(true) // 行/磁贴无障碍语义 = 条目名,缩略图免重复朗读
    }
}
