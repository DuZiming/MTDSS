package com.mtdss.android.serve

import android.Manifest
import android.content.ContentUris
import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Log
import androidx.core.content.ContextCompat
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.Ops
import com.mtdss.android.ipc.asLongOrNull
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch
import java.io.Closeable
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream

/** 相册条目(根列表的目录项;children = 相册内照片数,T5b「N 项」语义)。 */
data class PhotoAlbum(val name: String, val count: Int)

/** 照片条目(photoId = App 侧资产标识,0x07 导出凭据;name = 批内唯一名原料)。 */
data class PhotoAsset(val photoId: String, val name: String, val size: Long, val mtimeMs: Long)

/** 导出源:原始字节流 + 元数据宣称尺寸(尺寸守卫判据)+ 落盘显示名。 */
class PhotoExportSource(val input: InputStream, val size: Long, val displayName: String) : Closeable {
    override fun close() = input.close()
}

/**
 * 相册库数据面(实机 = MediaStore 双表查询;JVM 单测注入伪表)。查询失败
 * (未授权态穿透/卷未挂载)一律 null = 调用方拒答。
 */
interface PhotoLibraryFace {
    /** 相册列表(children = 照片数);查询失败 → null。 */
    fun albums(): List<PhotoAlbum>?

    /** 相册内照片(最新在前;无此相册/查询失败 → null)。 */
    fun photos(albumName: String): List<PhotoAsset>?

    /** 导出源(原始字节;资产不可用/拒读 → null)。 */
    fun openExport(photoId: String): PhotoExportSource?
}

/** 0x06 entries 逐项投影(相册目录项 / 照片文件项二形,字段与 daemon photo.rs 解析面逐一对齐)。 */
internal sealed interface PhotoEntry {
    data class Album(val name: String, val count: Int) : PhotoEntry
    data class Photo(val name: String, val size: Long, val mtimeMs: Long, val photoId: String) : PhotoEntry
}

/** vpath 列表结果;[Refused] = 拒答词面(0x06 error)。 */
internal sealed interface PhotoListing {
    data class Ok(val entries: List<PhotoEntry>, val truncated: Boolean) : PhotoListing
    data class Refused(val reason: String) : PhotoListing
}

/**
 * 照片桥应答器(V3-4 §4.3;iOS PhotoServe.swift 契约镜像):消费 daemon 的
 * `fs.photo_list` / `fs.photo_export` 事件,经 fdpass **照片通道**(0x06/0x07,
 * 专用实例)应答——引擎把 `/照片图库` 虚拟根(根 = 相册列表,相册 = 照片
 * 列表)逐请求实问本类。
 *
 * - **权限即数据面**(对齐 iOS;本任务定案与 iOS 的 ok:false 分歧):未授权
 *   **不应答**(零帧)→ 引擎等待超时 → 虚拟根隐藏(桥级负缓存 TTL 内不再
 *   重探)。判定**实时**(每次应答前现查)——授权翻转下一问即生效,无需
 *   重启/重注册。运行时请求时机 = 设置页/首用,**不因对端浏览而弹窗**
 *   (本应答器零权限请求面;未授权 = 根不在,对端侧无感知)。
 * - **列表**(0x06):根("")= 相册列表(目录项,children = 照片数);相册名
 *   = 照片文件项(最新在前;name = 批内唯一名——原始文件名优先,撞名
 *   ` (n)` 顺延,R8 终审 I-1 同语言;photo_id = 资产标识 = 0x07 导出凭据)。
 *   重名相册取先(v1 边界,iOS 同)。单帧帽 [LISTING_CAP](fdpass payload
 *   64 KiB 上限防协议断连;超出 truncated=true,照片项按新→旧保前缀)。
 * - **导出**(0x07):原始字节流拷贝到自有临时目录 `{home}/photo-serve/`
 *   (`<requestId>-<名>` 请求级唯一名)回绝对路径——引擎读发后 unlink;
 *   **尺寸守卫**:流实读字节数与元数据宣称不符(盘损/中途失效)删临时拒导。
 * - **启动清扫**:挂载即清空 photo-serve(崩溃残件自愈;引擎侧终态 unlink
 *   两侧兜底,iOS 同哲学)。
 *
 * 生命周期:AppGraph 构造即订阅(SourceProvider 同纪律);畸形事件帧静默
 * 丢弃不炸订阅。
 */
