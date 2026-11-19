/// iOS 引擎布局派生(v2-P5 T8,spec §5.2 的 Swift 侧单一真源):
/// - home = App 私有 `{container}/Library/mtdss`(Files App 不可见;
///   session/settings/tasks/identity.seed/daemon.sock/fds.sock 全在其下);
/// - inbox(发布根/收件箱)= `{container}/Documents`(UIFileSharingEnabled
///   → Files App「我的 iPhone/MTDSS」可见)——与 Rust 桥
///   (crates/ios-bridge `mtdss_ios_start`)同款派生规则,两侧不得漂移;
/// - staging = `{home}/.mtdss`(引擎 `inbox_staging` 注入面,桥派生;
///   此处仅文档面 + 断言面)。
///
/// UDS 预算:`sockaddr_un.sun_path` 104 字节(macOS/iOS 含结尾 NUL)——
/// 真机最坏容器路径 + `/Library/mtdss` + `/daemon.sock` 必须 < 104
/// (真机 102 字节可容;模拟器容器路径 ≈170 字节必超,冒烟必须用短 home
/// env 覆盖,T4/T7 已知面)。
import Foundation

public struct EngineLayout: Equatable, Sendable {

    /// 引擎数据目录(桥 start 的单参;App 侧同时据它定位 fds.sock)。
    public let home: String

    public init(home: String) {
        self.home = home
    }

    /// 生产派生:容器 Documents → home = `{Documents 父}/Library/mtdss`。
    public init(containerDocuments: String) {
        let container = Self.parent(of: containerDocuments)
        self.init(home: container + "/" + Self.libraryName + "/" + Self.homeName)
    }

    // MARK: 布局常数(测试钉面;与桥/引擎逐一对照)

    public static let libraryName = "Library"
    public static let homeName = "mtdss"
    public static let documentsName = "Documents"
    /// 收件暂存目录名(引擎 STAGING_DIR 的 iOS 注入形态;App 私有隐藏目录)。
    public static let stagingName = ".mtdss"
    public static let daemonSocketName = "daemon.sock"
    public static let fdpassSocketName = "fds.sock"
    /// `sockaddr_un.sun_path` 长度(含结尾 NUL;路径本身须 < 此值)。
    public static let sunPathLimit = 104

    // MARK: 派生面

    /// 发布根/收件箱 = 容器 Documents(与桥的 recv_dir/authorized_roots
    /// 派生同一条规则):iOS 布局(home = `{container}/Library/mtdss`)下
    /// Documents 与 Library 同级——父目录的父才是容器根;平铺 home(DEBUG
    /// env 覆盖 `{override}/home`)父目录一级即 `{override}/Documents`。
    public var inbox: String {
        let firstParent = Self.parent(of: home)
        let base: String =
            (firstParent as NSString).lastPathComponent == Self.libraryName
            ? Self.parent(of: firstParent) : firstParent
        return base + "/" + Self.documentsName
    }

    /// 收件暂存区(文档/断言面;真实落盘由引擎按注入参数执行)。
    public var staging: String {
        home + "/" + Self.stagingName
    }

    /// 照片导出临时根(R8 终审 I-2 重定位):`{home}/photo-serve`——home
    /// 为 App 私有目录(Files App 不可见),但**在 `.mtdss` 暂存区命名空间
    /// 之外**:对端 sender 自选顶层 rel "photo-serve" 的收件中间态会暂存/
    /// 落位于 `<staging>/photo-serve`,serve 目录若落暂存区内,挂载清扫/
    /// 逐请求目录建删会在中断窗摧毁已暂存终件。0x07 导出件落此,引擎读发
    /// 后 unlink 文件,App 挂载清扫回收残目录;协议侧同时把 "photo-serve"
    /// 列入 rel 保留首段(manifest 校验,纵深防御)。目录名 App 侧单一
    /// 定义点(引擎无契约面——只消费 0x07 回传的绝对路径)。
    public var photoServeDirectory: String {
        home + "/" + Self.photoServeName
    }
    /// 照片导出临时目录名(R7)。
    public static let photoServeName = "photo-serve"

    /// App 缩略图生成件临时目录(V4-1 T11):`{home}/thumb-serve`——daemon
    /// `preview::thumb::thumb_serve_dir` 同锚,两侧不得漂移(Android
    /// MtdssEngine.thumbServeDir 同锚同纪律)。与引擎缓存目录 thumb-tmp
    /// **刻意分立**:App 挂载清扫整目录清空,若同目录会把引擎 LRU 缓存一
    /// 并抹掉(daemon preview::thumb.rs 模块注钉死;目录本体归 App——建目
    /// 录/清扫,引擎只读 + 逐件 unlink,serve-tmp/photo-serve 三面同构)。
    public var thumbServeDirectory: String {
        home + "/" + Self.thumbServeName
    }
    /// 缩略图生成件临时目录名(V4-1 T11)。
    public static let thumbServeName = "thumb-serve"

    /// App 转码段目录(V4-1 T14):`{home}/rendition-serve`——daemon
    /// `preview::rendition::rendition_serve_dir` 同锚,两侧不得漂移(Android
    /// MtdssEngine.renditionServeDir 同锚同纪律;段文件 = App 侧转码产物,
    /// daemon 读面按 `{serve}/{session}/{index}.m4s` 前缀校验读)。thumb-serve
    /// 同款「暂存区命名空间之外」裁定 + 与引擎缓存目录分立(挂载清扫整
    /// 目录清空;目录本体归 App——建目录/清扫/cancel 即清,引擎只读不删)。
    public var renditionServeDirectory: String {
        home + "/" + Self.renditionServeName
    }
    /// 转码段临时目录名(V4-1 T14)。
    public static let renditionServeName = "rendition-serve"

    /// 引擎 IPC UDS(ipc::socket_path 同源)。
    public var daemonSocketPath: String {
        home + "/" + Self.daemonSocketName
    }

    /// fdpass 查询/发布应答通道(serve_fdpass 同源)。
    public var fdpassSocketPath: String {
        home + "/" + Self.fdpassSocketName
    }

    /// UDS 预算:两 socket 路径 UTF-8 字节数都 < 104(NUL 含内)。
    /// 超预算 = bind 必败(模拟器容器路径的已知形态),调用方须换短 home。
    public var socketPathsFitSunPath: Bool {
        daemonSocketPath.utf8.count < Self.sunPathLimit
            && fdpassSocketPath.utf8.count < Self.sunPathLimit
    }

    /// 隐藏面断言:暂存区不得落在发布根下(Files App 可见面)——杀 App
    /// 中间态不可见的布局前提。
    public var stagingHiddenFromInbox: Bool {
        staging != inbox && !staging.hasPrefix(inbox + "/")
    }

    /// 剥最后一段路径(POSIX 形态;无尾斜杠口径,父不存在的根返回自身)。
    static func parent(of path: String) -> String {
        guard let idx = path.lastIndex(of: "/"), idx > path.startIndex else {
            return path
        }
        return String(path[..<idx])
    }
}
