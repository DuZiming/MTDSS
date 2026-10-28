package com.mtdss.android.inbox

import android.content.ContentResolver
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.util.Log
import android.webkit.MimeTypeMap
import java.io.FileInputStream
import java.io.FileNotFoundException

/**
 * MediaStore 收件面的薄操作缝(收件箱侧唯一 Android 框界面,口径镜像
 * send 链的 SafFace/ContentResolverSafFace):行定位/增删/真开 fd/截断/揭开。
 * 决策逻辑([MediaStoreInboxStore] 的发布编排、落位行预查)只依赖本抽象,
 * JVM 单测注入伪件钉死;真实 MediaProvider 行为由实机验证(F11:MIUI 裸删
 * 后残留 done 行、insert 撞行抛错/返 null 不自动改名;F14:删行不删文件,
 * 半拷件残留公开收件箱)。
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

    /** 定点删行(F11 僵尸验尸清理 / 发布驱逐 / 拷贝失败清半行)→ 删除行数
     *  (0 = provider 拒删未抛——F11 现场仪表化观测点)。 */
    fun deleteRow(rowId: Long): Int

    /** 截断行文件为 0 长(F14 复用原语;实机以 "rwt" 模式开 = rw + O_TRUNC,
     *  开完即关,拷贝阶段另行 "rw" 开)。stale 行(行存文件亡)必抛
     *  FileNotFoundException。 */
    fun truncateRow(rowId: Long)

    /** 真开行 fd(mode "r" = 验尸/对账,"rw" = 发布拷贝写入;不截断);
     *  stale 行(裸删后行残留)必抛 FileNotFoundException。 */
    fun openRow(rowId: Long, mode: String): ParcelFileDescriptor

    /** 揭开行(IS_PENDING=0,系统文件管理器可见;发布收尾)。 */
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

    override fun deleteRow(rowId: Long): Int = resolver.delete(uriOf(rowId), null, null)

    override fun truncateRow(rowId: Long) {
        // "rwt" = rw + truncate:openFileDescriptor 层面的 O_TRUNC(设计允许
        // 「截断原语以 rwt 模式开」),close 即完成截断;行归本 App 所有
        // (MTDSS 子树),owner 对 pending 行可写。stale 行 open 必抛
        // FileNotFoundException → publish 带因收口。
        resolver.openFileDescriptor(uriOf(rowId), "rwt")?.close()
            ?: throw FileNotFoundException("openFileDescriptor(rwt) 返回 null")
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
 * 收件箱发布的 MediaStore 面(P4a 磁盘侧;实现保持薄——发布编排决策在本
 * 类 JVM 可验,落位映射在 [InboxPlacement],Android 存储行为收进
 * [MediaFace]/[ContentResolverMediaFace] 缝后,实机验证):
 *
 * - **落位**:MediaStore.Downloads 集合,rel 的段映射进多段
 *   `RELATIVE_PATH = Download/MTDSS/<dir…>/`(**恒带尾 '/',对齐
 *   MediaProvider 归一化——F12:不带则一切等值查询落空,插得进、查不见)
 *   + 末段 DISPLAY_NAME(映射与段消毒见 [InboxPlacement],JVM 钉死);
 * - **发布编排**(spec §3.2/§6.4,决策矩阵 JVM 钉死):
 *   ①同落位 pending 行预查(F14 复用机器,见下)→ ②落位行预查(F11 机器:
 *   僵尸验尸驱逐 / 真文件在场沿 [TransferPolicy]:skip 命中弃件记 Skipped,
 *   其余驱逐+插入)→ ③insert(IS_PENDING 包裹拷贝瞬间)→ ④流拷贝(8 MiB
 *   缓冲 + 实拷等长对账)→ ⑤揭开;④⑤任一失败 **清 pending 半行**再收
 *   Failed(spec §4:发布自身的原子性沿用 IS_PENDING 包裹,半行不残留;
 *   暂存未动,daemon ok:false → 保存失败,重发走 republish 零网络重发布);
 * - **F14 自家 pending 行复用**(杀 App 于保存中 → 重发布路径):发布①
 *   预查命中同落位 pending 半行时 **复用而非删除**——截断归零半拷件后
 *   直接拷入该行揭开,不删行不重插。案由(设备实证,MIUI):删行 **不删
 *   文件**,半拷件(GB 级)残留公开收件箱成用户可见垃圾;重插同路径撞
 *   残件,MediaProvider 自动改名 "(1)"——「孤儿 + 改名」双双回归。落位
 *   键命中即自家(MTDSS 子树只有本 App 写 MTDSS pending 行),复用天然
 *   免疫;复用路径任何失败一律 **保留行** 带因收口 Failed(删行即 F14
 *   病灶;行保留则 republish 再来一轮复用+截断,自愈);
 * - **IS_PENDING 生命周期**:发布内部不再承载传输中间态(P3 形态已退役),
 *   只包裹拷贝瞬间——插入即 pending(对其他 App 不可见),揭开即可见;
 * - **在场判据(F10)**:exists 不信行数据——行查询仅用于定位,真开
 *   fd("r")+ statSize 对账才是终判据(MIUI 文件管理器裸删后行残留、SIZE
 *   字段照旧,行查询会撒谎;stale 行的 open 必抛 → 不在场 → daemon FULL)。
 */
class MediaStoreInboxStore(
    private val face: MediaFace,
    /** 仪表化旁路(F11 现场核查:预查/驱逐/拷贝各分支各打一行;tag 与
     *  InboxProvider 同——logcat 单一过滤面;JVM 桩面 Log.i 无操作)。 */
    private val log: (String) -> Unit = { Log.i("InboxProvider", it) },
    /**
     * 拷贝面注入缝:实机缺省 = [copyStagingInto](暂存终件 → pending 行 fd
     * 的 8 MiB 流拷贝 + 等长对账);JVM 桩面无真 fd,单测注入伪拷贝钉死
     * 发布编排。抛异常 = 拷贝失败(publish 统一清 pending 半行收口)。
     */
    private val copyIntoRow: (rowId: Long, stagingPath: String, expectedSize: Long) -> Unit = { rowId, path, size ->
        copyStagingInto(face, rowId, path, size, log)
    },
) : InboxStore {

    /** 实机面直通(注入缝口径同 ContentResolverSafFace;单测注入伪 MediaFace)。 */
    constructor(context: Context) : this(ContentResolverMediaFace(context.contentResolver))

    // ── 在场查询(fs.query_dest/0x04,保留面)──────────────────────────────

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
            log("在场:查询异常 rel=$rel (${e.javaClass.simpleName}: ${e.message})")
            throw InboxStoreException("在场查询失败(rel=$rel): ${e.message}", e)
        }
        if (id == null) {
            log("在场:无行 rel=$rel")
            return false
        }
        return try {
            face.openRow(id, "r").use { pfd ->
                // 判据分叉点:开抛 vs 开成功+尺寸不符——F11 现场须分清
                val stat = pfd.statSize
                val hit = fdSizeMatches(stat, size)
                log("在场:${if (hit) "命中" else "尺寸不符"} rel=$rel id=$id stat=$stat want=$size")
                hit
            }
        } catch (e: Exception) {
            // open/stat 的任何异常(stale 行 FileNotFoundException、Security 等)
            // 都是「不在场」的证据而非查询故障:按 false 收口,宁可重传。
            log("在场:真开抛 rel=$rel id=$id (${e.javaClass.simpleName}: ${e.message})")
            false
        }
    }

    // ── 发布(fs.publish/0x05,新增面)──────────────────────────────────────

    override fun publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult {
        // 消毒不过拒发布(与 daemon validate_rel_path 同口径的第二道防御)
        val placement = InboxPlacement.place(rel)
            ?: return PublishResult.Failed("非法相对路径(拒发布): $rel")
        // ① 自家 pending 行预查(F14 复用):落位键命中即自家——MTDSS 子树
        //    只有本 App 写 MTDSS pending 行(发布中死 App 的半行/旧版残留)。
        //    命中 → 复用该行(截断归零半拷件后拷入揭开),绝不 deleteRow:
        //    MIUI 删行不删文件,半拷件残留公开收件箱 + 重插撞残件被 provider
        //    改名 "(1)"(F14 设备实证)。查询异常带因收口——预查不明时不得
        //    盲走后续(presenceAt 不分 pending/done,自家半行可能被误判真
        //    文件遭驱逐,退回 F14 原样)。
        val pendingId = try {
            face.rowIdAt(placement, pendingOnly = true)
        } catch (e: Exception) {
            log("发布:pending 预查异常 rel=$rel (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("pending 行预查失败(rel=$rel): ${e.message}")
        }
        if (pendingId != null) return republishIntoPending(pendingId, rel, stagingPath, size)
        // ② 落位行预查(F11 机器复用,零改动):僵尸驱逐 / 真文件在场沿 policy
        when (val clash = presenceAt(placement, rel)) {
            is Presence.Failed -> return PublishResult.Failed(clash.reason)

            is Presence.RealFile -> {
                if (policy.skips(rel)) {
                    // skip 命中弃件:既有文件保留,暂存由 daemon 收尾清理
                    // (0x05 ok:true);记录打标后 final_state 记录门+0x04
                    // 查询照常闭环(§6.2/§6.4)
                    log("发布:skip 命中弃件 rel=$rel id=${clash.rowId}")
                    return PublishResult.Skipped
                }
                // 驱逐 + 插入(缺省 overwrite「落最新」/overwrite 命中同路)
                val deleted = try {
                    face.deleteRow(clash.rowId)
                } catch (e: Exception) {
                    return PublishResult.Failed("落位行驱逐失败(rel=$rel): ${e.message}")
                }
                log("发布:驱逐在场行 rel=$rel id=${clash.rowId} 删行数=$deleted")
            }

            Presence.None -> Unit
        }
        // ③ 插入(IS_PENDING 包裹拷贝瞬间;插入失败无行可清)
        val rowId = try {
            face.insertPending(placement, mimeTypeOf(placement.displayName))
        } catch (e: Exception) {
            return PublishResult.Failed("MediaStore 插入失败: $rel (${e.javaClass.simpleName}: ${e.message})")
        } ?: return PublishResult.Failed("MediaStore 插入失败: $rel (insert 返回 null)")
        // ④ 流拷贝(8 MiB + 等长对账;失败清半行)
        try {
            copyIntoRow(rowId, stagingPath, size)
        } catch (e: Exception) {
            val cleaned = runCatching { face.deleteRow(rowId) }.getOrDefault(-1)
            log("发布:拷贝失败清行 rel=$rel id=$rowId 删行数=$cleaned (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("发布拷贝失败(rel=$rel): ${e.message}")
        }
        // ⑤ 揭开(失败亦清行:完整半行不残留,重发 republish 再来)
        try {
            face.revealRow(rowId)
        } catch (e: Exception) {
            val cleaned = runCatching { face.deleteRow(rowId) }.getOrDefault(-1)
            log("发布:揭开失败清行 rel=$rel id=$rowId 删行数=$cleaned (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("发布揭开失败(rel=$rel): ${e.message}")
        }
        log("发布:完成 rel=$rel id=$rowId size=$size")
        return PublishResult.Published
    }

    /**
     * 复用自家 pending 行的发布(F14):截断归零半拷件 → 拷入该行 → 揭开,
     * 全程不 deleteRow 不 insertPending——文件不孤儿、路径不空、无改名。
     * 落位策略([TransferPolicy])不经此路:自家 pending 半行是本 App 的
     * 暂存残件而非用户文件,skip 名单不适用。任何失败一律 **保留行** 带因
     * 收口 [PublishResult.Failed](删行正是 F14 病灶:MIUI 删行不删文件;
     * 行保留则 republish 再来一轮复用+截断,自愈),暂存不动(daemon
     * ok:false → 保存失败,重发零网络重发布)。
     */
    private fun republishIntoPending(rowId: Long, rel: String, stagingPath: String, size: Long): PublishResult {
        // 打开行 fd 并截断(实机 "rwt" 开):半拷件归零。失败 = 复用打开
        // 失败(stale pending 行 FileNotFoundException 等),行保留带因收口。
        try {
            face.truncateRow(rowId)
        } catch (e: Exception) {
            log("发布:复用截断失败 行保留 rel=$rel id=$rowId (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("pending 行复用失败(rel=$rel): ${e.message}")
        }
        log("发布:复用 pending 行 rel=$rel id=$rowId 已截断")
        try {
            copyIntoRow(rowId, stagingPath, size)
        } catch (e: Exception) {
            log("发布:复用拷贝失败 行保留 rel=$rel id=$rowId (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("发布拷贝失败(rel=$rel): ${e.message}")
        }
        try {
            face.revealRow(rowId)
        } catch (e: Exception) {
            log("发布:复用揭开失败 行保留 rel=$rel id=$rowId (${e.javaClass.simpleName}: ${e.message})")
            return PublishResult.Failed("发布揭开失败(rel=$rel): ${e.message}")
        }
        log("发布:完成(复用 pending 行) rel=$rel id=$rowId size=$size")
        return PublishResult.Published
    }

    /** 落位行预查结果(发布撞名三分支)。 */
    private sealed interface Presence {
        /** 无行/僵尸已驱逐:干净插入路径。 */
        object None : Presence

        /** 真文件在场的 done 行(驱逐决策归 publish 沿 policy)。 */
        data class RealFile(val rowId: Long) : Presence

        /** 查询/驱逐失败(带因收口,绝不盲删盲插)。 */
        data class Failed(val reason: String) : Presence
    }

    /**
     * 落位 done 行预查(F11 验尸原语,与 exists/F10 同源):无行 →
     * [Presence.None];真开 "r" 抛 = 僵尸(裸删后行存文件亡)→ 删行(只删
     * **验明**僵尸的行,查询失败绝不盲删;本落位归属本 App 的收件箱子树,
     * 删除无责任问题)后 [Presence.None];真开成功 = 真文件在场 →
     * [Presence.RealFile]。查询异常 → [Presence.Failed](不盲插——插入必撞
     * 残留行退回 F11 卡死原样)。
     */
    private fun presenceAt(placement: InboxPlacement.Placement, rel: String): Presence {
        val id = try {
            face.rowIdAt(placement, pendingOnly = false)
        } catch (e: Exception) {
            log("发布:落位查询异常 rel=$rel (${e.javaClass.simpleName}: ${e.message})")
            return Presence.Failed("落位行查询失败(rel=$rel): ${e.message}")
        } ?: return Presence.None
        return try {
            face.openRow(id, "r").use { pfd ->
                // 真开成功 ≠ 真文件存疑清零:stat 一并打点(F11 现场分叉)
                log("发布:真文件在场 rel=$rel id=$id stat=${pfd.statSize}")
            }
            Presence.RealFile(id)
        } catch (e: Exception) {
            // 验明僵尸(文件已亡):删行后再走插入。删除失败带因收口——
            // 静默则 insert 撞同一条残留行,词面退回 F11 原样。
            val deleted = try {
                face.deleteRow(id)
            } catch (de: Exception) {
                log("发布:僵尸删除异常 rel=$rel id=$id (${de.javaClass.simpleName}: ${de.message})")
                return Presence.Failed("僵尸行删除失败(rel=$rel): ${de.message}")
            }
            log("发布:僵尸已驱逐 rel=$rel id=$id 开尸因=${e.javaClass.simpleName} 删行数=$deleted")
            Presence.None
        }
    }

    companion object {

        /** 发布拷贝缓冲:8 MiB(终件可达 GB 级,大缓冲降拷贝循环开销)。 */
        private const val COPY_BUFFER_BYTES: Int = 8 * 1024 * 1024

        /** fd 上 fstat 的尺寸对账(F10):statSize<0(ParcelFileDescriptor「不可知」)
         *  或与期望不符 → 不在场(安全方向:宁可重传,不可假完成)。 */
        fun fdSizeMatches(statSize: Long, expected: Long): Boolean = statSize >= 0 && statSize == expected

        /** 按扩展名猜 MIME;未知兜 application/octet-stream(getSingleton 用
         *  `?.`——实机恒非 null 行为不变,JVM 桩面不 NPE)。 */
        fun mimeTypeOf(name: String): String {
            val ext = name.substringAfterLast('.', "").lowercase()
            if (ext.isEmpty()) return "application/octet-stream"
            return MimeTypeMap.getSingleton()?.getMimeTypeFromExtension(ext)
                ?: "application/octet-stream"
        }

        /**
         * 发布拷贝(实机缺省拷贝面):暂存终件 → pending 行 fd 的流拷贝
         * (8 MiB 缓冲),实拷字节数对账 [expectedSize](暂存件已过 finalize
         * 哈希,拷贝即等长;不符 = 读到残件,按失败收口防假发布)。行 fd
         * 生命周期交 AutoCloseOutputStream 托管(use 关流即关行 fd,恰一次);
         * 暂存源独立 use——源缺席/不可读时 FileInputStream 构造先抛(JVM 单测
         * 由此钉住「拷贝失败清行」的真实路径)。任何失败抛异常(publish 统一
         * 清 pending 半行收口)。
         */
        fun copyStagingInto(
            face: MediaFace,
            rowId: Long,
            stagingPath: String,
            expectedSize: Long,
            log: (String) -> Unit = {},
        ) {
            ParcelFileDescriptor.AutoCloseOutputStream(face.openRow(rowId, "rw")).use { out ->
                FileInputStream(stagingPath).use { input ->
                    var copied = 0L
                    val buf = ByteArray(COPY_BUFFER_BYTES)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        copied += n
                    }
                    if (copied != expectedSize) {
                        throw java.io.IOException("拷贝字节数不符:实拷 $copied 预期 $expectedSize")
                    }
                    log("发布:拷贝完成 id=$rowId 字节=$copied")
                }
            }
        }
    }
}
