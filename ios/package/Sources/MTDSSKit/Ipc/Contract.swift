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
    public static let loginSubmitCode = "login.submit_code"
    public static let loginStatus = "login.status"
    public static let loginEvict = "login.evict"
    public static let loginCancel = "login.cancel"
    public static let loginQrBegin = "login.qr_begin"
    public static let loginQrStatus = "login.qr_status"
    public static let qrClaim = "qr.claim"
    public static let qrApprove = "qr.approve"
    public static let approvalRespond = "approval.respond"
    public static let kvGet = "kv.get"
    public static let kvPut = "kv.put"
    public static let loginLogout = "login.logout"
    public static let list = "list"
    public static let stat = "stat"
    public static let delete = "delete"
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
    public static let eventDevicesChanged = "event.devices_changed"
    /// 账号数据变更通告(V3-3 spec §7;params {key} = 变更 KV 键,
    /// 客户端凭 key 重拉全量,favorites 消费面)。
    public static let eventDataChanged = "event.data_changed"
    public static let eventApproval = "event.approval"
    public static let eventDeviceEvicted = "event.device_evicted"
    public static let eventOnline = "event.online"
    public static let eventTransfer = "event.transfer"
    public static let eventNotify = "event.notify"

    /// 收件目的地在场查询帧(daemon → App;P4a §6.5,经 fdpass 0x04 应答)。
    public static let eventQueryDest = "fs.query_dest"
    /// 收件发布请求帧(daemon → App;P4a §3.2,App move 暂存件进收件箱后
    /// 经 fdpass 0x05 应答)。
    public static let eventPublish = "fs.publish"
    /// 照片列表请求帧(daemon → App;R7 照片图库虚拟根,path = 虚拟子路径
    /// ""=根/相册名,App 经 fdpass 0x06 应答)。
    public static let eventPhotoList = "fs.photo_list"
    /// 照片导出请求帧(daemon → App;R7,photo_id = 资产标识,App 导出原始
    /// 字节落自有临时目录后经 fdpass 0x07 回路径)。
    public static let eventPhotoExport = "fs.photo_export"
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
    /// 回显验证码(dev 回显门开);产线邮件直投响应为 `{}`,解析容缺省为
    /// 空串(final review C1),空 = 不自动预填(spec §3.6「有则填」)。
    public let code: String

    public init(code: String) { self.code = code }

    public init(_ json: JSONValue) throws {
        self.code = try json.optString("code") ?? ""
    }
}

/// 登录 ok 收口投影(device_id + 终端名;LoginModel ``succeeded`` 一次性
/// 屏内信号与 SessionModel ``applyLogin`` 的载荷;name 缺席 = daemon 槽内
/// 无候选名,终端名以账本/平台缺省回退)。
public struct LoginVerifyResult: Equatable, Sendable {
    public let deviceId: String
    public let name: String?

    public init(deviceId: String, name: String?) {
        self.deviceId = deviceId
        self.name = name
    }
}

/// 登录步进(daemon step_json 的 status 判别面;login.verify / login.submit_code /
/// login.status / login.evict / login.qr_status 五 op 共用,字段名与 golden
/// 一字对齐;镜像 android LoginStep)。ok = 收口(会话已落盘;续接 op 用
/// 槽内候选名,缺席 nil)。
public enum LoginStep: Equatable, Sendable {
    /// 收口(会话已落盘)。
    case ok(LoginVerifyResult)
    /// 扫码轮询面(等待扫码确认;V3-4 消费,本任务仅契约绑定)。
    case pending(expiresAt: Int64?)
    /// 审批挂起(受信终端弹窗或 6 位码直输;expiresAt = 到期秒)。
    case approval(sessionId: String, expiresAt: Int64?)
    /// 配额超额(踢选屏:消费单次 enroll_token 移出所选设备)。
    case limit(enrollToken: String, devices: [DirectoryDevice])
    /// 轮询终态:被拒。
    case denied
    /// 轮询终态:超时(服务端 TTL 收口)。
    case expired

    /// status 键路由解析(未知值/字段缺失即契约违规,不猜)。
    public init(_ json: JSONValue) throws {
        let status = try json.req("status").asString()
        switch status {
        case "ok":
            self = .ok(LoginVerifyResult(
                deviceId: try json.req("device_id").asString(),
                name: try json.optString("name"),
            ))
        case "pending":
            self = .pending(expiresAt: try json.optInt64("expires_at"))
        case "approval":
            self = .approval(
                sessionId: try json.req("session_id").asString(),
                expiresAt: try json.optInt64("expires_at"),
            )
        case "limit":
            self = .limit(
                enrollToken: try json.req("enroll_token").asString(),
                devices: try (try json.req("devices").asArray()).map { try DirectoryDevice($0) },
            )
        case "denied":
            self = .denied
        case "expired":
            self = .expired
        default:
            throw RpcError(code: -32602, message: "未知登录步进 status: \(status)")
        }
    }
}

