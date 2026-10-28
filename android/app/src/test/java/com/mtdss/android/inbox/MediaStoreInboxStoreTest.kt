package com.mtdss.android.inbox

import android.os.ParcelFileDescriptor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.io.FileNotFoundException

/**
 * 发布决策矩阵 JVM 单测(F11 起 MediaStore 面收进 [MediaFace] 缝——
 * ContentResolver 的 query/insert/delete 在 API 37 已 final,不可 JVM 伪件;
 * 镜像 send 链 SafFace 口径):发布(尾杠落位 + IS_PENDING 包裹拷贝瞬间 +
 * 流拷贝 + 揭开)、撞名沿 policy 的全部分支、F14 自家 pending 行复用编排
 * 在此钉死;拷贝本体(真流)不可 JVM 伪,经构造缝注入伪拷贝钉发布编排,
 * 真实缺省面的失败路径以「桩面构造即抛」钉同一条清行收口;真拷贝面
 * (8 MiB + 等长对账)与 rwt 截断语义由实机验证。
 */
class MediaStoreInboxStoreTest {

    /** 伪拷贝缝:记录调用 / 注入失败;[events] 与伪面共享时序日志(钉
     *  「截断先于拷贝」的复用编排次序)。 */
    private class FakeCopy(private val events: MutableList<String>? = null) {
        val calls = mutableListOf<Triple<Long, String, Long>>()
        var failWith: RuntimeException? = null

        fun asCopier(): (Long, String, Long) -> Unit = { rowId, path, size ->
            failWith?.let { throw it }
            events?.add("copy@$rowId")
            calls += Triple(rowId, path, size)
        }
    }

    private fun logs() = mutableListOf<String>()

    private fun newStore(
        face: FakeMediaFace,
        copy: (Long, String, Long) -> Unit,
        log: MutableList<String>,
    ): MediaStoreInboxStore = MediaStoreInboxStore(face, { log += it }, copy)

    // ── 决策矩阵 ────────────────────────────────────────────────────────────

    @Test
    fun `发布成功 插入拷贝揭开`() {
        val face = FakeMediaFace()
        val copy = FakeCopy()
        val log = logs()
        val r = newStore(face, copy.asCopier(), log)
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        // 编排(现行插行路径):pending 预查(无 pending 行)→ 无落位行直插
        // → 拷贝(行 id/路径/尺寸原样)→ 揭开
        assertEquals(1, face.pendingQueries)
        assertEquals(1, face.inserted)
        assertEquals(listOf(Triple(100L, "/staging/small.txt", 2025L)), copy.calls)
        assertEquals(listOf(100L), face.revealedRows)
        assertTrue("行不得删", face.deletedRows.isEmpty())
    }

