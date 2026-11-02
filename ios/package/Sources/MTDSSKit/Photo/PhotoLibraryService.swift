/// 照片图库数据面(R5c,方案 A 2026-11-02 用户裁定):照片图库 = 收件箱
/// 之外的**特殊来源**(非信任目录、非文件系统目录——信任目录机制不动)。
///
/// **协议化(Ruling E 同款:逻辑归 Kit 可测,绑定归 App)**:Photos 框架
/// 是 iOS-only 系统框架,宿主单测(macOS)不可触碰——协议与状态面全在
/// Kit,`PHPhotoLibrary` 实现只在 App 目标(Photo/PHPhotoLibraryService)。
/// 模型纯 Foundation:
/// - ``PhotoLibraryPermission``:iOS 照片授权四态(restricted 并入 denied
///   ——不可用即出指引面,不区分成因);
/// - ``PhotoAlbum``:相册(最近项目 = `isRecents` 置顶默认档,计数 = 照片数);
/// - ``PhotoAsset``:单张照片(原始文件名 + 拍摄时间;视频 v1 不入面)。
///
/// 发送流 = **T11 验证过的管线**(导出 → 暂存收件箱 → 路径形态上传):
/// VM 把选中资产导出到临时目录(实现方保原始字节),``PhotoSendPipeline``
/// 三闭包注入(App 侧接 `SendService.send` + `transfer.wait` + 删除暂存件;
/// 暂存复用 `ShareQueueStore.moveIntoInbox` 撞名顺延)——与分享扩展消费面
/// 同一条已实证链路。
import Foundation

/// 照片图库授权态(iOS `PHAuthorizationStatus` 的 Kit 投影)。
public enum PhotoLibraryPermission: Equatable, Sendable {
    /// 未询问(首次进入图库页时才弹系统授权,App 启动不弹)。
    case notDetermined
    /// 拒绝/restricted(合并:均出「前往设置」指引面)。
    case denied
    /// 完整授权。
    case authorized
    /// 受限访问(iOS 14+ limited:仅所选子集——浏览/发送照常,子集内)。
    case limited
}

/// 相册(最近项目 = `isRecents` 恒首的默认档;count = 相册内照片数,
/// v1 只面照片,视频不计)。
public struct PhotoAlbum: Equatable, Sendable, Identifiable, Hashable {
    public let id: String
    public let name: String
    public let count: Int
    /// 最近项目档(实现方保证排序恒首;网格页标题 = name)。
    public let isRecents: Bool

    public init(id: String, name: String, count: Int, isRecents: Bool = false) {
        self.id = id
        self.name = name
        self.count = count
        self.isRecents = isRecents
    }
}

/// 单张照片(v1 照片 only;视频不入网格与发送面)。
public struct PhotoAsset: Equatable, Sendable, Identifiable, Hashable {
    public let id: String
    /// 原始文件名(如 IMG_1234.HEIC;实现方取资源面原始名,缺省 nil 走回退命名)。
    public let filename: String?
    /// 拍摄时间 epoch 毫秒(0 = 未知;回退命名用)。
    public let creationDateMs: Int64

    public init(id: String, filename: String?, creationDateMs: Int64) {
        self.id = id
        self.filename = filename
        self.creationDateMs = creationDateMs
    }
}

/// 发送命名(spec 方案 A:原文件名优先,缺省回退 IMG_<拍摄时间>_<序号>.HEIC;
/// 纯函数,宿主可测)。序号 = 发送批内 1 起顺位(批内重名由导出方再顺延)。
public enum PhotoAssetNaming {

    /// 批内第 `index`(1 起)资产的发送文件名。
    public static func displayName(filename: String?, creationDateMs: Int64, index: Int) -> String {
        if let filename, !filename.isEmpty {
            // 路径分隔/盘符冒号净化(ShareQueueStore.sanitizedName 同语言)
            let stripped = filename.split(separator: "/").last.map(String.init) ?? filename
            return stripped
                .replacingOccurrences(of: ":", with: "_")
                .replacingOccurrences(of: "\\", with: "_")
        }
        let stamp = Self.timestamp(creationDateMs)
        return "IMG_\(stamp)_\(index).HEIC"
    }

    /// 拍摄时间戳段(yyyyMMddHHmmss;0 = 未知,全零占位——确定性,可测)。
    static func timestamp(_ ms: Int64) -> String {
        guard ms > 0 else { return "00000000000000" }
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyyMMddHHmmss"
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "UTC")
        return formatter.string(from: Date(timeIntervalSince1970: Double(ms) / 1000))
    }
}

/// 照片图库数据源(Kit 协议;App 目标 PHPhotoLibrary 实现,宿主测试注入伪)。
public protocol PhotoLibraryService: Sendable {

    /// 当前授权态(不弹窗;纯查询)。
    func permissionStatus() async -> PhotoLibraryPermission

    /// 请求授权(首次调用弹系统窗;结果 = 请求后的授权态)。
    func requestAccess() async -> PhotoLibraryPermission

    /// 授权/图库变化流(订阅即发当前值;此后实现方在有变化时推送——
    /// 设置页改权限、用户增删照片都算)。VM 据此活体跟随。
    func permissionChanges() -> AsyncStream<PhotoLibraryPermission>

    /// 相册列表(最近项目恒首;其后用户相册;空 = [])。
    func albums() async throws -> [PhotoAlbum]

    /// 相册内照片(照片 only,新→旧;`albumId` = nil 走最近项目缺省档)。
    func assets(albumId: String?) async throws -> [PhotoAsset]

    /// 导出原始字节到目录(保原始 HEIC/JPG 不转码;文件名 = 发送命名规则,
    /// 批内撞名顺延;返回落盘文件 URL,顺序 = ids 序)。
    func exportAssets(ids: [String], to directory: URL) async throws -> [URL]
}

/// 发送管道注入面(T11 管线的 App 侧绑定;测试注入桩):
/// - `send`:暂存件绝对路径组 + 目标设备 → 作业 id(App 侧 `SendService.send`,
///   Ruling I dir 解析收口在内);
/// - `waitTerminal`:作业 id → 终态名(done/failed/cancelled/interrupted;
///   nil = 限时内未达终态——VM 据此决定是否清理暂存件);
/// - `cleanup`:删除暂存件(仅终态 done 后由 VM 调)。
public struct PhotoSendPipeline: Sendable {
    public var send: @Sendable ([String], DirectoryDevice) async throws -> String
    public var waitTerminal: @Sendable (String) async -> String?
    public var cleanup: @Sendable ([String]) async -> Void

    public init(
        send: @escaping @Sendable ([String], DirectoryDevice) async throws -> String,
        waitTerminal: @escaping @Sendable (String) async -> String?,
        cleanup: @escaping @Sendable ([String]) async -> Void,
    ) {
        self.send = send
        self.waitTerminal = waitTerminal
        self.cleanup = cleanup
    }
}

/// 照片发送链错误(VM 包装为用户文案;`userErrorText` 消费)。
public struct PhotoSendError: Error {
    public let message: String
    public init(_ message: String) { self.message = message }
}
