/// 预览四容器与分区面(V4-1 T25「四容器一分区」;钦定 mockup
/// mobile-preview.html;安卓 T23 PreviewViewers.kt 同构):
/// - image = 自研 zoom 查看器(双指缩放 1x-8x / 放大拖拽 1x 复位;thumb
///   先显 → raw 渐进,HEIC = jpeg 端点优先,[PreviewRoute/imageFaces(name:)]
///   序)+ 胶片条 + 四动作图标条(r48 #3 相册化,见 PreviewView);
/// - video = 相册化播放面(r49 #2):海报框(缩略图+播放钮)点按进
///   WKWebView previewer 播放框(autoPlay 旗;playsinline 双保险);
/// - audio = AVPlayer 原流直连(raw?t=,VideoPlayer 标准控制器);
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
 * 图片查看器(自研 zoom):满屏居中等比(**背景随父面**——r48 #3 相册化:
 * 默认浅底/沉浸黑底由 PreviewView 决定,查看器本体透明不参与),双指缩放
 * (1x–8x)+ 放大拖拽(1x 复位);thumb 先显(回环缩略图端点,快速首帧)
 * → 主图渐进(jpeg/raw 端点,[PreviewRoute/imageFaces(name:)] 序:HEIC =
 * jpeg 优先,raw 兜底);全面失败 = 字面 + 重试。装载走
 * ``ThumbImageLoader/shared``(NSCache 按 URL 键,同 URL 复进零重拉)。
 */
struct PreviewImageViewer: View {
    let entry: DirEntry
    let handle: PreviewOpenResult
    /// 横向滑切(field r24 #4→r25 #3 分流:scale=1 时**屏幕中央**左/右滑
    /// = 上一张/下一张;放大态的拖拽归平移,不触发)。delta = ±1。
    var onSwipe: ((Int) -> Void)? = nil
    /// **左屏边缘右滑返回**(field r25 #3:与中央滑切分流——手势起点在
    /// 左缘 28pt 内且向右位移 = 返回,任意容器同语义)。
    var onEdgeBack: (() -> Void)? = nil
    /// 点按切换界面态(r48 #3 相册化:点图 = 进沉浸黑,chrome/状态栏全隐;
    /// 再点回浅底)。
    var onToggleChrome: (() -> Void)? = nil
    /// 主图就绪回调(field r24 #5 信息面板分辨率;宽×高像素)。
    var onMeta: ((Int, Int) -> Void)? = nil
    /// 加载/失败面配色(r48 #3):true = 沉浸黑底白字;false = 浅底主色
    /// 字(跟系统深浅色)。
    var faceDark: Bool = false

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
            Color.clear // 背景随父面(相册化:默认浅底/沉浸黑由 PreviewView 供)
            if let mainImage {
                face(mainImage)
            } else if let thumbImage {
                face(thumbImage)
            } else if facesExhausted {
                failureFace
            } else {
                ProgressView().tint(faceDark ? .white.opacity(0.7) : Color(uiColor: .systemGray))
            }
        }
        .clipped()
        .contentShape(Rectangle())
        .simultaneousGesture(magnify)
        .simultaneousGesture(drag)
        // 点按切换界面态(r48 #3:点图 = 进沉浸;再点回浅底 chrome)
        .onTapGesture { onToggleChrome?() }
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
            .onEnded { value in
                dragStart = nil
                // 手势分流(field r25 #3):左缘起右滑 = 返回(任意缩放态,
                // 与系统边缘手势同位);未放大态中央横向滑 = 切图(左滑 =
                // 下一张,右滑 = 上一张);放大态中央拖拽 = 平移(不触发)。
                let dx = value.translation.width
                let dy = value.translation.height
                guard abs(dx) > abs(dy) * 1.6, abs(dx) > 60 else { return }
                if value.startLocation.x < 28, dx > 0 {
                    onEdgeBack?()
                    return
                }
                guard scale <= 1, let onSwipe else { return }
                onSwipe(dx < 0 ? 1 : -1)
            }
    }

    private func face(_ cg: CGImage) -> some View {
        Image(uiImage: UIImage(cgImage: cg))
            .resizable()
            .scaledToFit() // 等比(mockup 确认点③长图同浏览页缩略图口径)
            .scaleEffect(scale)
            .offset(offset)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// 失败面(全面耗尽):字面 + 重试(重试 = nonce 翻新全链重拉);配色
    /// 随 faceDark(沉浸黑底白字/浅底主色字,PreviewErrorFace 同式)。
    private var failureFace: some View {
        VStack(spacing: 10) {
            Text("图片加载失败")
                .font(.system(size: 14))
                .foregroundStyle(faceDark ? AnyShapeStyle(.white.opacity(0.85)) : AnyShapeStyle(Color.primary))
                .multilineTextAlignment(.center)
            Button("重试") { retryNonce += 1 }
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(faceDark ? AnyShapeStyle(Color.white) : AnyShapeStyle(Color.accentColor))
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
                onMeta?(image.width, image.height) // r24 #5:分辨率回填信息面板
                return
            }
            guard !Task.isCancelled else { return }
        }
        if !Task.isCancelled { facesExhausted = true }
    }
}

