/// SharedDeviceCache — 设备列表缓存(T11,spec §4.7/§5.4):主 App 在
/// devices 事件/前台/启动时写入 App Group `{root}/cache/devices.json`;
/// 扩展读之渲染目标设备行。**过期口径:>5min(readWithStaleness stale=true)
/// 或缺失/损坏(nil)→ 扩展侧全部置灰 + 「打开 App 刷新」提示**——陈旧
/// 在线态宁可置灰,不误导用户点死设备。条目面 = 目录原样四元组
/// (device_id/name/platform/online;与 devices RPC/golden 同源)。

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

/// 缓存快照(刷新时刻 + 条目组;蛇形键 golden 化)。
public struct SharedDeviceCacheSnapshot: Codable, Equatable, Sendable {
    public let refreshedAt: Date
    public let devices: [SharedDeviceEntry]

    public init(refreshedAt: Date, devices: [SharedDeviceEntry]) {
        self.refreshedAt = refreshedAt
        self.devices = devices
    }

    enum CodingKeys: String, CodingKey {
        case refreshedAt = "refreshed_at"
        case devices
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
    public func write(devices: [SharedDeviceEntry], now: Date) throws {
        let snapshot = SharedDeviceCacheSnapshot(refreshedAt: now, devices: devices)
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
    /// 距今超过 ``stalenessLimit``(调用方全置灰 + 提示)。
    public func readWithStaleness(now: Date) -> (devices: [SharedDeviceEntry], stale: Bool)? {
        guard let snapshot = read() else { return nil }
        return (snapshot.devices, now.timeIntervalSince(snapshot.refreshedAt) > Self.stalenessLimit)
    }
}
