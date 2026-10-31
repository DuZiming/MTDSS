/// IPC 契约面:op 常量 + 类型化请求构造 + 应答/事件解析模型。
/// 字段与 crates/ipc/contract 目录下 *.json 对齐(单一真源,ContractGoldenTests
/// 全量锁定;daemon 侧对照 crates/daemon/src/api.rs)。类型面镜像
/// android/.../ipc/Contract.kt。
///
/// 请求参数构造**不关心键序**——IpcClient 序列化时统一按 UTF-8 字节序规范
/// 写出(serde_json 字节口径);应答解析同样与键序无关。

import Foundation

/// RPC 方法名与事件方法名常量(线协议字符串)。
public enum Ops {
    public static let hello = "hello"
    public static let status = "status"
    public static let devices = "devices"
    public static let loginRequestCode = "login.request_code"
    public static let loginVerify = "login.verify"
    public static let loginLogout = "login.logout"
    public static let list = "list"
    public static let stat = "stat"
    public static let transferStart = "transfer.start"
    public static let transferList = "transfer.list"
    public static let transferPause = "transfer.pause"
    public static let transferResume = "transfer.resume"
    public static let transferCancel = "transfer.cancel"
    public static let transferWait = "transfer.wait"
    public static let transferReinitiate = "transfer.reinitiate"
    public static let transferClear = "transfer.clear"

    public static let eventLogin = "event.login"
    public static let eventDevices = "event.devices"
    public static let eventOnline = "event.online"
    public static let eventTransfer = "event.transfer"
    public static let eventNotify = "event.notify"
}

// MARK: - 数据模型(字段名与 golden 契约一一对应)

/// 本机设备摘要(hello/status 的 device 投影;未登录 → device 整体为 null)。
public struct DeviceSummary: Equatable, Sendable {
    public let deviceId: String
    public let name: String?

    public init(deviceId: String, name: String?) {
        self.deviceId = deviceId
        self.name = name
    }

    public init(_ json: JSONValue) throws {
        self.deviceId = try json.req("device_id").asString()
        self.name = try json.optString("name")
    }

    /// hello/status 的 device 投影:键缺失/JSON null → nil(未登录面)。
    static func parse(_ json: JSONValue?) throws -> DeviceSummary? {
        guard let json, !json.isNull else { return nil }
        return try DeviceSummary(json)
    }
}

/// 设备目录条目(devices / event.devices;name/platform 可空)。
public struct DirectoryDevice: Equatable, Sendable {
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

    public init(_ json: JSONValue) throws {
        self.deviceId = try json.req("device_id").asString()
        self.name = try json.optString("name")
        self.platform = try json.optString("platform")
        self.online = try json.req("online").asBool()
    }
}

public struct HelloResult: Equatable, Sendable {
    public let version: String
    public let loggedIn: Bool
    public let device: DeviceSummary?

    public init(version: String, loggedIn: Bool, device: DeviceSummary?) {
        self.version = version
        self.loggedIn = loggedIn
        self.device = device
    }

    public init(_ json: JSONValue) throws {
        self.version = try json.req("version").asString()
        self.loggedIn = try json.req("logged_in").asBool()
        self.device = try DeviceSummary.parse(json["device"])
    }
}

public struct StatusResult: Equatable, Sendable {
    public let loggedIn: Bool
    public let online: Bool
    public let server: String?
    public let recvDir: String?
    public let device: DeviceSummary?

    public init(loggedIn: Bool, online: Bool, server: String?, recvDir: String?, device: DeviceSummary?) {
        self.loggedIn = loggedIn
        self.online = online
        self.server = server
        self.recvDir = recvDir
        self.device = device
    }

    public init(_ json: JSONValue) throws {
        self.loggedIn = try json.req("logged_in").asBool()
        self.online = try json.req("online").asBool()
        self.server = try json.optString("server")
        self.recvDir = try json.optString("recv_dir")
        self.device = try DeviceSummary.parse(json["device"])
    }
}

public struct DevicesResult: Equatable, Sendable {
    public let devices: [DirectoryDevice]
    public let stale: Bool

    public init(devices: [DirectoryDevice], stale: Bool) {
        self.devices = devices
        self.stale = stale
    }

