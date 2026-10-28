package com.mtdss.android.fdpass

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import com.mtdss.android.ipc.ContractViolationException
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asObject
import java.io.Closeable
import java.io.File
import java.io.FileDescriptor
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap

/**
 * fdpass 通道(App 侧;`{filesDir}/mtdss/fds.sock`,协议对照 crates/ipc/src/fdpass.rs)。
 *
 * 单连接 + 单读循环的分复用:
 * - **发送**:整帧序列化写出(帧头+payload **单次 write**,fd 随帧交付;
 *   失败路径解除 fd 武装防错挂后续帧);
 * - **读循环**:0x81 ack 按 request_id 路由到挂起的应答等待;应答一律
 *   handler 内即时回(P4a:0x03 事后回程面已随收件 fd 桥退役),不假设
 *   应答序;迟到的 ack(已超时摘除)静默忽略;
 * - **读取按帧边界切分**(每帧恰读帧头 5 字节 + payload len 字节):SCM_RIGHTS
 *   的 fd 随携它的 sendmsg 首数据字节交付,帧边界对齐读取保证 fd 归属当前帧
 *   无歧义(与 fdpass.rs read_frame 的同一不变式);
 * - **协议异常**(未知 kind / 帧随附 fd 数超限 / payload 非法 / 帧内 EOF)一律
 *   断连,镜像 daemon 侧防御收口;ack 本不该带 fd——单个 fd 防御关闭
 *   后容忍继续,超量即断连并逐个关闭(不泄漏)。
 *
 * 连接生命周期:按需连接(首次发送时),断连不自动重连——下一次发送重建。
 * 发送由事件驱动(fs.publish/fs.query_dest/fs.request_source 只可能来自活着
 * 的 daemon);无事件即无发送,无需常驻自愈(与 IpcClient 的「daemon 套接字
 * 冷启动窗口」语义不同:这里的事件到达本身即是 daemon 就位的证明)。
 *
 * **通道实例纪律(P4a 改二)**:daemon 侧 SourceResolver 与 DestQueries 的
 * request_id 各自独立自增(两计数器都从 0 起),ack 帧不携 kind——0x01 与
 * 查询通道(0x04/0x05)**必须各用一条 [FdPassChannel] 实例**(各自连接与
 * 等待表,id 撞号天然不可能跨实例)。查询通道内部 0x04 与 0x05 共用同一
 * resolver 与**统一 id 计数器**(spec §6.5)——App 侧单通道单表,同号撞不出
 * 错路(P3 时代 0x02/0x04 分计数器共实例的串扰面已随 0x02 退役根治)。
 */
