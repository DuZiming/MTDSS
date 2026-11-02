/// 照片图库 Photos 框架绑定(R5c 数据面 → R7 Ruling P 重定位:照片库 =
/// 其他设备可读来源;Kit ``PhotoLibraryService`` 协议的 App 目标实现——
/// Photos 为 iOS-only 系统框架,Kit/宿主测试不触碰本文件,LocalNotifier
/// 先案同款。R5c 的 App 内照片 UI 已撤,消费方 = Kit PhotoResponder
/// 0x06/0x07 照片桥应答):
/// - **权限**:`PHPhotoLibrary.authorizationStatus(for: .readWrite)` 查询
///   (对端浏览+下载要读资产 → readWrite 档;limited = 子集照常);请求
///   只在设置页「照片图库权限」段发起(App 启动不弹)。restricted 并入
///   denied(不可用同出指引面)。
/// - **相册**:最近项目(smartAlbumUserLibrary)恒首(标题钉「最近项目」
///   中文,与全 App 语言一致)+ 用户相册(albumRegular,系统本地化名);
///   计数 = 相册内照片数(mediaType image 谓词,v1 不含视频)。
/// - **资产**:照片 only,新→旧;原始文件名走 `asset.value(forKey:
///   "filename")` KVC(简报钦定口径);资源字节数走 `PHAssetResource`
///   KVC "fileSize"(0x06 文件项 size,声明值面)。
/// - **导出**:`PHAssetResourceManager.writeData` 落**原始字节**(HEIC/JPG
///   不转码;iCloud 资产放行网络拉取)——文件名 = ``PhotoAssetNaming``
///   (Kit 纯函数),批内撞名 ` (n)` 顺延。
/// - **缩略图**(App 面扩展,协议外):PHImageManager 同步请求出方图,
///   NSCache 缓存;网格 Cell 直接消费本类,不入 Kit 面。

import MTDSSKit
import Photos
import UIKit

/// **@MainActor 裁定**:PhotoKit 拉取(PHAsset/PHAssetCollection fetch)在
/// iOS 26 sim 的**非主线程实挂**(实证:同帧同型 fetch 主线程 10 资产在列/
/// 后台线程永不返回)——实现面整体主线程化;重活(PHAssetResourceManager
/// 写盘/PHImageManager 缩略图)本就走自有队列,await 挂起不占主线程。
@MainActor
final class PHPhotoLibraryService: PhotoLibraryService, @unchecked Sendable {

    static let shared = PHPhotoLibraryService()

    // MARK: 权限

    func permissionStatus() async -> PhotoLibraryPermission {
        Self.map(PHPhotoLibrary.authorizationStatus(for: .readWrite))
    }

    func requestAccess() async -> PhotoLibraryPermission {
        Self.map(await PHPhotoLibrary.requestAuthorization(for: .readWrite))
    }

    private static func map(_ status: PHAuthorizationStatus) -> PhotoLibraryPermission {
        switch status {
        case .notDetermined: .notDetermined
        case .restricted, .denied: .denied
        case .authorized: .authorized
        case .limited: .limited
        @unknown default: .denied
        }
    }

    /// 授权/图库变化流:PHPhotoLibraryChangeObserver 桥接(授权状态与照片
    /// 增删都触发 photoLibraryDidChange;订阅即发当前值兜底竞窗)。
    func permissionChanges() -> AsyncStream<PhotoLibraryPermission> {
        AsyncStream { continuation in
            let box = ChangeBox { continuation.yield(Self.map(PHPhotoLibrary.authorizationStatus(for: .readWrite))) }
            PHPhotoLibrary.shared().register(box)
            continuation.onTermination = { _ in
                PHPhotoLibrary.shared().unregisterChangeObserver(box)
            }
            continuation.yield(Self.map(PHPhotoLibrary.authorizationStatus(for: .readWrite)))
        }
    }

    /// 观察者盒(register 持强引用,onTermination 注销)。
    private final class ChangeBox: NSObject, PHPhotoLibraryChangeObserver, @unchecked Sendable {
        private let onChange: () -> Void
        init(onChange: @escaping () -> Void) {
            self.onChange = onChange
            super.init()
        }
        func photoLibraryDidChange(_ changeInstance: PHChange) {
            onChange()
        }
    }

