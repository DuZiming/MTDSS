package com.mtdss.android.ipc

/**
 * IPC 契约面:op 常量 + 类型化请求构造 + 应答/事件解析模型。
 * 字段与 crates/ipc/contract 目录下 *.json 对齐(单一真源,ContractGoldenTest
 * 全量锁定;daemon 侧对照 crates/daemon/src/api.rs)。
 *
 * 请求参数构造**不关心键序**——IpcClient 序列化时统一按字母序规范写出
 * (serde_json 字节口径);应答解析同样与键序无关。
 */
@JvmInline
value class Op(val method: String)

/** RPC 方法名与事件方法名常量(线协议字符串)。 */
object Ops {
    const val HELLO = "hello"
    const val STATUS = "status"
    const val DEVICES = "devices"
    const val LOGIN_REQUEST_CODE = "login.request_code"
    const val LOGIN_VERIFY = "login.verify"
    const val LOGIN_SUBMIT_CODE = "login.submit_code"
    const val LOGIN_STATUS = "login.status"
    const val LOGIN_EVICT = "login.evict"
    const val LOGIN_CANCEL = "login.cancel"
    const val LOGIN_QR_BEGIN = "login.qr_begin"
    const val LOGIN_QR_STATUS = "login.qr_status"
    const val LOGIN_LOGOUT = "login.logout"
    const val APPROVAL_RESPOND = "approval.respond"
    const val QR_CLAIM = "qr.claim"
    const val QR_APPROVE = "qr.approve"
    const val KV_GET = "kv.get"
    const val KV_PUT = "kv.put"
    const val LIST = "list"
    const val STAT = "stat"
    const val TRANSFER_START = "transfer.start"
    const val TRANSFER_LIST = "transfer.list"
    const val TRANSFER_PAUSE = "transfer.pause"
    const val TRANSFER_RESUME = "transfer.resume"
    const val TRANSFER_CANCEL = "transfer.cancel"
    const val TRANSFER_WAIT = "transfer.wait"
    const val TRANSFER_REINITIATE = "transfer.reinitiate"
    const val TRANSFER_CLEAR = "transfer.clear"

    const val EVENT_LOGIN = "event.login"
    const val EVENT_DEVICES = "event.devices"
    const val EVENT_ONLINE = "event.online"
    const val EVENT_TRANSFER = "event.transfer"
    const val EVENT_NOTIFY = "event.notify"
    const val EVENT_APPROVAL = "event.approval"
    const val EVENT_DEVICE_EVICTED = "event.device_evicted"
    const val EVENT_DEVICES_CHANGED = "event.devices_changed"
}

// ── 数据模型(字段名与 golden 契约一一对应)────────────────────────────────

/** 本机设备摘要(hello/status 的 device 投影;未登录 → device 整体为 null)。 */
data class DeviceSummary(val deviceId: String, val name: String?)

/** 设备目录条目(devices / event.devices;name/platform 可空)。 */
data class DirectoryDevice(
    val deviceId: String,
    val name: String?,
    val platform: String?,
    val online: Boolean,
)

data class HelloResult(val version: String, val loggedIn: Boolean, val device: DeviceSummary?)

data class StatusResult(
    val loggedIn: Boolean,
    val online: Boolean,
    val server: String?,
    val recvDir: String?,
    val device: DeviceSummary?,
)

data class DevicesResult(val devices: List<DirectoryDevice>, val stale: Boolean)

data class LoginRequestCodeResult(val code: String)

/**
 * 登录步进(daemon step_json 的 status 判别面;login.verify / login.submit_code /
 * login.status / login.evict / login.qr_status 五 op 共用,字段名与 golden
 * 一字对齐)。ok = 收口(会话已落盘;name 为终端名,续接 op 用槽内候选名,
 * 缺席为 null)。
 */
sealed interface LoginStep {
    data class Ok(val deviceId: String, val name: String?) : LoginStep

    /** 扫码轮询面(等待扫码确认;V3-4 消费,本任务仅契约绑定)。 */
    data class Pending(val expiresAt: Long?) : LoginStep

    /** 审批挂起(受信终端弹窗或 6 位码直输;expiresAt = 到期秒)。 */
    data class Approval(val sessionId: String, val expiresAt: Long?) : LoginStep

    /** 配额超额(踢选屏:消费单次 enroll_token 移出所选设备)。 */
    data class Limit(val enrollToken: String, val devices: List<DirectoryDevice>) : LoginStep

    /** 轮询终态:被拒 / 超时(服务端 TTL 收口)。 */
    data object Denied : LoginStep
    data object Expired : LoginStep
}