    public init(_ json: JSONValue) throws {
        self.devices = try (try json.req("devices").asArray()).map { try DirectoryDevice($0) }
        self.stale = try json.req("stale").asBool()
    }
}

public struct LoginRequestCodeResult: Equatable, Sendable {
    public let code: String

    public init(code: String) { self.code = code }

    public init(_ json: JSONValue) throws {
        self.code = try json.req("code").asString()
    }
}

public struct LoginVerifyResult: Equatable, Sendable {
    public let deviceId: String
    public let name: String

    public init(deviceId: String, name: String) {
        self.deviceId = deviceId
        self.name = name
    }

    public init(_ json: JSONValue) throws {
        self.deviceId = try json.req("device_id").asString()
        self.name = try json.req("name").asString()
    }
}

/// 远端目录条目(list / stat)。
public struct DirEntry: Equatable, Sendable {
    public let name: String
    public let path: String
    public let size: Int64
    public let mtimeMs: Int64
    public let isDir: Bool

    public init(name: String, path: String, size: Int64, mtimeMs: Int64, isDir: Bool) {
        self.name = name
        self.path = path
        self.size = size
        self.mtimeMs = mtimeMs
        self.isDir = isDir
    }

    public init(_ json: JSONValue) throws {
        self.name = try json.req("name").asString()
        self.path = try json.req("path").asString()
        self.size = try json.req("size").asInt64()
        self.mtimeMs = try json.req("mtime_ms").asInt64()
        self.isDir = try json.req("is_dir").asBool()
    }
}

public struct ListResult: Equatable, Sendable {
    public let entries: [DirEntry]
    public let truncated: Bool

    public init(entries: [DirEntry], truncated: Bool) {
        self.entries = entries
        self.truncated = truncated
    }

    public init(_ json: JSONValue) throws {
        self.entries = try (try json.req("entries").asArray()).map { try DirEntry($0) }
        self.truncated = try json.req("truncated").asBool()
    }
}

public struct StatResult: Equatable, Sendable {
    public let entry: DirEntry

    public init(entry: DirEntry) { self.entry = entry }

    public init(_ json: JSONValue) throws {
        self.entry = try DirEntry(json.req("entry"))
    }
}

/// fd 桥上传条目元数据(transfer.start 的 upload-bridge 形态;ref = App 侧
/// SourceResolver 引用;rel = 目标相对路径;size/mtime = 预扫元数据)。
public struct BridgeItem: Equatable, Sendable {
    public let ref: String
    public let rel: String
    public let size: Int64
    public let mtime: Int64

    public init(ref: String, rel: String, size: Int64, mtime: Int64) {
        self.ref = ref
        self.rel = rel
        self.size = size
        self.mtime = mtime
    }
}

public struct TransferStartResult: Equatable, Sendable {
    public let jobId: String

    public init(jobId: String) { self.jobId = jobId }

    public init(_ json: JSONValue) throws {
        self.jobId = try json.req("job_id").asString()
    }
}

/// 传输作业快照(transfer.list 条目;实时进度事件见 TransferProgress)。
public struct TransferJob: Equatable, Sendable {
    public let id: String
    public let kind: String
    public let name: String
    public let state: String
    public let filesDone: Int64
    public let filesTotal: Int64
    public let bytesDone: Int64
    public let bytesTotal: Int64
    public let speedBps: Int64
    public let skipped: Int64
    public let relay: Bool
    public let src: String?
    public let dst: String?
    public let error: String?

    public init(
        id: String, kind: String, name: String, state: String,
        filesDone: Int64, filesTotal: Int64, bytesDone: Int64, bytesTotal: Int64,
        speedBps: Int64, skipped: Int64, relay: Bool,
        src: String?, dst: String?, error: String?,
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.state = state
        self.filesDone = filesDone
        self.filesTotal = filesTotal
        self.bytesDone = bytesDone
        self.bytesTotal = bytesTotal
        self.speedBps = speedBps
        self.skipped = skipped
        self.relay = relay
        self.src = src
        self.dst = dst
        self.error = error
    }