    @Test
    fun `撞名真文件在场 skip 命中弃件记 Skipped`() {
        val face = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = false // 真开 "r" 成功 = 真文件在场(非僵尸)
        }
        val copy = FakeCopy()
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy(skip = listOf("small.txt")))
        assertEquals(PublishResult.Skipped, r)
        assertEquals("在场行保留(弃件 = 尊重既有文件)", 0, face.deletedRows.size)
        assertEquals("不得插入", 0, face.inserted)
        assertEquals("不得拷贝", 0, copy.calls.size)
        assertEquals("不得揭开", 0, face.revealedRows.size)
    }

    @Test
    fun `撞名真文件在场 缺省策略(双名单未命中)驱逐后发布`() {
        // 缺省 overwrite(「落最新」):发布撞名是本端第二次撞——引擎侧
        // unique_target 改名已使首发免撞,此处撞上只可能是后出现的第三方件
        val face = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = false
        }
        val copy = FakeCopy()
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertEquals(listOf(42L), face.deletedRows) // 驱逐在场行
        assertEquals(1, face.inserted)
        assertEquals(1, copy.calls.size)
        assertEquals(listOf(100L), face.revealedRows)
    }

    @Test
    fun `撞名真文件在场 overwrite 命中驱逐后发布`() {
        val face = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = false
        }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy(overwrite = listOf("small.txt")))
        assertEquals(PublishResult.Published, r)
        assertEquals(listOf(42L), face.deletedRows)
        assertEquals(1, face.inserted)
    }

    @Test
    fun `F11 僵尸行验尸驱逐后发布`() {
        // MIUI 文件管理器裸删后的残留形态:行在、真开 "r" 必抛 → 驱逐再插
        val face = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = true
        }
        val copy = FakeCopy()
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertEquals(listOf(42L), face.deletedRows)
        assertEquals(1, face.inserted)
        assertEquals(1, copy.calls.size)
        assertEquals(listOf(100L), face.revealedRows)
    }

    @Test
    fun `F14 pending 行命中 复用截断拷贝揭开 不删行不重插`() {
        // 杀 App 于保存中 → 重发布:自家 pending 半行在场,复用而非删除
        // (MIUI 删行不删文件——删行孤儿半拷件、重插撞残件被改名 "(1)")。
        // 落位策略不经复用路:pending 半行是本 App 暂存残件非用户文件,skip
        // 名单不适用(仍 Published 而非 Skipped)。
        val face = FakeMediaFace().apply { pendingRowId = 77L }
        val copy = FakeCopy(face.events)
        val log = logs()
        val r = newStore(face, copy.asCopier(), log)
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy(skip = listOf("small.txt")))
        assertEquals(PublishResult.Published, r)
        // 复用编排:截断先于拷贝;拷入与揭开的都是被复用行(77)
        assertEquals(listOf("truncate@77", "copy@77"), face.events)
        assertEquals(listOf(Triple(77L, "/staging/small.txt", 2025L)), copy.calls)
        assertEquals(listOf(77L), face.revealedRows)
        // F14 铁面:不删行、不重插
        assertTrue("复用路径不得删行:${face.deletedRows}", face.deletedRows.isEmpty())
        assertEquals("复用路径不得重插", 0, face.inserted)
    }

    @Test
    fun `F14 复用截断失败 Failed 带因 行保留不拷贝`() {
        val face = FakeMediaFace().apply {
            pendingRowId = 77L
            failTruncateWith = RuntimeException("stale pending row: file gone")
        }
        val copy = FakeCopy(face.events)
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须点名复用失败:${failed.reason}", failed.reason.contains("pending 行复用失败"))
        assertTrue("词面须带底层因:${failed.reason}", failed.reason.contains("stale pending row"))
        assertTrue("截断失败不留成功事件,拷贝不启动:${face.events}", face.events.isEmpty())
        assertEquals("失败行保留(删行即 F14 病灶;republish 再来一轮复用)", 0, face.deletedRows.size)
        assertEquals(0, copy.calls.size)
        assertEquals(0, face.inserted)
        assertEquals(0, face.revealedRows.size)
    }

    @Test
    fun `F14 复用拷贝失败 行保留不删不重插`() {
        val face = FakeMediaFace().apply { pendingRowId = 77L }
        val copy = FakeCopy().apply { failWith = RuntimeException("EIO: 磁盘满") }
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须带底层因:${failed.reason}", failed.reason.contains("磁盘满"))
        assertEquals("删行即 F14 病灶(孤儿半拷件):复用失败行保留待重发", 0, face.deletedRows.size)
        assertEquals(0, face.inserted)
        assertEquals(0, face.revealedRows.size)
    }

    @Test
    fun `F14 复用揭开失败 行保留`() {
        val face = FakeMediaFace().apply {
            pendingRowId = 77L
            failRevealWith = RuntimeException("provider died")
        }
        val copy = FakeCopy()
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertTrue(r is PublishResult.Failed)
        assertEquals("复用失败行保留(对现行插行路的「揭开失败清行」刻意不对称)", 0, face.deletedRows.size)
        assertEquals(1, copy.calls.size)
        assertEquals(0, face.inserted)
    }

    @Test
    fun `pending 行预查异常 Failed 不盲走后续`() {
        // 预查不明时不得盲走落位预查/插入:presenceAt 不分 pending/done,
        // 自家半行可能被误判真文件遭驱逐,退回 F14 原样
        val face = FakeMediaFace().apply { failPendingQueryWith = RuntimeException("provider died") }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("pending 行预查失败"))
        assertEquals(0, face.inserted)
        assertTrue(face.deletedRows.isEmpty())
    }

    @Test
    fun `拷贝失败清 pending 行且不揭开`() {
        val face = FakeMediaFace()
        val copy = FakeCopy().apply { failWith = RuntimeException("EIO: 磁盘满") }
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须带底层因:${failed.reason}", failed.reason.contains("磁盘满"))
        assertEquals(1, face.inserted)
        assertEquals("半行必清(spec §4:IS_PENDING 包裹拷贝瞬间,失败即删)", listOf(100L), face.deletedRows)
        assertEquals(0, face.revealedRows.size)
    }

    @Test
    fun `真实缺省拷贝面失败 同走清行收口`() {
        // 真实缺省拷贝面在 JVM 桩面无真 fd(构造输出流即抛,先于源文件触碰)
        // ——钉住「缺省面失败与伪面失败同一条清行收口路径」;真流拷贝本体
        // (8 MiB + 等长对账)由实机验证。
        val missing = File("/nonexistent-mtdss-jvm/staging.bin").absolutePath
        val face = FakeMediaFace()
        val r = newStore(face, ::defaultCopierForTest, logs()).publish("a.bin", missing, 8, TransferPolicy())
        assertTrue(r is PublishResult.Failed)
        assertEquals("半行必清", listOf(100L), face.deletedRows)
        assertEquals(0, face.revealedRows.size)
    }

    @Test
    fun `插入失败带因 Failed 且不动行`() {
        val face = FakeMediaFace().apply { failInsertWith = RuntimeException("insert clash") }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("MediaStore 插入失败"))
        assertTrue(failed.reason.contains("RuntimeException: insert clash"))
        assertEquals(0, face.deletedRows.size)
        assertEquals(0, face.revealedRows.size)
    }

    @Test
    fun `落位行查询异常 Failed 不盲插`() {
        // done 行预查失败即收口失败(绝不盲删/盲插——插入必撞残留行,F11 设备场实证)
        val face = FakeMediaFace().apply { failDoneQueryWith = RuntimeException("provider died") }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("落位行查询失败"))
        assertEquals(0, face.inserted)
    }

    @Test
    fun `僵尸行驱逐失败收口 Failed 不插入`() {
        val face = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = true
            failDeleteWith = RuntimeException("provider died")
        }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须点名删除失败:${failed.reason}", failed.reason.contains("僵尸行删除失败"))
        assertEquals(0, face.inserted)
    }

    @Test
    fun `在场行驱逐失败收口 Failed 不插入`() {
        val face = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = false
            failDeleteWith = RuntimeException("provider died")
        }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须点名驱逐失败:${failed.reason}", failed.reason.contains("落位行驱逐失败"))
        assertEquals(0, face.inserted)
    }

    @Test
    fun `揭开失败清 pending 行收口 Failed`() {
        val face = FakeMediaFace().apply { failRevealWith = RuntimeException("provider died") }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertTrue(r is PublishResult.Failed)
        assertEquals("揭开失败的完整行亦清(暂存未动,重发 republish 再来)", listOf(100L), face.deletedRows)
    }

    @Test
    fun `非法 rel 拒发布 且不触面`() {
        val face = FakeMediaFace()
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("../bad", "/staging/x", 1, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("非法相对路径"))
        assertEquals(0, face.pendingQueries)
        assertEquals(0, face.inserted)
    }

    @Test
    fun `嵌套 rel 落位尾杠形态携带目录树`() {
        // 拷贝缝收到的只是行 id/路径;落位尾杠形态经 FakeMediaFace 的记录面钉
        val placements = mutableListOf<InboxPlacement.Placement>()
        val face = FakeMediaFace().apply { onInsert = { placements += it } }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("docs/sub/a.bin", "/staging/x", 1, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertEquals("Download/MTDSS/docs/sub/", placements.single().relativePath) // F12 尾杠
        assertEquals("a.bin", placements.single().displayName)
    }

    // ── 伪件(MediaFace 域概念,JVM 无框架依赖)─────────────────────────

    /** 缺省真实拷贝面的测试桥(JVM 桩面仅验「缺文件先抛」路径)。 */
    private fun defaultCopierForTest(rowId: Long, stagingPath: String, expectedSize: Long) {
        MediaStoreInboxStore.copyStagingInto(
            object : MediaFace {
                override fun rowIdAt(placement: InboxPlacement.Placement, pendingOnly: Boolean): Long? = null
                override fun insertPending(placement: InboxPlacement.Placement, mimeType: String): Long? = null
                override fun deleteRow(rowId: Long): Int = 0
                override fun truncateRow(rowId: Long) {}
                override fun openRow(rowId: Long, mode: String): ParcelFileDescriptor = FakePfd()
                override fun revealRow(rowId: Long) {}
            },
            rowId,
            stagingPath,
            expectedSize,
        ) { /* JVM 桩面静默 */ }
    }

    private class FakeMediaFace : MediaFace {
        /** 同落位 done(非 pending)行 id;null = 无行。 */
        var doneRowId: Long? = null

        /** 同落位 pending 行 id(F14 复用分支开关);null = 无 pending 行。 */
        var pendingRowId: Long? = null

        /** 真开 "r" 验尸:抛 = 僵尸(文件已亡);不抛 = 返回 fd(真文件)。 */
        var openRThrows = false
        var failPendingQueryWith: RuntimeException? = null
        var failDoneQueryWith: RuntimeException? = null
        var failTruncateWith: RuntimeException? = null
        var failDeleteWith: RuntimeException? = null
        var failInsertWith: RuntimeException? = null
        var failRevealWith: RuntimeException? = null

        /** insert 落位记录面(尾杠形态断言用)。 */
        var onInsert: ((InboxPlacement.Placement) -> Unit)? = null

        /** 面事件时序日志(F14 复用编排「截断先于拷贝」断言;拷贝缝同步追加)。 */
        val events = mutableListOf<String>()

        val deletedRows = mutableListOf<Long>()
        var pendingQueries = 0
        val truncatedRows = mutableListOf<Long>()
        var inserted = 0
        val revealedRows = mutableListOf<Long>()
        private val pfd = FakePfd()

        override fun rowIdAt(placement: InboxPlacement.Placement, pendingOnly: Boolean): Long? {
            (if (pendingOnly) failPendingQueryWith else failDoneQueryWith)?.let { throw it }
            return if (pendingOnly) {
                pendingQueries++
                pendingRowId
            } else {
                doneRowId
            }
        }

        override fun truncateRow(rowId: Long) {
            failTruncateWith?.let { throw it }
            truncatedRows += rowId
            events += "truncate@$rowId"
        }

        override fun deleteRow(rowId: Long): Int {
            failDeleteWith?.let { throw it }
            deletedRows += rowId
            doneRowId = null
            pendingRowId = null
            return 1
        }

        override fun insertPending(placement: InboxPlacement.Placement, mimeType: String): Long? {
            failInsertWith?.let { throw it }
            onInsert?.invoke(placement)
            inserted++
            return 100L
        }

        override fun openRow(rowId: Long, mode: String): ParcelFileDescriptor {
            if (mode == "r" && openRThrows) throw FileNotFoundException("stale row: no underlying file")
            return pfd // "rw"(发布拷贝)与 "r"(验尸)同 fd 语义
        }

        override fun revealRow(rowId: Long) {
            failRevealWith?.let { throw it }
            revealedRows += rowId
        }
    }

    /** 伪 fd:JVM 桩面下 close 无操作(真机 close 由 MediaProvider 走真 fd)。 */
    private class FakePfd : ParcelFileDescriptor(null) {
        override fun close() {}
    }
}
