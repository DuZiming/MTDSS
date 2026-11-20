/// 预览四容器与分区面(V4-1 T25「四容器一分区」;钦定 mockup
/// mobile-preview.html;安卓 T23 PreviewViewers.kt 同构):
/// - image = 自研 zoom 查看器(双指缩放 1x-8x / 放大拖拽 1x 复位;thumb
///   先显 → raw 渐进,HEIC = jpeg 端点优先,[PreviewRoute/imageFaces(name:)]
///   序)+ ‹› 箭头 + 页点;
/// - video/audio = AVPlayer 原流直连(raw?t=,移动端原生解码不经转码——
///   spec 裁定,VideoPlayer 标准控制器不另出图);
/// - doc(pdf/office/md/文本)= WKWebView 宿主页 + previewer bundle(自管
///   scheme 同源面,boot 经 WKUserScript 注入,[PreviewBoot] 单点);
/// - unsupported = 类型页 + 下载引导。真机解码/WebView 真渲染留冒烟。

import AVKit
import Combine
import MTDSSKit
import SwiftUI
import UIKit
import WebKit

// MARK: 图片容器(自研 zoom)

/// 图片查看器双指缩放上限(mockup 确认点③只钉交互,上限取常规 8x;安卓
/// IMAGE_MAX_ZOOM 同值)。
let previewMaxZoom: CGFloat = 8

/**
 * 图片查看器(自研 zoom;mockup Android 列骨架):黑底居中等比,双指缩放
 * (1x–8x)+ 放大拖拽(1x 复位);thumb 先显(回环缩略图端点,快速首帧)
 * → 主图渐进(jpeg/raw 端点,[PreviewRoute/imageFaces(name:)] 序:HEIC =
 * jpeg 优先,raw 兜底);全面失败 = 字面 + 重试。装载走
 * ``ThumbImageLoader/shared``(NSCache 按 URL 键,同 URL 复进零重拉)。
 */
struct PreviewImageViewer: View {
    let entry: DirEntry
    let handle: PreviewOpenResult

    @State private var thumbImage: CGImage?
    @State private var mainImage: CGImage?
    @State private var facesExhausted = false
    @State private var retryNonce = 0
    // 缩放/拖拽态(切图/重试即复位;mockup 确认点③双指缩放+放大拖拽)
    @State private var scale: CGFloat = 1
    @State private var zoomAnchor: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var dragStart: CGSize?

    private var thumbUrl: URL? {
        URL(string: BrowseThumbs.thumbUrl(port: handle.port, handle: handle.handle, token: handle.token))
    }

    var body: some View {
        ZStack {
            Color.black
            if let mainImage {
                face(mainImage)
            } else if let thumbImage {
                face(thumbImage)
            } else if facesExhausted {
                failureFace
            } else {
                ProgressView().tint(.white)
            }
        }
        .clipped()
        .contentShape(Rectangle())
        .simultaneousGesture(magnify)
        .simultaneousGesture(drag)
        .task(id: retryNonce) { await load() }
    }

    /// 双指缩放(1x–8x;缩回 1x = 拖拽复位,mockup 确认点③)。
    private var magnify: some Gesture {
        MagnificationGesture()
            .onChanged { value in
                scale = min(max(zoomAnchor * value, 1), previewMaxZoom)
                if scale <= 1 { offset = .zero }
            }
            .onEnded { _ in zoomAnchor = scale }
    }

    /// 放大拖拽(1x 下拖拽不位移;松手态收口)。
    private var drag: some Gesture {
        DragGesture()
            .onChanged { value in
                guard scale > 1 else { return }
                let start = dragStart ?? offset
                dragStart = start
                offset = CGSize(
                    width: start.width + value.translation.width,
                    height: start.height + value.translation.height,
                )
            }
            .onEnded { _ in dragStart = nil }
    }

