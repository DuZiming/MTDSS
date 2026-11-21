package com.mtdss.android.serve

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
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
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileDescriptor
import java.io.FileOutputStream
import java.io.IOException

/** 缩略图请求宽钳制上界(daemon preview::thumb `THUMB_MAX_WIDTH` 同值;降
 *  采样参数面双侧同口径——旧 daemon 透传防御,巨 w 的位图分配是内存炸弹面)。 */
internal const val THUMB_MAX_WIDTH: Int = 1024

/**
 * 缩略图生成面(实机 = [AndroidThumbEncoder];JVM 单测注入伪编码器——
 * android.jar 桩不可真解码,解码正确性归实机冒烟)。生成 ≤w 宽 JPEG 字节;
 * 失败/不可解 → null(拒答,引擎落既有回退)。
 */
fun interface ThumbEncoder {
    /** [source] 的 fd 副本随用随闭([OpenedSource.release] 归调用方);阻塞面,调用方在 IO 线程。 */
    fun jpeg(source: OpenedSource, displayName: String, w: Int): ByteArray?
}

/**
 * 实机编码器(V4-1 T9;spec §4「Android 源:MediaMetadataRetriever(视频)
 * + BitmapFactory(图片,含 HEIC 系统解码)」):显示名扩展名分型——视频 =
 * [MediaMetadataRetriever] 抽帧,其余 = [BitmapFactory] 两段采样解码
 * (inSampleSize 整数降采样 + 兜底等比缩到 ≤w),统一 JPEG 压缩回字节。
 * fd 直驱([MediaMetadataRetriever.setDataSource(FileDescriptor)] /
 * [BitmapFactory.decodeFileDescriptor] 内部自 dup,源副本生命周期 = 解码窗)。
 */
object AndroidThumbEncoder : ThumbEncoder {
    private const val TAG = "ThumbEncoder"

    /** 视频扩展名分型表(小写;表外一律走图片解码——BitmapFactory 对非图
     *  源如实 null,不误伤未知视频容器之外的类型面)。 */
    private val VIDEO_EXTS = setOf("mp4", "mov", "m4v", "mkv", "webm", "3gp", "avi", "ts")

    override fun jpeg(source: OpenedSource, displayName: String, w: Int): ByteArray? {
        val ext = displayName.substringAfterLast('.', "").lowercase()
        val raw = try {
            if (ext in VIDEO_EXTS) videoFrame(source.fd) else sampleBitmap(source.fd, w)
        } catch (e: Exception) {
            Log.w(TAG, "缩略图解码失败 $displayName: ${e.message}")
            null
        } ?: return null
        // 兜底等比缩到 ≤w(抽帧/采样后仍超宽时;等比不裁切,查看端语义)
        val scaled = if (raw.width > w || raw.height > w) {
            val scale = w.toFloat() / maxOf(raw.width, raw.height)
            Bitmap.createScaledBitmap(
                raw,
                maxOf(1, (raw.width * scale).toInt()),
                maxOf(1, (raw.height * scale).toInt()),
                true,
            )
        } else {
            raw
        }
        val out = ByteArrayOutputStream()
        scaled.compress(Bitmap.CompressFormat.JPEG, 85, out)
        return out.toByteArray().takeIf { it.isNotEmpty() }
    }

    /** 视频抽帧(首帧最近关键帧;release 恒收,retriever 自 dup fd)。 */
    private fun videoFrame(fd: FileDescriptor): Bitmap? {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(fd)
            return retriever.getFrameAtTime(0, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
        } finally {
            runCatching { retriever.release() }
        }
    }

    /** 图片两段采样解码(边界探尺寸 → inSampleSize 整数降采样 → 真解码;
     *  HEIC 系统解码覆盖,解码失败/非图源如实 null)。 */
    private fun sampleBitmap(fd: FileDescriptor, w: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFileDescriptor(fd, null, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= w || bounds.outHeight / (sample * 2) >= w) {
            sample *= 2
        }
        val opts = BitmapFactory.Options().apply { inSampleSize = sample }
        return BitmapFactory.decodeFileDescriptor(fd, null, opts)
    }
}

