/// PhotoLibraryViewModel — 照片图库视图模型(R5c 方案 A;Kit 宿主可测,
/// 服务协议注入):权限门(首用弹授权 → 拒绝出指引态)→ 相册列表 → 网格
/// → 选择 → 发送。
///
/// - **权限状态机**:notDetermined(视图出授权引导)/ denied(视图出
///   「前往设置」指引)/ authorized | limited(拉相册;limited = 子集内
///   照常浏览/发送)。变化流(`consumePermissionChanges`,视图 `.task`
///   持有)活体跟随:新授权 → 拉相册;撤权 → 清相册/收网格。
/// - **选择模式**:与浏览页 ``BrowseViewModel`` 同语义(Ruling O 2026-11-02):
///   确认 = 只退出;全选再点 = 清空;选中集按网格序收口。滑动删除不适用
///   (照片非我方文件,无删除动作)。
/// - **发送流(T11 管线复用)**:导出原始字节(临时目录)→ 暂存收件箱
///   (`ShareQueueStore.moveIntoInbox` 撞名顺延——与分享扩展消费同口)→
///   ``PhotoSendPipeline/send``(App 侧 `SendService.send` 路径形态)→
///   **终态 done 才清暂存件**;超时/失败留暂存(收件箱可见可寻回,与 T11
///   「发送失败 = 文件留暂存」同哲学)。
///
/// 宿主可测:纯 MTDSSKit,无 SwiftUI/Photos。

import Combine
import Foundation
@MainActor
public final class PhotoLibraryViewModel: ObservableObject {

    private let service: any PhotoLibraryService
    /// 暂存收件箱绝对路径(生产 = EngineLayout.inbox;测试 = 临时目录)。
    public let stagingInbox: String
    /// 发送管道(App 侧绑定/测试桩)。
    private let pipeline: PhotoSendPipeline

    public init(service: any PhotoLibraryService, stagingInbox: String, pipeline: PhotoSendPipeline) {
        self.service = service
        self.stagingInbox = stagingInbox
        self.pipeline = pipeline
    }

    // MARK: 权限面

    /// 当前授权态(初值 = notDetermined;`permissionChecked` 门防首帧误导)。
    @Published public private(set) var permission: PhotoLibraryPermission = .notDetermined
    /// 首次查询已完成(UI 出真实门面,不再按初值误显授权引导)。
    @Published public private(set) var permissionChecked = false

    /// 授权可浏览(完整/受限同面;limited 子集由实现方收口)。
    public var isAuthorized: Bool { permission == .authorized || permission == .limited }

    /// 查询当前授权态(进页一次;authorized/limited 即拉相册)。
    public func refreshPermission() async {
        permission = await service.permissionStatus()
        permissionChecked = true
        if isAuthorized {
            await loadAlbumsIfNeeded()
        }
    }

    /// 请求授权(首用进页弹系统窗;App 启动不弹)。结果落权限面,
    /// authorized/limited 即拉相册。
    public func requestAccess() async {
        permission = await service.requestAccess()
        permissionChecked = true
        if isAuthorized {
            await loadAlbumsIfNeeded()
        } else {
            clearAuthorizedContent()
        }
    }

    /// 授权/图库变化流消费(视图 `.task` 持有;离场即取消)。
    /// 授权态变化活体跟随:新授权 → 拉相册;撤权 → 清相册/收网格。
    public func consumePermissionChanges() async {
        for await state in service.permissionChanges() {
            permission = state
            permissionChecked = true
            if isAuthorized {
                // 变化(含照片增删)→ 相册计数活体刷新
                await loadAlbums()
            } else {
                clearAuthorizedContent()
            }
        }
    }

    /// 撤权收口:清相册与已开网格(授权内容不残留在拒绝态页面)。
    private func clearAuthorizedContent() {
        albums = []
        albumsLoading = false
        albumsError = nil
        closeAlbum()
    }

    // MARK: 相册面

    @Published public private(set) var albums: [PhotoAlbum] = []
    @Published public private(set) var albumsLoading = false
    @Published public private(set) var albumsError: String?

    /// 拉相册(仅未加载时;`loadAlbums` 为活体刷新口)。
    private func loadAlbumsIfNeeded() async {
        guard albums.isEmpty, albumsError == nil, !albumsLoading else { return }
        await loadAlbums()
    }

    /// 拉相册列表(无条件;错误落 ``albumsError``,旧值清空)。
    public func loadAlbums() async {
        guard !albumsLoading else { return }
        albumsLoading = true
        albumsError = nil
        do {
            albums = try await service.albums()
            albumsLoading = false
        } catch {
            albums = []
            albumsLoading = false
            albumsError = Self.userErrorText(for: error)
        }
    }

    // MARK: 网格面

    @Published public private(set) var currentAlbum: PhotoAlbum?
    @Published public private(set) var assets: [PhotoAsset] = []
    @Published public private(set) var assetsLoading = false
    @Published public private(set) var assetsError: String?

    /// 打开相册(拉网格;相册切格即重拉,无跨相册缓存——网格数据轻)。
    public func open(album: PhotoAlbum) async {
        currentAlbum = album
        await fetchAssets(album: album)
    }

    /// 关闭网格(弹回相册列表;选择态一并清)。
    public func closeAlbum() {
        currentAlbum = nil
        assets = []
        assetsError = nil
        if isSelecting { exitSelection() }
    }

