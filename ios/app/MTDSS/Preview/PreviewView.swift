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
/// - 视频/音频 = AVPlayer 原流直连(raw?t=;移动端原生解码不经转码——spec
///   裁定,VideoPlayer 标准控制器);
/// - 文档(pdf/office/md/文本)= WKWebView 宿主页 + previewer bundle(自管
///   scheme 同源面,boot 经 WKUserScript 注入,形同安卓/桌面,
///   ``PreviewProxySchemeHandler``);
/// - 不支持格式 = 类型页 + 下载引导(previewer unsupported 同词面)。
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
    /// 邻图句柄预取仓(field r25 #2:滑切闪屏根因 = 换图后 preview_open
    /// IPC 往返 + 全链重载;图开即预取前后邻,stepImage 命中仓 = 瞬切)。
    /// 未消费的预取句柄不显式收口——daemon 60s 空闲回收自清(量 ≤2/步)。
    @State private var prefetch: [String: PreviewOpenResult] = [:]
    /// 胶片条缩略图句柄池(r48 #3:浏览磁贴同款接线,device/浏览档 init
    /// 定死;屏面卸载 releaseAll 全量收口——池按需开槽,不用不付成本)。
    private let stripPool: PreviewHandlePool
    // 弹层/反馈面
    @State private var showInfo = false
    @State private var showForward = false
    @State private var openAsFile: PreviewOpenAsFile?
    @State private var toast: String?
    @State private var actionError: String?
    @State private var actionBusy = false
    /// open-as 整件取数(V4-1 T25 fix round 1 Important #2:Kit 流式落盘面
    /// ——downloadTask 文件搬运,大件 mkv/avi 不整件入 RAM;目录自净/净化
    /// 名/传输缝单测归 Kit ``PreviewOpenAsFetcher``)。
    private let openAsFetcher = PreviewOpenAsFetcher()

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

    var body: some View {
        Group {
            if container == .image {
                imageScreen
            } else {
                VStack(spacing: 0) {
                    topBar
                    content
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

    @ViewBuilder private var content: some View {
        ZStack {
            switch container {
            case .image:
                EmptyView() // 图片容器在 body 分流进 imageScreen,不可达(防御)
            case .video, .audio:
                mediaContent
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
        // field r25 #3:非图片容器 = **左屏边缘右滑返回**(图片容器的边缘
        // 手势在查看器内部分流,同语义;中央横向手势不在此面)。
        .gesture(
            DragGesture(minimumDistance: 24).onEnded { value in
                let dx = value.translation.width
                let dy = value.translation.height
                guard value.startLocation.x < 28, dx > 60, abs(dx) > abs(dy) * 1.6 else { return }
                dismiss()
            }
        )
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
            PreviewImageActionBar(
                onDownload: doDownload,
                onForward: { showForward = true },
                onOpenAs: doOpenAs,
                openAsEnabled: handleOpen,
                onInfo: { showInfo = true }
            )
        }
    }

    @ViewBuilder private var mediaContent: some View {
        ZStack {
            Color.black
            switch handleUi {
            case .open(let result):
                // r31b(用户裁定「和 PC 统一,都用 HLS」):视频恒走源端
                // HLS 消费(doc 容器 previewer hls.js 面)——mp4 同转码,
                // 源端 H.264 统一出口;AVPlayer 直连路真机系统性失败
                // (格式无关)随撤。audio 仍 AVPlayer 原生。
                if container == .video {
                    PreviewDocViewer(injectionJs: docInjectionJs(result, forceKind: "video"))
                        .id(docInjectionJs(result, forceKind: "video"))
                } else if let url = URL(string: PreviewRoute.rawUrl(port: result.port, handle: result.handle, token: result.token)) {
                    PreviewMediaViewer(url: url)
                } else {
                    PreviewErrorFace(message: PreviewErrors.genericFace) { retryNonce += 1 }
                }
            case .failed(let face):
                PreviewErrorFace(message: face) { retryNonce += 1 }
            case .loading, .none:
                PreviewLoadingFace()
            }
        }
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
    /// token:)``)。r31:视频非 AV 原生容器复用本容器消费源端 HLS——kind
    /// = video + urls {raw, hls, thumb}(previewer video 挂载面,与桌面同)。
    private func docInjectionJs(_ result: PreviewOpenResult, forceKind: String? = nil) -> String {
        PreviewBoot.injectionJs(
            PreviewBoot.bootJson(
                reqJson: PreviewBoot.requestJson(
                    kind: forceKind ?? PreviewRoute.docKindOf(name: current.name) ?? "text",
                    // r40(用户裁定「只要流式,不做下载再播放」):视频播放
                    // 地址 = **http 回环直连**(媒体进程跨源 HTTP 不受 CORS
                    // ——Range/206 流式本行;自定义 scheme 慢流挂 = r38 病
                    // 灶,与此不同路)。文档族仍走同源代理(fetch 受 CORS)。
                    rawUrl: forceKind == "video"
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
                    entryKey: "\(current.path)|\(current.mtimeMs)|\(current.size)"
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
        Button("用其他应用打开", action: doOpenAs)
            .disabled(!handleOpen)
        Button("查看信息") { showInfo = true }
    }

    private var handleOpen: Bool {
        if case .open = handleUi { return true }
        return false
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

    /// 邻图预取(field r25 #2):当前图前后邻的 preview_open 发 preemptive
    /// IPC,结果入仓;切图时 openHandleIfNeeded 命中仓即免往返(闪屏主
    /// 根因)。仓键 = path;预取失败静默(切图走正常开路)。
    private func prefetchNeighbors() {
        guard container == .image else { return }
        let imgs = images
        guard imgs.count > 1, let base = imageIndex else { return }
        for delta in [-1, 1] {
            let neighbor = imgs[PreviewGallery.wrapIndex(base + delta, imgs.count)]
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
    private func doOpenAs() {
        guard case .open(let result) = handleUi else { return }
        guard !actionBusy else { return }
        actionBusy = true
        let entry = current
        Task {
            defer { actionBusy = false }
            do {
                guard let raw = URL(string: PreviewRoute.rawUrl(port: result.port, handle: result.handle, token: result.token)) else {
                    throw RpcError(code: 1, message: "句柄地址非法")
                }
                let file = try await openAsFetcher.fetch(entryName: entry.name, rawUrl: raw)
                openAsFile = PreviewOpenAsFile(url: file)
            } catch {
                actionError = "打开失败:" + BrowseViewModel.userErrorText(for: error)
            }
        }
    }
}