/**
 * 缩略图桥应答器(V4-1 T9 预览波;daemon 侧 jobs.rs ThumbBridge /
 * fdpass.rs 0x0B 契约为真源):消费 daemon 的 `fs.request_thumb` 事件,按
 * path 分流两源开 fd,经 [ThumbEncoder] 生成 ≤w 宽 JPEG 落自有临时目录
 * `{home}/thumb-serve/`,经 fdpass **thumb 通道**(0x0B,专用实例)回
 * `{ok, path, request_id}`——引擎读字节入源端缓存后 unlink(缩略图缺位
 * 只是降级:拒答时引擎落既有回退,不回错误帧)。
 *
 * - **路径分流**(与 SeekResponder 同面,photoVpathSub/resolveServeRoot
 *   共享单点防漂移):`/照片图库` 前缀 → **MediaStore 臂**(`相册/名` →
 *   批内唯一名重放([PhotoResponder.uniqueName] 同序)→
 *   [PhotoLibraryFace.openSeekable]);其余 → **SAF 臂**(根解析 = 共享
 *   单点 → 父列表定位终段 → [SafFace.openRead])。根自身/目录不可出图(拒答)。
 * - **降采样参数**:`w` 钳制 1..[THUMB_MAX_WIDTH](与 daemon 同值)后原样
 *   到达编码器;视频/图片分型凭据 = 显示名扩展名(编码器内部分型)。
 * - **源生命周期**:解码窗即源生命周期——编码后 `OpenedSource.release()`
 *   (fd 副本已交编码器;SeekResponder 交付即 release 同哲学)。
 * - **生成件生命周期**:落盘 `{requestId}-{名}.jpg`(请求级唯一名),引擎
 *   读毕 unlink;**启动清扫**:挂载即清空 thumb-mp(崩溃残件自愈;目录与
 *   引擎缓存目录 thumb-tmp 分立——清扫整目录清空,同目录会抹掉引擎 LRU
 *   缓存)。
 * - **生命周期**:AppGraph 构造即订阅(SeekResponder 同纪律);畸形事件帧
 *   可拒答的拒答、无路由键的静默丢弃;应答帧 ack ok:false = 请求已超时/
 *   未知,记日志即毕。
 */