/** login.qr_begin 投影(daemon 拼 https://www.mtdss.top/qr?t=<token>,120s TTL;
 * 扫码器 UI 随 V3-4 落,本任务仅契约绑定)。 */
data class QrBeginResult(val qrUrl: String, val token: String, val expiresAt: Long)

/** kv.get 投影(未命中为 typed not_found 错误面,非空 result)。 */
data class KvGetResult(val value: String)

/** kv.put 投影(UPSERT 成功回执)。 */
data class KvPutResult(val ok: Boolean)

/** qr.claim 投影(候选面拍平,确认弹层数据源;daemon 持会话 Bearer 代打)。 */
data class QrClaimResult(
    val candidateName: String,
    val candidatePlatform: String,
    val expiresAt: Long,
)

/** qr.approve 投影(status 集合 ok|denied|expired|limit;limit 为防御面——
 * 配额超额收口在 PC 侧 qr_status 轮询,本 op 恒不投影 limit)。 */
data class QrApproveResult(val status: String)

/** 远端目录条目(list / stat)。children = 目录直接子项数(v2-P5 T5b 增量
 * 可选面:仅目录项携带,文件项/未知缺省 null——proto3 optional 线上语义;
 * 全量计数含 dotfiles,显示过滤归 UI 侧)。 */
data class DirEntry(
    val name: String,
    val path: String,
    val size: Long,
    val mtimeMs: Long,
    val isDir: Boolean,
    val children: Long? = null,
)

data class ListResult(val entries: List<DirEntry>, val truncated: Boolean)

data class StatResult(val entry: DirEntry)

/** fd 桥上传条目元数据(transfer.start 的 upload-bridge 形态,T4 协议:
 * ref = App 侧 SourceResolver 引用;rel = 目标相对路径;size/mtime = 预扫元数据)。 */
data class BridgeItem(val ref: String, val rel: String, val size: Long, val mtime: Long)

data class TransferStartResult(val jobId: String)

/** 传输作业快照(transfer.list 条目;实时进度事件见 [TransferProgress])。 */
data class TransferJob(
    val id: String,
    val kind: String,
    val name: String,
    val state: String,
    val filesDone: Long,
    val filesTotal: Long,
    val bytesDone: Long,
    val bytesTotal: Long,
    val speedBps: Long,
    val skipped: Long,
    val relay: Boolean,
    val src: String?,
    val dst: String?,
    val error: String?,
)

data class TransferListResult(val jobs: List<TransferJob>)

data class TransferWaitResult(
    val state: String,
    val error: String?,
    val filesDone: Long,
    val filesTotal: Long,
    val bytesTotal: Long,
    val skipped: Long,
)

/** event.transfer 的进度参数面(与 TransferJob 同字段族,无 name/src/dst)。 */
data class TransferProgress(
    val id: String,
    val kind: String,
    val state: String,
    val filesDone: Long,
    val filesTotal: Long,
    val bytesDone: Long,
    val bytesTotal: Long,
    val speedBps: Long,
    val skipped: Long,
    val relay: Boolean,
    val error: String?,
)

/** 类型化事件(event.* 八枚;未知方法 → null,消费方跳过保持前向兼容)。 */
sealed interface MtdssEvent {
    data class Login(val loggedIn: Boolean) : MtdssEvent
    data class Online(val online: Boolean) : MtdssEvent
    data class Devices(val devices: List<DirectoryDevice>) : MtdssEvent
    data class Transfer(val progress: TransferProgress) : MtdssEvent
    data class Notify(val title: String, val body: String) : MtdssEvent

    /** 登录审批请求(受信终端弹窗:候选设备名/平台 + 6 位审批码)。 */
    data class Approval(
        val sessionId: String,
        val candidateName: String,
        val candidatePlatform: String?,
        val code: String,
        val expiresAt: Long?,
    ) : MtdssEvent

    /** 本设备已被移除(params 恒空对象;清会话回登录态)。 */
    data object DeviceEvicted : MtdssEvent

    /** 设备表增删通告(daemon 随后自刷快照广播 event.devices)。 */
    data object DevicesChanged : MtdssEvent

