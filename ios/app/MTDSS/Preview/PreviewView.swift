/// 全屏预览屏(V4-1 T25;钦定 mockup mobile-preview.html;安卓 T23 同构,
/// spec「四容器一分区」):浏览行/磁贴/收件箱子行/快捷树文件行/混合页文件
/// 行点入全屏推入(fullScreenCover),浅色骨架 + 深色观看面(mockup iOS 列
/// 系统栏白底;媒体容器画面区黑底同安卓):
/// - 图片 = 相册化(r48 #3 用户裁定「像 iphone 相册一样」):自研 zoom 查看
///   器(双指缩放/拖拽/中央滑切;thumb 先显 → raw 渐进,HEIC = jpeg 端点
///   优先)恒满屏居中——顶栏/胶片条/动作条全**覆盖层**化,显隐不参与布局
///   (点按进沉浸,图片位置纹丝不动);默认底 = 系统深浅色(浅色白),沉浸
///   态恒黑 + 状态栏隐;底部 = 四动作图标条(下载/转发/用其他应用打开/
///   查看信息)叠兄弟图胶片条(点选跳图);
/// - 视频 = 相册化(r49 #2 用户裁定):白底跟系统 + 中央海报框(缩略图
///   + 播放钮)点按进播放(WKWebView previewer 播放框,海报同比,autoPlay
///   入场即试播)+ 中央滑切兄弟视频 + 底部动作条 + 左缘右划返回;
/// - 音频 = previewer mountAudio(r51 #1:WKWebView 播放条卡片,http 直连
///   与视频同径;顶/底栏 r49 #1 统一);
/// - 文档(pdf/office/md/文本)= WKWebView 宿主页 + previewer bundle(自管
///   scheme 同源面,boot 经 WKUserScript 注入,形同安卓/桌面,
///   ``PreviewProxySchemeHandler``;底部动作条 + 左缘返回 r49 #1 统一);
/// - 不支持格式 = 类型页 + 下载引导(previewer unsupported 同词面;底
///   部动作条 r49 #1 统一)。
///
/// 「···」菜单(全容器):下载 / 转发 / 用其他应用打开(在场句柄 raw 拷
/// 缓存 + UIActivityViewController)/ 查看信息(名字·大小·路径·设备)。
///
/// 降级面:源离线 =「源设备离线,无法预览」(mockup 确认点⑤);T10
/// `icloud_not_local` 词透出 =「原件在 iCloud,需先下载」(远端 iPhone 源
/// 场景 + 自机照片库未卸载项同词,不静默全量拉);不支持 = 类型页。转发/
/// 下载 = 既有流入口(DevicePickerSheet / SendService 下载+本机 local_copy
/// 收件箱落点),发起即 toast,进度归传输列表。
///
/// 句柄生命周期:条目切换 = 旧句柄 close(先收后开)+ 新条目 open(在途
/// 取消不入账——迟到应答即弃并补 close,安卓 ensureActive 同款竞态闭合);
/// 屏面卸载全量收口(detached 不随生命周期取消;daemon 60s 空闲回收终极
/// 兜底)。图库切换集 = 兄弟集图片类([PreviewGallery],安卓同构)。

import MTDSSKit
import SwiftUI

/// 预览推入载荷(五入口共构:浏览行/磁贴 = BrowseView;收件箱子行/快捷树
/// 文件行 = FavoritesRootList;混合页文件行 = FavoritesMixedView)。siblings
/// = 所在列表兄弟集(展示序;图库 ‹› 切换集取其图片类,[PreviewGallery])。
struct PreviewLaunch: Identifiable {
    let id = UUID()
    /// 浏览源设备(nil = 本机源,引擎本机臂;远端 = 对端 id)。下载/转发
    /// 分档同浏览档(isRemote)。
    let deviceId: String?
    let isRemote: Bool
    /// 顶栏副文/信息页设备段(「3.3 MB · MIX 3」mockup 形后半)。
    let deviceLabel: String
    let entry: DirEntry
    let siblings: [DirEntry]
}

/// 句柄 UI 态(nil = 不开启——unsupported 容器零 preview_open)。
private enum PreviewHandleUi: Equatable {
    case loading
    case open(PreviewOpenResult)
    case failed(String)
}

/// 副标题文案(「3.3 MB · MIX 3」mockup 形;尺寸缺席 = 单段)。
func previewSubtitle(size: Int64, deviceLabel: String) -> String {
    if size > 0 {
        return "\(BrowseFormat.size(size)) · \(deviceLabel)"
    }
    return deviceLabel
}

/// 待「用其他应用打开」的缓存件(sheet 载荷;Identifiable 收口)。
private struct PreviewOpenAsFile: Identifiable {
    let id = UUID()
    let url: URL
}