class ThumbResponder(
    private val saf: SafFace,
    private val inboxGrant: InboxGrantLookup,
    private val roots: RootLookup,
    private val photos: PhotoLibraryFace,
    /** 生成件临时目录锚 = {engine_home}/thumb-serve(MtdssEngine.thumbServeDir;
     *  与 daemon preview::thumb 的 thumb_serve_dir 同锚,两侧不得漂移)。 */
    private val thumbDir: File,
    private val channel: FdPassChannel,
    ipc: IpcClient,
    private val scope: CoroutineScope,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val encoder: ThumbEncoder = AndroidThumbEncoder,
) {
    private val subscribed = CompletableDeferred<Unit>()

    companion object {
        /** 本桥消费的事件集(订阅过滤)。 */
        private val THUMB_EVENTS = setOf(Ops.EVENT_REQUEST_THUMB)

        private const val TAG = "ThumbResponder"
    }

    init {
        scope.launch {
            // 启动清扫(App 侧残件自愈;引擎读毕 unlink 之外的兜底)
            runCatching { sweep() }.onFailure { Log.w(TAG, "thumb-serve 清扫失败: ${it.message}") }
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    if (frame.method in THUMB_EVENTS) {
                        scope.launch(ioDispatcher) {
                            runCatching { handle(frame) }
                                .onFailure { Log.w(TAG, "thumb 事件处置异常: ${it.message}") }
                        }
                    }
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    /** 单事件处置(测试直驱面;阻塞 SAF/MediaStore/生成面在调用方线程)。 */
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
            ?: return refuse(requestId, "事件缺 path")
        val wRaw = try {
            frame.params.opt("w")?.asLongOrNull()
        } catch (e: Exception) {
            null
        } ?: return refuse(requestId, "事件缺 w")
        val w = wRaw.coerceIn(1L, THUMB_MAX_WIDTH.toLong()).toInt()
        val src = openThumbSource(path).fold(
            onSuccess = { it },
            onFailure = { e -> return refuse(requestId, e.message ?: "App 无法生成缩略图") },
        )
        val bytes = try {
            encoder.jpeg(src.opened, src.displayName, w)
        } catch (e: Exception) {
            Log.w(TAG, "缩略图生成异常 ${src.displayName}: ${e.message}")
            null
        }
        // 解码窗即源生命周期:编码后释放(fd 副本已交编码器;交付即 release
        // 同哲学)——生成失败面同样释放,不泄漏
        src.opened.release()
        if (bytes == null || bytes.isEmpty()) {
            return refuse(requestId, "无法生成缩略图: ${src.displayName}")
        }
        deliver(requestId, src.displayName, bytes)
    }

    // ── 路径分流两源(SAF/MediaStore)──────────────────────────────────────

    /** 开源结果:fd 源 + 显示名(编码器视频/图片分型凭据 + 生成件命名)。 */
    internal class ThumbSource(val opened: OpenedSource, val displayName: String)

    /**
     * vpath → 源(可 seek fd 只读;缩略图解码只读面与 seek 同原语)。
     * `/照片图库` 前缀 = MediaStore 臂,其余 = SAF 服务桥根面;根自身/目录/
     * 缺席/未授权一律失败词面(拒答 → 引擎回退)。行走面 = [BridgeSources]
     * 共享单点(V4-1 T13 提取,三桥一份;what = 「出图」钉本桥词面)。阻塞
     * 面,调用方在 IO 线程。
     */
    internal fun openThumbSource(path: String): Result<ThumbSource> =
        BridgeSources.open(path, "出图", saf, inboxGrant, roots, photos)
            .map { ThumbSource(it.opened, it.displayName) }

    // ── 生成件落盘与帧化 ──────────────────────────────────────────────────

    /** 生成件落盘 → 0x0B `{ok, path, request_id}` → 0x81 ack 回执。**不删**
     *  生成件(引擎异步读——ack 只证明帧受理,不证明字节已读;引擎读毕
     *  unlink,本侧挂载清扫兜底,serve-tmp 同哲学)。 */
    private suspend fun deliver(requestId: Long, displayName: String, bytes: ByteArray) {
        thumbDir.mkdirs()
        // 显示名单段化(SAF/MediaStore 显示名不含分隔符,防御性替换)
        val dest = File(thumbDir, "$requestId-${displayName.replace('/', '_')}.jpg")
        try {
            FileOutputStream(dest).use { it.write(bytes) }
        } catch (e: IOException) {
            runCatching { dest.delete() }
            return refuse(requestId, "缩略图落盘失败: ${e.message}")
        }
        channel.sendThumbData(requestId, dest.absolutePath).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x0B 应答被拒(请求已超时/未知)id=$requestId") },
            onFailure = { e -> Log.w(TAG, "0x0B 应答发送失败 id=$requestId: ${e.message}") },
        )
    }

    private suspend fun refuse(requestId: Long, reason: String) {
        Log.w(TAG, "thumb 桥拒答 id=$requestId: $reason")
        channel.sendThumbData(requestId, null, reason).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x0B 拒答被拒(请求已超时/未知)id=$requestId") },
            onFailure = { e -> Log.w(TAG, "0x0B 拒答发送失败 id=$requestId: ${e.message}") },
        )
    }

    /** 启动清扫:thumb-serve 整目录清空(残件自愈;目录本体保留)。与引擎
     *  缓存目录 thumb-tmp 分立——本清扫若同目录会把引擎 LRU 缓存抹掉。 */
    private fun sweep() {
        thumbDir.mkdirs()
        thumbDir.listFiles()?.forEach { item ->
            runCatching { item.deleteRecursively() }
                .onFailure { Log.w(TAG, "缩略图残件清理失败 ${item.name}: ${it.message}") }
        }
    }
}