    companion object {
        fun parse(frame: EventFrame): MtdssEvent? = when (frame.method) {
            Ops.EVENT_LOGIN -> Login(frame.params.req("logged_in").asBoolean())
            Ops.EVENT_ONLINE -> Online(frame.params.req("online").asBoolean())
            Ops.EVENT_DEVICES -> Devices(frame.params.req("devices").asArray().map { Contract.parseDirectoryDevice(it) })
            Ops.EVENT_TRANSFER -> Transfer(Contract.parseTransferProgress(frame.params))
            Ops.EVENT_NOTIFY -> Notify(
                title = frame.params.req("title").asString(),
                body = frame.params.req("body").asString(),
            )
            Ops.EVENT_APPROVAL -> Approval(
                sessionId = frame.params.req("session_id").asString(),
                candidateName = frame.params.req("candidate_name").asString(),
                candidatePlatform = frame.params.opt("candidate_platform").asStringOrNull(),
                code = frame.params.req("code").asString(),
                expiresAt = frame.params.opt("expires_at").asLongOrNull(),
            )
            Ops.EVENT_DEVICE_EVICTED -> DeviceEvicted
            Ops.EVENT_DEVICES_CHANGED -> DevicesChanged
            else -> null
        }
    }
}

/**
 * 类型化请求参数构造 + 应答解析(纯函数,无 IO)。
 * 每个请求形态一个 builder,应答按 result 值解析(信封由 IpcClient 剥)。
 */
object Contract {

    fun emptyParams(): JVal.Obj = JVal.Obj()

    /** login.request_code {server, email, ca?}(ca = 自签 CA 证书路径,空则键省略)。 */
    fun loginRequestCodeParams(server: String, email: String, ca: String? = null): JVal.Obj {
        val entries = linkedMapOf<String, JVal>(
            "server" to JVal.str(server),
            "email" to JVal.str(email),
        )
        if (ca != null) entries["ca"] = JVal.str(ca)
        return JVal.Obj(entries)
    }

    /** login.verify {code, email, name}。 */
    fun loginVerifyParams(code: String, email: String, name: String): JVal.Obj = JVal.Obj(
        "code" to JVal.str(code),
        "email" to JVal.str(email),
        "name" to JVal.str(name),
    )

    /** login.submit_code {session_id, code}(等待审批面的 6 位码直输)。 */
    fun loginSubmitCodeParams(sessionId: String, code: String): JVal.Obj = JVal.Obj(
        "session_id" to JVal.str(sessionId),
        "code" to JVal.str(code),
    )

    /** login.status {session_id}(审批轮询)。 */
    fun loginStatusParams(sessionId: String): JVal.Obj = JVal.Obj(
        "session_id" to JVal.str(sessionId),
    )

    /** login.evict {enroll_token, device_id}(踢选屏动作)。 */
    fun loginEvictParams(enrollToken: String, deviceId: String): JVal.Obj = JVal.Obj(
        "enroll_token" to JVal.str(enrollToken),
        "device_id" to JVal.str(deviceId),
    )

    /** login.qr_begin {server, name?, ca?}(可省键缺省即省略;扫码器随 V3-4)。 */
    fun loginQrBeginParams(server: String, name: String?, ca: String? = null): JVal.Obj {
        val entries = linkedMapOf<String, JVal>("server" to JVal.str(server))
        if (name != null) entries["name"] = JVal.str(name)
        if (ca != null) entries["ca"] = JVal.str(ca)
        return JVal.Obj(entries)
    }

    /** login.qr_status {token}。 */
    fun loginQrStatusParams(token: String): JVal.Obj = JVal.Obj(
        "token" to JVal.str(token),
    )

    /** approval.respond {session_id, allow}(受信终端弹窗应答)。 */
    fun approvalRespondParams(sessionId: String, allow: Boolean): JVal.Obj = JVal.Obj(
        "session_id" to JVal.str(sessionId),
        "allow" to JVal.bool(allow),
    )

    /** kv.get {key}。 */
    fun kvGetParams(key: String): JVal.Obj = JVal.Obj(
        "key" to JVal.str(key),
    )

    /** kv.put {key, value}。 */
    fun kvPutParams(key: String, value: String): JVal.Obj = JVal.Obj(
        "key" to JVal.str(key),
        "value" to JVal.str(value),
    )

    /** qr.claim {token}(手机侧扫码认领;daemon 持会话 Bearer 代打)。 */
    fun qrClaimParams(token: String): JVal.Obj = JVal.Obj(
        "token" to JVal.str(token),
    )

    /** qr.approve {token, allow}(认领者裁决)。 */
    fun qrApproveParams(token: String, allow: Boolean): JVal.Obj = JVal.Obj(
        "token" to JVal.str(token),
        "allow" to JVal.bool(allow),
    )

    /** list {device, path}(path 空串 = 根卷)。 */
    fun listParams(device: String, path: String): JVal.Obj = JVal.Obj(
        "device" to JVal.str(device),
        "path" to JVal.str(path),
    )

    /** stat {device, path}。 */
    fun statParams(device: String, path: String): JVal.Obj = JVal.Obj(
        "device" to JVal.str(device),
        "path" to JVal.str(path),
    )

