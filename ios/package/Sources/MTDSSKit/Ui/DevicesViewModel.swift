/// DevicesViewModel — 设备页视图模型(spec §4.4 设备列表根):devices 现拉
/// (daemon 对服务器目录权威现拉,失败回退缓存 stale=true),**本机在列**
/// (R5b #3 真机轮:服务器目录恒含本机,列表不再滤己——本机行标「本机」
/// 不可进,浏览自己 = 首页收件箱冗余;DevicePickerSheet 仍滤己,不发自己),
/// 在线在前(离线置灰沉底,同 DevicePickerSheet 语言);「···」= 刷新
/// (spec §4.1:设备列表根页为「刷新」,load 即现拉)。错误态 T5
/// BrowseViewModel 同款:errorText + 清列表,重试/刷新承担恢复。
///
/// 每个仓库调用均经 ``RpcGuard/withTimeout(seconds:_:)`` 限时。宿主可测:
/// 纯 MTDSSKit,无 SwiftUI。

import Foundation

/// 设备平台图标/显示名(spec §4.4 mockup device-pages:桌面=显示器、
/// iOS=iPhone、Android=手机、未知=终端;平台名首字母大写,apple 系保原生
/// 写法)。R5b #3 自 App SelectionBar 提入 Kit——本机行副文格式化
/// (``DevicesRowFormat``)与 App 行渲染同源单份。
public enum DeviceIcon {
    /// SF 图标名。
    public static func symbol(_ platform: String?) -> String {
        switch platform?.lowercased() {
        case "windows", "linux", "macos": "display"
        case "ios": "iphone"
        case "android": "smartphone"
        default: "terminal"
        }
    }

    /// 平台显示名(mockup 「Windows · 在线」;未知平台/缺省 = 「未知」)。
    public static func displayName(_ platform: String?) -> String {
        switch platform?.lowercased() {
        case "windows": "Windows"
        case "linux": "Linux"
        case "macos": "macOS"
        case "ios": "iOS"
        case "android": "Android"
        case .some(let other): other.capitalized
        case nil: "未知"
        }
    }
}

/// 设备行副文(R5b #3):本机 = 「平台 · 本机」(不可进标记即徽章);
/// 对端 = 「平台 · 在线/离线」。纯函数,宿主可测。
public enum DevicesRowFormat {
    public static func subtitle(platform: String?, isSelf: Bool, online: Bool) -> String {
        let base = DeviceIcon.displayName(platform)
        return isSelf ? "\(base) · 本机" : "\(base) · \(online ? "在线" : "离线")"
    }
}

@MainActor
public final class DevicesViewModel: ObservableObject {

    /// 单次 devices 调用限时(秒;daemon 侧现拉 5s 超时,给足余量)。
    static let callTimeout: TimeInterval = 10

    // MARK: 只读状态(UI 订阅)

    /// daemon 目录原样面(未滤;可见面走 ``devices``)。
    @Published public private(set) var rawDevices: [DirectoryDevice] = []
    /// daemon 现拉失败回退缓存(UI 注记「缓存」,数据仍可用)。
    @Published public private(set) var stale = false
    @Published public private(set) var loading = false
    @Published public private(set) var errorText: String?

    /// 本机 device_id(status.device;AppModel 投影晚于首拉——晚到即重投影,
    /// 本机行标记实时收敛)。nil = 未知(AppModel 未就绪)→ 行在列但暂按
    /// 对端样(短窗,与 T7 时代「先显示后滤除」的收敛窗同类)。
    @Published public var selfDeviceId: String?

    /// 可见设备列表 = **本机在列**(R5b #3:不滤己)+ 在线在前(离线沉底)。
    public var devices: [DirectoryDevice] {
        Self.project(rawDevices)
    }

    // MARK: 依赖

    private let repo: MtdssRepository

    public init(repo: MtdssRepository, selfDeviceId: String?) {
        self.repo = repo
        self.selfDeviceId = selfDeviceId
    }

    // MARK: 现拉/刷新

    /// devices 现拉(进入标签/「···」刷新/错误重试同一入口)。
    public func load() async {
        loading = true
        errorText = nil
        do {
            let result = try await RpcGuard.withTimeout(seconds: Self.callTimeout) { [repo] in
                try await repo.devices()
            }
            rawDevices = result.devices
            stale = result.stale
            loading = false
        } catch {
            // R5a 真机轮 #4:取消 = 良性收口(下拉刷新被 SwiftUI 取消/
            // 离场),保留既有列表,不伪装「连接中断」(BrowseViewModel 同款)。
            if error is CancellationError {
                loading = false
                return
            }
            rawDevices = []
            stale = false
            loading = false
            errorText = BrowseViewModel.userErrorText(for: error)
        }
    }

    /// event.devices 增量收敛(T6 缓办携带:设备页随目录事件秒级跟线,
    /// 不等手动刷新):全量重投影,现拉数据面(陈旧 stale 标记一并清除)。
    public func apply(_ event: [DirectoryDevice]) {
        rawDevices = event
        stale = false
    }

    // MARK: 内部

    /// 目录投影(R5b #3):**不滤己**(服务器目录恒含本机,列表如实呈现,
    /// 本机行由视图按 ``selfDeviceId`` 标「本机」并禁点)+ 在线在前(组内
    /// 保序——daemon 目录投影即 name/platform 语义顺序,客户端不二次排序
    /// 免翻)。.online 键排序(Bool:false<true)即离线沉底,稳定。
    static func project(_ devices: [DirectoryDevice]) -> [DirectoryDevice] {
        devices.sorted { a, b in
            if a.online != b.online { return a.online }
            return false
        }
    }
}