class PhotoResponder(
    private val photos: PhotoLibraryFace,
    /** 导出临时目录锚 = {engine_home}/photo-serve(MtdssEngine.photoServeDir;
     * 与 daemon photo::serve_dir 同锚,两侧不得漂移)。 */
    private val serveDir: File,
    private val channel: FdPassChannel,
    ipc: IpcClient,
    private val scope: CoroutineScope,
    /** 权限门(实时判定;33+ READ_MEDIA_IMAGES+VIDEO / 29-32
     *  READ_EXTERNAL_STORAGE,manifest 按 tier 声明)。false = 不应答。 */
    private val authorized: () -> Boolean,
    /** 单帧条目帽(iOS PhotoResponder.listingEntryCap = 200 同值)。 */
    private val listingCap: Int = LISTING_CAP,
    private val ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
) {
    private val subscribed = CompletableDeferred<Unit>()

    companion object {
        /** 单帧条目上限(iOS 200 同值:条目 ~170-200B,200 项 ≈ 40KiB)。 */
        const val LISTING_CAP: Int = 200

        /** 本桥消费的事件集(订阅过滤)。 */
        private val PHOTO_EVENTS = setOf(Ops.EVENT_PHOTO_LIST, Ops.EVENT_PHOTO_EXPORT)

        private const val TAG = "PhotoResponder"

        /** 导出拷贝缓冲(8 MiB;GB 级源大缓冲降循环开销)。 */
        private const val COPY_BUFFER_BYTES: Int = 8 * 1024 * 1024

        /**
         * 批内唯一名:原始名优先,撞名 ` (n)` 顺延(序号插扩展名前,R8
         * 终审 I-1 同语言);used = 批内已占用名集(原地记账)。
         */
        internal fun uniqueName(raw: String, used: MutableSet<String>): String {
            if (used.add(raw)) return raw
            val dot = raw.lastIndexOf('.')
            val base = if (dot > 0) raw.substring(0, dot) else raw
            val ext = if (dot > 0) raw.substring(dot) else ""
            var n = 1
            while (true) {
                val candidate = "$base ($n)$ext"
                if (used.add(candidate)) return candidate
                n++
            }
        }
    }

    init {
        scope.launch {
            // 启动清扫(App 侧 unlink 义务;崩溃残件自愈)
            runCatching { sweep() }.onFailure { Log.w(TAG, "photo-serve 清扫失败: ${it.message}") }
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    if (frame.method in PHOTO_EVENTS) {
                        scope.launch(ioDispatcher) {
                            runCatching { handle(frame) }
                                .onFailure { Log.w(TAG, "photo 事件处置异常 ${frame.method}: ${it.message}") }
                        }
                    }
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    /**
     * 单事件处置(测试直驱面)。权限门先行:未授权**不应答**(零帧,引擎
     * 按等待超时 → 根隐藏收口);其余失败 = ok:false 拒答(引擎 Refused 臂
     * ——桥仍在,仅该请求失败)。
     */
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
        if (!authorized()) {
            // 权限即数据面:不应答(引擎等待超时 → 虚拟根隐藏)
            Log.i(TAG, "照片权限未授予,不应答 id=$requestId(引擎侧根隐藏)")
            return
        }
        when (frame.method) {
            Ops.EVENT_PHOTO_LIST -> {
                val path = frame.params.opt("path")?.asStringOrNull()
                    ?: return refuse(FdWire.KIND_PHOTO_LIST, requestId, "事件缺 path")
                when (val listing = listEntries(path)) {
                    is PhotoListing.Ok -> reply(
                        FdWire.KIND_PHOTO_LIST, requestId,
                        FdWire.listReplyPayload(requestId, listing.entries.map(::entryPayload), listing.truncated),
                    )

                    is PhotoListing.Refused -> refuse(FdWire.KIND_PHOTO_LIST, requestId, listing.reason)
                }
            }

            Ops.EVENT_PHOTO_EXPORT -> {
                val photoId = frame.params.opt("photo_id")?.asStringOrNull()
                    ?: return refuse(FdWire.KIND_PHOTO_EXPORT, requestId, "事件缺 photo_id")
                exportAsset(photoId, requestId).fold(
                    onSuccess = { f -> reply(FdWire.KIND_PHOTO_EXPORT, requestId, FdWire.exportReplyPayload(requestId, f.absolutePath)) },
                    onFailure = { e -> refuse(FdWire.KIND_PHOTO_EXPORT, requestId, e.message ?: "导出失败") },
                )
            }

            else -> Unit // 非本桥事件
        }
    }

    // ── 列表(0x06)────────────────────────────────────────────────────────

    /** vpath 列表("" = 根 = 相册列表;相册名 = 照片列表)。阻塞面,IO 线程调用。 */
    internal fun listEntries(path: String): PhotoListing {
        val rows: List<PhotoEntry> = if (path.isEmpty()) {
            val albums = photos.albums() ?: return PhotoListing.Refused("相册查询失败")
            albums.map { PhotoEntry.Album(name = it.name, count = it.count) }
        } else {
            val assets = photos.photos(path) ?: return PhotoListing.Refused("无此相册: $path")
            val used = HashSet<String>()
            assets.map { asset ->
                PhotoEntry.Photo(
                    name = uniqueName(asset.name, used),
                    size = asset.size,
                    mtimeMs = asset.mtimeMs,
                    photoId = asset.photoId,
                )
            }
        }
        val truncated = rows.size > listingCap
        return PhotoListing.Ok(rows.take(listingCap), truncated)
    }

    // ── 导出(0x07)────────────────────────────────────────────────────────

    /** 导出一件:原始字节 → `photo-serve/<requestId>-<名>`;尺寸守卫(流实
     *  读 vs 元数据宣称)不符删临时拒导。 */
    fun exportAsset(photoId: String, requestId: Long): Result<File> {
        val src = photos.openExport(photoId)
            ?: return Result.failure(IllegalStateException("资产不可用: $photoId"))
        serveDir.mkdirs()
        // 请求级唯一名(request_id 引擎侧单调不复用;防在途同名覆写)
        val dest = File(serveDir, "$requestId-${src.displayName.substringAfterLast('/')}")
        return try {
            src.use { source ->
                FileOutputStream(dest).use { out ->
                    val buf = ByteArray(COPY_BUFFER_BYTES)
                    var copied = 0L
                    while (true) {
                        val n = source.input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        copied += n
                    }
                    if (copied != source.size) {
                        throw IOException("导出尺寸不符(实拷 $copied 预期 ${source.size}): $photoId")
                    }
                }
            }
            Result.success(dest)
        } catch (e: Exception) {
            dest.delete()
            Result.failure(e)
        }
    }

    // ── 内部 ──────────────────────────────────────────────────────────────

    /** 条目 → 0x06 线上形(相册 `{children, is_dir, mtime_ms, name, size}` /
     *  照片 `{is_dir, mtime_ms, name, photo_id, size}`;字母序由 toCanonicalJson 统一)。 */
    private fun entryPayload(e: PhotoEntry): JVal.Obj = when (e) {
        is PhotoEntry.Album -> JVal.Obj(
            "children" to JVal.num(e.count),
            "is_dir" to JVal.bool(true),
            "mtime_ms" to JVal.num(0L),
            "name" to JVal.str(e.name),
            "size" to JVal.num(0L),
        )

        is PhotoEntry.Photo -> JVal.Obj(
            "is_dir" to JVal.bool(false),
            "mtime_ms" to JVal.num(e.mtimeMs),
            "name" to JVal.str(e.name),
            "photo_id" to JVal.str(e.photoId),
            "size" to JVal.num(e.size),
        )
    }

    /** 启动清扫:清空 photo-serve 内容(目录保留;逐项失败留痕不中断)。 */
    private fun sweep() {
        serveDir.mkdirs()
        serveDir.listFiles()?.forEach { item ->
            runCatching { item.deleteRecursively() }
                .onFailure { Log.w(TAG, "导出残件清理失败 ${item.name}: ${it.message}") }
        }
    }

    private suspend fun reply(kind: Int, requestId: Long, payload: JVal.Obj) {
        channel.sendReply(kind, requestId, payload).fold(
            onSuccess = { ack -> if (!ack.ok) Log.w(TAG, "0x%02x 应答被拒(请求已超时/未知)id=$requestId".format(kind)) },
            onFailure = { e -> Log.w(TAG, "0x%02x 应答发送失败 id=$requestId: ${e.message}".format(kind)) },
        )
    }

    private suspend fun refuse(kind: Int, requestId: Long, reason: String) {
        Log.w(TAG, "照片桥拒答 kind=0x%02x id=$requestId: $reason".format(kind))
        reply(kind, requestId, FdWire.refusePayload(requestId, reason))
    }
}

/**
 * 照片权限授予判定(V3-4 §4.3 权限矩阵;minSdk 29 无 ≤28 面):
 * API 33+ = READ_MEDIA_IMAGES + READ_MEDIA_VIDEO 双权;API 29-32 =
 * READ_EXTERNAL_STORAGE(MIX 3 冒烟机在此档)。实时判定(每次应答前现查)。
 */
fun photoPermissionGranted(context: Context): Boolean {
    fun read(p: String): Boolean =
        ContextCompat.checkSelfPermission(context, p) == PackageManager.PERMISSION_GRANTED
    return if (Build.VERSION.SDK_INT >= 33) {
        read(Manifest.permission.READ_MEDIA_IMAGES) && read(Manifest.permission.READ_MEDIA_VIDEO)
    } else {
        read(Manifest.permission.READ_EXTERNAL_STORAGE)
    }
}

/**
 * 实机相册库面(MediaStore 双表:Images + Video;minSdk 29,
 * BUCKET_DISPLAY_NAME/RELATIVE_PATH 族列可用)。photoId = `img:<_id>` /
 * `vid:<_id>`(双表 _id 空间独立,前缀防撞)。
 */
class MediaStorePhotoLibrary(private val context: Context) : PhotoLibraryFace {

    private data class Row(
        val photoId: String,
        val album: String,
        val name: String,
        val size: Long,
        val mtimeMs: Long,
    )

    /** 双表行扫描([albumName] null = 全量;DATE_MODIFIED DESC = 最新在前)。 */
    private fun scanRows(albumName: String?): List<Row>? = try {
        val rows = ArrayList<Row>()
        scanCollection(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, "img:", albumName, rows)
        scanCollection(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, "vid:", albumName, rows)
        rows
    } catch (e: Exception) {
        Log.w(TAG, "相册查询失败(${e.javaClass.simpleName}: ${e.message})")
        null
    }

    private fun scanCollection(uri: Uri, prefix: String, albumName: String?, out: MutableList<Row>) {
        val projection = arrayOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.BUCKET_DISPLAY_NAME,
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.SIZE,
            MediaStore.MediaColumns.DATE_MODIFIED,
        )
        val selection = if (albumName != null) "${MediaStore.MediaColumns.BUCKET_DISPLAY_NAME}=?" else null
        val args = if (albumName != null) arrayOf(albumName) else null
        context.contentResolver.query(
            uri,
            projection,
            selection,
            args,
            "${MediaStore.MediaColumns.DATE_MODIFIED} DESC",
        )?.use { c ->
            while (c.moveToNext()) {
                val bucket = c.getString(1) ?: continue // 无相册归属行:相册名是路由键,跳过
                val name = c.getString(2) ?: continue
                out += Row(
                    photoId = "$prefix${c.getLong(0)}",
                    album = bucket,
                    name = name,
                    size = if (c.isNull(3)) 0L else c.getLong(3),
                    // MediaStore 的 date_modified 以秒计(DATE_MODIFIED 秒 → ms)
                    mtimeMs = if (c.isNull(4)) 0L else c.getLong(4) * 1000L,
                )
            }
        }
    }

    override fun albums(): List<PhotoAlbum>? {
        val rows = scanRows(null) ?: return null
        return rows.groupBy { it.album }
            .map { (name, list) -> PhotoAlbum(name, list.size) }
            .sortedBy { it.name }
    }

    override fun photos(albumName: String): List<PhotoAsset>? =
        scanRows(albumName)?.map { PhotoAsset(it.photoId, it.name, it.size, it.mtimeMs) }

    override fun openExport(photoId: String): PhotoExportSource? {
        val colon = photoId.indexOf(':')
        if (colon <= 0) return null
        val id = photoId.substring(colon + 1).toLongOrNull() ?: return null
        val uri = when (photoId.substring(0, colon)) {
            "img" -> ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, id)
            "vid" -> ContentUris.withAppendedId(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, id)
            else -> return null
        }
        // 单查补元数据(显示名/宣称尺寸;投影被拒按 0 兜底,尺寸守卫以流实读对账)
        var displayName = "asset"
        var size = 0L
        try {
            context.contentResolver.query(
                uri,
                arrayOf(MediaStore.MediaColumns.DISPLAY_NAME, MediaStore.MediaColumns.SIZE),
                null,
                null,
                null,
            )?.use { c ->
                if (c.moveToFirst()) {
                    displayName = c.getString(0) ?: displayName
                    size = if (c.isNull(1)) 0L else c.getLong(1)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "导出元数据查询失败 $photoId: ${e.message}")
        }
        val input = try {
            context.contentResolver.openInputStream(uri) ?: return null
        } catch (e: Exception) {
            Log.w(TAG, "导出打开失败 $photoId: ${e.message}")
            return null
        }
        return PhotoExportSource(input, size, displayName)
    }

    private companion object {
        private const val TAG = "MediaStorePhotos"
    }
}