    /// 原地重载当前相册(错误重试口)。
    public func reloadAssets() async {
        guard let album = currentAlbum else { return }
        await fetchAssets(album: album)
    }

    private func fetchAssets(album: PhotoAlbum) async {
        assetsLoading = true
        assetsError = nil
        do {
            assets = try await service.assets(albumId: album.isRecents ? nil : album.id)
            assetsLoading = false
        } catch {
            assets = []
            assetsLoading = false
            assetsError = Self.userErrorText(for: error)
        }
    }

    // MARK: 选择模式(spec §4.2 同骨架;Ruling O:确认 = 只退出)

    @Published public private(set) var isSelecting = false
    /// 选中资产 id 集(键 = PhotoAsset.id)。
    @Published public private(set) var selection: Set<String> = []

    /// 进入选择模式(菜单「选择」/ 格长按两入口)。
    public func enterSelection() {
        isSelecting = true
        selection = []
    }

    /// 退出选择模式(选择集清空)。
    public func exitSelection() {
        isSelecting = false
        selection = []
    }

    /// 格点按切换选中(选择态下点按不开查看器)。
    public func toggle(_ id: String) {
        if selection.contains(id) {
            selection.remove(id)
        } else {
            selection.insert(id)
        }
    }

    /// 全选当前网格;已全选再点 = 清空(iOS 照片/文件 App 惯用)。
    public func selectAll() {
        let all = assets.map(\.id)
        if all.allSatisfy(selection.contains) && !all.isEmpty {
            selection.removeAll()
        } else {
            selection = Set(all)
        }
    }

    /// 选中资产(网格序;不可见项不收)。
    public var selectedAssets: [PhotoAsset] {
        assets.filter { selection.contains($0.id) }
    }

    /// 蓝圈确认(Ruling O:**只 = 退出选择模式**,无副作用;操作仅自底栏)。
    public func confirmSelection() {
        exitSelection()
    }

    // MARK: 发送(T11 管线复用)

    /// 最近一次动作失败文案(UI 弹提示后置 nil 清除)。
    @Published public var lastActionError: String?

    /// 发送进行中(重入闸;动作发起面防连点)。
    @Published public private(set) var sending = false

    /// 等作业终态的限时(秒;App 侧 transfer.wait 包 RpcGuard;超时 = 未达
    /// 终态 → 暂存件保留不清)。
    public static let cleanupWaitTimeout: TimeInterval = 180

    /// 发送选中照片(Ruling O:视图层先捕获 ``selectedAssets`` 再退出选择,
    /// 本口只认传入资产;返回 true = 发送已受理):
    /// ① 导出原始字节到临时目录(命名 = ``PhotoAssetNaming``;批次结束即删
    ///    临时目录);② 逐文件暂存收件箱(撞名顺延,T11 同口);③ 管道发送
    ///    (App 侧 SendService);④ **终态 done 才清暂存件**——超时/失败留
    ///    暂存(收件箱可见可寻回;与 T11 失败留暂存同哲学)。
    @discardableResult
    public func send(_ assets: [PhotoAsset], to device: DirectoryDevice) async -> Bool {
        guard !sending, !assets.isEmpty else { return false }
        sending = true
        defer { sending = false }
        do {
            // ① 导出(临时目录;文件名 = 发送命名,批内撞名由实现方顺延)
            let exportDir = FileManager.default.temporaryDirectory
                .appendingPathComponent("mtdss-photo-export-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: exportDir, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: exportDir) }
            let files: [URL]
            do {
                files = try await service.exportAssets(ids: assets.map(\.id), to: exportDir)
            } catch {
                throw PhotoSendError("照片导出失败:\(error.localizedDescription)")
            }
            guard !files.isEmpty else {
                throw PhotoSendError("照片导出失败:未得到导出文件")
            }
            // ② 暂存收件箱(T11 moveIntoInbox:撞名 ` (n)` 顺延;rename 优先)
            var staged: [String] = []
            do {
                for file in files {
                    staged.append(try ShareQueueStore.moveIntoInbox(
                        fileURL: file, name: file.lastPathComponent, inbox: stagingInbox))
                }
            } catch {
                throw PhotoSendError("暂存失败:\(error.localizedDescription)")
            }
            // ③ 发送(路径形态;Ruling I dir 解析收口在 SendService)
            let jobId: String
            do {
                jobId = try await pipeline.send(staged, device)
            } catch {
                // 失败留暂存(不静默丢;错误文案上浮)——与 T11 同哲学
                lastActionError = Self.userErrorText(for: error)
                return false
            }
            // ④ 终态 done 才清暂存件;未达终态/其他终态 → 留暂存(收件箱
            //    可见可寻回;源件没了会让重试/续传无源)
            let terminal = await pipeline.waitTerminal(jobId)
            if terminal == "done" {
                await pipeline.cleanup(staged)
            }
            return true
        } catch {
            lastActionError = Self.userErrorText(for: error)
            return false
        }
    }

    /// 错误用户文案(传输/超时同浏览页口径;照片链错误原文直出)。
    public static func userErrorText(for error: Error) -> String {
        if let photo = error as? PhotoSendError { return photo.message }
        if let rpc = error as? RpcError { return rpc.message }
        if error is RpcGuard.TimeoutError { return "请求超时" }
        return "发送失败:\(error.localizedDescription)"
    }
}