    // MARK: 相册

    func albums() async throws -> [PhotoAlbum] {
        var result: [PhotoAlbum] = []
        // 最近项目(smartAlbumUserLibrary):**只走 subtype .any 全扫描过滤**
        // ——iOS 26 sim 上按 subtype 直取(smartAlbumUserLibrary 单值)实挂
        // (实证 DIAG:subtypeAny 扫描 21 条在列,单值直取永不返回;真机
        // 行为等价,扫描含全部智能相册)。扫不到 → 以全库照片数合成伪相册
        // (id="recents";assets 面 isRecents 传 nil 即全库照片,天然成立)。
        var recentsCollection: PHAssetCollection?
        let allSmart = PHAssetCollection.fetchAssetCollections(
            with: .smartAlbum, subtype: .any, options: nil)
        allSmart.enumerateObjects { collection, _, _ in
            if collection.assetCollectionSubtype == .smartAlbumUserLibrary {
                recentsCollection = collection
            }
        }
        // 最近项目计数 = 全库照片数(Recents ≡ v1 照片 only 面的全集;同时
        // 规避 collection 域拉取的另一潜在挂面)
        let total = PHAsset.fetchAssets(with: Self.imageFetchOptions()).count
        result.append(PhotoAlbum(
            id: recentsCollection?.localIdentifier ?? "recents",
            name: "最近项目",
            count: total,
            isRecents: true,
        ))
        // 用户相册(albumRegular;系统本地化名;v1 不含智能/云共享相册)
        let userAlbums = PHAssetCollection.fetchAssetCollections(
            with: .album, subtype: .albumRegular, options: nil)
        userAlbums.enumerateObjects { collection, _, _ in
            result.append(PhotoAlbum(
                id: collection.localIdentifier,
                name: collection.localizedTitle ?? "未命名相册",
                count: Self.imageCount(in: collection),
            ))
        }
        return result
    }

    private static func imageFetchOptions() -> PHFetchOptions {
        let options = PHFetchOptions()
        options.predicate = NSPredicate(format: "mediaType == %d", PHAssetMediaType.image.rawValue)
        return options
    }

    private static func imageCount(in collection: PHAssetCollection) -> Int {
        PHAsset.fetchAssets(in: collection, options: imageFetchOptions()).count
    }

    // MARK: 资产

    func assets(albumId: String?) async throws -> [PhotoAsset] {
        let options = PHFetchOptions()
        options.predicate = NSPredicate(format: "mediaType == %d", PHAssetMediaType.image.rawValue)
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        let fetch: PHFetchResult<PHAsset>
        if let albumId,
           let collection = PHAssetCollection.fetchAssetCollections(
               withLocalIdentifiers: [albumId], options: nil).firstObject {
            fetch = PHAsset.fetchAssets(in: collection, options: options)
        } else {
            // 最近项目缺省档 = 全库照片过滤
            fetch = PHAsset.fetchAssets(with: options)
        }
        var result: [PhotoAsset] = []
        fetch.enumerateObjects { asset, _, _ in
            result.append(PhotoAsset(
                id: asset.localIdentifier,
                filename: asset.value(forKey: "filename") as? String,
                creationDateMs: Int64((asset.creationDate?.timeIntervalSince1970 ?? 0) * 1000),
                // 资源字节数(R7:0x06 文件项 size——对端进度/记录面的声明值;
                // 清单以导出件 fstat 为准,此处偏差无害)。KVC "fileSize" 为
                // PHAssetResource 事实口径(公开 API 无尺寸面);不可得 → nil。
                resourceSize: Self.resourceSize(of: asset),
            ))
        }
        return result
    }

    /// 原始资源字节数(KVC "fileSize";不可得 = nil——0x06 回 0,引擎清单
    /// 以导出件 fstat 为准,显示偏差无害)。
    private static func resourceSize(of asset: PHAsset) -> Int64? {
        guard let resource = originalResource(of: asset) else { return nil }
        return (resource.value(forKey: "fileSize") as? NSNumber)?.int64Value
    }

    // MARK: 导出(原始字节)

