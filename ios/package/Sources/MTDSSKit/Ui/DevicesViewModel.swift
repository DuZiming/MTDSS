/// DevicesViewModel — 设备页视图模型(spec §4.4 设备列表根):devices 现拉
/// (daemon 对服务器目录权威现拉,失败回退缓存 stale=true),本机滤除、
/// 在线在前(离线置灰沉底,同 DevicePickerSheet 语言);「···」= 刷新
/// (spec §4.1:设备列表根页为「刷新」,load 即现拉)。错误态 T5
/// BrowseViewModel 同款:errorText + 清列表,重试/刷新承担恢复。
///
/// 每个仓库调用均经 ``RpcGuard/withTimeout(seconds:_:)`` 限时。宿主可测:
/// 纯 MTDSSKit,无 SwiftUI。

import Foundation

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
    /// 可见面即时收敛)。nil = 未知(AppModel 未就绪)→ 不过滤,不误删。
    @Published public var selfDeviceId: String?

    /// 可见设备列表 = 本机滤除 + 在线在前(离线沉底)。
    public var devices: [DirectoryDevice] {
        Self.project(rawDevices, excluding: selfDeviceId)
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

    /// 目录投影:本机滤除 + 在线在前(组内保序——daemon 目录投影即
    /// name/platform 语义顺序,客户端不二次排序免翻)。.online 键排序
    /// (Bool:false<true)即离线沉底,稳定。
    static func project(_ devices: [DirectoryDevice], excluding selfId: String?) -> [DirectoryDevice] {
        devices
            .filter { device in device.deviceId != selfId }
            .sorted { a, b in
                if a.online != b.online { return a.online }
                return false
            }
    }
}
