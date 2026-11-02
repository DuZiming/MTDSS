/// 照片图库页(R5c 方案 A;首页第二固定行推入的特殊来源,非信任目录):
/// - **权限门**(spec 方案 A:首次进本页才请求,App 启动不弹):
///   未查询 = 转圈;notDetermined = 授权引导(「授权访问照片」按钮 →
///   系统弹窗);denied = 指引面(「前往设置」深链系统设置);
///   authorized / limited = 相册列表(limited = 子集照常)。
/// - **相册列表**:最近项目(默认档)恒首 + 用户相册,行 = 图标+名+计数;
///   点行推入网格页(内层 NavigationStack,返回弹回本页)。
/// - 网格页/选择/发送见 ``PhotoGridView``(VM 同一枚)。
///
/// 权限变化流(`consumePermissionChanges`)随页持有:设置页改权限/
/// 前台回归活体跟随(新授权拉相册,撤权收面)。

import MTDSSKit
import SwiftUI

struct PhotoLibraryView: View {
    @Environment(AppModel.self) private var model
    @StateObject private var vm: PhotoLibraryViewModel
    @State private var navPath: [PhotoRoute] = []

    init(repo: MtdssRepository) {
        _vm = StateObject(wrappedValue: PhotoLibraryViewModel(
            service: PHPhotoLibraryService.shared,
            stagingInbox: ProductionEngine.layout().inbox,
            pipeline: PhotoSendPipeline(
                // 发送 = SendService.send(Ruling I dir 解析收口在内;T11 同管线)
                send: { paths, device in
                    try await SendService(repo: repo).send(paths: paths, to: device)
                },
                // 终态等待(限时;超时 = 未达终态 → 暂存件保留不清)
                waitTerminal: { jobId in
                    (try? await RpcGuard.withTimeout(seconds: PhotoLibraryViewModel.cleanupWaitTimeout) {
                        try await repo.transferWait(id: jobId)
                    })?.state
                },
                // 清理 = 删暂存件(仅 done 终态由 VM 调)
                cleanup: { paths in
                    for path in paths {
                        try? FileManager.default.removeItem(atPath: path)
                    }
                },
            ),
        ))
    }

    var body: some View {
        NavigationStack(path: $navPath) {
            content
                .navigationDestination(for: PhotoRoute.self) { route in
                    switch route {
                    case .grid(let album):
                        PhotoGridView(vm: vm, album: album, onPopToRoot: { navPath = [] })
                    }
                }
        }
        .background(Color(uiColor: .systemBackground))
        .toolbar(.hidden, for: .navigationBar)
        .task { await vm.refreshPermission() }
        // 权限/图库变化活体跟随(设置页改权限、照片增删;离场即取消)
        .task { await vm.consumePermissionChanges() }
        #if DEBUG
        .task { await debugPhotoIfHooked() }
        #endif
    }

    // MARK: 权限门

    @ViewBuilder private var content: some View {
        if !vm.permissionChecked {
            gatePage {
                VStack(spacing: 8) {
                    Spacer()
                    ProgressView()
                    Spacer()
                }
            }
        } else if vm.isAuthorized {
            PhotoAlbumsList(vm: vm, onOpen: { navPath.append(.grid($0)) })
        } else if vm.permission == .denied {
            // 拒绝:指引面(前往设置;不反复弹系统窗)
            gatePage {
                VStack(spacing: 14) {
                    Spacer()
                    Image(systemName: "photo.badge.exclamationmark")
                        .font(.system(size: 40))
                        .foregroundStyle(Color(uiColor: .systemGray))
                    Text("未获照片访问权限")
                        .font(.system(size: 17, weight: .medium))
                    Text("MTDSS 需要访问照片图库,才能浏览与发送照片。\n请在系统设置中开启权限。")
                        .font(.system(size: 13))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .multilineTextAlignment(.center)
                    Button {
                        Self.openSystemSettings()
                    } label: {
                        Text("前往设置")
                            .font(.system(size: 15, weight: .medium))
                            .padding(.horizontal, 20)
                            .padding(.vertical, 10)
                            .background(Color.accentColor, in: Capsule())
                            .foregroundStyle(.white)
                    }
                    Spacer()
                }
            }
        } else {
            // notDetermined:授权引导(按钮触发系统弹窗,只此一处请求)
            gatePage {
                VStack(spacing: 14) {
                    Spacer()
                    Image(systemName: "photo.on.rectangle")
                        .font(.system(size: 40))
                        .foregroundStyle(Color(uiColor: .systemGray))
                    Text("浏览与发送你的照片")
                        .font(.system(size: 17, weight: .medium))
                    Text("MTDSS 需要访问照片图库,以便浏览与发送你选择的照片。")
                        .font(.system(size: 13))
                        .foregroundStyle(Color(uiColor: .systemGray))
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                    Button {
                        Task { await vm.requestAccess() }
                    } label: {
                        Text("授权访问照片")
                            .font(.system(size: 15, weight: .medium))
                            .padding(.horizontal, 20)
                            .padding(.vertical, 10)
                            .background(Color.accentColor, in: Capsule())
                            .foregroundStyle(.white)
                    }
                    Spacer()
                }
            }
        }
    }