    /** transfer.start 路径形态上传 {kind:"upload", files, to, dir, skip, overwrite, cut}。 */
    fun transferStartUploadParams(
        files: List<String>,
        to: String,
        dir: String,
        skip: List<String>,
        overwrite: List<String>,
        cut: Boolean,
    ): JVal.Obj = JVal.Obj(
        "kind" to JVal.str("upload"),
        "files" to JVal.Arr(files.map { JVal.str(it) }),
        "to" to JVal.str(to),
        "dir" to JVal.str(dir),
        "skip" to JVal.Arr(skip.map { JVal.str(it) }),
        "overwrite" to JVal.Arr(overwrite.map { JVal.str(it) }),
        "cut" to JVal.bool(cut),
    )

    /**
     * transfer.start fd 桥形态上传 {kind:"upload", source:"bridge", items, to,
     * dir, skip, overwrite, cut}(T4:App 经 SourceResolver 按需开 fd,懒传递)。
     */
    fun transferStartUploadBridgeParams(
        items: List<BridgeItem>,
        to: String,
        dir: String,
        skip: List<String>,
        overwrite: List<String>,
        cut: Boolean,
    ): JVal.Obj = JVal.Obj(
        "kind" to JVal.str("upload"),
        "source" to JVal.str("bridge"),
        "items" to JVal.Arr(
            items.map { item ->
                JVal.Obj(
                    "ref" to JVal.str(item.ref),
                    "rel" to JVal.str(item.rel),
                    "size" to JVal.num(item.size),
                    "mtime" to JVal.num(item.mtime),
                )
            },
        ),
        "to" to JVal.str(to),
        "dir" to JVal.str(dir),
        "skip" to JVal.Arr(skip.map { JVal.str(it) }),
        "overwrite" to JVal.Arr(overwrite.map { JVal.str(it) }),
        "cut" to JVal.bool(cut),
    )

    /** transfer.start 下载 {kind:"download", device, paths, dir, skip, overwrite, cut}。
     * dir 收件箱哨兵 "inbox"(T6)或绝对路径。 */
    fun transferStartDownloadParams(
        device: String,
        paths: List<String>,
        dir: String,
        skip: List<String>,
        overwrite: List<String>,
        cut: Boolean,
    ): JVal.Obj = JVal.Obj(
        "kind" to JVal.str("download"),
        "device" to JVal.str(device),
        "paths" to JVal.Arr(paths.map { JVal.str(it) }),
        "dir" to JVal.str(dir),
        "skip" to JVal.Arr(skip.map { JVal.str(it) }),
        "overwrite" to JVal.Arr(overwrite.map { JVal.str(it) }),
        "cut" to JVal.bool(cut),
    )

    /** pause/resume/cancel/wait/reinitiate/clear 共用的 {id} 参数。 */
    fun transferIdParams(id: String): JVal.Obj = JVal.Obj("id" to JVal.str(id))

    // ── 应答解析(输入 = 信封剥壳后的 result 值;与键序无关)────────────────

    fun parseHello(v: JVal): HelloResult = HelloResult(
        version = v.req("version").asString(),
        loggedIn = v.req("logged_in").asBoolean(),
        device = parseDeviceSummary(v.opt("device")),
    )

    fun parseStatus(v: JVal): StatusResult = StatusResult(
        loggedIn = v.req("logged_in").asBoolean(),
        online = v.req("online").asBoolean(),
        server = v.opt("server").asStringOrNull(),
        recvDir = v.opt("recv_dir").asStringOrNull(),
        device = parseDeviceSummary(v.opt("device")),
    )

    fun parseDevices(v: JVal): DevicesResult = DevicesResult(
        devices = v.req("devices").asArray().map(::parseDirectoryDevice),
        stale = v.req("stale").asBoolean(),
    )

    fun parseLoginRequestCode(v: JVal): LoginRequestCodeResult =
        LoginRequestCodeResult(code = v.req("code").asString())

    /** 登录步进判别(status 键路由;未知值/字段缺失即契约违规,不猜)。 */
    fun parseLoginStep(v: JVal): LoginStep = when (val status = v.req("status").asString()) {
        "ok" -> LoginStep.Ok(
            deviceId = v.req("device_id").asString(),
            name = v.opt("name").asStringOrNull(),
        )
        "pending" -> LoginStep.Pending(expiresAt = v.opt("expires_at").asLongOrNull())
        "approval" -> LoginStep.Approval(
            sessionId = v.req("session_id").asString(),
            expiresAt = v.opt("expires_at").asLongOrNull(),
        )
        "limit" -> LoginStep.Limit(
            enrollToken = v.req("enroll_token").asString(),
            devices = v.req("devices").asArray().map(::parseDirectoryDevice),
        )
        "denied" -> LoginStep.Denied
        "expired" -> LoginStep.Expired
        else -> throw ContractViolationException("未知登录步进 status: $status")
    }

