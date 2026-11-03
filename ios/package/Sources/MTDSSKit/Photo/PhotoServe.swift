/// 照片桥应答器(R7 Ruling P:照片库对其他设备只读可浏览/可下载,App 内
/// 零照片 UI):消费 daemon 的 `fs.photo_list` / `fs.photo_export` 事件,
/// 经 fdpass 查询通道应答(0x06 列表 / 0x07 导出)——引擎把 App 照片库
/// 投影成 `/照片图库` 虚拟根(根 = 相册列表,相册 = 照片列表)。
///
/// - **权限门**:仅 `authorized` / `limited` 应答;`notDetermined` /
///   `denied` 即回 ok:false(引擎按「桥不可用」收口 → 虚拟根隐藏)。授权
///   撤销后引擎侧无需通知面——每次浏览/导出都是新请求,现查现答(权限
///   变化 = 下一问即拒)。
/// - **列表**:根("")= `service.albums()`(目录项:children = 相册内照片
///   数,T5b「N 项」语义);相册名 = `service.assets(albumId:)`(文件项:
///   name = 批内唯一名——原始文件名优先,缺省回退命名按批内真实顺位,
///   撞名 ` (n)` 顺延,R8 终审 I-1;size = 资源字节数,mtime_ms =
///   拍摄时间,photo_id = 资产标识——0x07 导出凭据)。相册按名匹配(重名
///   取先,v1 边界)。
/// - **导出**:逐请求独立子目录(`photo-serve/<request_id>/`,防在途同名
///   覆写)→ `service.exportAssets` 原始字节(R5c 导出代码,不转码)→ 回
///   绝对路径;引擎读发后 unlink 文件,残目录由启动清扫回收。
/// - **启动清扫**:挂载即清空 `photo-serve/`(崩溃残件自愈,与引擎侧
///   「终态 unlink」两侧兜底)。
///
/// 生命周期:App boot 时随 EventHub 订阅流 `run` 起来(事件流终结即退,
/// 与引擎同寿;引擎监督重启后由调用方重挂,InboxPublisher 同款)。

import Foundation

/// 0x06 条目的线上形态(与 crates/daemon/src/photo.rs 的解析面逐字段对齐;
/// 字母序由 FdPassAnswering 的 sortedKeys 序列化统一)。
public enum PhotoServeEntry: Equatable, Sendable {

    /// 相册(目录项):children = 相册内照片数。
    case album(name: String, count: Int)

    /// 照片(文件项):name = 批内唯一名(原始文件名/回退命名,撞名
    /// ` (n)` 顺延),size = 资源字节数,mtimeMs = 拍摄时间(0 = 未知),
    /// photoId = 0x07 导出凭据。
    case photo(name: String, size: Int64, mtimeMs: Int64, photoId: String)

    var name: String {
        switch self {
        case .album(let name, count: _): name
        case .photo(let name, size: _, mtimeMs: _, photoId: _): name
        }
    }

    /// 条目 JSON(payload "entries" 数组元素)。
    var jsonObject: [String: Any] {
        switch self {
        case .album(let name, let count):
            return [
                "children": count,
                "is_dir": true,
                "mtime_ms": 0,
                "name": name,
                "size": 0,
            ]
        case .photo(let name, let size, let mtimeMs, let photoId):
            return [
                "is_dir": false,
                "mtime_ms": Int(clamping: mtimeMs),
                "name": name,
                "photo_id": photoId,
                "size": Int(clamping: size),
            ]
        }
    }

    /// 解码(测试钉面;引擎不回发条目,本向仅钉形状防漂移)。
    public static func parse(_ json: [String: Any]) -> PhotoServeEntry? {
        guard let name = json["name"] as? String else { return nil }
        if json["is_dir"] as? Bool == true {
            let count = (json["children"] as? NSNumber)?.intValue ?? 0
            return .album(name: name, count: count)
        }
        guard let photoId = json["photo_id"] as? String else { return nil }
        let size = (json["size"] as? NSNumber)?.int64Value ?? 0
        let mtime = (json["mtime_ms"] as? NSNumber)?.int64Value ?? 0
        return .photo(name: name, size: size, mtimeMs: mtime, photoId: photoId)
    }
}

/// 文件面抽象(宿主单测注入临时目录;生产 = DefaultPhotoServeFilesystem)。
public protocol PhotoServingFilesystem: Sendable {
    /// 清空目录内容(启动清扫;目录不存在则创建)。
    func sweep(directory: String)

    /// 逐请求导出子目录(`root/<request_id>`;已存在先清,幂等)。
    func makeRequestDirectory(root: String, requestId: UInt64) throws -> String
}

/// 生产文件面(FileManager)。
public struct DefaultPhotoServeFilesystem: PhotoServingFilesystem {

    public init() {}

    public func sweep(directory: String) {
        let fm = FileManager.default
        try? fm.createDirectory(atPath: directory, withIntermediateDirectories: true)
        guard let items = try? fm.contentsOfDirectory(atPath: directory) else { return }
        for item in items {
            try? fm.removeItem(atPath: directory + "/" + item)
        }
    }

    public func makeRequestDirectory(root: String, requestId: UInt64) throws -> String {
        let dir = root + "/" + String(requestId)
        let fm = FileManager.default
        if fm.fileExists(atPath: dir) {
            try fm.removeItem(atPath: dir)
        }
        try fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
        return dir
    }
}

/// 照片桥应答器:构造后随事件流 run(单消费)。
public struct PhotoResponder: Sendable {

    /// 单帧条目上限(fdpass 单帧 payload 64 KiB——localIdentifier+文件名
    /// 的条目约 170-200B,不设帽的大相册会超限触发协议断连;截断语义与
    /// fs 列表一致:truncated=true 上达对端,照片项按新→旧保前缀)。
    public static let listingEntryCap = 200

