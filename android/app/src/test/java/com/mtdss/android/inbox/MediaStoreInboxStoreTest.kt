package com.mtdss.android.inbox

import android.os.ParcelFileDescriptor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.io.FileNotFoundException

/**
 * 发布决策矩阵 JVM 单测(F11 起 MediaStore 面收进 [MediaFace] 缝——
 * ContentResolver 的 query/insert/delete 在 API 37 已 final,不可 JVM 伪件;
 * 镜像 send 链 SafFace 口径):发布(点文件临时名插入 + IS_PENDING 包裹拷贝
 * 瞬间 + 流拷贝 + 揭开 + 完成改正名 + 回查碰撞甄别,F14b 编排)、撞名沿
 * policy 的全部分支在此钉死;拷贝本体(真流)不可 JVM 伪,经构造缝注入伪
 * 拷贝钉发布编排,真实缺省面的失败路径以「桩面构造即抛」钉同一条清行收口;
 * 真拷贝面(8 MiB + 等长对账)、update 改名与回查的真实 provider 行为
 * (含 "(N)" 自动改名碰撞)由实机验证。
 */
class MediaStoreInboxStoreTest {

    /** 伪拷贝缝:记录调用 / 注入失败;[events] 与伪面共享时序日志(钉
     *  「插入→拷贝→揭开→改名」的 F14b 编排次序)。 */
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

    // ── F14b 编排(点文件临时名 + 完成改正名 + 碰撞回查)────────────────────

    @Test
    fun `发布成功 临时行拷贝揭开改正名回查终名`() {
        val face = FakeMediaFace()
        val copy = FakeCopy(face.events)
        val log = logs()
        val r = newStore(face, copy.asCopier(), log)
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        // 编排(F14b):插入临时点文件行 → 拷贝(行 id/路径/尺寸原样)→
        // 揭开 → 完成改正名(终名)
        assertEquals(listOf("insert@100", "copy@100", "reveal@100", "rename@100"), face.events)
        assertEquals(1, face.inserted)
        assertEquals("插入恒走点文件临时名(终名完成时才落)", 1, face.insertedNames.size)
        assertTrue(face.insertedNames.single().matches(TEMP_NAME_SHAPE))
        assertEquals(listOf(Triple(100L, "/staging/small.txt", 2025L)), copy.calls)
        assertEquals(listOf(100L), face.revealedRows)
        assertEquals("改正名落请求终名", listOf(100L to "small.txt"), face.renamedRows)
        assertTrue("成功路不得删行", face.deletedRows.isEmpty())
    }

    @Test
    fun `F14b 改名碰撞 回查实际名 Failed 行保留`() {
        // 终名路径被无行孤儿文件占用(F14 现场第二形:属主进程死亡回收
        // pending 行但留文件):provider update 不报错而自动改 "(1)"——
        // 回查实际名 ≠ 请求名。内容已完整落盘于实际名:诚实告知词面 +
        // 行保留(删行留更糟的无行孤儿)。
        val face = FakeMediaFace().apply { renameCollisionTo = "small (1).txt" }
        val copy = FakeCopy()
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须点名碰撞:${failed.reason}", failed.reason.contains("目标名被残留文件占用"))
        assertTrue("词面须带实际名:${failed.reason}", failed.reason.contains("small (1).txt"))
        assertTrue("词面须告知手动改名出口:${failed.reason}", failed.reason.contains("文件管理器手动改名"))
        assertEquals("拷贝/揭开已完成(内容安全落盘)", 1, copy.calls.size)
        assertEquals(listOf(100L), face.revealedRows)
        assertEquals("碰撞行保留(内容挂实际名,删行即无行孤儿)", 0, face.deletedRows.size)
    }

