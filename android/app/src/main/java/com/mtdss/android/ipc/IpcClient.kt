package com.mtdss.android.ipc

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong
import kotlin.coroutines.coroutineContext

/**
 * 事件帧(通知帧:无 id,method + params;线上形如
 * `{"jsonrpc":"2.0","method":"event.*","params":…}`,读循环剥掉 jsonrpc 前缀)。
 */
data class EventFrame(val method: String, val params: JVal)

/** 连接态(UI 观察/调试屏直读;自愈重连的可见面)。 */
sealed interface ConnState {
    data object Disconnected : ConnState
    data object Connecting : ConnState
    data object Connected : ConnState
}

/**
 * RPC/传输层错误:code(-32000 = 传输层,与 Rust IpcClient 同码;其余为业务码),
 * kind = 服务端 `error.data.kind` 稳定码(NOT_FOUND/PERMISSION_DENIED/…,可空)。
 */
class RpcException(val code: Long, message: String, val kind: String? = null) : Exception(message)

/** 连接建立失败(传输层,尚未成帧)。 */
class IpcConnectException(message: String, cause: Throwable? = null) : Exception(message, cause)

/** 契约违规:应答帧畸形或 result 形状与类型化解析不符。 */
class ContractViolationException(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * MTDSS daemon 的 JSON-RPC 客户端(NDJSON 线格式,一行一帧;协议事实对照
 * crates/ipc/src/lib.rs):
 * - 请求 `{"jsonrpc":"2.0","id":N,"method":…,"params":…}`(键序 = serde_json
 *   字母序,[buildRequestLine] 逐字节对齐 golden);
 * - 应答 `{"id":N,"result":…}` / `{"id":N,"error":{code,message,data?}}` →
 *   按 id 路由到挂起调用;错误 → [RpcException](kind 取自 data.kind);
 * - 通知帧(无 id)→ [events] 事件流;
 * - 单帧上限 8 MiB,超限即断连(镜像 daemon 的 read_capped_line 口径);
 * - daemon 会断开慢消费者(写队列 64)——由此设计**断连自愈**:
 *   在飞调用立即失败(RpcException -32000),后台按 [backoff] 重连,
 *   **重连成功后补拉 transfer.list** 并发布到 [transferListRefresh]
 *   (事件流是 SharedFlow,重连后续流,订阅方无感)。
 *
 * 生命周期:[scope] 为宿主协程域(应用级或屏级);[close] 关停一切并幂等。
 * 首连失败不阻塞:[connect] 快速返回失败,后台自愈循环继续重试——冷启动
 * 「引擎拉起 → socket 可连」的间隙由此自然覆盖。
 */
class IpcClient(
    private val factory: SocketFactory,
    scope: CoroutineScope,
    private val backoff: (attempt: Int) -> Long = DEFAULT_BACKOFF,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : Closeable {

    /** 实机入口:LocalSocket 连 `{filesDir}/mtdss/daemon.sock`。 */
    constructor(
        socketFile: File,
        scope: CoroutineScope,
        backoff: (attempt: Int) -> Long = DEFAULT_BACKOFF,
    ) : this(LocalSocketFactory(socketFile), scope, backoff)

    // ── 对外面 ─────────────────────────────────────────────────────────────

    private val _events = MutableSharedFlow<EventFrame>(replay = 0, extraBufferCapacity = 256)

    /** 服务端推送事件(客户端生命周期;重连不中断订阅)。慢订阅方溢出时丢新帧。 */
    val events: SharedFlow<EventFrame> = _events

    private val _transferListRefresh =
        MutableSharedFlow<Result<TransferListResult>>(replay = 1, extraBufferCapacity = 16)

    /**
     * 重连后补拉 transfer.list 的结果(replay=1:晚到的订阅方也能拿到最近一次;
     * 首连不触发——补拉是重连语义,首次数据由屏面自行拉取)。
     */
    val transferListRefresh: SharedFlow<Result<TransferListResult>> = _transferListRefresh

    private val _connectionState = MutableStateFlow<ConnState>(ConnState.Disconnected)

    val connectionState: StateFlow<ConnState> = _connectionState

    /** 建立连接;快速失败语义(失败即返,不重试——后台自愈循环接管后续)。 */
    suspend fun connect(): Result<Unit> = try {
        ensureConnected()
        Result.success(Unit)
    } catch (e: IpcConnectException) {
        Result.failure(e)
    }

    /** 原始调用:result 值原样返回;错误转 [RpcException]。 */
    suspend fun call(op: Op, params: JVal.Obj = JVal.Obj()): Result<JVal> = call(op, params) { it }

    /** 类型化调用:成功应答的 result 值经 [parse] 投影(抛出即契约违规)。 */
    suspend fun <T> call(op: Op, params: JVal.Obj, parse: (JVal) -> T): Result<T> {
        val conn = try {
            ensureConnected()
        } catch (e: IpcConnectException) {
            return Result.failure(e)
        }
        val id = nextId.getAndIncrement()
        val deferred = CompletableDeferred<JVal>()
        pending[id] = deferred
        return try {
            try {
                // 整帧写入互斥:并发 call 的帧不交错(镜像 Rust 写半工互斥)
                conn.writeMutex.withLock {
                    withContext(ioDispatcher) {
                        conn.raw.output.write(buildRequestLine(id, op.method, params))
                        conn.raw.output.flush()
                    }
                }
            } catch (e: IOException) {
                // 写失败 = 连接已坏:统一断连清理(在飞失败 + 自愈)
                handleDisconnect(conn)
                throw RpcException(CODE_TRANSPORT, "ipc 写入失败: ${e.message}")
            }
            val v = deferred.await()
            Result.success(parse(v))
        } catch (e: CancellationException) {
            throw e
        } catch (e: RpcException) {
            Result.failure(e)
        } catch (e: Throwable) {
            Result.failure(ContractViolationException("应答解析失败(${op.method}): ${e.message}", e))
        } finally {
            pending.remove(id, deferred)
        }
    }

    // ── 类型化面(spec §4.2 便携列表;T9-T11 各屏的数据源)──────────────────

    suspend fun hello(): Result<HelloResult> =
        call(Op(Ops.HELLO), Contract.emptyParams()) { Contract.parseHello(it) }

    suspend fun status(): Result<StatusResult> =
        call(Op(Ops.STATUS), Contract.emptyParams()) { Contract.parseStatus(it) }

    suspend fun devices(): Result<DevicesResult> =
        call(Op(Ops.DEVICES), Contract.emptyParams()) { Contract.parseDevices(it) }

    suspend fun loginRequestCode(server: String, email: String, ca: String? = null): Result<LoginRequestCodeResult> =
        call(Op(Ops.LOGIN_REQUEST_CODE), Contract.loginRequestCodeParams(server, email, ca)) { Contract.parseLoginRequestCode(it) }

    suspend fun loginVerify(code: String, email: String, name: String): Result<LoginVerifyResult> =
        call(Op(Ops.LOGIN_VERIFY), Contract.loginVerifyParams(code, email, name)) { Contract.parseLoginVerify(it) }

    suspend fun loginLogout(): Result<Unit> =
        call(Op(Ops.LOGIN_LOGOUT), Contract.emptyParams()) { it.asObject(); Unit }

    suspend fun list(device: String, path: String): Result<ListResult> =
        call(Op(Ops.LIST), Contract.listParams(device, path)) { Contract.parseList(it) }

    suspend fun stat(device: String, path: String): Result<StatResult> =
        call(Op(Ops.STAT), Contract.statParams(device, path)) { Contract.parseStat(it) }

    suspend fun transferStartUpload(
        files: List<String>,
        to: String,
        dir: String,
        skip: List<String> = emptyList(),
        overwrite: List<String> = emptyList(),
        cut: Boolean = false,
    ): Result<TransferStartResult> =
        call(Op(Ops.TRANSFER_START), Contract.transferStartUploadParams(files, to, dir, skip, overwrite, cut)) {
            Contract.parseTransferStart(it)
        }

    suspend fun transferStartUploadBridge(
        items: List<BridgeItem>,
        to: String,
        dir: String,
        skip: List<String> = emptyList(),
        overwrite: List<String> = emptyList(),
        cut: Boolean = false,
    ): Result<TransferStartResult> =
        call(Op(Ops.TRANSFER_START), Contract.transferStartUploadBridgeParams(items, to, dir, skip, overwrite, cut)) {
            Contract.parseTransferStart(it)
        }

    suspend fun transferStartDownload(
        device: String,
        paths: List<String>,
        dir: String,
        skip: List<String> = emptyList(),
        overwrite: List<String> = emptyList(),
        cut: Boolean = false,
    ): Result<TransferStartResult> =
        call(Op(Ops.TRANSFER_START), Contract.transferStartDownloadParams(device, paths, dir, skip, overwrite, cut)) {
            Contract.parseTransferStart(it)
        }

    suspend fun transferList(): Result<TransferListResult> =
        call(Op(Ops.TRANSFER_LIST), Contract.emptyParams()) { Contract.parseTransferList(it) }

    suspend fun transferPause(id: String): Result<Unit> =
        call(Op(Ops.TRANSFER_PAUSE), Contract.transferIdParams(id)) { it.asObject(); Unit }

    suspend fun transferResume(id: String): Result<Unit> =
        call(Op(Ops.TRANSFER_RESUME), Contract.transferIdParams(id)) { it.asObject(); Unit }

    suspend fun transferCancel(id: String): Result<Unit> =
        call(Op(Ops.TRANSFER_CANCEL), Contract.transferIdParams(id)) { it.asObject(); Unit }

    suspend fun transferWait(id: String): Result<TransferWaitResult> =
        call(Op(Ops.TRANSFER_WAIT), Contract.transferIdParams(id)) { Contract.parseTransferWait(it) }

    suspend fun transferReinitiate(id: String): Result<Unit> =
        call(Op(Ops.TRANSFER_REINITIATE), Contract.transferIdParams(id)) { it.asObject(); Unit }

    suspend fun transferClear(id: String): Result<Unit> =
        call(Op(Ops.TRANSFER_CLEAR), Contract.transferIdParams(id)) { it.asObject(); Unit }

    /** 关停:断开全部连接、取消读循环/自愈/补拉协程、失败一切在飞调用。幂等。 */
    override fun close() {
        val (conns, heal) = synchronized(lock) { closeLocked() }
        heal?.cancel()
        supervisor.cancel()
        conns.forEach { c -> runCatching { c.raw.close() } }
        drainPending(RpcException(CODE_TRANSPORT, "ipc 客户端已关闭"))
    }

    companion object {
        /** 传输层错误码(与 Rust IpcClient 同码;-32000 = 传输/断连/超限/写入失败)。 */
        const val CODE_TRANSPORT: Long = -32000L

        /** 单帧字节上限(8 MiB,镜像 daemon 的 MAX_FRAME_BYTES)。 */
        internal const val MAX_FRAME_BYTES: Int = 8 * 1024 * 1024

        /** 默认退避:500ms 起、倍增、封顶 10s。 */
        val DEFAULT_BACKOFF: (attempt: Int) -> Long = { attempt ->
            minOf(500L shl (attempt - 1).coerceAtMost(5), 10_000L)
        }
    }

    // ── 内部:状态与连接管理 ────────────────────────────────────────────────

    private val supervisor = SupervisorJob(scope.coroutineContext[Job])

    /** 内部协程域(读循环/自愈/补拉);[close] 取消全部。 */
    private val clientScope = CoroutineScope(supervisor + ioDispatcher)

    /** 状态互斥体下的可变面:closed / current / healJob / connRegistry / 状态流。 */
    private val lock = Any()

    @Volatile
    private var closed = false

    private var current: Conn? = null

    @Volatile
    private var healJob: Job? = null

    @Volatile
    private var lastConnectError: IOException? = null

    private val connRegistry = java.util.concurrent.CopyOnWriteArrayList<Conn>()

    /** 等待响应表:id → 应答(应答到达或断连时完成)。 */
    private val pending = ConcurrentHashMap<Long, CompletableDeferred<JVal>>()

    private val nextId = AtomicLong(1) // 首调 id=1,与 golden 逐字节锚对齐

    /** 连接建立串行化:并发 call/自愈同时要连时,只开一条新连接。 */
    private val connectMutex = Mutex()

    private fun closeLocked(): Pair<List<Conn>, Job?> {
        if (closed) return emptyList<Conn>() to null
        closed = true
        current = null
        val heal = healJob
        healJob = null
        val conns = connRegistry.toList()
        connRegistry.clear()
        _connectionState.value = ConnState.Disconnected
        return conns to heal
    }

    /** 已连即复用;未连则尝试一条新连接(失败 → null,不抛)。 */
    private suspend fun ensureConnected(): Conn {
        synchronized(lock) {
            current?.let { return it }
            if (closed) throw IpcConnectException("ipc 客户端已关闭")
        }
        val conn = tryConnect()
        if (conn != null) return conn
        synchronized(lock) {
            if (closed) throw IpcConnectException("ipc 客户端已关闭")
        }
        scheduleHealIfNeeded() // 首连失败与断连同径:后台自愈接管
        throw IpcConnectException("ipc 连接失败: ${lastConnectError?.message ?: "未知原因"}")
    }

    /**
     * 开一条连接(或复用他者刚开的)。IO 失败/已关 → null(不抛);
     * 成功 → 安装为当前连接并启动读循环。
     */
    private suspend fun tryConnect(): Conn? = connectMutex.withLock {
        synchronized(lock) {
            current?.let { return@withLock it }
            if (closed) return@withLock null
            _connectionState.value = ConnState.Connecting
        }
        val raw = try {
            withContext(ioDispatcher) { factory.connect() }
        } catch (e: IOException) {
            lastConnectError = e
            synchronized(lock) {
                if (!closed && current == null) _connectionState.value = ConnState.Disconnected
            }
            return@withLock null
        }
        synchronized(lock) {
            when {
                closed -> {
                    runCatching { raw.close() }
                    null
                }
                current != null -> {
                    // 他者已连(自愈与并发 call 竞态):弃用自己的新连接
                    runCatching { raw.close() }
                    current
                }
                else -> {
                    val conn = Conn(raw)
                    connRegistry.add(conn)
                    clientScope.launch { readLoop(conn) }
                    current = conn
                    _connectionState.value = ConnState.Connected
                    conn
                }
            }
        }
    }

    /** 自愈循环:退避重试至连接建立(或 close)。finally 兜底再武装——
     *  循环退出瞬间连接又断的竞态窗口由此闭合。 */
    private suspend fun healLoop() {
        try {
            var attempt = 1
            while (true) {
                synchronized(lock) {
                    if (closed) return
                    if (current == null) _connectionState.value = ConnState.Connecting
                }
                delay(backoff(attempt))
                synchronized(lock) {
                    if (closed || current != null) return
                }
                if (tryConnect() != null) {
                    refreshAfterReconnect()
                    return
                }
                attempt++
            }
        } finally {
            synchronized(lock) {
                if (healJob === coroutineContext[Job]) healJob = null
                if (!closed && current == null && healJob?.isActive != true) {
                    healJob = clientScope.launch { healLoop() }
                }
            }
        }
    }

    private fun scheduleHealIfNeeded() {
        synchronized(lock) {
            if (!closed && current == null && healJob?.isActive != true) {
                healJob = clientScope.launch { healLoop() }
            }
        }
    }

    /** 重连成功后的补拉:fire-and-forget——若在自愈循环体内同步等待,补拉挂死
     *  会占住自愈槽位(连接再断即无人重连),故独立协程发车;断连即快速失败,
     *  close 即随域取消。 */
    private fun refreshAfterReconnect() {
        clientScope.launch {
            val r = call(Op(Ops.TRANSFER_LIST), Contract.emptyParams()) { Contract.parseTransferList(it) }
            _transferListRefresh.tryEmit(r)
        }
    }

    /** 连接收尾:清除当前连接、失败一切在飞调用、按需排自愈。幂等(按连接身份)。 */
    private fun handleDisconnect(conn: Conn) {
        synchronized(lock) {
            if (closed || current !== conn) return // 陈旧连接或已关停:无事可做
            current = null
            connRegistry.remove(conn)
            _connectionState.value = ConnState.Disconnected
        }
        drainPending(RpcException(CODE_TRANSPORT, "ipc 连接已断开"))
        runCatching { conn.raw.close() }
        // 收尾二次排空:call 的「注册 pending → 写帧」与断连收尾竞态时,迟注册
        // 的在飞调用须在流关闭后兜底失败(否则其 deferred 无人完成即悬挂)——
        // CHM 的按键 remove 线性化,close 之后必能见到 close 之前的注册。
        drainPending(RpcException(CODE_TRANSPORT, "ipc 连接已断开"))
        scheduleHealIfNeeded()
    }

    private fun drainPending(error: RpcException) {
        for (entry in pending.entries) {
            if (pending.remove(entry.key, entry.value)) entry.value.completeExceptionally(error)
        }
    }

    // ── 内部:读循环与帧解析 ────────────────────────────────────────────────

    private suspend fun readLoop(conn: Conn) {
        try {
            val reader = CappedLineReader(conn.raw.input, MAX_FRAME_BYTES)
            while (true) {
                val line = reader.readLine() ?: break // EOF(行中截断帧同丢弃,镜像 Rust)
                val frame = try {
                    JVal.parse(String(line, Charsets.UTF_8))
                } catch (_: IllegalArgumentException) {
                    continue // 无法解析的帧直接丢弃(镜像 Rust 读半工口径)
                }
                val obj = frame as? JVal.Obj ?: continue
                val id = (obj["id"] as? JVal.Num)?.raw?.toLongOrNull()
                if (id != null) {
                    // 响应帧:按 id 路由;无人认领(过期/重复)即忽略
                    val deferred = pending.remove(id) ?: continue
                    parseResponseEnvelope(obj).fold(
                        { deferred.complete(it) },
                        { deferred.completeExceptionally(it) },
                    )
                } else {
                    // 通知帧(无 id):事件
                    val method = (obj["method"] as? JVal.Str)?.v ?: continue
                    val params = obj["params"] ?: continue // 缺 params 的畸形事件帧丢弃(镜像 Rust)
                    _events.tryEmit(EventFrame(method, params))
                }
            }
        } catch (_: FrameTooLargeException) {
            // 超限帧:按协议违约断连(与 daemon 对端行为对称)
        } catch (_: IOException) {
            // 连接结束:统一走 finally 断连清理
        } finally {
            handleDisconnect(conn)
        }
    }

    /** 应答信封:`{"id":N,"result":…}` 或 `{"id":N,"error":…}` → 结果/异常。 */
    private fun parseResponseEnvelope(obj: JVal.Obj): Result<JVal> {
        val err = obj["error"]
        if (err != null) {
            val e = err as? JVal.Obj
                ?: return Result.failure(RpcException(CODE_TRANSPORT, "畸形错误帧: error 非对象"))
            val code = (e["code"] as? JVal.Num)?.raw?.toLongOrNull()
                ?: return Result.failure(RpcException(CODE_TRANSPORT, "畸形错误帧: code 缺失或非整数"))
            val message = (e["message"] as? JVal.Str)?.v ?: ""
            val kind = ((e["data"] as? JVal.Obj)?.get("kind") as? JVal.Str)?.v
            return Result.failure(RpcException(code, message, kind))
        }
        return Result.success(obj["result"] ?: JVal.Null)
    }
}

/** 一条活动连接:原始双工流 + 整帧写互斥(并发 call 串行化)。 */
private class Conn(val raw: SocketConnection) {
    val writeMutex = Mutex()
}

/**
 * 带上限的行读取(镜像 mtdss_ipc::read_capped_line):读至 '\n'(不含),
 * 累计长度超 cap 即抛 [FrameTooLargeException](防异常对端撑爆内存);
 * EOF → null(残余无换行的截断帧同样按关闭处理)。
 */
internal class CappedLineReader(private val input: InputStream, private val cap: Int) {
    private val buf = ByteArray(64 * 1024)
    private var pos = 0
    private var end = 0

    fun readLine(): ByteArray? {
        val out = ByteArrayOutputStream(128)
        while (true) {
            if (pos >= end) {
                end = input.read(buf)
                pos = 0
                if (end < 0) return null
            }
            var i = pos
            while (i < end && buf[i] != '\n'.code.toByte()) i++
            if (i < end) { // 找到 '\n'
                if (out.size() + (i - pos) > cap) throw FrameTooLargeException()
                out.write(buf, pos, i - pos)
                pos = i + 1
                return out.toByteArray()
            }
            if (out.size() + (end - pos) > cap) throw FrameTooLargeException()
            out.write(buf, pos, end - pos)
            pos = end
        }
    }
}

internal class FrameTooLargeException : IOException("frame too large")

/**
 * 请求帧构造:`{"jsonrpc":"2.0","id":N,"method":…,"params":…}` + '\n'。
 * 键序 = 规范化字母序,与 crates/ipc/contract 的 *.req.json 逐字节一致
 * (serde_json 默认 BTreeMap 键序;ContractGoldenTest 锁定)。
 */
internal fun buildRequestLine(id: Long, method: String, params: JVal.Obj): ByteArray {
    val frame = JVal.Obj(
        "jsonrpc" to JVal.str("2.0"),
        "id" to JVal.num(id),
        "method" to JVal.str(method),
        "params" to params,
    )
    return (frame.toCanonicalJson() + "\n").toByteArray(Charsets.UTF_8)
}
