package com.mtdss.android.inbox

import android.content.ContentResolver
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import java.io.FileDescriptor
import java.io.FileNotFoundException

/**
 * MediaStore 收件面的薄操作缝(收件箱侧唯一 Android 框界面,口径镜像
 * send 链的 SafFace/ContentResolverSafFace):行定位/增删/真开 fd/揭开。
 * 决策逻辑([MediaStoreInboxStore] 的残留三态处置、fd 验尸)只依赖本抽象,
 * JVM 单测注入伪件钉死;真实 MediaProvider 行为由实机验证(F11:MIUI 裸删
 * 后残留 done 行、insert 撞行抛错/返 null 不自动改名)。
 *
 * 契约:**行 id 是唯一句柄**(调用方不再触 Uri);失败一律抛底层异常
 * (调用方按词面收口),查询无行返回 null 而非抛。
 */
interface MediaFace {

    /** 落位键 (RELATIVE_PATH, DISPLAY_NAME) 行查询 → 行 id;无行 null。
     *  pendingOnly = 谓词加 IS_PENDING=1(在途限定)。 */
    fun rowIdAt(placement: InboxPlacement.Placement, pendingOnly: Boolean): Long?

    /** 新建 IS_PENDING=1 收件条目 → 行 id;插入失败抛底层异常或返回 null
     *  (provider 拒——实机 MIUI 撞 done 行的两种形态,见类注释 F11)。 */
    fun insertPending(placement: InboxPlacement.Placement, mimeType: String): Long?

    /** 定点删行(F11 僵尸验尸后清理 / discard 终态收尾)。 */
    fun deleteRow(rowId: Long)

    /** 谓词批量删同落位 pending 行(create 前的陈旧残留预清理)。 */
    fun deletePendingAt(placement: InboxPlacement.Placement)

    /** 真开行 fd(mode "r" = 验尸/对账,"rw" = 暂存;**不截断**);stale 行
     *  (裸删后行残留)必抛 FileNotFoundException。 */
    fun openRow(rowId: Long, mode: String): ParcelFileDescriptor

    /** 揭开行(IS_PENDING=0,系统文件管理器可见;0x03 done:true 收尾)。 */
    fun revealRow(rowId: Long)
}

/** 实机面:ContentResolver/MediaStore 直连(薄,无逻辑)。 */
class ContentResolverMediaFace(private val resolver: ContentResolver) : MediaFace {

    override fun rowIdAt(placement: InboxPlacement.Placement, pendingOnly: Boolean): Long? =
        resolver.query(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI,
            arrayOf(MediaStore.MediaColumns._ID),
            wherePlacement(placement, pendingOnly),
            placementArgs(placement),
            null,
        )?.use { c ->
            if (c.moveToFirst()) c.getLong(0) else null
        }

    override fun insertPending(placement: InboxPlacement.Placement, mimeType: String): Long? {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, placement.displayName)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeType)
            put(MediaStore.MediaColumns.RELATIVE_PATH, placement.relativePath)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        return resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?.let(ContentUris::parseId)
    }

    override fun deleteRow(rowId: Long) {
        resolver.delete(uriOf(rowId), null, null)
    }

    override fun deletePendingAt(placement: InboxPlacement.Placement) {
        resolver.delete(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI,
            wherePlacement(placement, pendingOnly = true),
            placementArgs(placement),
        )
    }

    override fun openRow(rowId: Long, mode: String): ParcelFileDescriptor =
        resolver.openFileDescriptor(uriOf(rowId), mode)
            ?: throw FileNotFoundException("openFileDescriptor($mode) 返回 null")

    override fun revealRow(rowId: Long) {
        resolver.update(
            uriOf(rowId),
            ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) },
            null,
            null,
        )
    }

    private fun uriOf(rowId: Long) =
        ContentUris.withAppendedId(MediaStore.Downloads.EXTERNAL_CONTENT_URI, rowId)

    /** 落位键查询谓词:(RELATIVE_PATH, DISPLAY_NAME) + 可选 IS_PENDING=1。 */
    private fun wherePlacement(placement: InboxPlacement.Placement, pendingOnly: Boolean): String =
        buildString {
            append(MediaStore.MediaColumns.DISPLAY_NAME).append("=? AND ")
            append(MediaStore.MediaColumns.RELATIVE_PATH).append("=?")
            if (pendingOnly) append(" AND ").append(MediaStore.MediaColumns.IS_PENDING).append("=1")
        }

    private fun placementArgs(placement: InboxPlacement.Placement): Array<String> =
        arrayOf(placement.displayName, placement.relativePath)
}