// MARK: 信息面板(field r24 #5:类型/分辨率/ACM 三时间,格式化行)

/// 「查看信息」面板(sheet):名称/大小/格式(容器词面+扩展名)/分辨率
/// (图片 onMeta 回填,未得 = 不显行)/路径/设备/修改·创建·访问时间
/// (毫秒 → yyyy-MM-dd HH:mm;0/缺 = 「—」旧对端口径)。
struct PreviewInfoSheet: View {
    let entry: DirEntry
    let deviceLabel: String
    let container: PreviewContainer
    let resolution: String?

    @Environment(\.dismiss) private var dismiss

    private static let dateFormat: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd HH:mm"
        return f
    }()

    private func time(_ ms: Int64?) -> String {
        guard let ms, ms > 0 else { return "—" }
        return Self.dateFormat.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }

    /// 格式行:容器词面 + 大写扩展名(「图片 · JPG」)。
    private var typeLine: String {
        let containerWord: String
        switch container {
        case .image: containerWord = "图片"
        case .video: containerWord = "视频"
        case .audio: containerWord = "音频"
        case .doc: containerWord = "文档"
        case .unsupported: containerWord = "文件"
        }
        let ext = (entry.name as NSString).pathExtension
        return ext.isEmpty ? containerWord : "\(containerWord) · \(ext.uppercased())"
    }

    private func row(_ key: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(key)
                .font(.system(size: 13))
                .foregroundStyle(Color(uiColor: .systemGray))
                .frame(width: 76, alignment: .leading)
            Text(value)
                .font(.system(size: 13))
                .multilineTextAlignment(.leading)
            Spacer(minLength: 0)
        }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    row("名称", entry.name)
                    row("大小", BrowseFormat.size(entry.size))
                    row("格式", typeLine)
                    if let resolution {
                        row("分辨率", resolution)
                    }
                    row("路径", entry.path)
                    row("设备", deviceLabel)
                }
                Section("时间") {
                    row("修改时间", time(entry.mtimeMs))
                    row("创建时间", time(entry.ctimeMs))
                    row("访问时间", time(entry.atimeMs))
                }
            }
            .navigationTitle("文件信息")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
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
        // r49 #2:页内框形态播放(缺省 false 时 WebKit 缺 playsinline 语境
        // 会转全屏原生播放器,破坏相册化播放框;previewer 侧已补属性双保险)
        config.allowsInlineMediaPlayback = true
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

// MARK: 相册化通用件(r48-r49:图片/视频页共享 chrome 件)

/// 左缘右划返回带(r49 #1 全容器统一):屏左 28pt 透明原生条,右划即回
/// 调——WKWebView/AVPlayer 等 UIKit 内嵌视图会吃掉 SwiftUI 容器手势,
/// 原生条保证文档/音频/视频播放态的边缘返回恒可用(与系统边缘手势同位
/// 同宽;图片容器由查看器内部分流,不叠此件)。
struct PreviewEdgeBackZone: View {
    let onBack: () -> Void

    var body: some View {
        Color.clear
            .frame(width: 28)
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 24).onEnded { value in
                    let dx = value.translation.width
                    let dy = value.translation.height
                    guard dx > 50, abs(dx) > abs(dy) * 1.6 else { return }
                    onBack()
                }
            )
            .accessibilityHidden(true)
    }
}

/// 视频海报面(r49 #2 相册化:点按进播放前的静态框):回环缩略图端点
/// (ThumbImageLoader NSCache 同源)等比居中黑底框 + 中央播放钮,整框
/// 点按即进播放;onMeta 回抛海报宽高比(播放态 webview 框同比,竖屏
/// 视频不冤柱)。
struct PreviewVideoPosterFace: View {
    let entry: DirEntry
    let handle: PreviewOpenResult
    let onPlay: () -> Void
    var onMeta: ((CGFloat) -> Void)? = nil

    @State private var image: CGImage?

    private var thumbUrl: URL? {
        URL(string: BrowseThumbs.thumbUrl(port: handle.port, handle: handle.handle, token: handle.token))
    }

    /// 海报宽高比(未得 = 16:9;极端比钳回常规窗防条状占屏)。
    private var aspect: CGFloat {
        if let image {
            let a = CGFloat(image.width) / CGFloat(max(image.height, 1))
            return (0.4...2.6).contains(a) ? a : 16.0 / 9.0
        }
        return 16.0 / 9.0
    }

