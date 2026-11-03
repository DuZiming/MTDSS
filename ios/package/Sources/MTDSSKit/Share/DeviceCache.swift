/// SharedDeviceCache — 设备列表缓存(T11,spec §4.7/§5.4):主 App 在
/// devices 事件/前台/启动时写入 App Group `{root}/cache/devices.json`;
/// 扩展读之渲染目标设备行。**过期口径:>5min(readWithStaleness stale=true)
/// 或缺失/损坏(nil)→ 扩展侧全部置灰 + 「打开 App 刷新」提示**——陈旧
/// 在线态宁可置灰,不误导用户点死设备。条目面 = 目录原样四元组
/// (device_id/name/platform/online;与 devices RPC/golden 同源)。
///
/// R11 ①:缓存自本版起降为**兜底源**(引擎不可开机时的回退面)——主源 =
/// 扩展引擎早开机后的 devices 现拉(``ShareDeviceList``,R10 起扩展链
/// 引擎可用;用户报告「分享扩展设备列表和 App 内不同步」:主 App 后台期
/// 缓存必然陈旧,现拉在线态才与 App 内一致)。
/// R12 ①:快照增 `self_device_id`(缓存路径滤本机的载荷面;live 路径以
/// 引擎 status 自身 id 权威滤),可见行排序对齐 App 设备页
/// (``DeviceListOrder/onlineFirst``)。

import Foundation

/// 缓存条目(devices 目录原样;经 DirectoryDevice 投影)。
public struct SharedDeviceEntry: Codable, Equatable, Sendable {
    public let deviceId: String
    public let name: String?
    public let platform: String?
    public let online: Bool

    public init(deviceId: String, name: String?, platform: String?, online: Bool) {
        self.deviceId = deviceId
        self.name = name
        self.platform = platform
        self.online = online
    }

    /// 目录行投影(devices RPC 模型 → 缓存条目)。
    public init(_ device: DirectoryDevice) {
        self.init(
            deviceId: device.deviceId, name: device.name,
            platform: device.platform, online: device.online)
    }

    enum CodingKeys: String, CodingKey {
        case deviceId = "device_id"
        case name
        case platform
        case online
    }
}

/// 缓存快照(刷新时刻 + 条目组 + 本机 id;蛇形键 golden 化)。
/// R12 ①:`self_device_id` 增量字段(可选——旧缓存文件缺键解码 nil,扩展
/// 缓存路径据此滤本机行;App 侧选设备弹层滤己同语义,T11 先在缺口收口:
/// 扩展面板曾把本机列为发送目标)。
public struct SharedDeviceCacheSnapshot: Codable, Equatable, Sendable {
    public let refreshedAt: Date
    public let devices: [SharedDeviceEntry]
    /// 写入端(主 App)本机 device_id(status.device;nil = 旧文件/未知)。
    public let selfDeviceId: String?

    public init(refreshedAt: Date, devices: [SharedDeviceEntry], selfDeviceId: String? = nil) {
        self.refreshedAt = refreshedAt
        self.devices = devices
        self.selfDeviceId = selfDeviceId
    }

    enum CodingKeys: String, CodingKey {
        case refreshedAt = "refreshed_at"
        case devices
        case selfDeviceId = "self_device_id"
    }
}

public struct SharedDeviceCache: Sendable {

    /// 过期线(秒;spec「主 App 定期写入;扩展读到陈旧缓存按离线置灰」)。
    public static let stalenessLimit: TimeInterval = 300

    /// 缓存文件 = `{root}/cache/devices.json`(root = App Group 容器)。
    public let fileURL: URL

    public init(root: URL) {
        self.fileURL = root
            .appendingPathComponent("cache", isDirectory: true)
            .appendingPathComponent("devices.json")
    }

    /// 写入(原子;目录即建)。仅主 App 前台期间调用(spec 口径)。
    public func write(
        devices: [SharedDeviceEntry], selfDeviceId: String? = nil, now: Date,
    ) throws {
        let snapshot = SharedDeviceCacheSnapshot(
            refreshedAt: now, devices: devices, selfDeviceId: selfDeviceId)
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        let data = try encoder.encode(snapshot)
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: fileURL, options: .atomic)
    }

    /// 读快照(缺失/损坏 = nil)。
    public func read() -> SharedDeviceCacheSnapshot? {
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return try? decoder.decode(SharedDeviceCacheSnapshot.self, from: data)
    }

    /// 读 + 过期判定:`nil` = 缺失/损坏(调用方全置灰);`stale` = 刷新
    /// 距今超过 ``stalenessLimit``(调用方全置灰 + 提示);`selfDeviceId` =
    /// 快照载荷的本机 id(扩展缓存路径滤己,旧文件缺键 = nil 不滤)。
    public func readWithStaleness(now: Date) -> (
        devices: [SharedDeviceEntry], stale: Bool, selfDeviceId: String?
    )? {
        guard let snapshot = read() else { return nil }
        return (
            snapshot.devices,
            now.timeIntervalSince(snapshot.refreshedAt) > Self.stalenessLimit,
            snapshot.selfDeviceId
        )
    }
}

