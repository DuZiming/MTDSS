package com.mtdss.android.serve

import android.util.Log
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.Ops
import com.mtdss.android.ipc.asLongOrNull
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import com.mtdss.android.send.OpenedSource
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.SafFace
import com.mtdss.android.transcode.AndroidRenditionTranscoder
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 段就绪回告缝(管线 → 0x0C):产段即告(daemon 按 (会话, 段序) 记账);
 * 仅此一面——管线失败由响应器捕获统一回告,取消面静默。阻塞面
 * (管线线程直调,实现方 launch 转发)。fun interface(JVM 单测 SAM 面)。
 */
fun interface SegmentNotifier {
    /** 段文件落盘完毕(index 0 = init 段,T12 段序约定)。 */
    fun onSegment(index: Int, file: File)
}

/**
 * 转码管线工厂缝(JVM 注入伪管线测协议;实机 = [AndroidRenditionTranscoder]
 * ——MediaCodec 解→编无 JVM 档,解码正确性归实机冒烟)。实现契约:
 * - 阻塞面,调方在 IO 线程 await 全程;终局(EOF/失败/取消)必释放
 *   [source](fd 副本所有权随入参交出);
 * - EOF = 正常返回;失败 = 抛异常(响应器统一转 0x0C ok:false 回告);
 * - [isCancelled] 逐循环轮询(取消失能后 ≤ 单次 dequeue 超时内退出);
 * - 段文件落 [sessionDir] 后经 [notifier] 回告;index 0 = init 段。
 */
fun interface TranscoderFactory {
    fun transcode(
        source: OpenedSource,
        displayName: String,
        startAtMs: Long,
        sessionDir: File,
        notifier: SegmentNotifier,
        isCancelled: () -> Boolean,
    )
}

/**
 * 转码桥应答器(V4-1 T13 预览波;daemon 侧 jobs.rs RenditionBridge /
 * fdpass 0x0C 契约为真源):消费 daemon 的 `fs.rendition_open` /
 * `fs.rendition_cancel` 事件——源解析([BridgeSources] 共享单点)→ 会话
 * 目录 `{home}/rendition-serve/{session}/` → **回执先行**(daemon open
 * 等待窗只盖源解析;真管线启动在回执之后异步,慢启动不烧等待窗)→
 * 管线逐段产出落盘 + 0x0C `{index, ok, path, request_id}` 段回告(daemon
 * 按需读已产段,T12「读已产段」语义);失败 = 0x0C `{error, ok:false}`
 * (daemon 落 unsupported → 查看端回退原流)。
 *
 * - **会话键**:session = open 的 request_id(daemon 单调源;cancel/段回告
 *   按它路由;本地目录名即会话键,daemon 读面按会话目录前缀校验 fail-closed)。
 * - **start_at**:起点毫秒透传工厂(seek = 「起点段」重开会话的原料;
 *   就近关键帧落地在管线内,MediaExtractor SEEK_TO_PREVIOUS_SYNC)。
 * - **cancel**:置取消旗 → 删段目录 → 回 `{ok, request_id}`;未知 session
 *   幂等照成(daemon cancel 同语义;残件自愈面)。管线在下个轮询点退出,
 *   写盘竞窗的孤儿件由挂载清扫兜底。
 * - **生成件生命周期**:段文件归 App 管——cancel 即清;管线失败**不删**
 *   (已产段 daemon 照读可播,失败标记只对未产段生效);挂载清扫整目录清
 *   (daemon 只读不删,thumb-serve 三面同构)。
 * - **生命周期**:AppGraph 构造即订阅(SeekResponder 同纪律);畸形事件帧
 *   可拒答的拒答、无路由键的静默丢弃;应答帧 ack ok:false = 请求已超时/
 *   未知,记日志即毕。
 */