    @Test
    fun `临时名恒点文件 uuid 形态且每次新`() {
        val shapes = mutableListOf<String>()
        repeat(2) {
            val face = FakeMediaFace()
            newStore(face, FakeCopy().asCopier(), logs())
                .publish("a.bin", "/staging/a.bin", 8, TransferPolicy())
            shapes += face.insertedNames.single()
        }
        shapes.forEach {
            assertTrue("临时名须 dot+8hex+.tmp 形态:$it", it.matches(TEMP_NAME_SHAPE))
        }
        assertNotEquals("uuid 每次新(残留件恒异名,不成碰撞源)", shapes[0], shapes[1])
    }

    // ── 决策矩阵 ────────────────────────────────────────────────────────────

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
    fun `拷贝失败清临时行且不揭开`() {
        // 活进程内拷贝失败:清临时行(F14b——临时行是随机点文件名,MIUI
        // 删行不删文件的残留至多不可见点文件,不占终名路径不成碰撞源)
        val face = FakeMediaFace()
        val copy = FakeCopy().apply { failWith = RuntimeException("EIO: 磁盘满") }
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须带底层因:${failed.reason}", failed.reason.contains("磁盘满"))
        assertEquals(1, face.inserted)
        assertEquals("被清的是刚插入的临时行", listOf(100L), face.deletedRows)
        assertEquals(0, face.revealedRows.size)
        assertEquals("失败不得落终名", 0, face.renamedRows.size)
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
        assertEquals("临时行必清", listOf(100L), face.deletedRows)
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
    fun `揭开失败清临时行收口 Failed`() {
        val face = FakeMediaFace().apply { failRevealWith = RuntimeException("provider died") }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertTrue(r is PublishResult.Failed)
        assertEquals("揭开失败的完整临时行亦清(暂存未动,重发 republish 再来)", listOf(100L), face.deletedRows)
        assertEquals("失败不得落终名", 0, face.renamedRows.size)
    }

    @Test
    fun `改名失败清临时行收口 Failed`() {
        // 改名义失败时行仍在临时点文件名:清行至多残留不可见点文件
        val face = FakeMediaFace().apply { failRenameWith = RuntimeException("provider died") }
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("发布改名失败"))
        assertEquals(listOf(100L), face.deletedRows)
        assertEquals(1, face.revealedRows.size)
    }

