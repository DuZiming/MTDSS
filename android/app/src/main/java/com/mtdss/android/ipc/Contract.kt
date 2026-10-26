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
    const val LOGIN_LOGOUT = "login.logout"
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

data class LoginVerifyResult(val deviceId: String, val name: String)

/** 远端目录条目(list / stat)。 */
data class DirEntry(
    val name: String,
    val path: String,
    val size: Long,
    val mtimeMs: Long,
    val isDir: Boolean,
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

/** 类型化事件(event.* 五枚;未知方法 → null,消费方跳过保持前向兼容)。 */
sealed interface MtdssEvent {
    data class Login(val loggedIn: Boolean) : MtdssEvent
    data class Online(val online: Boolean) : MtdssEvent
    data class Devices(val devices: List<DirectoryDevice>) : MtdssEvent
    data class Transfer(val progress: TransferProgress) : MtdssEvent
    data class Notify(val title: String, val body: String) : MtdssEvent

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

    fun parseLoginVerify(v: JVal): LoginVerifyResult = LoginVerifyResult(
        deviceId = v.req("device_id").asString(),
        name = v.req("name").asString(),
    )

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