    public init(_ json: JSONValue) throws {
        self.id = try json.req("id").asString()
        self.kind = try json.req("kind").asString()
        self.name = try json.req("name").asString()
        self.state = try json.req("state").asString()
        self.filesDone = try json.req("files_done").asInt64()
        self.filesTotal = try json.req("files_total").asInt64()
        self.bytesDone = try json.req("bytes_done").asInt64()
        self.bytesTotal = try json.req("bytes_total").asInt64()
        self.speedBps = try json.req("speed_bps").asInt64()
        self.skipped = try json.req("skipped").asInt64()
        self.relay = try json.req("relay").asBool()
        self.src = try json.optString("src")
        self.dst = try json.optString("dst")
        self.error = try json.optString("error")
    }
}

public struct TransferListResult: Equatable, Sendable {
    public let jobs: [TransferJob]

    public init(jobs: [TransferJob]) { self.jobs = jobs }

    public init(_ json: JSONValue) throws {
        self.jobs = try (try json.req("jobs").asArray()).map { try TransferJob($0) }
    }
}

public struct TransferWaitResult: Equatable, Sendable {
    public let state: String
    public let error: String?
    public let filesDone: Int64
    public let filesTotal: Int64
    public let bytesTotal: Int64
    public let skipped: Int64

    public init(state: String, error: String?, filesDone: Int64, filesTotal: Int64, bytesTotal: Int64, skipped: Int64) {
        self.state = state
        self.error = error
        self.filesDone = filesDone
        self.filesTotal = filesTotal
        self.bytesTotal = bytesTotal
        self.skipped = skipped
    }

    public init(_ json: JSONValue) throws {
        self.state = try json.req("state").asString()
        self.error = try json.optString("error")
        self.filesDone = try json.req("files_done").asInt64()
        self.filesTotal = try json.req("files_total").asInt64()
        self.bytesTotal = try json.req("bytes_total").asInt64()
        self.skipped = try json.req("skipped").asInt64()
    }
}

/// event.transfer 的进度参数面(与 TransferJob 同字段族,无 name/src/dst)。
public struct TransferProgress: Equatable, Sendable {
    public let id: String
    public let kind: String
    public let state: String
    public let filesDone: Int64
    public let filesTotal: Int64
    public let bytesDone: Int64
    public let bytesTotal: Int64
    public let speedBps: Int64
    public let skipped: Int64
    public let relay: Bool
    public let error: String?

    public init(
        id: String, kind: String, state: String,
        filesDone: Int64, filesTotal: Int64, bytesDone: Int64, bytesTotal: Int64,
        speedBps: Int64, skipped: Int64, relay: Bool, error: String?,
    ) {
        self.id = id
        self.kind = kind
        self.state = state
        self.filesDone = filesDone
        self.filesTotal = filesTotal
        self.bytesDone = bytesDone
        self.bytesTotal = bytesTotal
        self.speedBps = speedBps
        self.skipped = skipped
        self.relay = relay
        self.error = error
    }

    public init(_ json: JSONValue) throws {
        self.id = try json.req("id").asString()
        self.kind = try json.req("kind").asString()
        self.state = try json.req("state").asString()
        self.filesDone = try json.req("files_done").asInt64()
        self.filesTotal = try json.req("files_total").asInt64()
        self.bytesDone = try json.req("bytes_done").asInt64()
        self.bytesTotal = try json.req("bytes_total").asInt64()
        self.speedBps = try json.req("speed_bps").asInt64()
        self.skipped = try json.req("skipped").asInt64()
        self.relay = try json.req("relay").asBool()
        self.error = try json.optString("error")
    }
}

/// 类型化事件(event.* 五枚)。未知方法不产生事件(消费方无感,保持前向兼容);
/// 已知事件的畸形参数同帧丢弃(形状由 ContractGoldenTests 锁定)。
public enum IpcEvent: Equatable, Sendable {
    case login(loggedIn: Bool)
    case online(online: Bool)
    case devices([DirectoryDevice])
    case transfer(TransferProgress)
    case notify(title: String, body: String)