    @Test
    fun `回查异常 行保留收口 Failed`() {
        // 改名义已成后回查抛:行状态不明,删行恐成无行孤儿——行保留诚实收口
        val face = FakeMediaFace().apply { failNameQueryWith = RuntimeException("provider died") }
        val copy = FakeCopy()
        val r = newStore(face, copy.asCopier(), logs())
            .publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("发布回查失败"))
        assertEquals("回查不明行保留(删行即无行孤儿)", 0, face.deletedRows.size)
        assertEquals("拷贝/揭开已走到(失败在末段回查)", 1, copy.calls.size)
        assertEquals(listOf(100L), face.revealedRows)
    }

    @Test
    fun `非法 rel 拒发布 且不触面`() {
        val face = FakeMediaFace()
        val r = newStore(face, FakeCopy().asCopier(), logs())
            .publish("../bad", "/staging/x", 1, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("非法相对路径"))
        assertEquals(0, face.inserted)
        assertTrue(face.deletedRows.isEmpty())
    }

    @Test
    fun `嵌套 rel 落位尾杠形态携带目录树`() {
        // 拷贝缝收到的只是行 id/路径;落位尾杠形态经 FakeMediaFace 的记录面钉
        val placements = mutableListOf<InboxPlacement.Placement>()
        val face = FakeMediaFace().apply { onInsert = { placement, _, _ -> placements += placement } }
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
                override fun rowIdAt(placement: InboxPlacement.Placement): Long? = null
                override fun insertPending(
                    placement: InboxPlacement.Placement,
                    displayName: String,
                    mimeType: String,
                ): Long? = null
                override fun deleteRow(rowId: Long): Int = 0
                override fun renameRow(rowId: Long, displayName: String) {}
                override fun displayNameOf(rowId: Long): String? = null
                override fun openRow(rowId: Long, mode: String): ParcelFileDescriptor = FakePfd()
                override fun revealRow(rowId: Long) {}
            },
            rowId,
            stagingPath,
            expectedSize,
        ) { /* JVM 桩面静默 */ }
    }

    private class FakeMediaFace : MediaFace {
        /** 同落位行 id(发布预查/在场查询);null = 无行。 */
        var doneRowId: Long? = null

        /** 真开 "r" 验尸:抛 = 僵尸(文件已亡);不抛 = 返回 fd(真文件)。 */
        var openRThrows = false
        var failDoneQueryWith: RuntimeException? = null
        var failDeleteWith: RuntimeException? = null
        var failInsertWith: RuntimeException? = null
        var failRevealWith: RuntimeException? = null
        var failRenameWith: RuntimeException? = null
        var failNameQueryWith: RuntimeException? = null

        /** 碰撞模拟(null = 改名成功落请求名):renameRow 后 provider 因终名
         *  路径被无行孤儿文件占用而自动改名的形态("small (1).txt")——
         *  update 不报错,碰撞只经回查 [displayNameOf] 甄别(实机行为)。 */
        var renameCollisionTo: String? = null

        /** insert 落位记录面(尾杠形态断言用;临时名/MIME 随参透出)。 */
        var onInsert: ((InboxPlacement.Placement, String, String) -> Unit)? = null

        /** 面事件时序日志(F14b 编排「插入→拷贝→揭开→改名」断言;拷贝缝同步追加)。 */
        val events = mutableListOf<String>()

        val deletedRows = mutableListOf<Long>()
        var inserted = 0
        val insertedNames = mutableListOf<String>()
        val revealedRows = mutableListOf<Long>()
        val renamedRows = mutableListOf<Pair<Long, String>>()
        private val rowNames = mutableMapOf<Long, String>()
        private val pfd = FakePfd()

        override fun rowIdAt(placement: InboxPlacement.Placement): Long? {
            failDoneQueryWith?.let { throw it }
            return doneRowId
        }

        override fun insertPending(
            placement: InboxPlacement.Placement,
            displayName: String,
            mimeType: String,
        ): Long? {
            failInsertWith?.let { throw it }
            onInsert?.invoke(placement, displayName, mimeType)
            inserted++
            insertedNames += displayName
            events += "insert@100"
            rowNames[100L] = displayName
            return 100L
        }

        override fun deleteRow(rowId: Long): Int {
            failDeleteWith?.let { throw it }
            deletedRows += rowId
            doneRowId = null
            rowNames.remove(rowId)
            return 1
        }

        override fun renameRow(rowId: Long, displayName: String) {
            failRenameWith?.let { throw it }
            renamedRows += rowId to displayName
            rowNames[rowId] = renameCollisionTo ?: displayName
            events += "rename@$rowId"
        }

        override fun displayNameOf(rowId: Long): String? {
            failNameQueryWith?.let { throw it }
            return rowNames[rowId]
        }

        override fun openRow(rowId: Long, mode: String): ParcelFileDescriptor {
            if (mode == "r" && openRThrows) throw FileNotFoundException("stale row: no underlying file")
            return pfd // "rw"(发布拷贝)与 "r"(验尸)同 fd 语义
        }

        override fun revealRow(rowId: Long) {
            failRevealWith?.let { throw it }
            revealedRows += rowId
            events += "reveal@$rowId"
        }
    }

    /** 伪 fd:JVM 桩面下 close 无操作(真机 close 由 MediaProvider 走真 fd)。 */
    private class FakePfd : ParcelFileDescriptor(null) {
        override fun close() {}
    }

    companion object {
        /** F14b 点文件临时名形态:dot 开头 + `.mtdss-pub-` + 8 位 hex + `.tmp`。 */
        private val TEMP_NAME_SHAPE = Regex("""^\.mtdss-pub-[0-9a-f]{8}\.tmp$""")
    }
}
