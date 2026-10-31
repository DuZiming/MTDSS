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

    /// 其他设备(本机滤除;在线在前、离线沉底)。
    @Published public private(set) var devices: [DirectoryDevice] = []
    /// daemon 现拉失败回退缓存(UI 注记「缓存」,数据仍可用)。
    @Published public private(set) var stale = false
    @Published public private(set) var loading = false
    @Published public private(set) var errorText: String?

    // MARK: 依赖

    private let repo: MtdssRepository
    /// 本机 device_id(status.device;未就绪为 nil → 不过滤,不误删)。
    private let selfDeviceId: String?

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
            devices = Self.project(result.devices, excluding: selfDeviceId)
            stale = result.stale
            loading = false
        } catch {
            devices = []
            stale = false
            loading = false
            errorText = BrowseViewModel.userErrorText(for: error)
        }
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
