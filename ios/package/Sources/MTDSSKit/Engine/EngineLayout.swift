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