/// 设备列表来源状态机(R11 ①,用户报告「分享扩展设备列表和 App 内不同
/// 步」):面板装载先读缓存渲染(即刻,不空等),随后引擎早开机现拉
/// devices **换入实时表**(在线态新鲜 = 与 App 内设备页同源);引擎不可
/// 开机(未同步/超 UDS 预算)或 devices 查询失败 → 缓存兜底(既有陈旧
/// 置灰语义只属该路径)。
///
///     init ──loadCache──▶ cache(rows, stale) / missing
///     cache/missing ──adoptLive──▶ live(rows)(换入,实时在线态)
///     cache/missing ──markLiveUnavailable──▶ 原态保留 + liveUnavailable
///
/// R12 ①(App/扩展列表完全统一,用户报告「列表不一致/排序不对齐/排除本机;
/// Linux 在线却被置灰」):
/// - **本机滤除**:``selfDeviceId`` 双源——缓存路径 = 快照
///   `self_device_id` 载荷(``loadCache(devices:stale:selfDeviceId:)``),
///   live 路径 = 扩展引擎 status 自身 id(``adoptLive(_:selfDeviceId:)``,
///   引擎与主 App 同设备身份,权威面);可见行 ``rows`` 恒滤本机(与 App
///   选设备弹层滤己同语义——不给发给自己)。
/// - **排序对齐 App**:``rows`` 走 ``DeviceListOrder/onlineFirst``(在线
///   在前组内保序)——与 App 设备页 ``DevicesViewModel/project`` 同一函数
///   单份,两列表逐行同序。
/// - **全平台可选**:平台不再参与可选性(Ruling I 客户端拒发已退役,见
///   ``SendService``;Linux/macOS 在线行不再置灰)。
///
/// 纯状态机宿主可测(Ruling E);编排(boot + devices 查询)在扩展目标
/// ``DirectSendEngine/liveDevices()``。
public struct ShareDeviceList: Equatable, Sendable {

    /// 列表来源(视图渲染分档:缓存档带陈旧置灰面;实时档恒新鲜)。
    public enum Source: Equatable, Sendable {
        /// 缓存渲染(stale = 过期置灰面;主 App 后台未刷新形)。
        case cache([SharedDeviceEntry], stale: Bool)
        /// 无缓存(缺失/损坏;「先打开 MTDSS App」提示)。
        case missing
        /// 引擎实时(devices RPC 现拉;在线态与 App 内一致)。
        case live([SharedDeviceEntry])
    }

    public private(set) var source: Source = .missing

    /// 实时面尝试已失败(引擎不可开机/devices 查询失败)——缓存态原样
    /// 兜底渲染,失败静默(R10 降级同语言;点选仍有 pick 时重试 boot 的
    /// 第二机会)。
    public private(set) var liveUnavailable = false

    /// 本机 device_id(缓存路径 = 快照载荷;live 路径 = 引擎 status 自身
    /// id,权威覆写;nil = 未知 → 行不滤,R11 旧形兼容)。
    public private(set) var selfDeviceId: String?

    public init() {}

    /// 可见行 = self 滤除 + 在线在前(App 设备页同源投影);missing = 空。
    public var rows: [SharedDeviceEntry] {
        switch source {
        case .cache(let rows, _), .live(let rows):
            return DeviceListOrder.onlineFirst(rows.filter { $0.deviceId != selfDeviceId }) {
                $0.online
            }
        case .missing: return []
        }
    }

    /// 缓存装载(面板装载即读;幂等可重读;live 已换入后再读**不回退**,
    /// 本面板装载序恒先缓存后 live,防御面不计)。`selfDeviceId` = 快照
    /// `self_device_id` 载荷(R12 ①)。
    public mutating func loadCache(
        devices: [SharedDeviceEntry]?, stale: Bool = false, selfDeviceId: String? = nil,
    ) {
        self.selfDeviceId = selfDeviceId
        guard let devices else {
            source = .missing
            return
        }
        source = .cache(devices, stale: stale)
    }

    /// 实时表换入(boot 成功 + devices 应答;任何源态可换——晚到不拒)。
    /// `selfDeviceId` = 扩展引擎 status 自身 id(权威面,覆写缓存载荷)。
    public mutating func adoptLive(_ devices: [SharedDeviceEntry], selfDeviceId: String? = nil) {
        if selfDeviceId != nil { self.selfDeviceId = selfDeviceId }
        source = .live(devices)
    }

    /// 实时面不可用标记(幂等;缓存/缺失态原样保留)。
    public mutating func markLiveUnavailable() {
        liveUnavailable = true
    }
}