class FdPassChannel(
    private val factory: FdSocketFactory,
    scope: CoroutineScope,
    private val ackTimeoutMs: Long = 5_000L,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : Closeable {

    /** 实机入口:LocalSocket 连 `{filesDir}/mtdss/fds.sock`。 */
    constructor(socketFile: File, scope: CoroutineScope) : this(LocalFdSocketFactory(socketFile), scope)

    private val supervisor = SupervisorJob(scope.coroutineContext[Job])

    /** 内部协程域(读循环);[close] 取消全部。 */
    private val channelScope = CoroutineScope(supervisor + ioDispatcher)

    /** 状态互斥体下的可变面:closed / current。 */
    private val lock = Any()

    @Volatile
    private var closed = false

    private var current: Conn? = null

    /** 连接建立串行化:并发 send 同时要连时只开一条新连接。 */
    private val connectMutex = Mutex()

    /** 整帧写互斥(setOutboundFds + write 必须原子成对)。 */
    private val writeMutex = Mutex()

    /** 应答等待表:requestId → ack(应答到达或断连时完成)。单通道单表——
     *  表内 id 由对端单 resolver 单计数器保证全序唯一(查询通道 0x04/0x05
     *  同源;源通道 0x01 独立实例)。 */
    private val pendingAcks = ConcurrentHashMap<Long, CompletableDeferred<FdAck>>()

    /**
     * 应答 fs.publish(P4a 发布协议):发 0x05 publish_result(**无 fd**)
     * 并等 0x81 ack。ok:false 时 [error] 携带 App 发布失败文案(daemon 透传
     * 进「保存失败(N 项): …」终因)。
     *
     * 失败/超时无需补偿——0x05 帧多半已在途(daemon 收到即落定);丢帧时
     * daemon 侧 30s 发布等待超时自会「保存失败」收口(暂存保留,§4)。
     */
    suspend fun sendPublishResult(requestId: Long, ok: Boolean, error: String? = null): Result<FdAck> =
        sendFdFrame(FdWire.KIND_PUBLISH_RESULT, requestId, FdWire.publishResultPayload(requestId, ok, error), null)

    /**
     * 应答 fs.query_dest(F7 在场查询):发 0x04 dest_query(**无 fd**)
     * 并等 0x81 ack。存在性由调用方(InboxProvider 查 MediaStore 后)给出;
     * 失败/超时无需补偿——daemon 侧等待超时即保守按「不在场」重传
     * (失败方向铁律:宁可重传,不可假完成)。
     */
    suspend fun sendDestQuery(requestId: Long, exists: Boolean, size: Long): Result<FdAck> =
        sendFdFrame(FdWire.KIND_DEST_QUERY, requestId, FdWire.destQueryPayload(requestId, exists, size), null)

    /**
     * 应答 fs.request_source(T10 SourceProvider):发 0x01 source_fd 并等
     * 0x81 ack。fd 非 null = 交付源(只读);null = 拒开(授权被撤/源消失
     * 等,[error] 随 payload,daemon 词面收口「App 拒开源(…)」后任务
     * failed「源不可用」)。
     *
     * **通道实例纪律**:见类注释「通道实例纪律(P4a 改二)」——0x01 与
     * 查询通道必须分实例,AppGraph 按此装配(fdpassQuery = 查询通道
     * 0x04/0x05,发送源 0x01 直构于 SourceProvider)。
     */
    suspend fun sendSourceFd(requestId: Long, fd: FileDescriptor?, error: String? = null): Result<FdAck> {
        val payload =
            if (fd != null) FdWire.sourceFdPayload(requestId)
            else FdWire.sourceFdRefusePayload(requestId, error ?: "App 拒开源")
        return sendFdFrame(FdWire.KIND_SOURCE_FD, requestId, payload, fd)
    }

    /** 各 kind 共用的发送本体(帧 + 可选 fd + ack 等待)。 */
    private suspend fun sendFdFrame(kind: Int, requestId: Long, payload: JVal.Obj, fd: FileDescriptor?): Result<FdAck> {
        val conn = try {
            ensureConnected()
        } catch (e: IOException) {
            return Result.failure(e)
        }
        val frame = FdWire.encodeFrame(kind, payload)
        val deferred = CompletableDeferred<FdAck>()
        pendingAcks[requestId] = deferred
        return try {
            try {
                writeMutex.withLock {
                    withContext(ioDispatcher) {
                        // fd 随帧交付:武装后单次 write 整帧;失败即解除武装
                        // (防 fd 错挂后续帧)并按断连收口
                        conn.raw.setOutboundFds(fd?.let { arrayOf(it) })
                        try {
                            conn.raw.output.write(frame)
                            conn.raw.output.flush()
                        } catch (e: IOException) {
                            conn.raw.setOutboundFds(null)
                            throw e
                        }
                    }
                }
            } catch (e: IOException) {
                handleDisconnect(conn)
                return Result.failure(e)
            }
            val ack = try {
                withTimeoutOrNull(ackTimeoutMs) { deferred.await() }
            } catch (e: IOException) {
                // 断连/关停:在飞应答等待被排空失败 → 统一收进 Result
                return Result.failure(e)
            } ?: return Result.failure(FdPassTimeoutException(ackTimeoutMs))
            Result.success(ack)
        } finally {
            pendingAcks.remove(requestId, deferred)
        }
    }

    /** 关停:断开连接、取消读循环、失败一切在飞应答等待。幂等。 */
    override fun close() {
        val conn: Conn?
        synchronized(lock) {
            if (closed) return
            closed = true
            conn = current
            current = null
        }
        supervisor.cancel()
        runCatching { conn?.raw?.close() }
        drainPendingAcks(IOException("fdpass 通道已关闭"))
    }

    // ── 内部:连接管理 ─────────────────────────────────────────────────────

    private class Conn(val raw: FdSocketConnection)

    /** 已连即复用;未连则开一条新连接(IOException 上抛,不重试)。 */
    private suspend fun ensureConnected(): Conn {
        synchronized(lock) {
            current?.let { return it }
            if (closed) throw IOException("fdpass 通道已关闭")
        }
        return connectMutex.withLock {
            synchronized(lock) {
                current?.let { return@withLock it }
                if (closed) throw IOException("fdpass 通道已关闭")
            }
            val raw = withContext(ioDispatcher) { factory.connect() }
            synchronized(lock) {
                when {
                    closed -> {
                        runCatching { raw.close() }
                        throw IOException("fdpass 通道已关闭")
                    }

                    current != null -> {
                        // 他者已连(竞态):弃用自己的新连接
                        runCatching { raw.close() }
                        current!!
                    }

                    else -> {
                        val conn = Conn(raw)
                        current = conn
                        channelScope.launch { readLoop(conn) }
                        conn
                    }
                }
            }
        }
    }

    /** 连接收尾:清当前连接、失败一切在飞应答等待。幂等(按连接身份)。 */
    private fun handleDisconnect(conn: Conn) {
        synchronized(lock) {
            if (closed || current !== conn) return
            current = null
        }
        runCatching { conn.raw.close() }
        // 二次排空:与 send 的「注册 pending → 写帧」竞态时,迟注册的等待
        // 在流关闭后兜底失败(杜悬挂;CHM 按键 remove 线性化保证可见性)
        drainPendingAcks(IOException("fdpass 连接已断开"))
        drainPendingAcks(IOException("fdpass 连接已断开"))
    }

    private fun drainPendingAcks(error: IOException) {
        for (entry in pendingAcks.entries) {
            if (pendingAcks.remove(entry.key, entry.value)) entry.value.completeExceptionally(error)
        }
    }

    // ── 内部:读循环与帧解析 ───────────────────────────────────────────────

    private class RawFrame(val kind: Int, val payload: JVal.Obj)

    private suspend fun readLoop(conn: Conn) {
        try {
            while (true) {
                val frame = readFrame(conn.raw) ?: break // 帧边界上的干净 EOF
                when (frame.kind) {
                    FdWire.KIND_ACK -> {
                        val ack = FdWire.parseAck(frame.payload)
                        pendingAcks.remove(ack.requestId)?.complete(ack)
                    }

                    else -> throw FdWireException("未知或暂不支持的 fdpass 消息 kind 0x%02x".format(frame.kind))
                }
            }
        } catch (_: FdWireException) {
            // 协议异常:断连收口(镜像 daemon 防御行为)
        } catch (_: IOException) {
            // 连接结束:统一走 finally 断连清理
        } finally {
            handleDisconnect(conn)
        }
    }

    /** 读一帧:`kind u8 | len u32 BE | JSON` + 可选随帧 fd;帧边界干净 EOF → null。 */
    private fun readFrame(raw: FdSocketConnection): RawFrame? {
        val fds = mutableListOf<FileDescriptor>()
        val header = readExact(raw, FdWire.HEADER_LEN, fds) ?: return null
        val (kind, len) = FdWire.parseHeader(header)
        val payloadBytes = readExact(raw, len, fds)
            ?: throw FdWireException("fdpass 对端中途关闭(payload 截断)")
        if (fds.size > 1) {
            // 协议允许 0/1:超量即协议异常断连,收到的 fd 逐个关闭(不泄漏)
            fds.forEach { runCatching { raw.closeInboundFd(it) } }
            throw FdWireException("fdpass 帧随附 fd 数超限(协议允许 0/1,实收 ${fds.size})")
        }
        if (fds.size == 1) {
            // ack 契约上不带 fd:防御关闭后容忍继续(单帧单 fd 未破坏帧界)
            runCatching { raw.closeInboundFd(fds[0]) }
        }
        val payload = try {
            JVal.parse(String(payloadBytes, Charsets.UTF_8)).asObject()
        } catch (e: IllegalArgumentException) {
            throw FdWireException("fdpass payload 非法 JSON: ${e.message}")
        } catch (e: ContractViolationException) {
            throw FdWireException("fdpass payload 非对象: ${e.message}")
        }
        return RawFrame(kind, payload)
    }

    /**
     * 恰读 n 字节(帧边界对齐:多读会偷走下一帧的字节与 fd 归属);每次底层
     * read 后收割随帧 fd 累积到当前帧。首字节前 EOF → null(干净关闭);
     * 帧内 EOF → 协议异常。
     */
    private fun readExact(raw: FdSocketConnection, n: Int, fdsOut: MutableList<FileDescriptor>): ByteArray? {
        if (n == 0) return ByteArray(0)
        val buf = ByteArray(n)
        var off = 0
        while (off < n) {
            val r = raw.input.read(buf, off, n - off)
            if (r < 0) {
                if (off == 0) return null
                throw FdWireException("fdpass 对端中途关闭(帧内截断)")
            }
            off += r
            fdsOut += raw.harvestInboundFds()
        }
        return buf
    }
}

/** ack 等待超时(应答可能在途但未及;调用方按「结果未知」处理)。 */
class FdPassTimeoutException(ms: Long) : IOException("fdpass ack 超时(${ms}ms)")