struct PreviewView: View {
    let repo: MtdssRepository
    let launch: PreviewLaunch

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    /// 当前条目(图库 ‹› 切换面;初始 = 点入条目)。
    @State private var current: DirEntry
    @State private var handleUi: PreviewHandleUi? = .loading
    @State private var retryNonce = 0
    /// 图片容器沉浸态(r48 #3 相册化:点按切换——沉浸 = 恒黑底 + 顶/底
    /// chrome 与状态栏全隐;非图片容器无此态)。
    @State private var immersive = false
    /// 图片像素分辨率(field r24 #5 信息面板;onMeta 回填,切图清空)。
    @State private var imagePixels: String?
    /// 邻条目句柄预取仓(field r25 #2:滑切闪屏根因 = 换条目后 preview_open
    /// IPC 往返 + 全链重载;开图/开视频即预取前后邻,滑切命中仓 = 瞬切;
    /// r49 #2 扩视频滑切面)。未消费的预取句柄不显式收口——daemon 60s
    /// 空闲回收自清(量 ≤2/步)。
    @State private var prefetch: [String: PreviewOpenResult] = [:]
    /// 视频播放态(r49 #2 相册化:false = 海报框,点按进播放 true)。
    @State private var videoPlaying = false
    /// 视频框宽高比(海报回填;播放态 webview 同比框)。
    @State private var videoAspect: CGFloat = 16.0 / 9.0
    /// 胶片条缩略图句柄池(r48 #3:浏览磁贴同款接线,device/浏览档 init
    /// 定死;屏面卸载 releaseAll 全量收口——池按需开槽,不用不付成本)。
    private let stripPool: PreviewHandlePool
    /// 媒体 WebView 常驻盒(r53:音频/视频播放态切条目零页面重载——
    /// reboot 注新 boot,宿主页销毁旧挂载挂新)。
    private let mediaWebBox = PreviewWebViewBox()
    /// 媒体 WebView 已挂(r53:音频容器开句柄即挂且容器内恒驻;视频 =
    /// 播放会话内恒驻;loading/failed 以覆盖层叠,不再拆 WebView)。
    @State private var mediaWebLive = false
    // 弹层/反馈面
    @State private var showInfo = false
    @State private var showForward = false
    @State private var openAsFile: PreviewOpenAsFile?
    @State private var toast: String?
    @State private var actionError: String?
    @State private var actionBusy = false
    /// open-as 整件取数(V4-1 T25 fix round 1 Important #2:Kit 流式落盘面
    /// ——downloadTask 文件搬运,大件 mkv/avi 不整件入 RAM;目录自净/净化
    /// 名/传输缝单测归 Kit ``PreviewOpenAsFetcher``;r54 #4:进度缝 + 取消)。
    private let openAsFetcher = PreviewOpenAsFetcher()
    /// r54 #4:open-as 下载进度(0..1;nil = 不在途)。
    @State private var openAsProgress: Double?
    /// r54 #4:open-as 取数任务柄(HUD 取消钮消费)。
    @State private var openAsTask: Task<Void, Never>?

    init(repo: MtdssRepository, launch: PreviewLaunch) {
        self.repo = repo
        self.launch = launch
        _current = State(initialValue: launch.entry)
        // 胶片条池(浏览页同款接线:本机 = device_id 缺省走引擎本机臂;
        // 远端 = 对端 id + size/mtime 条目快照,句柄时变键)。
        let deviceId = launch.deviceId
        let remote = launch.isRemote
        stripPool = PreviewHandlePool(
            open: { entry in
                try await repo.previewOpen(
                    deviceId: remote ? deviceId : nil,
                    path: entry.path,
                    mtimeMs: entry.mtimeMs,
                    size: entry.size,
                )
            },
            close: { handle in
                try await repo.previewClose(handle: handle)
            },
        )
    }

    private var container: PreviewContainer {
        PreviewRoute.containerOf(name: current.name)
    }

    private var needsHandle: Bool {
        container != .unsupported
    }

    /// 图库切换集(‹› = 兄弟集图片类;mockup 确认点③;切集含当前条目兜底)。
    private var images: [DirEntry] {
        let sib = PreviewGallery.imageSiblings(launch.siblings)
        if sib.contains(where: { $0.path == launch.entry.path }) { return sib }
        return [launch.entry] + sib
    }

    private var imageIndex: Int? {
        images.firstIndex(where: { $0.path == current.path })
    }

    /// 视频切换集(r49 #2 相册化:兄弟集视频类,展示序;切集含当前条目
    /// 兜底——同 images 口径)。
    private var videos: [DirEntry] {
        let sib = PreviewGallery.videoSiblings(launch.siblings)
        if sib.contains(where: { $0.path == launch.entry.path }) { return sib }
        return [launch.entry] + sib
    }

