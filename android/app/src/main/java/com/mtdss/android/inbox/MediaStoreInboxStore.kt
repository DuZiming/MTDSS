package com.mtdss.android.inbox

import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import java.io.FileDescriptor

/**
 * 收件箱暂存的 MediaStore 面(T6 磁盘侧;实现保持薄——决策逻辑在
 * [InboxLogic],此处只有 Android 存储行为,实机验证):
 *
 * - **落位**:MediaStore.Downloads 集合,`RELATIVE_PATH = "Download/MTDSS"`
 *   (spec §5.1:API 29+ 自有贡献零权限),DISPLAY_NAME = rel 的 '/' 归一为
 *   "__"(镜像 e2e 伪 provider 的暂存命名,防嵌套 rel 落子目录且天然防
 *   路径穿越);
 * - **IS_PENDING 生命周期**:新建即 IS_PENDING=1(对其他 App 不可见),
 *   0x03 done:true → 置 0(系统文件管理器可见);done:false / 应答被拒 /
 *   发送失败 → 删除条目;
 * - **fd 语义**:"rw" 打开(T6:daemon 对 fd 做 set_len 预分配 + finalize
 *   整文件读回校验,须可读可写;**resume 重开不截断**——"w" 会截断,禁用);
 * - **resume 未命中兜底**:按 DISPLAY_NAME+RELATIVE_PATH+IS_PENDING=1 重查
 *   (App 重启丢内存 Uri 表后的恢复面,与 T6「拒开 → daemon 清陈旧记录 →
 *   全新重收」闭环);
 * - **同名 pending 残留**:create 前清除同名 pending 条目(pending 仅本
 *   App 可见、属暂存性质;fresh 收件只在 daemon 无记录时发生,同名 pending
 *   即陈旧残留——清之避免 MediaStore 自动改名 "name (1)" 污染终名)。
 */
class MediaStoreInboxStore(private val context: Context) : InboxStore {

    private val resolver get() = context.contentResolver

    override fun create(rel: String, size: Long): InboxHandle {
        val name = displayName(rel)
        deletePending(name)
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeTypeOf(name))
            put(MediaStore.MediaColumns.RELATIVE_PATH, INBOX_RELATIVE_PATH)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: throw InboxStoreException("MediaStore 插入失败: $rel")
        return openHandle(uri, rel)
    }

    override fun reopen(rel: String, size: Long): InboxHandle? {
        val uri = pendingUri(displayName(rel)) ?: return null
        return openHandle(uri, rel)
    }

    /** 打开暂存句柄:"rw"(可读可写、不截断——见类注释 fd 语义)。 */
    private fun openHandle(uri: Uri, rel: String): InboxHandle {
        val pfd = try {
            resolver.openFileDescriptor(uri, "rw")
        } catch (e: Exception) {
            throw InboxStoreException("暂存打开失败(rel=$rel): ${e.message}", e)
        } ?: throw InboxStoreException("暂存打开失败(rel=$rel): openFileDescriptor 返回 null")
        return Handle(uri, pfd)
    }

    /** 同名在途(pending)条目查询 → uri;无则 null。 */
    private fun pendingUri(name: String): Uri? {
        resolver.query(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI,
            arrayOf(MediaStore.MediaColumns._ID),
            "${MediaStore.MediaColumns.DISPLAY_NAME}=? AND " +
                "${MediaStore.MediaColumns.RELATIVE_PATH}=? AND " +
                "${MediaStore.MediaColumns.IS_PENDING}=1",
            arrayOf(name, INBOX_RELATIVE_PATH),
            null,
        )?.use { c ->
            if (c.moveToFirst()) {
                return ContentUris.withAppendedId(MediaStore.Downloads.EXTERNAL_CONTENT_URI, c.getLong(0))
            }
        }
        return null
    }

    /** 清除同名 pending 条目(create 前的陈旧残留清理,见类注释)。 */
    private fun deletePending(name: String) {
        resolver.delete(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI,
            "${MediaStore.MediaColumns.DISPLAY_NAME}=? AND " +
                "${MediaStore.MediaColumns.RELATIVE_PATH}=? AND " +
                "${MediaStore.MediaColumns.IS_PENDING}=1",
            arrayOf(name, INBOX_RELATIVE_PATH),
        )
    }

    private inner class Handle(
        private val uri: Uri,
        private val pfd: ParcelFileDescriptor,
    ) : InboxHandle {
        override val rawFd: FileDescriptor get() = pfd.fileDescriptor

        override fun release() {
            runCatching { pfd.close() }
        }

        override fun reveal() {
            // 揭开:IS_PENDING=0,系统文件管理器可见(daemon 已完成写入;
            // 本地 fd 副本一并关闭)
            release()
            runCatching {
                resolver.update(
                    uri,
                    ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) },
                    null,
                    null,
                )
            }
        }

        override fun discard() {
            release()
            runCatching { resolver.delete(uri, null, null) }
        }
    }

    companion object {
        /** 收件箱落位:Downloads/MTDSS(MediaStore.Downloads 的 RELATIVE_PATH 形态)。 */
        const val INBOX_RELATIVE_PATH: String = "Download/MTDSS"

        /** rel → DISPLAY_NAME:'/' 归一为 "__"(嵌套 rel 平铺,镜像 e2e 伪 provider 暂存命名)。 */
        fun displayName(rel: String): String = rel.replace("/", "__")

        /** 按扩展名猜 MIME;未知兜 application/octet-stream。 */
        fun mimeTypeOf(name: String): String {
            val ext = name.substringAfterLast('.', "").lowercase()
            if (ext.isEmpty()) return "application/octet-stream"
            return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
                ?: "application/octet-stream"
        }
    }
}
