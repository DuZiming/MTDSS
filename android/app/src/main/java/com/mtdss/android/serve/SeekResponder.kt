package com.mtdss.android.serve

import android.util.Log
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.Ops
import com.mtdss.android.ipc.asLongOrNull
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import com.mtdss.android.send.OpenedSource
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.SafFace
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch

/** 照片图库虚拟根(daemon photo.rs PHOTO_ROOT 同串;vpath 前缀分流键)。 */
internal const val PHOTO_ROOT_VPATH: String = "/照片图库"

/**
 * 照片子路径解析(daemon photo_vpath 同口径:`/照片图库` → "";其余前缀
 * 剥除)。seek 源桥/缩略图桥**共用单点**(两桥各自实现必漂移,V3-4 根解
 * 析提共享单点同教训)。
 */
internal fun photoVpathSub(path: String): String? = when {
    path == PHOTO_ROOT_VPATH -> ""
    path.startsWith("$PHOTO_ROOT_VPATH/") -> path.removePrefix("$PHOTO_ROOT_VPATH/").trimEnd('/')
    else -> null
}

/**
 * 可 seek fd 源桥应答器(V4-1 T8 预览波;daemon 侧 jobs.rs SeekBridge /
 * fdpass 0x0D 契约为真源):消费 daemon 的 `fs.request_seek` 事件,开
 * **可 seek 只读 fd** 经 fdpass **seek 通道**(0x0D,专用实例)交付——
 * 预览按需读对手机源(SAF 服务桥根 / 照片图库)的原语面,视频随机读免
 * 整件导出;不可用 = 拒开(引擎落「导出再读」回退,轨 T3 已备)。
 *
 * - **路径分流**(单 `path` 参数 = 完整 vpath,daemon 侧规范化):
 *   `/照片图库` 前缀 → **MediaStore 臂**(`相册/名` → [PhotoLibraryFace.photos]
 *   批内唯一名重放([PhotoResponder.uniqueName] 同序)解析 photoId →
 *   [PhotoLibraryFace.openSeekable]);其余 → **SAF 臂**(根 vpath 解析 =
 *   [resolveServeRoot](与 ServeResponder 共享单点)→ 父列表定位终段
 *   (exportTo 同形)→ [SafFace.openRead])。根自身/目录不可 seek(拒开)。
 * - **拒开语言**:openSeekable/openRead 失败、根缺席、路径畸形一律无 fd
 *   0x0D + error 词面(daemon 落导出回退,词面仅日志消费);**不可 seek
 *   流的探测在 daemon 落点**(管道流拒测,App 侧不预判——MediaStore/SAF
 *   直开文件恒可 seek,管道仅 provider 退化形)。
 * - **生命周期**:AppGraph 构造即订阅(SourceProvider 同纪律);畸形事件
 *   帧静默丢弃不炸订阅;应答帧 ack ok:false = 请求已超时/未知,记日志即毕。
 */
class SeekResponder(
    private val saf: SafFace,
    private val inboxGrant: InboxGrantLookup,
    private val roots: RootLookup,
    private val photos: PhotoLibraryFace,
    private val channel: FdPassChannel,
    ipc: IpcClient,
    private val scope: CoroutineScope,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) {
    private val subscribed = CompletableDeferred<Unit>()

    companion object {
        /** 本桥消费的事件集(订阅过滤)。 */
        private val SEEK_EVENTS = setOf(Ops.EVENT_REQUEST_SEEK)

        private const val TAG = "SeekResponder"
    }

    init {
        scope.launch {
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    if (frame.method in SEEK_EVENTS) {
                        scope.launch(ioDispatcher) {
                            runCatching { handle(frame) }
                                .onFailure { Log.w(TAG, "seek 事件处置异常: ${it.message}") }
                        }
                    }
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    /** 单事件处置(测试直驱面;阻塞 SAF/MediaStore 面在调用方线程)。 */
    suspend fun handle(frame: EventFrame) {
        val requestId = try {
            frame.params.opt("request_id")?.asLongOrNull()
        } catch (e: Exception) {
            null
        }
        if (requestId == null) {
            Log.w(TAG, "${frame.method} 帧缺 request_id,丢弃")
            return
        }
        val path = frame.params.opt("path")?.asStringOrNull()
            ?: return refuse(channel, requestId, "事件缺 path")
        val opened = openSeek(path).fold(
            onSuccess = { it },
            onFailure = { e -> return refuse(channel, requestId, e.message ?: "App 拒开 seek 源") },
        )
        deliver(channel, requestId, opened)
    }

    /**
     * vpath → 可 seek 只读 fd。`/照片图库` 前缀 = MediaStore 臂,其余 =
     * SAF 服务桥根面;根自身/目录/缺席/未授权一律失败词面(拒开 → 引擎
     * 导出回退)。行走面 = [BridgeSources] 共享单点(V4-1 T13 提取,三桥
     * 一份;what = 「seek」钉本桥词面)。阻塞面,调用方在 IO 线程。
     */
    internal fun openSeek(path: String): Result<OpenedSource> =
        BridgeSources.open(path, "seek", saf, inboxGrant, roots, photos)
            .map { it.opened }

    // ── 帧化 ──────────────────────────────────────────────────────────────

    /** 已开 fd 的应答:0x0D(+fd)→ 0x81 ack 回执;本地 fd 副本随即
     *  release(SourceProvider.deliver 同款——SCM_RIGHTS 在 sendmsg 时已
     *  复制,引擎侧持收到的副本)。 */
    private suspend fun deliver(channel: FdPassChannel, requestId: Long, opened: OpenedSource) {
        val result = channel.sendSeekFd(requestId, opened.fd)
        opened.release()
        result.fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x0D 应答被拒(请求已超时/未知)id=$requestId") },
            onFailure = { e -> Log.w(TAG, "0x0D 应答发送失败 id=$requestId: ${e.message}") },
        )
    }

    private suspend fun refuse(channel: FdPassChannel, requestId: Long, reason: String) {
        Log.w(TAG, "seek 桥拒开 id=$requestId: $reason")
        channel.sendSeekFd(requestId, null, reason).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x0D 拒开被拒(请求已超时/未知)id=$requestId") },
            onFailure = { e -> Log.w(TAG, "0x0D 拒开发送失败 id=$requestId: ${e.message}") },
        )
    }
}