    private var videoIndex: Int? {
        videos.firstIndex(where: { $0.path == current.path })
    }

    /// 音频切换集(r52 #4:兄弟集音频类,展示序;含当前条目兜底)。
    private var audios: [DirEntry] {
        let sib = PreviewGallery.audioSiblings(launch.siblings)
        if sib.contains(where: { $0.path == launch.entry.path }) { return sib }
        return [launch.entry] + sib
    }

    private var audioIndex: Int? {
        audios.firstIndex(where: { $0.path == current.path })
    }

    var body: some View {
        Group {
            if container == .image {
                imageScreen
            } else {
                // r49 #1 全容器统一:顶栏 + 内容 + 底部动作条(图片页
                // chrome 覆盖层化另走 imageScreen)
                VStack(spacing: 0) {
                    topBar
                    content
                    bottomActionBar
                }
            }
        }
        .background(Color(uiColor: .systemBackground))
        .toast($toast)
        // 句柄生命周期:条目切换/重试 = 旧句柄先收后开(在途取消不入账);
        // 屏面卸载全量收口。任务键 = path|容器分档|重试(unsupported 翻档
        // 同收口——容器恒随条目,分档位兜底防词表外扩串档)。
        .task(id: "\(current.path)|\(needsHandle ? "f" : "u")|\(retryNonce)") {
            await openHandleIfNeeded()
        }
        // r53:海报点按进播放 = 播放会话首挂 WebView(boot 由视图分支
        // 构造期注入承载;句柄已在场,无需重开)
        .onChange(of: videoPlaying) { _, playing in
            if playing && !mediaWebLive && container == .video {
                mediaWebLive = true
            }
        }
        .onDisappear {
            // 收口面:在场句柄 close + 胶片条池全量收口(detached 不随视图
            // 生命周期取消,排队收口必须走完;BrowseView onDisappear 同款
            // 纪律)
            closePresentHandle()
            let pool = stripPool
            Task.detached { await pool.releaseAll() }
        }
        // 信息面板(field r24 #5:alert → sheet;类型/分辨率/ACM 三时间,
        // 格式化行;分辨率 = 图片容器加载后 onMeta 回填)。
        .sheet(isPresented: $showInfo) {
            PreviewInfoSheet(
                entry: current,
                deviceLabel: launch.deviceLabel,
                container: container,
                resolution: imagePixels
            )
        }
        // 转发弹层(分档 = 浏览档;来源设备前置滤,daemon 拒 from==to)
        .sheet(isPresented: $showForward) {
            DevicePickerSheet(
                title: launch.isRemote ? "转发 1 项到…" : "发送 1 项到…",
                repo: repo,
                selfDeviceId: model.selfDeviceId,
                excluding: launch.isRemote ? launch.deviceId : nil,
            ) { device in
                doForward(to: device)
            }
        }
        // 用其他应用打开(系统分享面;完成即收)
        .sheet(item: $openAsFile) { file in
            ActivitySheet(url: file.url) { openAsFile = nil }
        }
        // r54 #4:open-as 下载进度 HUD(百分比 + 取消;远端大件下载可见)
        .overlay {
            if let progress = openAsProgress {
                openAsHud(progress)
            }
        }
        // 动作失败提示(下载/转发/打开)
        .alert(
            "操作失败",
            isPresented: Binding(
                get: { actionError != nil },
                set: { if !$0 { actionError = nil } },
            ),
        ) {
            Button("好", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
    }

    // MARK: 内容分区(四容器一分区;图片容器走 ``imageScreen`` 覆盖层化,
    // 不在此面——image 分支防御性空置)

    /// r54 #4:open-as 下载进度 HUD——居中小卡(不确定态转圈/已知总长
    /// 百分比进度条)+ 取消钮。
    @ViewBuilder private func openAsHud(_ progress: Double) -> some View {
        ZStack {
            Color.black.opacity(0.25)
                .ignoresSafeArea()
            VStack(spacing: 12) {
                if progress > 0 {
                    ProgressView(value: progress)
                        .frame(width: 200)
                    Text("\(Int((progress * 100).rounded()))%")
                        .font(.system(size: 13, weight: .medium))
                        .monospacedDigit()
                } else {
                    ProgressView()
                }
                Text("正在准备文件…")
                    .font(.system(size: 12))
                    .foregroundStyle(Color(uiColor: .systemGray))
                Button("取消") {
                    openAsTask?.cancel()
                }
                .font(.system(size: 14, weight: .medium))
            }
            .padding(20)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
            .padding(40)
        }
    }

    @ViewBuilder private var content: some View {
        ZStack {
            switch container {
            case .image:
                EmptyView() // 图片容器在 body 分流进 imageScreen,不可达(防御)
            case .video:
                videoContent
            case .audio:
                audioContent
            case .doc:
                docContent
            case .unsupported:
                // 黑底面外:类型页浅底(mockup iOS 列白底骨架;页词面同安卓)
                PreviewUnsupportedFace(entry: current, onDownload: doDownload)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color(uiColor: .systemBackground))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // r49 #1 全容器统一左缘右划返回:原生边缘带(r25 #3 的容器级
        // 手势在 WKWebView/AVPlayer 等 UIKit 内嵌面上会被吃掉,边缘带
        // 恒可用;图片容器由查看器内部分流,不叠此件)
        .overlay(alignment: .leading) {
            PreviewEdgeBackZone { dismiss() }
        }
    }

    /// 非图片容器底部动作条(r49 #1 全容器统一;材质伸进底部安全区)。
    private var bottomActionBar: some View {
        PreviewActionBar(
            onDownload: doDownload,
            onForward: { showForward = true },
            onOpenAs: doOpenAs,
            openAsEnabled: true, // r54 #4:全类型常开(句柄自足)
            onInfo: { showInfo = true }
        )
        .background(.regularMaterial, ignoresSafeAreaEdges: .bottom)
    }

    // MARK: 图片屏(r48 #3 相册化:chrome 全覆盖层化——图片面恒满屏居中,
    // 顶栏/胶片条/动作条显隐不参与布局,点按进沉浸图片位置纹丝不动)

    /// 图片屏骨架:底色(默认 = 系统深浅色,浅色白;沉浸恒黑)+ 满屏图面
    /// + chrome 覆盖层;状态栏随沉浸隐。**图面与底色双 ignoresSafeArea**
    /// ——状态栏/主页指示条显隐不改布局,「图片位置不动」的另一半。
    private var imageScreen: some View {
        ZStack {
            (immersive ? Color.black : Color(uiColor: .systemBackground))
                .ignoresSafeArea()
            imageViewerFace
                .ignoresSafeArea()
            chromeOverlay
        }
        .statusBarHidden(immersive)
        .animation(.easeInOut(duration: 0.22), value: immersive)
    }

    /// 图面(满屏):查看器/失败/加载三态;失败/加载面配色随沉浸
    /// (黑底白字/浅底主色字)。
    @ViewBuilder private var imageViewerFace: some View {
        switch handleUi {
        case .open(let result):
            PreviewImageViewer(
                entry: current,
                handle: result,
                onSwipe: { delta in stepImage(delta) },
                onEdgeBack: { dismiss() },
                onToggleChrome: { immersive.toggle() },
                onMeta: { w, h in imagePixels = "\(w) × \(h)" },
                faceDark: immersive
            )
            .id("\(current.path)|\(result.handle)")
        case .failed(let face):
            PreviewErrorFace(message: face, onRetry: { retryNonce += 1 }, dark: immersive)
        case .loading, .none:
            PreviewLoadingFace(dark: immersive)
        }
    }

    /// chrome 覆盖层:顶栏 + 底部(胶片条 + 动作条);沉浸态全隐(渐隐
    /// 过渡)。中央区域不挡点按(Spacer 非命中面,点按直达查看器;顶/底
    /// 条自身的按钮/滚动归其面)。
    @ViewBuilder private var chromeOverlay: some View {
        VStack(spacing: 0) {
            if !immersive {
                topBar
                    // 材质伸进顶部安全区(状态栏背后;图面满屏下方的可读面)
                    .background(.regularMaterial, ignoresSafeAreaEdges: .top)
                    .transition(.opacity)
            }
            Spacer()
            if !immersive {
                imageBottomChrome
                    // 材质伸进底部安全区(主页指示条背后;图面满屏下方的可读面)
                    .background(.regularMaterial, ignoresSafeAreaEdges: .bottom)
                    .transition(.opacity)
            }
        }
    }

    /// 底部 chrome:胶片条(兄弟图 >1 才显;点选跳图)叠四动作图标条
    /// (动作与「···」菜单同函数,单一事实源)。
    @ViewBuilder private var imageBottomChrome: some View {
        VStack(spacing: 0) {
            if images.count > 1 {
                PreviewFilmstrip(
                    images: images,
                    currentPath: current.path,
                    pool: stripPool,
                    onSelect: { entry in jumpToImage(entry) }
                )
            }
            PreviewActionBar(
                onDownload: doDownload,
                onForward: { showForward = true },
                onOpenAs: doOpenAs,
                openAsEnabled: true, // r54 #4:全类型常开(句柄自足)
                onInfo: { showInfo = true }
            )
        }
    }

    // MARK: 视频屏(r49 #2 相册化:白底跟系统 + 中央海报框点按进播放 +
    // 中央滑切切换 + 底部动作条[body 外层统一];左缘返回 = 边缘带)

    /// 视频面:海报态(缩略图 + 播放钮)/播放态(WKWebView previewer
    /// 播放框,海报同比;autoPlay 旗入场即试播,WebKit 手势策略拒则
    /// 控制条单击)。中央横向滑切切换兄弟视频(播放态 webview 区内的
    /// 手势归 WebKit,滑切在框外白面/上下留白恒可用)。
    @ViewBuilder private var videoContent: some View {
        ZStack {
            Color(uiColor: .systemBackground)
            // r53:播放会话内 WebView 恒驻(**不随句柄态卸载**——loading/
            // failed 以覆盖层叠;首挂构造期注入,切视频经 mediaWebBox.reboot
            // 换面零重载,滑切闪屏与音频同修)
            if videoPlaying && mediaWebLive {
                PreviewDocViewer(injectionJs: initialMediaBootJs, box: mediaWebBox)
                    .aspectRatio(videoAspect, contentMode: .fit)
                    .background(Color.black)
            }
            switch handleUi {
            case .open(let result):
                if !videoPlaying {
                    PreviewVideoPosterFace(
                        entry: current,
                        handle: result,
                        onPlay: { videoPlaying = true },
                        onMeta: { videoAspect = $0 }
                    )
                    // r50 #3(用户裁定「去掉白边」):海报框与播放框同形全宽
                    // (r49 的 8pt 横距 = 白页两侧露白根因)
                }
            case .failed(let face):
                // 播放态 = 深字覆盖旧画面;海报态 = 浅底主色字
                PreviewErrorFace(message: face, onRetry: { retryNonce += 1 }, dark: videoPlaying)
            case .loading, .none:
                if videoPlaying {
                    Color.black.opacity(0.35)
                    PreviewLoadingFace()
                } else {
                    PreviewLoadingFace(dark: false)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .gesture(
            DragGesture(minimumDistance: 24).onEnded { value in
                let dx = value.translation.width
                let dy = value.translation.height
                guard abs(dx) > 60, abs(dx) > abs(dy) * 1.6 else { return }
                stepVideo(dx < 0 ? 1 : -1)
            }
        )
    }

    /// 音频面(r51 #1:AVPlayer 直连撤——r24 实锤 AVPlayer 真机系统性失败
    /// [格式无关],视频 r31b 已挪 WKWebView,音频同径):previewer
    /// mountAudio(`<audio controls>` http 直连,与视频 r40 同面);白底
    /// 居中播放条卡片(顶栏/底栏由 body 统一)。r52 #4:中央横滑切换
    /// 兄弟音频(webview 条内手势归 WebKit,白面/留白区恒可用——与视频
    /// 播放态同边界)。
    @ViewBuilder private var audioContent: some View {
        ZStack {
            Color(uiColor: .systemBackground)
            if mediaWebLive {
                // r53:WebView 容器内恒驻(首挂构造期注入;切条目经
                // mediaWebBox.reboot 换面,零页面重载——滑切闪屏根修);
                // 卡面高度随 r53 音频卡(previewer 黑底舞台)
                PreviewDocViewer(injectionJs: initialMediaBootJs, box: mediaWebBox)
                    .frame(height: 200)
            }
            // loading/failed = 覆盖层(r53:不再拆 WebView——切换在途的
            // 短窗里旧卡面驻留,优于白闪)
            if case .loading = handleUi {
                PreviewLoadingFace(dark: false)
            }
            if case .failed(let face) = handleUi {
                PreviewErrorFace(message: face, onRetry: { retryNonce += 1 }, dark: false)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .gesture(
            DragGesture(minimumDistance: 24).onEnded { value in
                let dx = value.translation.width
                let dy = value.translation.height
                guard abs(dx) > 60, abs(dx) > abs(dy) * 1.6 else { return }
                stepAudio(dx < 0 ? 1 : -1)
            }
        )
    }

    @ViewBuilder private var docContent: some View {
        ZStack {
            Color(uiColor: .systemBackground)
            switch handleUi {
            case .open(let result):
                // key(js) 形(安卓同构):boot 变 = 重建 WebView(旧体
                // dismantleUIView 收口)
                PreviewDocViewer(injectionJs: docInjectionJs(result))
                    .id(docInjectionJs(result))
            case .failed(let face):
                // 浅底面:主色字(dark:false——容器面即宿主页浅底)
                PreviewErrorFace(message: face, onRetry: { retryNonce += 1 }, dark: false)
            case .loading, .none:
                PreviewLoadingFace(dark: false)
            }
        }
    }

    /// boot 注入 JS 单点([PreviewBoot] 形 = 桌面 popout / 安卓 PreviewBoot
    /// 逐字同;raw 走自管 scheme 面,``PreviewRoute/proxyRawUrl(port:handle:
    /// token:)``)。视频非 AV 原生容器复用本容器消费源端(r31b:AVPlayer
    /// 直连真机系统性失败,视频恒走 previewer webview)。r49 #2:autoPlay
    /// 旗(海报点按进播放,入场即试播)。
    private func docInjectionJs(_ result: PreviewOpenResult, forceKind: String? = nil, autoPlay: Bool = false) -> String {
        PreviewBoot.injectionJs(
            PreviewBoot.bootJson(
                reqJson: PreviewBoot.requestJson(
                    kind: forceKind ?? PreviewRoute.docKindOf(name: current.name) ?? "text",
                    // r40(用户裁定「只要流式,不做下载再播放」):视频/音频
                    // 播放地址 = **http 回环直连**(媒体元素跨源 HTTP 不受
                    // CORS——Range/206 流式本行;自定义 scheme 慢流挂 =
                    // r38 病灶,与此不同路)。文档族仍走同源代理(fetch 受
                    // CORS)。
                    rawUrl: forceKind == "video" || forceKind == "audio"
                        ? PreviewRoute.rawUrl(port: result.port, handle: result.handle, token: result.token)
                        : PreviewRoute.proxyRawUrl(port: result.port, handle: result.handle, token: result.token),
                    name: current.name,
                    size: current.size,
                    // r35 直读测试轮(撤转码消费)——不携 hls;视频形态
                    // 恒流式(r40:http 直连;blob 旗撤,预览器机器休眠)。
                    hlsUrl: nil,
                    thumbUrl: forceKind == "video"
                        ? PreviewRoute.proxyThumbUrl(port: result.port, handle: result.handle, token: result.token)
                        : nil,
                    // r45:封面持久缓存键(previewer IndexedDB 面)
                    entryKey: "\(current.path)|\(current.mtimeMs)|\(current.size)",
                    // r49 #2:点按进播放的自动播旗
                    autoPlay: autoPlay
                ),
                handle: result.handle,
            ),
        )
    }

    // MARK: 顶栏(mockup:‹ / 名字+副文 / ···;浅底,iOS 列系统栏白底)

    private var topBar: some View {
        ZStack {
            HStack(spacing: 12) {
                Button {
                    dismiss()
                } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 18, weight: .bold))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 30, height: 34)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("返回"))
                Spacer()
                Menu {
                    menuContent
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 17, weight: .bold))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 34, height: 34)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel(Text("更多"))
            }
            VStack(spacing: 1) {
                Text(current.name)
                    .font(.system(size: 16, weight: .semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .padding(.horizontal, 60)
                Text(previewSubtitle(size: current.size, deviceLabel: launch.deviceLabel))
                    .font(.system(size: 12))
                    .foregroundStyle(Color(uiColor: .systemGray))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
    }

    /// 「···」菜单(全容器;下载/转发/用其他应用打开/查看信息——图片
    /// 容器底部动作条同函数单一事实源。r48 #3:撤「全屏查看」项,点按进
    /// 沉浸已覆盖,菜单不再重复)。
    @ViewBuilder private var menuContent: some View {
        Button("下载") { doDownload() }
        Button("转发") { showForward = true }
        Button("用其他应用打开", action: doOpenAs) // r54 #4:全类型常开
        Button("查看信息") { showInfo = true }
    }

    // MARK: 图库切换

    private func stepImage(_ delta: Int) {
        let imgs = images
        guard !imgs.isEmpty else { return }
        let base = imageIndex ?? 0
        let next = imgs[PreviewGallery.wrapIndex(base + delta, imgs.count)]
        guard next.path != current.path else { return }
        imagePixels = nil // 分辨率随图换新(onMeta 重新回填)
        current = next
        prefetchNeighbors()
    }

    /// 胶片条点选跳图(r48 #3;与滑切同链:分辨率重置 + 预取邻图,
    /// openHandleIfNeeded 预取仓命中即瞬切)。
    private func jumpToImage(_ entry: DirEntry) {
        guard entry.path != current.path else { return }
        imagePixels = nil
        current = entry
        prefetchNeighbors()
    }

    /// 视频滑切(r49 #2:与图片滑切同构——环绕 + 邻预取;r53 播放态
    /// **保持**:播着切 = 下一支自动续播[WebView 恒驻换面],海报态切 =
    /// 下一支出海报)。
    private func stepVideo(_ delta: Int) {
        let vids = videos
        guard !vids.isEmpty else { return }
        let base = videoIndex ?? 0
        let next = vids[PreviewGallery.wrapIndex(base + delta, vids.count)]
        guard next.path != current.path else { return }
        current = next
        prefetchNeighbors()
    }

    /// 音频滑切(r52 #4:同构——环绕 + 邻预取;切换即停当前换新,
    /// 新条目出控制条待播)。
    private func stepAudio(_ delta: Int) {
        let auds = audios
        guard !auds.isEmpty else { return }
        let base = audioIndex ?? 0
        let next = auds[PreviewGallery.wrapIndex(base + delta, auds.count)]
        guard next.path != current.path else { return }
        current = next
        prefetchNeighbors()
    }

    /// 邻图预取(field r25 #2):当前图前后邻的 preview_open 发 preemptive
    /// IPC,结果入仓;切图时 openHandleIfNeeded 命中仓即免往返(闪屏主
    /// 根因)。仓键 = path;预取失败静默(切图走正常开路)。
    private func prefetchNeighbors() {
        // r49 #2/r52 #4:预取扩视频/音频(滑切同患);切换集按容器取类
        guard container == .image || container == .video || container == .audio else { return }
        let siblings =
            container == .image ? images : (container == .video ? videos : audios)
        let base =
            container == .image
            ? (imageIndex ?? 0)
            : (container == .video ? (videoIndex ?? 0) : (audioIndex ?? 0))
        guard siblings.count > 1 else { return }
        for delta in [-1, 1] {
            let neighbor = siblings[PreviewGallery.wrapIndex(base + delta, siblings.count)]
            guard neighbor.path != current.path, prefetch[neighbor.path] == nil else { continue }
            let path = neighbor.path
            let mtime = neighbor.mtimeMs
            let size = neighbor.size
            let deviceId = launch.isRemote ? launch.deviceId : nil
            Task {
                if let result = try? await repo.previewOpen(
                    deviceId: deviceId, path: path, mtimeMs: mtime, size: size
                ) {
                    prefetch[path] = result
                }
            }
        }
    }

    // MARK: 句柄生命周期(条目切换旧 close 新 open;屏面卸载全量收口)

    private func openHandleIfNeeded() async {
        if !needsHandle {
            // 翻入 unsupported(词表外扩/切条目):旧在场句柄同收口(新任务
            // 接棒旧任务的账——旧任务已完结,收口归本臂)
            closePresentHandle()
            handleUi = nil
            return
        }
        // 换条目/重试:旧在场句柄先收(先收后开;detached 不随生命周期取消)
        closePresentHandle()
        handleUi = .loading
        // field r25 #2:预取仓命中 = 免 IPC 往返瞬切(取走即清,防陈句柄)
        if let hit = prefetch.removeValue(forKey: current.path) {
            handleUi = .open(hit)
            mediaWebSwapIfNeeded(hit)
            prefetchNeighbors()
            return
        }
        do {
            let result = try await repo.previewOpen(
                deviceId: launch.isRemote ? launch.deviceId : nil,
                path: current.path,
                mtimeMs: current.mtimeMs,
                size: current.size,
            )
            // 条目已切走/屏面已卸 = 迟到应答不入账(在途 IPC 应答即弃,补
            // close 闭合竞态——安卓 ensureActive 同款)
            guard !Task.isCancelled else {
                let handle = result.handle
                Task.detached { try? await repo.previewClose(handle: handle) }
                return
            }
            handleUi = .open(result)
            mediaWebSwapIfNeeded(result)
            prefetchNeighbors() // field r25 #2:首开即预取邻图(滑切瞬切)
        } catch {
            guard !Task.isCancelled else { return }
            handleUi = .failed(PreviewErrors.userFace(BrowseViewModel.userErrorText(for: error)))
        }
    }

    /// 在场句柄收口(nil 安全;handleUi 复位 loading 由调用臂决定)。
    private func closePresentHandle() {
        if case .open(let prior) = handleUi {
            handleUi = nil
            let handle = prior.handle
            Task.detached { try? await repo.previewClose(handle: handle) }
        }
    }

    /// 媒体 WebView 首挂注入(r53:构造期一次性——挂载渲染拍在场句柄的
    /// boot;后续换面全走 ``mediaWebSwapIfNeeded`` 的 reboot)。
    private var initialMediaBootJs: String {
        if case .open(let result) = handleUi {
            return docInjectionJs(
                result,
                forceKind: container == .video ? "video" : "audio",
                autoPlay: videoPlaying
            )
        }
        return ""
    }

    /// r53 媒体 WebView 换面/首挂:音频容器恒用;视频仅播放态。首挂 =
    /// 翻 live 旗(boot 由视图分支构造期注入承载);在场 = reboot 注新
    /// boot(宿主页销毁旧挂载挂新,零页面重载)。
    private func mediaWebSwapIfNeeded(_ result: PreviewOpenResult) {
        guard container == .audio || container == .video else { return }
        if container == .video && !videoPlaying { return }
        if mediaWebLive {
            mediaWebBox.reboot(
                docInjectionJs(
                    result,
                    forceKind: container == .video ? "video" : "audio",
                    autoPlay: videoPlaying
                )
            )
        } else {
            mediaWebLive = true
        }
    }

    // MARK: 动作(下载/转发/用其他应用打开;既有流入口,brief 钉)

    /// 下载到本机收件箱(远端 = SendService.download recv_dir 落点;本机 =
    /// local_copy dir=收件箱绝对路径, FavoritesView.downloadTreeRow 同口径;
    /// 90s 受理限时同 SendService.transferStartTimeout)。
    private func doDownload() {
        guard !actionBusy else { return }
        actionBusy = true
        let path = current.path
        let isRemote = launch.isRemote
        let deviceId = launch.deviceId
        let recvDir = model.recvDir
        Task {
            defer { actionBusy = false }
            do {
                if isRemote {
                    _ = try await SendService(repo: repo)
                        .download(device: deviceId ?? "", paths: [path])
                } else {
                    guard let recvDir, !recvDir.isEmpty else {
                        throw RpcError(code: 1, message: "下载发起失败,请重试。")
                    }
                    _ = try await RpcGuard.withTimeout(seconds: SendService.transferStartTimeout) { [repo] in
                        try await repo.transferStartLocalCopy(paths: [path], dir: recvDir)
                    }
                }
                toast = "已开始 1 项 · 进度见传输列表"
            } catch {
                actionError = BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    /// 转发到设备 B(选设备弹层点选后;分档 = 浏览档:远端 = remote_copy
    /// 数据不经操作端,本机 = upload 收件箱哨兵——BrowseView startSend/
    /// startForward 同口径)。
    private func doForward(to device: DirectoryDevice) {
        guard !actionBusy else { return }
        actionBusy = true
        let path = current.path
        let isRemote = launch.isRemote
        let from = launch.deviceId ?? ""
        Task {
            defer { actionBusy = false }
            do {
                let send = SendService(repo: repo)
                if isRemote {
                    _ = try await send.forward(from: from, paths: [path], to: device)
                } else {
                    _ = try await send.send(paths: [path], to: device)
                }
                toast = "已开始 1 项 · 进度见传输列表"
            } catch {
                actionError = BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    /// 用其他应用打开(在场句柄 raw 拷缓存 + 系统分享面;安卓 doOpenAs 同构
    /// ——目录自净 + 名字净化,扩展名保留 = 系统打开器路由依据)。整件取数
    /// = Kit [PreviewOpenAsFetcher](downloadTask 流式落盘;fix round 1
    /// Important #2:Data 全量缓冲撤,大件不整件入 RAM)。
    /// 用其他应用打开(r54 #4 用户裁定全类型常开):**句柄自足**——
    /// 支持预览的类型用在场句柄;不支持的类型现开临时句柄(取毕即关)。
    /// 全文件整件下载(Caches/preview-open,每次清空重下 = B 案钉死)→
    /// 系统分享面;进度 HUD(百分比+取消)根除「点了没反应」观感(远端
    /// 大件下载本来就长,静默 = 像没反应)。
    private func doOpenAs() {
        guard !actionBusy else { return }
        actionBusy = true
        openAsProgress = 0
        let entry = current
        let isRemote = launch.isRemote
        let deviceId = launch.deviceId
        openAsTask = Task {
            defer {
                actionBusy = false
                openAsProgress = nil
                openAsTask = nil
            }
            do {
                // r54 #4:句柄自足(不支持类型现开临时句柄,取毕即关)
                let result: PreviewOpenResult
                let tempHandle: String?
                if case .open(let present) = handleUi {
                    result = present
                    tempHandle = nil
                } else {
                    let opened = try await repo.previewOpen(
                        deviceId: isRemote ? deviceId : nil,
                        path: entry.path,
                        mtimeMs: entry.mtimeMs,
                        size: entry.size,
                    )
                    result = opened
                    tempHandle = opened.handle
                }
                let raw: URL
                if let raw0 = URL(string: PreviewRoute.rawUrl(port: result.port, handle: result.handle, token: result.token)) {
                    raw = raw0
                } else {
                    throw RpcError(code: 1, message: "句柄地址非法")
                }
                let file = try await openAsFetcher.fetch(entryName: entry.name, rawUrl: raw) { written, total in
                    Task { @MainActor in
                        openAsProgress = total > 0 ? Double(written) / Double(total) : 0
                    }
                }
                if let tempHandle {
                    let h = tempHandle
                    Task.detached { try? await repo.previewClose(handle: h) }
                }
                openAsFile = PreviewOpenAsFile(url: file)
            } catch {
                guard !Task.isCancelled else { return } // 取消:静默收场
                if let urlErr = error as? URLError, urlErr.code == .cancelled { return }
                actionError = "打开失败:" + BrowseViewModel.userErrorText(for: error)
            }
        }
    }
}
