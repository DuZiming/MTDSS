package com.mtdss.android.inbox

import android.os.ParcelFileDescriptor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.FileNotFoundException

/**
 * F10 fd 尺寸对账 + F11 僵尸 done 行验尸清理单测。F11 起 MediaStore 面收进
 * [MediaFace] 缝(ContentResolver 的 query/insert/delete 在 API 37 已 final,
 * 不可 JVM 伪件;镜像 send 链 SafFace 口径),create 的残留行三态处置由此
 * 在 JVM 钉死;真机 MediaProvider 行为(裸删行残留、insert 撞 done 行
 * 抛错/返 null)由设备场验证(F11)。
 */
class MediaStoreInboxStoreTest {

    @Test
    fun `fd 尺寸吻合才在场`() {
        assertTrue(MediaStoreInboxStore.fdSizeMatches(statSize = 2025, expected = 2025))
        // 尺寸不符(同名不同内容/残留行陈旧)→ 不在场,引擎 FULL 重收
        assertFalse(MediaStoreInboxStore.fdSizeMatches(statSize = 2024, expected = 2025))
    }

    @Test
    fun `fd 尺寸不可知按不在场`() {
        // statSize=-1(ParcelFileDescriptor「尺寸不可知」)→ 不在场
        // (失败方向铁律:宁可重传,不可假完成)
        assertFalse(MediaStoreInboxStore.fdSizeMatches(statSize = -1, expected = 2025))
    }

    @Test
    fun `F11 僵尸 done 行验尸后删除 插入成功`() {
        // MIUI 文件管理器裸删后的残留形态:行在、is_pending=0、真开 "r" 必抛
        val fake = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = true
        }
        val handle = MediaStoreInboxStore(fake).create("small.txt", 2025)
        assertTrue("僵尸行须验尸后删除", fake.zombieDeleted)
        assertEquals("残留行清除后插入恰一次", 1, fake.inserted)
        assertEquals("pending 预清理先行(F11 不改该流程)", 1, fake.pendingDeleted)
        handle.release() // 全新暂存可正常收尾(不抛)
    }

    @Test
    fun `F11 真文件在场的 done 行不删 插入失败带因上浮碰撞词面`() {
        val fake = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = false // 真开 "r" 成功 = 实文件在场(非僵尸)
            failInsertWith = RuntimeException("insert clash")
        }
        val e = tryInboxStoreException { MediaStoreInboxStore(fake).create("small.txt", 2025) }
        assertNotNull("插入冲突须抛 InboxStoreException(0x02 拒开口径)", e)
        assertFalse("只删验明僵尸的行,真文件行不得删", fake.zombieDeleted)
        assertTrue("词面须点名碰撞:${e!!.message}", e.message!!.contains("同落位终件在场冲突"))
        assertTrue("词面须带底层因:${e.message}", e.message!!.contains("RuntimeException: insert clash"))
        assertSame("cause 须挂底层异常(带因上浮纪律)", fake.failInsertWith, e.cause)
    }

    @Test
    fun `F11 僵尸行删除失败收口异常 CREATE_FAILED 不逸出`() {
        val fake = FakeMediaFace().apply {
            doneRowId = 42L
            openRThrows = true
            failDeleteWith = RuntimeException("provider died")
        }
        val store = MediaStoreInboxStore(fake)
        val e = tryInboxStoreException { store.create("small.txt", 2025) }
        assertNotNull(e)
        assertTrue("词面须点名删除失败:${e!!.message}", e.message!!.contains("僵尸行删除失败"))
        assertTrue("词面须带底层因:${e.message}", e.message!!.contains("provider died"))
        assertSame(fake.failDeleteWith, e.cause)
        assertEquals("插入未发生", 0, fake.inserted)
        // 不逸出:InboxLogic 收口为 CREATE_FAILED 拒开(provider 协程存活)
        val refuse = InboxLogic(store).onRequest(requestId = 11, rel = "small.txt", size = 2025, resume = false)
                as InboxDecision.Refuse
        assertTrue(refuse.reason.startsWith("CREATE_FAILED"))
        assertTrue(refuse.reason.contains("provider died"))
    }

    // ── 伪件(MediaFace 域概念,JVM 无框架依赖)─────────────────────────

    private fun tryInboxStoreException(f: () -> Unit): InboxStoreException? =
        try {
            f(); null
        } catch (e: InboxStoreException) {
            e
        }

    /** 伪 MediaFace:驱动 create 残留三态(rowIdAt 只应答 done 行——pending
     *  行由 deletePendingAt 语义性清除,不再设行)。 */
    private class FakeMediaFace : MediaFace {
        /** 同落位 done(非 pending)行 id;null = 无行。 */
        var doneRowId: Long? = null

        /** 真开 "r" 验尸:抛 = 僵尸(文件已亡);false = 返回 fd(真文件)。 */
        var openRThrows = false
        var failDeleteWith: RuntimeException? = null
        var failInsertWith: RuntimeException? = null
        var zombieDeleted = false
        var pendingDeleted = 0
        var inserted = 0
        private val pfd = FakePfd()

        override fun rowIdAt(placement: InboxPlacement.Placement, pendingOnly: Boolean): Long? =
            if (pendingOnly) null else doneRowId

        override fun deletePendingAt(placement: InboxPlacement.Placement) {
            pendingDeleted++
        }

        override fun deleteRow(rowId: Long): Int {
            failDeleteWith?.let { throw it }
            zombieDeleted = true
            doneRowId = null
            return 1
        }

        override fun insertPending(placement: InboxPlacement.Placement, mimeType: String): Long? {
            failInsertWith?.let { throw it }
            inserted++
            return 100L
        }

        override fun openRow(rowId: Long, mode: String): ParcelFileDescriptor {
            if (mode == "r" && openRThrows) throw FileNotFoundException("stale row: no underlying file")
            return pfd // "rw"(暂存)与 "r"(真文件验尸)同 fd 语义
        }

        override fun revealRow(rowId: Long) {}
    }

    /** 伪 fd:JVM 桩面下 close 无操作(真机 close 由 MediaProvider 走真 fd)。 */
    private class FakePfd : ParcelFileDescriptor(null) {
        override fun close() {}
    }
}