/// login.qr_begin 投影(daemon 拼 https://www.mtdss.top/qr?t=<token>,120s TTL;
/// 扫码器 UI 随 V3-4 落,本任务仅契约绑定)。
public struct QrBeginResult: Equatable, Sendable {
    public let qrUrl: String
    public let token: String
    public let expiresAt: Int64

    public init(qrUrl: String, token: String, expiresAt: Int64) {
        self.qrUrl = qrUrl
        self.token = token
        self.expiresAt = expiresAt
    }

    public init(_ json: JSONValue) throws {
        self.qrUrl = try json.req("qr_url").asString()
        self.token = try json.req("token").asString()
        self.expiresAt = try json.req("expires_at").asInt64()
    }
}

/// qr.claim 投影(候选面拍平,确认弹层数据源;daemon 持会话 Bearer 代打)。
public struct QrClaimResult: Equatable, Sendable {
    public let candidateName: String
    public let candidatePlatform: String
    public let expiresAt: Int64

    public init(candidateName: String, candidatePlatform: String, expiresAt: Int64) {
        self.candidateName = candidateName
        self.candidatePlatform = candidatePlatform
        self.expiresAt = expiresAt
    }

    public init(_ json: JSONValue) throws {
        self.candidateName = try json.req("candidate_name").asString()
        self.candidatePlatform = try json.req("candidate_platform").asString()
        self.expiresAt = try json.req("expires_at").asInt64()
    }
}

/// qr.approve 投影(status 集合 ok|denied|expired|limit;limit 为防御面——
/// 配额超额收口在 PC 侧 qr_status 轮询,本 op 恒不投影 limit)。
public struct QrApproveResult: Equatable, Sendable {
    public let status: String

    public init(status: String) {
        self.status = status
    }

    public init(_ json: JSONValue) throws {
        self.status = try json.req("status").asString()
    }
}

/// 远端目录条目(list / stat)。children = 目录直接子项数(v2-P5 T5b 增量
/// 可选面:仅目录项携带,文件项/未知缺省 nil——proto3 optional 线上语义;
/// 全量计数含 dotfiles,「N 项」显示过滤归 UI 侧)。
public struct DirEntry: Equatable, Sendable {
    public let name: String
    public let path: String
    public let size: Int64
    public let mtimeMs: Int64
    public let isDir: Bool
    public let children: Int64?

    public init(
        name: String, path: String, size: Int64, mtimeMs: Int64, isDir: Bool,
        children: Int64? = nil,
    ) {
        self.name = name
        self.path = path
        self.size = size
        self.mtimeMs = mtimeMs
        self.isDir = isDir
        self.children = children
    }