    /// 照片库数据源(App = PHPhotoLibraryService)。
    public let service: any PhotoLibraryService
    /// fdpass 查询通道(0x06/0x07 应答)。
    public let fdpass: any FdPassAnswering
    /// 导出临时根(`{home}/.mtdss/photo-serve`;App 私有隐藏目录)。
    public let serveRoot: String
    /// 文件面(清扫/请求目录)。
    public let files: any PhotoServingFilesystem

    public init(
        service: any PhotoLibraryService,
        fdpass: any FdPassAnswering,
        serveRoot: String,
        files: any PhotoServingFilesystem = DefaultPhotoServeFilesystem(),
    ) {
        self.service = service
        self.fdpass = fdpass
        self.serveRoot = serveRoot
        self.files = files
    }

    /// 事件流消费主环:挂载即清扫,然后 fs.photo_* 应答,其余事件直通忽略
    /// (各屏订阅面互不抢流)。
    public func run(_ events: AsyncStream<IpcEvent>) async {
        files.sweep(directory: serveRoot)
        for await event in events {
            await handle(event)
        }
    }

    /// 单事件处置(测试直驱面)。
    public func handle(_ event: IpcEvent) async {
        switch event {
        case .photoList(let requestId, let path):
            await answerList(requestId: requestId, path: path)
        case .photoExport(let requestId, let photoId):
            await answerExport(requestId: requestId, photoId: photoId)
        default:
            break
        }
    }

    // MARK: 列表(0x06)

    /// fs.photo_list → 0x06。权限门先行(非 authorized/limited 即拒答,
    /// 引擎按桥不可用收口 → 虚拟根隐藏)。
    func answerList(requestId: UInt64, path: String) async {
        guard await isAuthorized() else {
            await send(
                kind: FdPassKind.photoList, requestId: requestId,
                error: "照片权限未授予",
            )
            return
        }
        do {
            var entries: [PhotoServeEntry]
            if path.isEmpty {
                // 根 = 相册列表(目录项;children = 相册内照片数)
                entries = try await service.albums().map {
                    .album(name: $0.name, count: $0.count)
                }
            } else {
                // 相册名 → 相册(按名匹配,重名取先——v1 边界)→ 照片文件项。
                // 命名(R8 终审 I-1)批内唯一名收口:回退命名序号 = 批内真实
                // 顺位(原恒 1,同刻无名照片回退名互撞);撞名 ` (n)` 顺延
                // (InboxPublisher/ShareQueue 同语言)——0x06 面逐条唯一,
                // engine rel 面不再因 App 命名撞车硬败。
                guard let album = try await service.albums().first(where: { $0.name == path }) else {
                    await send(kind: FdPassKind.photoList, requestId: requestId, error: "无此相册")
                    return
                }
                let assets = try await service.assets(albumId: album.isRecents ? nil : album.id)
                var used: Set<String> = []
                entries = assets.enumerated().map { offset, asset in
                    let raw = PhotoAssetNaming.displayName(
                        filename: asset.filename,
                        creationDateMs: asset.creationDateMs,
                        index: offset + 1,
                    )
                    return .photo(
                        name: PhotoAssetNaming.uniqueName(raw, used: &used),
                        size: asset.resourceSize ?? 0,
                        mtimeMs: asset.creationDateMs,
                        photoId: asset.id,
                    )
                }
            }
            var truncated = false
            if entries.count > Self.listingEntryCap {
                entries = Array(entries.prefix(Self.listingEntryCap))
                truncated = true
            }
            let payload: [String: Any] = [
                "entries": entries.map(\.jsonObject),
                "ok": true,
                "request_id": Int(clamping: requestId),
                "truncated": truncated,
            ]
            _ = try await fdpass.send(kind: FdPassKind.photoList, json: payload)
        } catch {
            await send(kind: FdPassKind.photoList, requestId: requestId, error: "\(error)")
        }
    }

    // MARK: 导出(0x07)

    /// fs.photo_export → 0x07:原始字节导出到逐请求子目录,回绝对路径。
    /// 引擎读发后 unlink 文件(残目录由下次挂载清扫回收)。
    func answerExport(requestId: UInt64, photoId: String) async {
        guard await isAuthorized() else {
            await send(
                kind: FdPassKind.photoExport, requestId: requestId,
                error: "照片权限未授予",
            )
            return
        }
        do {
            let dir = try files.makeRequestDirectory(root: serveRoot, requestId: requestId)
            let urls = try await service.exportAssets(ids: [photoId], to: URL(fileURLWithPath: dir))
            guard let url = urls.first else {
                await send(kind: FdPassKind.photoExport, requestId: requestId, error: "导出无产物")
                return
            }
            _ = try await fdpass.send(kind: FdPassKind.photoExport, json: [
                "ok": true,
                "path": url.path,
                "request_id": Int(clamping: requestId),
            ])
        } catch {
            await send(kind: FdPassKind.photoExport, requestId: requestId, error: "\(error)")
        }
    }

    // MARK: 权限门 + 拒答

    /// 授权可答(authorized/limited;limited = 子集内照常)。
    func isAuthorized() async -> Bool {
        let status = await service.permissionStatus()
        return status == .authorized || status == .limited
    }

    /// ok:false 拒答(引擎按「桥不可用」收口)。
    func send(kind: UInt8, requestId: UInt64, error: String) async {
        _ = try? await fdpass.send(kind: kind, json: [
            "error": error,
            "ok": false,
            "request_id": Int(clamping: requestId),
        ])
    }
}