    /// 门面骨架(顶栏统一,内容换装)。
    private func gatePage<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(spacing: 0) {
            BrowseNavBar(title: "照片图库", showsArrows: false) { EmptyView() }
            content()
        }
    }

    /// 系统设置深链(本 App 设置页,照片权限项在其中)。
    static func openSystemSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }

    // MARK: DEBUG 截图/E2E 钩子(模拟器无触点驱动;真机/产线不编译,特此报备)

    #if DEBUG
    /// MTDSS_DEBUG_PHOTO_GRID=1:相册就绪后推入首个相册网格(截图面);
    /// MTDSS_DEBUG_PHOTO_SELECT=1:同上再进选择态勾 3 格(R5c 选择截图面);
    /// MTDSS_DEBUG_PHOTO_SEND=<device_id>|<platform>:最近项目前 2 张走
    /// vm.send 全链(导出→暂存→发送→等终态→清理),E2E 面。
    private func debugPhotoIfHooked() async {
        let env = ProcessInfo.processInfo.environment
        let wantSend = env["MTDSS_DEBUG_PHOTO_SEND"] ?? ""
        guard env["MTDSS_DEBUG_PHOTO_GRID"] == "1"
            || env["MTDSS_DEBUG_PHOTO_SELECT"] == "1" || !wantSend.isEmpty else { return }
        // 等权限+相册就绪(10s 上界)
        for _ in 0..<100 where !(vm.permissionChecked && vm.isAuthorized) || vm.albums.isEmpty {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard let album = vm.albums.first else {
            print("[R5C-E2E] PHOTO FAIL: 相册列表空")
            return
        }
        if !wantSend.isEmpty {
            // 发送链:打开最近项目 → 前 2 张 → vm.send(视图层同流:捕获后发送)
            await vm.open(album: album)
            let picked = Array(vm.assets.prefix(2))
            guard picked.count == 2 else {
                print("[R5C-E2E] PHOTO SEND FAIL: 照片不足 2(共 \(vm.assets.count))")
                return
            }
            print("[R5C-E2E] PHOTO SEND names=\(picked.map { $0.filename ?? "?" }.joined(separator: ","))")
            let parts = wantSend.split(separator: "|", maxSplits: 1).map(String.init)
            let device = DirectoryDevice(
                deviceId: parts[0], name: parts[0],
                platform: parts.count > 1 ? parts[1] : "windows", online: true)
            let ok = await vm.send(picked, to: device)
            print("[R5C-E2E] PHOTO SENT ok=\(ok) error=\(vm.lastActionError ?? "-")")
            return
        }
        navPath.append(.grid(album))
        if env["MTDSS_DEBUG_PHOTO_SELECT"] == "1" {
            for _ in 0..<100 where vm.assets.isEmpty {
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
            vm.enterSelection()
            vm.assets.prefix(3).forEach { vm.toggle($0.id) }
        }
    }
    #endif
}

/// 照片模块内层路由(相册列表 → 网格页)。
enum PhotoRoute: Hashable {
    case grid(PhotoAlbum)
}

/// 相册列表(最近项目恒首 + 用户相册;行 = 图标+名+计数)。
struct PhotoAlbumsList: View {
    @ObservedObject var vm: PhotoLibraryViewModel
    let onOpen: (PhotoAlbum) -> Void

    var body: some View {
        VStack(spacing: 0) {
            BrowseNavBar(title: "照片图库", showsArrows: false) { EmptyView() }
            listArea
        }
    }

    @ViewBuilder private var listArea: some View {
        if let error = vm.albumsError, vm.albums.isEmpty {
            ContentUnavailableView {
                Label("无法加载相册", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("重试") { Task { await vm.loadAlbums() } }
            }
        } else if vm.albumsLoading && vm.albums.isEmpty {
            VStack(spacing: 8) {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else {
            SinkingFooterList {
                ForEach(vm.albums) { album in
                    Button {
                        onOpen(album)
                    } label: {
                        FullBleedRow {
                            HStack(spacing: 12) {
                                Image(systemName: album.isRecents ? "photo.on.rectangle" : "photo.stack")
                                    .font(.system(size: 20))
                                    .foregroundStyle(Color.accentColor)
                                    .frame(width: 28)
                                Text(album.name)
                                    .font(.system(size: 17))
                                    .foregroundStyle(Color.primary)
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                                Text("\(album.count) 张")
                                    .font(.system(size: 12.5))
                                    .foregroundStyle(Color(uiColor: .systemGray))
                            }
                            .padding(.horizontal, 20)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(PressableRowStyle())
                    .accessibilityLabel(Text("\(album.name),\(album.count) 张"))
                }
            } footer: {
                ItemCountFooter(count: vm.albums.count)
            }
            .animation(.easeInOut(duration: 0.2), value: vm.albums)
        }
    }
}