    func exportAssets(ids: [String], to directory: URL) async throws -> [URL] {
        let fetch = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
        var byId: [String: PHAsset] = [:]
        fetch.enumerateObjects { asset, _, _ in byId[asset.localIdentifier] = asset }
        var usedNames = Set<String>()
        var urls: [URL] = []
        for (index, id) in ids.enumerated() {
            guard let asset = byId[id] else {
                throw PhotoSendError("照片已不可用(\(id))")
            }
            let base = PhotoAssetNaming.displayName(
                filename: asset.value(forKey: "filename") as? String,
                creationDateMs: Int64((asset.creationDate?.timeIntervalSince1970 ?? 0) * 1000),
                index: index + 1,
            )
            let dest = directory.appendingPathComponent(Self.uniqueName(base, used: &usedNames))
            // 原始字节直写(不转码;HEIC/JPG 原样;iCloud 资产允许网络拉取)
            guard let resource = Self.originalResource(of: asset) else {
                throw PhotoSendError("照片资源不可读:\(asset.value(forKey: "filename") ?? "?")")
            }
            let options = PHAssetResourceRequestOptions()
            options.isNetworkAccessAllowed = true
            try await PHAssetResourceManager.default().writeData(
                for: resource, toFile: dest, options: options)
            urls.append(dest)
        }
        return urls
    }

    /// 原始资源择优:.photo(原片字节,HEIC/JPG 保真)→ .fullSizePhoto
    /// → .alternatePhoto(编辑版)→ 兜底首个;全空 = nil(异常资产,拒导)。
    private static func originalResource(of asset: PHAsset) -> PHAssetResource? {
        let resources = PHAssetResource.assetResources(for: asset)
        let order: [PHAssetResourceType] = [.photo, .fullSizePhoto, .alternatePhoto]
        for type in order {
            if let match = resources.first(where: { $0.type == type }) { return match }
        }
        return resources.first
    }

    /// 批内撞名顺延:`name.ext` → `name (1).ext`…(moveIntoInbox 同语言)
    private static func uniqueName(_ base: String, used: inout Set<String>) -> String {
        var candidate = base
        if used.contains(candidate) {
            let ns = base as NSString
            let stem = ns.deletingPathExtension
            let ext = ns.pathExtension
            var n = 1
            repeat {
                candidate = ext.isEmpty ? "\(stem) (\(n))" : "\(stem) (\(n)).\(ext)"
                n += 1
            } while used.contains(candidate)
        }
        used.insert(candidate)
        return candidate
    }

    // MARK: 缩略图(网格消费;协议外 App 面)

    /// 缩略图缓存(上限 300 枚 / ~50MB,按像素字节计 cost——大相册滚动
    /// 不让缓存无界)
    private let thumbCache: NSCache<NSString, UIImage> = {
        let cache = NSCache<NSString, UIImage>()
        cache.countLimit = 300
        cache.totalCostLimit = 50 * 1024 * 1024
        return cache
    }()

    /// 方形缩略图(点按级联缓存;同步请求在 utility 线程跑,LazyVGrid 只
    /// 渲染可见格)。scale = 视图 displayScale(targetSize 换算像素)。
    func thumbnail(for assetId: String, side: CGFloat, scale: CGFloat) async -> UIImage? {
        let pixel = max(1, Int(side * max(1, scale)))
        let key = "\(assetId)#\(pixel)" as NSString
        if let cached = thumbCache.object(forKey: key) { return cached }
        guard let asset = PHAsset.fetchAssets(
            withLocalIdentifiers: [assetId], options: nil).firstObject else { return nil }
        let image: UIImage? = await Task.detached(priority: .utility) {
            let options = PHImageRequestOptions()
            options.isSynchronous = true
            options.deliveryMode = .highQualityFormat
            options.resizeMode = .fast
            options.isNetworkAccessAllowed = true
            var result: UIImage?
            PHImageManager.default().requestImage(
                for: asset,
                targetSize: CGSize(width: pixel, height: pixel),
                contentMode: .aspectFill,
                options: options,
            ) { image, _ in result = image }
            return result
        }.value
        if let image {
            thumbCache.setObject(
                image, forKey: key,
                cost: max(1, Int(image.size.width * image.scale) * Int(image.size.height * image.scale) * 4))
        }
        return image
    }
}