    /// 通知帧参数 → 类型化事件;未知方法 → nil。
    static func parse(method: String, params: JSONValue) -> IpcEvent? {
        switch method {
        case Ops.eventLogin:
            guard let v = try? params.req("logged_in").asBool() else { return nil }
            return .login(loggedIn: v)
        case Ops.eventOnline:
            guard let v = try? params.req("online").asBool() else { return nil }
            return .online(online: v)
        case Ops.eventDevices:
            guard let arr = try? params.req("devices").asArray(),
                  let devices = try? arr.map({ try DirectoryDevice($0) }) else { return nil }
            return .devices(devices)
        case Ops.eventTransfer:
            guard let p = try? TransferProgress(params) else { return nil }
            return .transfer(p)
        case Ops.eventNotify:
            guard let title = try? params.req("title").asString(),
                  let body = try? params.req("body").asString() else { return nil }
            return .notify(title: title, body: body)
        default:
            return nil
        }
    }
}

/// 类型化请求参数构造(纯函数,无 IO)。每个请求形态一个 builder,
/// 应答按 result 值经各模型的 `init(_ json:)` 解析(信封由 IpcClient 剥)。
public enum Contract {

    public static func emptyParams() -> JSONValue { .object([:]) }

    /// login.request_code {server, email, ca?}(ca = 自签 CA 证书路径,空则键省略)。
    public static func loginRequestCodeParams(server: String, email: String, ca: String? = nil) -> JSONValue {
        var obj: [String: JSONValue] = [
            "server": .string(server),
            "email": .string(email),
        ]
        if let ca { obj["ca"] = .string(ca) }
        return .object(obj)
    }

    /// login.verify {code, email, name}。
    public static func loginVerifyParams(code: String, email: String, name: String) -> JSONValue {
        .object([
            "code": .string(code),
            "email": .string(email),
            "name": .string(name),
        ])
    }

    /// list {device, path}(path 空串 = 根卷)。
    public static func listParams(device: String, path: String) -> JSONValue {
        .object([
            "device": .string(device),
            "path": .string(path),
        ])
    }

    /// stat {device, path}。
    public static func statParams(device: String, path: String) -> JSONValue {
        listParams(device: device, path: path)
    }

    /// transfer.start 路径形态上传 {kind:"upload", files, to, dir, skip, overwrite, cut}。
    public static func transferStartUploadParams(
        files: [String], to: String, dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) -> JSONValue {
        .object([
            "kind": .string("upload"),
            "files": .array(files.map { .string($0) }),
            "to": .string(to),
            "dir": .string(dir),
            "skip": .array(skip.map { .string($0) }),
            "overwrite": .array(overwrite.map { .string($0) }),
            "cut": .bool(cut),
        ])
    }

    /// transfer.start fd 桥形态上传 {kind:"upload", source:"bridge", items, to,
    /// dir, skip, overwrite, cut}(App 经 SourceResolver 按需开 fd,懒传递)。
    public static func transferStartUploadBridgeParams(
        items: [BridgeItem], to: String, dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) -> JSONValue {
        .object([
            "kind": .string("upload"),
            "source": .string("bridge"),
            "items": .array(items.map { item in
                .object([
                    "ref": .string(item.ref),
                    "rel": .string(item.rel),
                    "size": .int(item.size),
                    "mtime": .int(item.mtime),
                ])
            }),
            "to": .string(to),
            "dir": .string(dir),
            "skip": .array(skip.map { .string($0) }),
            "overwrite": .array(overwrite.map { .string($0) }),
            "cut": .bool(cut),
        ])
    }

    /// transfer.start 下载 {kind:"download", device, paths, dir, skip, overwrite, cut}。
    /// dir 收件箱哨兵 "inbox"(安卓平台投影)或绝对路径。
    public static func transferStartDownloadParams(
        device: String, paths: [String], dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) -> JSONValue {
        .object([
            "kind": .string("download"),
            "device": .string(device),
            "paths": .array(paths.map { .string($0) }),
            "dir": .string(dir),
            "skip": .array(skip.map { .string($0) }),
            "overwrite": .array(overwrite.map { .string($0) }),
            "cut": .bool(cut),
        ])
    }

    /// pause/resume/cancel/wait/reinitiate/clear 共用的 {id} 参数。
    public static func transferIdParams(id: String) -> JSONValue {
        .object(["id": .string(id)])
    }
}