/**
 * 收件箱暂存的 MediaStore 面(T6 磁盘侧;实现保持薄——决策逻辑在
 * [InboxLogic],落位映射在 [InboxPlacement],Android 存储行为收进
 * [MediaFace]/[ContentResolverMediaFace] 缝后,本类即 JVM 可验的编排决策,
 * 实机验证):
 *
 * - **落位**:MediaStore.Downloads 集合,rel 的段映射进多段
 *   `RELATIVE_PATH = Download/MTDSS/<dir…>` + 末段 DISPLAY_NAME(spec §5.1
 *   嵌套目录结构保真;映射与段消毒见 [InboxPlacement],JVM 钉死);
 * - **IS_PENDING 生命周期**:新建即 IS_PENDING=1(对其他 App 不可见),
 *   0x03 done:true → 置 0(系统文件管理器可见);done:false / 应答被拒 /
 *   发送失败 → 删除条目;
 * - **fd 语义**:"rw" 打开(T6:daemon 对 fd 做 set_len 预分配 + finalize
 *   整文件读回校验,须可读可写;**resume 重开不截断**——"w" 会截断,禁用);
 * - **resume 未命中兜底**:按 (RELATIVE_PATH, DISPLAY_NAME, IS_PENDING=1)
 *   重查(App 重启丢内存 Uri 表后的恢复面,与 T6「拒开 → daemon 清陈旧
 *   记录 → 全新重收」闭环;落位键二元组保证不跨 rel 误配);
 * - **在场判据(F10)**:exists 不信行数据——行查询仅用于定位,真开
 *   fd("r")+ statSize 对账才是终判据(MIUI 文件管理器裸删后行残留、SIZE
 *   字段照旧,行查询会撒谎;stale 行的 open 必抛 → 不在场 → daemon FULL)。
 * - **同名残留三态(F11)**:create 前同落位残留行按态处置——
 *   pending → 直接清(暂存性质,仅本 App 可见);done+僵尸(裸删后行存
 *   文件亡)→ 真开 fd 验尸(与 exists/F10 同一原语,open 必抛即验明)后
 *   删行、重插;done+真文件(open 成功)→ **不删**(只删验明僵尸的行),
 *   插入撞上带因拒开(词面带「同落位终件在场冲突」+ 底层异常类名/消息;
 *   MIUI MediaProvider 对撞 done 行抛错/返 null 而不自动改名,不预清则
 *   fresh 重收 CREATE_FAILED ×N 卡死——F11 设备场实证:裸删后残留
 *   `is_pending=0` 的 small.txt 行,插入撞之连败 4 轮)。
 */
class MediaStoreInboxStore(private val face: MediaFace) : InboxStore {

    /** 实机面直通(注入缝口径同 ContentResolverSafFace;单测注入伪 MediaFace)。 */
    constructor(context: Context) : this(ContentResolverMediaFace(context.contentResolver))

    override fun create(rel: String, size: Long): InboxHandle {
        val placement = InboxPlacement.place(rel)
            ?: throw InboxStoreException("非法相对路径(拒开口径): $rel")
        face.deletePendingAt(placement)
        val realFileAtPlacement = evictZombieDoneRow(placement, rel)
        val rowId = try {
            face.insertPending(placement, mimeTypeOf(placement.displayName))
        } catch (e: Exception) {
            // 带因上浮(ffb835d/f9bd9a1 口径):CREATE_FAILED 词面须携带底层
            // 异常类名+消息;真文件在场冲突时一并点名(用户可见的清理指引)
            val why = buildString {
                if (realFileAtPlacement) append("同落位终件在场冲突; ")
                append("(${e.javaClass.simpleName}: ${e.message})")
            }
            throw InboxStoreException("MediaStore 插入失败: $rel $why", e)
        } ?: throw InboxStoreException("MediaStore 插入失败: $rel (insert 返回 null)")
        return openHandle(rowId, rel)
    }

    override fun reopen(rel: String, size: Long): InboxHandle? {
        // 消毒不过按未命中收口(拒开 → daemon 清陈旧记录 → 全新重收 → create
        // 再拒,终因一致;正常流不会到达:daemon 侧 rel 已过 validate_rel_path)
        val placement = InboxPlacement.place(rel) ?: return null
        val id = face.rowIdAt(placement, pendingOnly = true) ?: return null
        return openHandle(id, rel)
    }

