package com.mtdss.android.ui.local

import android.content.Context
import android.provider.MediaStore

/**
 * 收件箱枚举薄面(MediaStore 查询 Downloads/MTDSS,即 T9 InboxPlacement
 * 的落位子树;无逻辑,实机验证):
 * - 范围:RELATIVE_PATH = `Download/MTDSS` 或 `Download/MTDSS/…`(精确
 *   前缀段匹配,LIKE 'Download/MTDSS%' 会误伤 `MTDSS2` 之类的旁支);
 * - 含本 App 的 IS_PENDING 条目(标「接收中」;他 App 的 pending 对查询
 *   不可见,天然无污染);
 * - 排序:修改时间倒序(最近的收件在前)。
 */
class InboxListing(private val context: Context) {

    data class InboxEntry(
        val displayName: String,
        /** 收件箱内相对路径(含文件名;'/' 分段,与 rel 语义同源)。 */
        val relPath: String,
        val size: Long,
        val mtimeMs: Long,
        val pending: Boolean,
    )

    fun query(): List<InboxEntry> {
        val resolver = context.contentResolver
        val base = com.mtdss.android.inbox.InboxPlacement.BASE_RELATIVE_PATH
        val projection = arrayOf(
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.RELATIVE_PATH,
            MediaStore.MediaColumns.SIZE,
            MediaStore.MediaColumns.DATE_MODIFIED,
            MediaStore.MediaColumns.IS_PENDING,
        )
        val out = ArrayList<InboxEntry>()
        try {
            resolver.query(
                MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                projection,
                "${MediaStore.MediaColumns.RELATIVE_PATH}=? OR ${MediaStore.MediaColumns.RELATIVE_PATH} LIKE ?",
                arrayOf(base, "$base/%"),
                "${MediaStore.MediaColumns.DATE_MODIFIED} DESC",
            )?.use { c ->
                while (c.moveToNext()) {
                    val display = c.getString(0) ?: continue
                    val relative = c.getString(1) ?: continue
                    val relPath = when {
                        relative == base -> ""
                        relative.startsWith("$base/") -> relative.removePrefix("$base/")
                        else -> relative // 防御:谓词已限定范围,不应到达
                    }
                    out += InboxEntry(
                        displayName = display,
                        relPath = relPath,
                        size = if (c.isNull(2)) 0L else c.getLong(2),
                        // MediaStore 的 date_modified 以秒计(DocumentsContract 面才是毫秒)
                        mtimeMs = if (c.isNull(3)) 0L else c.getLong(3) * 1000,
                        pending = !c.isNull(4) && c.getInt(4) == 1,
                    )
                }
            }
        } catch (_: Exception) {
            // 查询失败(卷未挂载等):空表呈现,不打断本机页
        }
        return out
    }
}