    private func face(_ cg: CGImage) -> some View {
        Image(uiImage: UIImage(cgImage: cg))
            .resizable()
            .scaledToFit() // 等比(mockup 确认点③长图同浏览页缩略图口径)
            .scaleEffect(scale)
            .offset(offset)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// 失败面(全面耗尽):字面 + 重试(重试 = nonce 翻新全链重拉)。
    private var failureFace: some View {
        VStack(spacing: 10) {
            Text("图片加载失败")
                .font(.system(size: 14))
                .foregroundStyle(.white.opacity(0.8))
                .multilineTextAlignment(.center)
            Button("重试") { retryNonce += 1 }
                .font(.system(size: 15, weight: .medium))
        }
    }

    /// 取数序:thumb 恒先显 → 主图逐面渐进([PreviewRoute/imageFaces(name:)]
    /// 序;失败面 = faceIndex 走完仍无图)。thumb 失败只是降级(主图照走)。
    private func load() async {
        thumbImage = nil
        mainImage = nil
        facesExhausted = false
        if let thumbUrl {
            thumbImage = await ThumbImageLoader.shared.image(for: thumbUrl)
        }
        for face in PreviewRoute.imageFaces(name: entry.name) {
            let url: URL?
            switch face {
            case "jpeg":
                url = URL(string: PreviewRoute.jpegUrl(port: handle.port, handle: handle.handle, token: handle.token))
            case "raw":
                url = URL(string: PreviewRoute.rawUrl(port: handle.port, handle: handle.handle, token: handle.token))
            default:
                url = nil
            }
            if let url, let image = await ThumbImageLoader.shared.image(for: url) {
                mainImage = image
                return
            }
            guard !Task.isCancelled else { return }
        }
        if !Task.isCancelled { facesExhausted = true }
    }
}

// MARK: 视频/音频容器(AVPlayer 原流直连)

/**
 * 媒体查看器(视频/音频;AVPlayer 原流直连 raw?t=,移动端原生解码不经
 * 转码——spec 裁定;mockup:系统级全屏播放器标准控制器,不另出图)。
 * 错误 = 失败字面 + 下载引导 + 重试(整链重建)。
 */
struct PreviewMediaViewer: View {
    let url: URL

    @State private var player: AVPlayer?
    @State private var failed = false
    @State private var attempt = 0
    @State private var statusObserver: AnyCancellable?

    var body: some View {
        ZStack {
            Color.black
            if failed {
                failureFace
            } else if let player {
                VideoPlayer(player: player)
            } else {
                ProgressView().tint(.white)
            }
        }
        .task(id: attempt) { setup() }
        .onDisappear { teardown() }
        // 生命周期接线(安卓 T23 fix round 1 Important #3 同面):后台即暂停
        // (音频不随 Home 背景续播;回前台**不**自动续——用户手点播放;幂等)
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.didEnterBackgroundNotification)) { _ in
            player?.pause()
        }
    }

    /// 整链装配(重试 = attempt 翻新重建;旧链先收口)。
    private func setup() {
        teardown()
        let item = AVPlayerItem(url: url)
        let newPlayer = AVPlayer(playerItem: item)
        player = newPlayer
        statusObserver = item.publisher(for: \.status)
            .receive(on: DispatchQueue.main)
            .sink { status in
                if status == .failed {
                    failed = true
                    newPlayer.pause()
                }
            }
        newPlayer.play()
    }

    /// 全量收口(观察者摘除 + 播放器释放;屏面卸载/重建两路同口)。
    private func teardown() {
        statusObserver?.cancel()
        statusObserver = nil
        player?.pause()
        player?.replaceCurrentItem(with: nil)
        player = nil
    }

    private var failureFace: some View {
        VStack(spacing: 8) {
            Text("无法播放此格式")
                .font(.system(size: 14))
                .foregroundStyle(.white.opacity(0.8))
                .multilineTextAlignment(.center)
            Text("可下载到收件箱后用其他应用打开")
                .font(.system(size: 12))
                .foregroundStyle(Color(uiColor: .init(white: 0.6, alpha: 1)))
            Button("重试") {
                failed = false
                attempt += 1
            }
            .font(.system(size: 15, weight: .medium))
            .padding(.top, 4)
        }
    }
}

// MARK: 文档容器(WKWebView + previewer bundle)

/**
 * 文档容器(WKWebView;pdf/office/md/文本 = previewer bundle 真渲染):宿主
 * 页 = mtdsspreview://localhost/preview.html(自管 scheme 同源面,见
 * ``PreviewProxySchemeHandler``——宿主页 + bundle 资产 + 回环代理同源,
 * fetch 零 CORS,previewer bundle 零改动)。boot(`{req, handle}` 形,
 * [PreviewBoot] 单点装配)经 WKUserScript(atDocumentEnd,宿主页模块脚本
 * 已挂监听)注入;JS 常开( brief 钉)。真渲染留冒烟。
 */
struct PreviewDocViewer: UIViewRepresentable {
    let injectionJs: String

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        // JS 开关(brief 钉;显式声明防默认值漂移)
        config.preferences.javaScriptEnabled = true
        // 自管 scheme handler(宿主页 + 资产 + 回环代理同源面;T25)
        config.setURLSchemeHandler(PreviewProxySchemeHandler(), forURLScheme: PreviewRoute.proxyScheme)
        // boot 注入单点([PreviewBoot.injectionJs];atDocumentEnd = 宿主页
        // 模块脚本(先执行)已挂 mtdss-boot 监听,赋值+事件两路必有一路接住)
        config.userContentController.addUserScript(
            WKUserScript(source: injectionJs, injectionTime: .atDocumentEnd, forMainFrameOnly: true),
        )
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = context.coordinator
        web.backgroundColor = .systemBackground
        web.scrollView.backgroundColor = .systemBackground
        if let page = URL(string: PreviewRoute.hostPageUrl) {
            web.load(URLRequest(url: page))
        }
        return web
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {}