    override fun exists(rel: String, size: Long): Boolean {
        // F7 在场查询:消毒不过按不在场收口(false → daemon FULL 重收,安全方向)。
        val placement = InboxPlacement.place(rel) ?: return false
        // F10 真开 fd 为准,行仅用于定位:MIUI 文件管理器裸删文件后
        // MediaStore 行可残留(SIZE 字段照旧),「行在且 SIZE 吻合」会撒谎
        // (假完成:daemon Skip 而实无文件)。终判据 = fd 真开成功且 fstat
        // 尺寸吻合;stale 行的 open 必抛(FileNotFoundException)→ 不在场。
        // 行定位查询失败仍按 ffb835d 契约收口为 InboxStoreException(镜像
        // openHandle 口径),由 InboxLogic.onQuery 统一转 false——绝不逸出到
        // provider 协程(appScope 无 CoroutineExceptionHandler,逸出即崩 App)。
        val id = try {
            face.rowIdAt(placement, pendingOnly = false)
        } catch (e: Exception) {
            throw InboxStoreException("在场查询失败(rel=$rel): ${e.message}", e)
        } ?: return false
        return try {
            face.openRow(id, "r").use { pfd -> fdSizeMatches(pfd.statSize, size) }
        } catch (_: Exception) {
            // open/stat 的任何异常(stale 行 FileNotFoundException、Security 等)
            // 都是「不在场」的证据而非查询故障:按 false 收口,宁可重传。
            false
        }
    }

    /** 打开暂存句柄:"rw"(可读可写、不截断——见类注释 fd 语义)。 */
    private fun openHandle(rowId: Long, rel: String): InboxHandle {
        val pfd = try {
            face.openRow(rowId, "rw")
        } catch (e: Exception) {
            throw InboxStoreException("暂存打开失败(rel=$rel): ${e.message}", e)
        }
        return Handle(rowId, pfd)
    }

    /**
     * F11 僵尸 done 行验尸清理(create 前,见类注释「同名残留三态」):
     * MIUI 文件管理器裸删文件后 MediaStore 残留 done 态行,insert 撞之
     * 抛错/返 null(pendingOnly 预清理罩不住)。验尸原语与 exists(F10)同源
     * ——真开 fd("r"):open 必抛 = 文件已亡(僵尸)→ 删行(本落位归属
     * 本 App 的收件箱子树,删除无责任问题)后照常走 insert;open 成功 =
     * 真文件在场 → 行保留,由 create 的插入路径撞上后带因拒开。
     *
     * 全部异常路径收口 [InboxStoreException](不逸出 provider 协程);只删
     * **验明**僵尸的行(查询失败即抛,绝不盲删)。
     *
     * @return true = 落位上有真文件在场的 done 行(保留未删,插入预期冲突)
     */
    private fun evictZombieDoneRow(placement: InboxPlacement.Placement, rel: String): Boolean {
        val id = try {
            face.rowIdAt(placement, pendingOnly = false)
        } catch (e: Exception) {
            throw InboxStoreException("落位 done 行查询失败(rel=$rel): ${e.message}", e)
        } ?: return false
        return try {
            face.openRow(id, "r").use { } // 真开成功 = 真文件在场:行保留(只删验明僵尸的)
            true
        } catch (e: Exception) {
            // 验明僵尸(文件已亡):删行后再走 insert。删除失败带因上浮——
            // 静默则 insert 撞同一条残留行,词面退回 F11 原样。
            try {
                face.deleteRow(id)
            } catch (de: Exception) {
                throw InboxStoreException("僵尸行删除失败(rel=$rel): ${de.message}", de)
            }
            false
        }
    }

    private inner class Handle(
        private val rowId: Long,
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
            runCatching { face.revealRow(rowId) }
        }

        override fun discard() {
            release()
            runCatching { face.deleteRow(rowId) }
        }
    }

    companion object {
        /** fd 上 fstat 的尺寸对账(F10):statSize<0(ParcelFileDescriptor「不可知」)
         *  或与期望不符 → 不在场(安全方向:宁可重传,不可假完成)。 */
        fun fdSizeMatches(statSize: Long, expected: Long): Boolean = statSize >= 0 && statSize == expected

        /** 按扩展名猜 MIME;未知兜 application/octet-stream(getSingleton 用
         *  `?.`——实机恒非 null 行为不变,JVM 桩面不 NPE,单测可入 create)。 */
        fun mimeTypeOf(name: String): String {
            val ext = name.substringAfterLast('.', "").lowercase()
            if (ext.isEmpty()) return "application/octet-stream"
            return MimeTypeMap.getSingleton()?.getMimeTypeFromExtension(ext)
                ?: "application/octet-stream"
        }
    }
}