class RenditionResponder(
    private val saf: SafFace,
    private val inboxGrant: InboxGrantLookup,
    private val roots: RootLookup,
    private val photos: PhotoLibraryFace,
    /** 段文件目录锚 = {engine_home}/rendition-serve(MtdssEngine
     *  .renditionServeDir;daemon 侧 rendition::rendition_serve_dir 同锚,
     *  两侧不得漂移)。 */
    private val renditionDir: File,
    private val channel: FdPassChannel,
    ipc: IpcClient,
    private val scope: CoroutineScope,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val factory: TranscoderFactory = AndroidRenditionTranscoder,
) {
    private val subscribed = CompletableDeferred<Unit>()

    /** 在册会话(session → 取消旗;cancel/终局摘表)。 */
    private val sessions = ConcurrentHashMap<Long, AtomicBoolean>()

    companion object {
        /** 本桥消费的事件集(订阅过滤)。 */
        private val RENDITION_EVENTS = setOf(Ops.EVENT_RENDITION_OPEN, Ops.EVENT_RENDITION_CANCEL)

        private const val TAG = "RenditionResponder"
    }

    init {
        scope.launch {
            // 启动清扫(App 侧残件自愈;daemon 只读不删的兜底面)
            runCatching { sweep() }.onFailure { Log.w(TAG, "rendition-serve 清扫失败: ${it.message}") }
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    if (frame.method in RENDITION_EVENTS) {
                        scope.launch(ioDispatcher) {
                            runCatching { handle(frame) }
                                .onFailure { Log.w(TAG, "rendition 事件处置异常: ${it.message}") }
                        }
                    }
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    /** 单事件处置(测试直驱面;阻塞源解析/管线面在调用方线程)。 */
    suspend fun handle(frame: EventFrame) {
        when (frame.method) {
            Ops.EVENT_RENDITION_OPEN -> handleOpen(frame)
            Ops.EVENT_RENDITION_CANCEL -> handleCancel(frame)
        }
    }

    // ── open(fs.rendition_open)────────────────────────────────────────────

    /** 会话在册项(取消旗;管线闭包捕获直持,cancel 后表内摘除仍生效)。 */
    private class SessionJob(val cancelled: AtomicBoolean = AtomicBoolean(false))

    private suspend fun handleOpen(frame: EventFrame) {
        val requestId = requestId(frame) ?: return // 无路由键静默丢弃
        val path = frame.params.opt("path")?.asStringOrNull()
            ?: return refuse(requestId, "事件缺 path")
        val startAtMs = frame.params.opt("start_at")?.asLongOrNull()
            ?: return refuse(requestId, "事件缺 start_at")
        // 源解析(共享单点):失败 = 拒答(daemon 落 unsupported → 查看端
        // 回退原流+海报;词面仅日志消费)
        val src = BridgeSources.open(path, "转码", saf, inboxGrant, roots, photos).fold(
            onSuccess = { it },
            onFailure = { e -> return refuse(requestId, e.message ?: "App 无法开启转码会话") },
        )
        // 会话目录(目录名 = 会话键;daemon 读面按此目录前缀校验)
        val sessionDir = File(renditionDir, requestId.toString())
        val dirOk = try {
            sessionDir.isDirectory || sessionDir.mkdirs()
        } catch (e: Exception) {
            false
        }
        if (!dirOk) {
            src.opened.release()
            return refuse(requestId, "会话目录创建失败: $sessionDir")
        }
        // 回执先行(管线受理;真管线启动在回执后异步)
        val job = SessionJob()
        sessions[requestId] = job.cancelled
        val acked = channel.sendRendition(requestId, FdWire.renditionAckPayload(requestId))
        if (acked.getOrNull()?.ok != true) {
            // daemon 未受理(已超时/帧丢失):不开管线(白烧电),即收
            sessions.remove(requestId, job.cancelled)
            src.opened.release()
            runCatching { sessionDir.deleteRecursively() }
            Log.w(TAG, "0x0C 回执未被受理 id=$requestId,放弃开启")
            return
        }
        val notifier = SegmentNotifier { index, file ->
            if (job.cancelled.get()) return@SegmentNotifier
            scope.launch {
                channel.sendRendition(
                    requestId,
                    FdWire.renditionSegmentPayload(requestId, index, file.absolutePath),
                ).onFailure {
                    Log.w(TAG, "0x0C 段回告发送失败 id=$requestId index=$index: ${it.message}")
                }
            }
        }
        scope.launch(ioDispatcher) {
            var failure: String? = null
            try {
                factory.transcode(src.opened, src.displayName, startAtMs, sessionDir, notifier) {
                    job.cancelled.get()
                }
            } catch (e: Exception) {
                failure = e.message ?: "转码管线异常"
            } finally {
                sessions.remove(requestId, job.cancelled)
                // fd 所有权随入参交出工厂(契约);此处兜底 release(幂等面)
                src.opened.release()
                if (failure != null && !job.cancelled.get()) {
                    Log.w(TAG, "转码管线失败 id=$requestId: $failure")
                    scope.launch {
                        channel.sendRendition(requestId, FdWire.refusePayload(requestId, "转码管线失败: $failure"))
                            .onFailure { Log.w(TAG, "0x0C 失败回告发送失败 id=$requestId: ${it.message}") }
                    }
                } else if (failure == null && !job.cancelled.get()) {
                    // field 修(0x0C eof 形):管线自然完毕通知——引擎记会话
                    // 完毕(越界段答段越界),查看端 playlist 封 ENDLIST
                    channel.sendRendition(requestId, FdWire.renditionEofPayload(requestId))
                        .onFailure { Log.w(TAG, "0x0C eof 通知发送失败 id=$requestId: ${it.message}") }
                }
            }
        }
    }

    // ── cancel(fs.rendition_cancel)────────────────────────────────────────

    private suspend fun handleCancel(frame: EventFrame) {
        val requestId = requestId(frame) ?: return // 无路由键静默丢弃
        val session = frame.params.opt("session")?.asLongOrNull()
        if (session == null) {
            // 畸形取消(缺 session):拒回告会误伤失败标记面(daemon 以
            // request_id 键失败),记日志静默毕(daemon cancel 发后即忘,
            // 无等待方)
            Log.w(TAG, "取消事件缺 session id=$requestId,丢弃")
            return
        }
        sessions.remove(session)?.set(true)
        // 段目录即清(cancel 语义;未知会话幂等照成——残件自愈面)。管线
        // 在下个轮询点退出;写盘竞窗孤儿件由挂载清扫兜底。
        val dir = File(renditionDir, session.toString())
        if (dir.exists()) {
            runCatching { dir.deleteRecursively() }
                .onFailure { Log.w(TAG, "段目录清理失败 id=$session: ${it.message}") }
        }
        channel.sendRendition(requestId, FdWire.renditionAckPayload(requestId)).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x0C 取消回执被拒 id=$requestId") },
            onFailure = { e -> Log.w(TAG, "0x0C 取消回执发送失败 id=$requestId: ${e.message}") },
        )
    }

    // ── 共用小件 ──────────────────────────────────────────────────────────

    /** 事件路由键(缺/非数 = null,调用方静默丢弃)。 */
    private fun requestId(frame: EventFrame): Long? = try {
        frame.params.opt("request_id")?.asLongOrNull()
    } catch (e: Exception) {
        null
    }

    /** 拒答帧(0x0C ok:false;open 待答窗内 = daemon 落 unsupported)。 */
    private suspend fun refuse(requestId: Long, reason: String) {
        Log.w(TAG, "转码桥拒答 id=$requestId: $reason")
        channel.sendRendition(requestId, FdWire.refusePayload(requestId, reason)).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x0C 拒答被拒 id=$requestId") },
            onFailure = { e -> Log.w(TAG, "0x0C 拒答发送失败 id=$requestId: ${e.message}") },
        )
    }

    /** 启动清扫:rendition-serve 整目录清空(残件自愈;目录本体保留)。与
     *  引擎缓存目录分立(thumb-serve 同哲学——清扫整目录清空,同目录会把
     *  引擎 LRU 缓存抹掉)。 */
    private fun sweep() {
        renditionDir.mkdirs()
        renditionDir.listFiles()?.forEach { item ->
            runCatching { item.deleteRecursively() }
                .onFailure { Log.w(TAG, "段残件清理失败 ${item.name}: ${it.message}") }
        }
    }
}