    /// 卸载收口(T23 fix round 1 Important #1 同课:doc 切换/屏面卸载不在
    /// 途加载残留;WKWebView 无显式 destroy,断加载 + 摘代理即终态)。
    static func dismantleUIView(_ uiView: WKWebView, coordinator: Coordinator) {
        uiView.stopLoading()
        uiView.navigationDelegate = nil
    }

    /// 导航守卫(T23 fix round 1 Important #2 的 iOS 面;特权面永不导外部
    /// 内容):仅自管资产面放行([PreviewRoute/linkNavigable(_:)];本
    /// scheme 初始加载同经此闸——放行位即宿主页),http(s) 外链点击交系统
    /// 浏览器,其余 scheme 静默拒。
    final class Coordinator: NSObject, WKNavigationDelegate {
        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping (WKNavigationActionPolicy) -> Void,
        ) {
            let raw = navigationAction.request.url?.absoluteString ?? ""
            if PreviewRoute.linkNavigable(raw) {
                decisionHandler(.allow)
                return
            }
            decisionHandler(.cancel)
            if let url = navigationAction.request.url,
               url.scheme?.lowercased() == "http" || url.scheme?.lowercased() == "https" {
                UIApplication.shared.open(url)
            }
        }
    }
}

// MARK: 分区面(不支持/失败/加载 + 图库几何件)

/// 不支持格式类型页(previewer unsupported.ts 同词面:「该格式暂未支持」;
/// 下载引导 = 原生下载动作替代桌面下载链接)。
struct PreviewUnsupportedFace: View {
    let entry: DirEntry
    let onDownload: () -> Void

    var body: some View {
        VStack(spacing: 6) {
            FileTypeIcon(isDir: false)
            Text(entry.name)
                .font(.system(size: 15, weight: .semibold))
                .multilineTextAlignment(.center)
                .lineLimit(3)
                .padding(.horizontal, 24)
                .padding(.top, 12)
            Text("该格式暂未支持")
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .padding(.top, 6)
            Text("可下载到收件箱后查看")
                .font(.system(size: 12))
                .foregroundStyle(Color(uiColor: .systemGray))
            Button("下载", action: onDownload)
                .font(.system(size: 15, weight: .medium))
                .padding(.top, 8)
        }
    }
}

/// 打开失败面(降级文案 [PreviewErrors] + 重试;dark = 黑观看面白字,
/// 文档容器浅底传 false = 主色字)。
struct PreviewErrorFace: View {
    let message: String
    let onRetry: () -> Void
    var dark: Bool = true

    var body: some View {
        VStack(spacing: 10) {
            Text(message)
                .font(.system(size: 14))
                .foregroundStyle(dark ? AnyShapeStyle(.white.opacity(0.85)) : AnyShapeStyle(Color.primary))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
            Button("重试", action: onRetry)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(dark ? AnyShapeStyle(Color.white) : AnyShapeStyle(Color.accentColor))
        }
    }
}

/// 加载面(句柄开启在途;dark = 黑观看面白转圈,文档容器浅底传 false)。
struct PreviewLoadingFace: View {
    var dark: Bool = true

    var body: some View {
        ProgressView()
            .tint(dark ? .white.opacity(0.7) : Color(uiColor: .systemGray))
    }
}

/// 页点(mockup Android 列底部:当前 = 16×5 圆角白条,余 = 5pt 圆点白
/// 40%;全尺寸 mockup 值)。图库 > 20 枚不渲点(横向溢出面;‹› 箭头照常),
/// 调用侧收口。
struct PreviewPageDots: View {
    let count: Int
    let active: Int

    var body: some View {
        HStack(spacing: 5) {
            ForEach(0..<count, id: \.self) { i in
                if i == active {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(Color.white)
                        .frame(width: 16, height: 5)
                } else {
                    Circle()
                        .fill(Color.white.opacity(0.4))
                        .frame(width: 5, height: 5)
                }
            }
        }
    }
}

/// ‹› 切换箭头(mockup:两侧竖直居中,白 70%,44pt 触面)。
struct PreviewNavArrow: View {
    let forward: Bool
    let onClick: () -> Void

    var body: some View {
        Button(action: onClick) {
            Image(systemName: forward ? "chevron.right" : "chevron.left")
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(.white.opacity(0.7))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(forward ? "下一张" : "上一张"))
    }
}
