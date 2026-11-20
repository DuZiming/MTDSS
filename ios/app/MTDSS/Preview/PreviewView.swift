/// 全屏预览屏(V4-1 T25;钦定 mockup mobile-preview.html;安卓 T23 同构,
/// spec「四容器一分区」):浏览行/磁贴/收件箱子行/快捷树文件行/混合页文件
/// 行点入全屏推入(fullScreenCover),浅色骨架 + 深色观看面(mockup iOS 列
/// 系统栏白底;媒体容器画面区黑底同安卓):
/// - 图片 = 自研 zoom 查看器(双指缩放/拖拽/‹› 切换;thumb 先显 → raw
///   渐进,HEIC = jpeg 端点优先)+ 底部操作条 下载·转发·全屏·信息 + 页点
///   (mockup);
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
    @State private var fullscreen = false
    // 弹层/反馈面
    @State private var showInfo = false
    @State private var showForward = false
    @State private var openAsFile: PreviewOpenAsFile?
    @State private var toast: String?
    @State private var actionError: String?
    @State private var actionBusy = false

    init(repo: MtdssRepository, launch: PreviewLaunch) {
        self.repo = repo
        self.launch = launch
        _current = State(initialValue: launch.entry)
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
        VStack(spacing: 0) {
            if !fullscreen {
                topBar
            }
            content
            if !fullscreen, container == .image {
                bottomBar
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
            // 收口面:在场句柄 close(detached 不随视图生命周期取消,排队
            // 收口必须走完;BrowseView onDisappear 同款纪律)
            closePresentHandle()
        }
        // 信息弹层(名字·大小·路径·设备,brief 钉)
        .alert("信息", isPresented: $showInfo) {
            Button("好", role: .cancel) {}
        } message: {
            Text("名字:\(current.name)\n大小:\(BrowseFormat.size(current.size))\n路径:\(current.path)\n设备:\(launch.deviceLabel)")
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

    // MARK: 内容分区(四容器)

    @ViewBuilder private var content: some View {
        ZStack {
            switch container {
            case .image:
                imageContent
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
    }

    @ViewBuilder private var imageContent: some View {
        ZStack {
            Color.black
            switch handleUi {
            case .open(let result):
                PreviewImageViewer(entry: current, handle: result)
                    .id("\(current.path)|\(result.handle)")
            case .failed(let face):
                PreviewErrorFace(message: face) { retryNonce += 1 }
            case .loading, .none:
                PreviewLoadingFace()
            }
        }
        // 图库切换面(仅图片容器;mockup:‹› 两侧 + 底部页点;全屏中藏)
        .overlay(alignment: .leading) {
            if images.count > 1, !fullscreen {
                PreviewNavArrow(forward: false) { stepImage(-1) }
                    .padding(.leading, 4)
            }
        }
        .overlay(alignment: .trailing) {
            if images.count > 1, !fullscreen {
                PreviewNavArrow(forward: true) { stepImage(1) }
                    .padding(.trailing, 4)
            }
        }
        // 页点限 20 枚内(横向溢出面;大图库仅 ‹› 切换)
        .overlay(alignment: .bottom) {
            if images.count > 1, !fullscreen, images.count <= 20 {
                PreviewPageDots(count: images.count, active: imageIndex ?? 0)
                    .padding(.bottom, 14)
            }
        }
        // 全屏:任意点按回操作条(安卓 onTap 同面)
        .overlay {
            if fullscreen {
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture { fullscreen = false }
            }
        }
    }

    @ViewBuilder private var mediaContent: some View {
        ZStack {
            Color.black
            switch handleUi {
            case .open(let result):
                if let url = URL(string: PreviewRoute.rawUrl(port: result.port, handle: result.handle, token: result.token)) {
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
    /// token:)``)。
    private func docInjectionJs(_ result: PreviewOpenResult) -> String {
        PreviewBoot.injectionJs(
            PreviewBoot.bootJson(
                reqJson: PreviewBoot.requestJson(
                    kind: PreviewRoute.docKindOf(name: current.name) ?? "text",
                    rawUrl: PreviewRoute.proxyRawUrl(port: result.port, handle: result.handle, token: result.token),
                    name: current.name,
                    size: current.size,
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

    /// 「···」菜单(全容器;安卓四项词面逐字):下载/转发/用其他应用打开
    /// (在场句柄才示能)/ 查看信息。
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

    // MARK: 底部操作条(仅图片容器,mockup:下载·转发·全屏·信息 均布)

    private var bottomBar: some View {
        HStack(spacing: 0) {
            barAction(icon: "square.and.arrow.down", label: "下载", action: doDownload)
            barAction(icon: "square.and.arrow.up", label: "转发") { showForward = true }
            barAction(icon: "arrow.up.left.and.arrow.down.right", label: "全屏") { fullscreen = true }
            barAction(icon: "info.circle", label: "信息") { showInfo = true }
        }
        .padding(.vertical, 10)
        .padding(.bottom, 4)
    }

    /// 底栏单动作(字形 + 10.5pt 字面;整格触面,安卓度量同值)。
    private func barAction(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: icon)
                    .font(.system(size: 20))
                    .foregroundStyle(Color.accentColor)
                    .frame(height: 24)
                Text(label)
                    .font(.system(size: 10.5))
                    .foregroundStyle(Color(uiColor: .systemGray))
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: 图库切换

    private func stepImage(_ delta: Int) {
        let imgs = images
        guard !imgs.isEmpty else { return }
        let base = imageIndex ?? 0
        current = imgs[PreviewGallery.wrapIndex(base + delta, imgs.count)]
    }

    // MARK: 句柄生命周期(条目切换旧 close 新 open;屏面卸载全量收口)

    private func openHandleIfNeeded() async {
        if !needsHandle {
            handleUi = nil
            return
        }
        // 换条目/重试:旧在场句柄先收(先收后开;detached 不随生命周期取消)
        closePresentHandle()
        handleUi = .loading
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
    /// ——目录自净 + 名字净化,扩展名保留 = 系统打开器路由依据)。
    private func doOpenAs() {
        guard case .open(let result) = handleUi else { return }
        guard !actionBusy else { return }
        actionBusy = true
        let entry = current
        Task {
            defer { actionBusy = false }
            do {
                let url = try await fetchOpenAsFile(entry: entry, handle: result)
                openAsFile = PreviewOpenAsFile(url: url)
            } catch {
                actionError = "打开失败:" + BrowseViewModel.userErrorText(for: error)
            }
        }
    }

    /// 整件拷取到缓存(目录自净:打开器只消费当次拷贝,旧件不留;文件名 =
    /// 时间戳前缀 + 净化原名,非法路径字符落下划线,安卓同象)。
    private func fetchOpenAsFile(entry: DirEntry, handle: PreviewOpenResult) async throws -> URL {
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("preview-open", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if let olds = try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) {
            for old in olds {
                try? FileManager.default.removeItem(at: old)
            }
        }
        let safe = entry.name.replacingOccurrences(
            of: "[\\\\/:*?\"<>|]",
            with: "_",
            options: .regularExpression,
        )
        let stampMs = Int64(Date().timeIntervalSince1970 * 1000)
        let file = dir.appendingPathComponent("\(stampMs)-\(String(safe.suffix(120)))")
        guard let url = URL(string: PreviewRoute.rawUrl(port: handle.port, handle: handle.handle, token: handle.token)) else {
            throw RpcError(code: 1, message: "句柄地址非法")
        }
        let (data, response) = try await PreviewHttpSession.value.data(from: url)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            let code = (response as? HTTPURLResponse)?.statusCode ?? -1
            throw RpcError(code: 1, message: "HTTP \(code)")
        }
        try data.write(to: file)
        return file
    }
}