    fun parseLoginQrBegin(v: JVal): QrBeginResult = QrBeginResult(
        qrUrl = v.req("qr_url").asString(),
        token = v.req("token").asString(),
        expiresAt = v.req("expires_at").asLong(),
    )

    fun parseKvGet(v: JVal): KvGetResult = KvGetResult(value = v.req("value").asString())

    fun parseKvPut(v: JVal): KvPutResult = KvPutResult(ok = v.req("ok").asBoolean())

    fun parseQrClaim(v: JVal): QrClaimResult = QrClaimResult(
        candidateName = v.req("candidate_name").asString(),
        candidatePlatform = v.req("candidate_platform").asString(),
        expiresAt = v.req("expires_at").asLong(),
    )

    fun parseQrApprove(v: JVal): QrApproveResult = QrApproveResult(status = v.req("status").asString())

    fun parseList(v: JVal): ListResult = ListResult(
        entries = v.req("entries").asArray().map(::parseDirEntry),
        truncated = v.req("truncated").asBoolean(),
    )

    fun parseStat(v: JVal): StatResult = StatResult(entry = parseDirEntry(v.req("entry")))

    fun parseTransferStart(v: JVal): TransferStartResult =
        TransferStartResult(jobId = v.req("job_id").asString())

    fun parseTransferList(v: JVal): TransferListResult =
        TransferListResult(jobs = v.req("jobs").asArray().map(::parseTransferJob))

    fun parseTransferWait(v: JVal): TransferWaitResult = TransferWaitResult(
        state = v.req("state").asString(),
        error = v.opt("error").asStringOrNull(),
        filesDone = v.req("files_done").asLong(),
        filesTotal = v.req("files_total").asLong(),
        bytesTotal = v.req("bytes_total").asLong(),
        skipped = v.req("skipped").asLong(),
    )

    // ── 私有共用投影 ──────────────────────────────────────────────────────

    /** hello/status 的 device 投影:null / Null 字面量 → null(未登录面)。 */
    private fun parseDeviceSummary(v: JVal?): DeviceSummary? {
        if (v == null || v is JVal.Null) return null
        return DeviceSummary(
            deviceId = v.req("device_id").asString(),
            name = v.opt("name").asStringOrNull(),
        )
    }

    internal fun parseDirectoryDevice(v: JVal): DirectoryDevice = DirectoryDevice(
        deviceId = v.req("device_id").asString(),
        name = v.opt("name").asStringOrNull(),
        platform = v.opt("platform").asStringOrNull(),
        online = v.req("online").asBoolean(),
    )

    private fun parseDirEntry(v: JVal): DirEntry = DirEntry(
        name = v.req("name").asString(),
        path = v.req("path").asString(),
        size = v.req("size").asLong(),
        mtimeMs = v.req("mtime_ms").asLong(),
        isDir = v.req("is_dir").asBoolean(),
        children = v.opt("children").asLongOrNull(),
    )

    internal fun parseTransferJob(v: JVal): TransferJob = TransferJob(
        id = v.req("id").asString(),
        kind = v.req("kind").asString(),
        name = v.req("name").asString(),
        state = v.req("state").asString(),
        filesDone = v.req("files_done").asLong(),
        filesTotal = v.req("files_total").asLong(),
        bytesDone = v.req("bytes_done").asLong(),
        bytesTotal = v.req("bytes_total").asLong(),
        speedBps = v.req("speed_bps").asLong(),
        skipped = v.req("skipped").asLong(),
        relay = v.req("relay").asBoolean(),
        src = v.opt("src").asStringOrNull(),
        dst = v.opt("dst").asStringOrNull(),
        error = v.opt("error").asStringOrNull(),
    )

    internal fun parseTransferProgress(v: JVal): TransferProgress = TransferProgress(
        id = v.req("id").asString(),
        kind = v.req("kind").asString(),
        state = v.req("state").asString(),
        filesDone = v.req("files_done").asLong(),
        filesTotal = v.req("files_total").asLong(),
        bytesDone = v.req("bytes_done").asLong(),
        bytesTotal = v.req("bytes_total").asLong(),
        speedBps = v.req("speed_bps").asLong(),
        skipped = v.req("skipped").asLong(),
        relay = v.req("relay").asBoolean(),
        error = v.opt("error").asStringOrNull(),
    )
}