    public init(_ json: JSONValue) throws {
        self.name = try json.req("name").asString()
        self.path = try json.req("path").asString()
        self.size = try json.req("size").asInt64()
        self.mtimeMs = try json.req("mtime_ms").asInt64()
        self.isDir = try json.req("is_dir").asBool()
        self.children = try json.optInt64("children")
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

/// `delete` 批内单条失败(字段与 daemon 序列化逐字对齐;code 为 FsError
/// 稳定错误码串,如 NOT_FOUND / PERMISSION_DENIED / INTERNAL)。
public struct DeleteFailure: Equatable, Sendable {
    public let path: String
    public let code: String
    public let message: String

    public init(path: String, code: String, message: String) {
        self.path = path
        self.code = code
        self.message = message
    }

    public init(_ json: JSONValue) throws {
        self.path = try json.req("path").asString()
        self.code = try json.req("code").asString()
        self.message = try json.req("message").asString()
    }
}

/// `delete` 响应:成功数 / 是否走了回收 / 逐条失败表(批不半途弃废)。
/// iOS 引擎恒永久删(android/ios 无回收设施,core fs 收口 force=true,
/// recycled 恒 false;桌面才有回收语义)。
public struct DeleteResult: Equatable, Sendable {
    public let deleted: Int64
    public let recycled: Bool
    public let failed: [DeleteFailure]

    public init(deleted: Int64, recycled: Bool, failed: [DeleteFailure]) {
        self.deleted = deleted
        self.recycled = recycled
        self.failed = failed
    }

    public init(_ json: JSONValue) throws {
        self.deleted = try json.req("deleted").asInt64()
        self.recycled = try json.req("recycled").asBool()
        self.failed = try (try json.req("failed").asArray()).map { try DeleteFailure($0) }
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

/// 类型化事件(event.* 九枚 + fs.* 收件应答四枚,T8 起 V3-1/V3-3 增量)。
/// 未知方法不产生事件(消费方无感,保持前向兼容);已知事件的畸形参数同帧
/// 丢弃(形状由 ContractGoldenTests 锁定)。
public enum IpcEvent: Equatable, Sendable {
    case login(loggedIn: Bool)
    case online(online: Bool)
    case devices([DirectoryDevice])
    case transfer(TransferProgress)
    case notify(title: String, body: String)
    /// fs.query_dest {request_id, rel, size}:收件箱 rel 在场查询(对账
    /// Skip 锚的第二层;InboxPublisher 按 App 收件箱真实行经 fdpass 0x04
    /// 应答,3s 窗)。
    case destQuery(requestId: UInt64, rel: String, size: UInt64)
    /// fs.publish {request_id, rel, staging_path, size}:发布请求(rel =
    /// final_rel,暂存区唯一名;InboxPublisher move 暂存件进收件箱本名后
    /// 经 fdpass 0x05 应答,30s 基值 + size/10MB 窗)。
    case publish(requestId: UInt64, rel: String, stagingPath: String, size: UInt64)
    /// fs.photo_list {request_id, path}:照片列表请求(R7;path = 虚拟子
    /// 路径,"" = 根 = 相册列表,相册名 = 相册内照片;PhotoResponder 经
    /// fdpass 0x06 应答,权限门在应答方)。
    case photoList(requestId: UInt64, path: String)
    /// fs.photo_export {request_id, photo_id}:照片导出请求(R7;导出原始
    /// 字节落自有临时目录后经 fdpass 0x07 回绝对路径)。
    case photoExport(requestId: UInt64, photoId: String)
    /// event.approval {session_id, candidate_name, candidate_platform, code,
    /// expires_at}:登录审批请求(V3-1;受信终端弹窗,只在前台生效)。
    case approval(
        sessionId: String, candidateName: String, candidatePlatform: String?,
        code: String, expiresAt: Int64,
    )
    /// event.device_evicted {}:本设备已被移除(会话面清账回登录态)。
    case deviceEvicted
    /// event.devices_changed {}:设备目录已变(daemon 随后自刷快照广播
    /// event.devices;客户端显式忽略,桌面同款口径)。
    case devicesChanged
    /// event.data_changed {key}:账号 KV 变更通告(V3-3 spec §7;key =
    /// 变更 KV 键,客户端凭 key 重拉全量,favorites 消费面)。
    case dataChanged(key: String)

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
        case Ops.eventQueryDest:
            guard let id = try? params.req("request_id").asUInt64(),
                  let rel = try? params.req("rel").asString(),
                  let size = try? params.req("size").asUInt64() else { return nil }
            return .destQuery(requestId: id, rel: rel, size: size)
        case Ops.eventPublish:
            guard let id = try? params.req("request_id").asUInt64(),
                  let rel = try? params.req("rel").asString(),
                  let stagingPath = try? params.req("staging_path").asString(),
                  let size = try? params.req("size").asUInt64() else { return nil }
            return .publish(requestId: id, rel: rel, stagingPath: stagingPath, size: size)
        case Ops.eventPhotoList:
            guard let id = try? params.req("request_id").asUInt64(),
                  let path = try? params.req("path").asString() else { return nil }
            return .photoList(requestId: id, path: path)
        case Ops.eventPhotoExport:
            guard let id = try? params.req("request_id").asUInt64(),
                  let photoId = try? params.req("photo_id").asString() else { return nil }
            return .photoExport(requestId: id, photoId: photoId)
        case Ops.eventApproval:
            guard let sessionId = try? params.req("session_id").asString(),
                  let candidateName = try? params.req("candidate_name").asString(),
                  let code = try? params.req("code").asString(),
                  let expiresAt = try? params.req("expires_at").asInt64() else { return nil }
            return .approval(
                sessionId: sessionId,
                candidateName: candidateName,
                candidatePlatform: try? params.optString("candidate_platform"),
                code: code,
                expiresAt: expiresAt,
            )
        case Ops.eventDeviceEvicted:
            return .deviceEvicted
        case Ops.eventDevicesChanged:
            return .devicesChanged
        case Ops.eventDataChanged:
            guard let key = try? params.req("key").asString() else { return nil }
            return .dataChanged(key: key)
        default:
            return nil
        }
    }
}

/// 类型化请求参数构造(纯函数,无 IO)。每个请求形态一个 builder,
/// 应答按 result 值经各模型的 `init(_ json:)` 解析(信封由 IpcClient 剥)。
public enum Contract {

    /// 本机保留别名(daemon 侧 list/stat/delete 的 `device` 参数取此值即
    /// 本机短路 core fs,不经网络、不要求会话;crates/daemon SELF_DEVICE_ALIAS
    /// 同字面)。未登录时本机 device_id 不存在,本地收件箱浏览(T13b 修#4)
    /// 以此为 device 参数。
    public static let selfDevice = "self"

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

    /// login.submit_code {session_id, code}(等待审批面的 6 位码直输)。
    public static func loginSubmitCodeParams(sessionId: String, code: String) -> JSONValue {
        .object([
            "session_id": .string(sessionId),
            "code": .string(code),
        ])
    }

    /// login.status {session_id}(审批轮询)。
    public static func loginStatusParams(sessionId: String) -> JSONValue {
        .object(["session_id": .string(sessionId)])
    }

    /// login.evict {enroll_token, device_id}(踢选屏动作)。
    public static func loginEvictParams(enrollToken: String, deviceId: String) -> JSONValue {
        .object([
            "enroll_token": .string(enrollToken),
            "device_id": .string(deviceId),
        ])
    }

    /// login.cancel {}(作废 daemon 续接槽;服务端会话 TTL 自收口)。
    public static func loginCancelParams() -> JSONValue {
        emptyParams()
    }

    /// login.qr_begin {server, name?, ca?}(可省键缺省即省略;扫码器随 V3-4)。
    public static func loginQrBeginParams(server: String, name: String?, ca: String? = nil) -> JSONValue {
        var obj: [String: JSONValue] = ["server": .string(server)]
        if let name { obj["name"] = .string(name) }
        if let ca { obj["ca"] = .string(ca) }
        return .object(obj)
    }

    /// login.qr_status {token}。
    public static func loginQrStatusParams(token: String) -> JSONValue {
        .object(["token": .string(token)])
    }

    /// approval.respond {session_id, allow}(受信终端弹窗应答)。
    public static func approvalRespondParams(sessionId: String, allow: Bool) -> JSONValue {
        .object([
            "session_id": .string(sessionId),
            "allow": .bool(allow),
        ])
    }

    /// qr.claim {token}(手机侧扫码认领;daemon 持会话 Bearer 代打)。
    public static func qrClaimParams(token: String) -> JSONValue {
        .object(["token": .string(token)])
    }

    /// qr.approve {token, allow}(认领者裁决)。
    public static func qrApproveParams(token: String, allow: Bool) -> JSONValue {
        .object([
            "token": .string(token),
            "allow": .bool(allow),
        ])
    }

    /// kv.get {key}。
    public static func kvGetParams(key: String) -> JSONValue {
        .object(["key": .string(key)])
    }

    /// kv.put {key, value}。
    public static func kvPutParams(key: String, value: String) -> JSONValue {
        .object([
            "key": .string(key),
            "value": .string(value),
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

    /// transfer.start 远端调度 {kind:"remote_copy", from, paths, to, dir,
    /// skip, overwrite, cut}(远端 A → 远端 B,数据不经操作端;转发语义,
    /// dir = B 端目标目录,收件箱哨兵 "inbox" 合法——remote_absolute_form
    /// 平台无关预检,v2-P3 T6)。
    public static func transferStartRemoteCopyParams(
        from: String, paths: [String], to: String, dir: String,
        skip: [String] = [], overwrite: [String] = [], cut: Bool = false,
    ) -> JSONValue {
        .object([
            "kind": .string("remote_copy"),
            "from": .string(from),
            "paths": .array(paths.map { .string($0) }),
            "to": .string(to),
            "dir": .string(dir),
            "skip": .array(skip.map { .string($0) }),
            "overwrite": .array(overwrite.map { .string($0) }),
            "cut": .bool(cut),
        ])
    }

    /// 同机删除 {device, paths, force}(本机删除 device = self device id,
    /// daemon ctrl 对 self 短路 core fs;force = 跳过回收直接永久删)。
    public static func deleteParams(device: String, paths: [String], force: Bool) -> JSONValue {
        .object([
            "device": .string(device),
            "paths": .array(paths.map { .string($0) }),
            "force": .bool(force),
        ])
    }

    /// pause/resume/cancel/wait/reinitiate/clear 共用的 {id} 参数。
    public static func transferIdParams(id: String) -> JSONValue {
        .object(["id": .string(id)])
    }
}