    var body: some View {
        ZStack {
            Color.black
            if let image {
                Image(uiImage: UIImage(cgImage: image))
                    .resizable()
                    .scaledToFit()
            } else {
                ProgressView().tint(.white.opacity(0.7))
            }
            Button(action: onPlay) {
                Image(systemName: "play.fill")
                    .font(.system(size: 30, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 68, height: 68)
                    .background(Color.black.opacity(0.45), in: Circle())
                    .overlay(Circle().strokeBorder(.white.opacity(0.7), lineWidth: 1.5))
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("播放"))
        }
        .aspectRatio(aspect, contentMode: .fit)
        .clipShape(Rectangle())
        .contentShape(Rectangle())
        // 播放钮外整框点按同样进播放(Photos 同感)
        .onTapGesture { onPlay() }
        .task(id: "\(entry.path)|\(handle.handle)") {
            image = nil
            guard let thumbUrl, let loaded = await ThumbImageLoader.shared.image(for: thumbUrl) else { return }
            guard !Task.isCancelled else { return }
            image = loaded
            onMeta?(aspect)
        }
    }
}

/// 胶片条:兄弟图横向缩略图带(当前图 accent 高亮框;点选跳图),当前图
/// 变更即居中滚动跟随。缩略图 = 浏览行/磁贴同款机制(``PreviewHandlePool``
/// 引用计数 + ``BrowseThumbSlot`` NSCache 装载;LazyHStack 只装可视胞,
/// 大图库不整池打开)。单图(兄弟图 ≤1)由调用侧不渲染。
struct PreviewFilmstrip: View {
    let images: [DirEntry]
    let currentPath: String
    let pool: PreviewHandlePool
    let onSelect: (DirEntry) -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 6) {
                    ForEach(images, id: \.path) { entry in
                        FilmstripCell(entry: entry, isCurrent: entry.path == currentPath, pool: pool)
                            .id(entry.path)
                            .onTapGesture { onSelect(entry) }
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
            }
            .frame(height: 64)
            .onAppear {
                proxy.scrollTo(currentPath, anchor: .center)
            }
            .onChange(of: currentPath) { _, newPath in
                withAnimation(.easeInOut(duration: 0.18)) {
                    proxy.scrollTo(newPath, anchor: .center)
                }
            }
        }
    }
}

/// 胶片条胞:52×52 圆角缩略图(浏览缩略图槽复用;等比不裁切,同确认点③
/// 口径),当前图 = 2.5pt accent 描边。
private struct FilmstripCell: View {
    let entry: DirEntry
    let isCurrent: Bool
    let pool: PreviewHandlePool

    var body: some View {
        BrowseThumbSlot(
            entry: entry,
            pool: pool,
            badge: "",
            badgeFont: 8,
            badgeRadius: 2,
            badgeInset: 2,
            badgePadH: 3,
        ) {
            Image(systemName: "photo")
                .font(.system(size: 16))
                .foregroundStyle(Color(uiColor: .systemGray3))
        }
        .frame(width: 52, height: 52)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(isCurrent ? Color.accentColor : Color.clear, lineWidth: 2.5)
        )
        .contentShape(RoundedRectangle(cornerRadius: 6))
    }
}

/// 预览底部动作条(r48 #3 图片相册化引入,r49 #1 全容器统一):四图标
/// 钮(图标 + 10pt 小字标签)——下载/转发/用其他应用打开/查看信息,动作
/// 与「···」菜单同函数(单一事实源;句柄门同面,openAs 未开 = 灰置)。
struct PreviewActionBar: View {
    let onDownload: () -> Void
    let onForward: () -> Void
    let onOpenAs: () -> Void
    let openAsEnabled: Bool
    let onInfo: () -> Void

    var body: some View {
        HStack(spacing: 0) {
            button("下载", system: "square.and.arrow.down", action: onDownload)
            button("转发", system: "arrowshape.turn.up.right", action: onForward)
            button("用其他应用打开", system: "arrow.up.forward.app", action: onOpenAs, enabled: openAsEnabled)
            button("查看信息", system: "info.circle", action: onInfo)
        }
        .frame(maxWidth: .infinity)
        .frame(height: 54)
    }

    @ViewBuilder
    private func button(_ title: String, system: String, action: @escaping () -> Void, enabled: Bool = true) -> some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: system)
                    .font(.system(size: 21))
                Text(title)
                    .font(.system(size: 10))
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }
            .frame(maxWidth: .infinity)
            .foregroundStyle(enabled ? Color.accentColor : Color(uiColor: .systemGray3))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(Text(title))
    }
}
