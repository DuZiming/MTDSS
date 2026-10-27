package com.mtdss.android.inbox

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.FileDescriptor

/**
 * InboxLogic 决策核心 JVM 单测(伪 InboxStore):请求分诊(全新 create /
 * 续传 reopen 不截断 / 未命中拒开 NOT_FOUND)、终态收尾(reveal/discard)、
 * 在途表路由与未知 id 容忍——T6 协议的可验决策面全在此,MediaStore 面
 * 由实机验证(MediaStoreInboxStore 保持薄)。
 */
class InboxLogicTest {

    private class FakeHandle(val rel: String) : InboxHandle {
        override val rawFd = FileDescriptor()
        var released = 0
        var revealed = 0
        var discarded = 0

        override fun release() {
            released++
        }

        override fun reveal() {
            revealed++
        }

        override fun discard() {
            discarded++
        }
    }

    private class FakeStore : InboxStore {
        val created = mutableListOf<Pair<String, Long>>()
        val reopened = mutableListOf<Pair<String, Long>>()

        /** reopen 命中面:rel → 在途暂存(取出即消耗,镜像 MediaStore 单条查询)。 */
        val pendingForReopen = linkedMapOf<String, FakeHandle>()

        /** create 抛出的消息(非 null 即失败)。 */
        var failCreate: String? = null

        override fun create(rel: String, size: Long): FakeHandle {
            failCreate?.let { throw InboxStoreException(it) }
            created += rel to size
            return FakeHandle(rel)
        }

        override fun reopen(rel: String, size: Long): FakeHandle? {
            reopened += rel to size
            return pendingForReopen.remove(rel)
        }

        /** exists 应答面:presentFor 定行在;failExistsFor 触发异常路径。 */
        val existsCalls = mutableListOf<Pair<String, Long>>()
        var presentFor: (String) -> Boolean = { false }
        var failExistsFor: String? = null
        override fun exists(rel: String, size: Long): Boolean {
            failExistsFor?.takeIf { it == rel }?.let { throw InboxStoreException(it) }
            existsCalls += rel to size
            return presentFor(rel)
        }
    }

    @Test
    fun `全新收走 create 且 commit done 揭开`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        val d = logic.onRequest(requestId = 1, rel = "甲.bin", size = 100, resume = false)
        val handle = ((d as InboxDecision.Answer).handle as FakeHandle)
        assertEquals(listOf("甲.bin" to 100L), store.created)
        assertTrue(store.reopened.isEmpty())
        assertEquals(0, handle.revealed + handle.discarded + handle.released)

        logic.onCommit(1, done = true)
        assertEquals(1, handle.revealed)
        assertEquals(0, handle.discarded)
    }

    @Test
    fun `续传走 reopen 且失败收尾 done false 清项`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        val existing = FakeHandle("断点件.bin")
        store.pendingForReopen["断点件.bin"] = existing

        val d = logic.onRequest(requestId = 2, rel = "断点件.bin", size = 8, resume = true)
        assertSame(existing, ((d as InboxDecision.Answer).handle as FakeHandle))
        assertTrue(store.created.isEmpty())
        assertEquals(listOf("断点件.bin" to 8L), store.reopened)

        logic.onCommit(2, done = false)
        assertEquals(1, existing.discarded)
        assertEquals(0, existing.revealed)
    }

    @Test
    fun `续传未命中拒开 NOT_FOUND 且 daemon 侧可降级全新重收`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        val d = logic.onRequest(requestId = 3, rel = "失锚.bin", size = 8, resume = true)
        val refuse = d as InboxDecision.Refuse
        assertTrue("拒开文案须带 NOT_FOUND(daemon 词面锚):${refuse.reason}", refuse.reason.contains("NOT_FOUND"))
    }

    @Test
    fun `create 失败转拒开`() {
        val store = FakeStore()
        store.failCreate = "MediaStore 插入失败"
        val logic = InboxLogic(store)
        val d = logic.onRequest(requestId = 4, rel = "乙.bin", size = 1, resume = false)
        val refuse = d as InboxDecision.Refuse
        assertTrue(refuse.reason.contains("MediaStore 插入失败"))
    }

    @Test
    fun `应答被拒与发送失败都清暂存`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        val h1 = (logic.onRequest(5, "a.bin", 1, false) as InboxDecision.Answer).handle as FakeHandle
        val h2 = (logic.onRequest(6, "b.bin", 1, false) as InboxDecision.Answer).handle as FakeHandle

        logic.onAckRejected(5) // ack ok:false:daemon 未受理(请求已超时)
        assertEquals(1, h1.discarded)

        logic.onSendFailed(6) // 0x02 写失败:fd 从未真正接手
        assertEquals(1, h2.discarded)
    }

    @Test
    fun `未知 request_id 的 commit 被忽略不崩`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        logic.onCommit(999, done = true)
        logic.onCommit(998, done = false)

        // 已终态(handle 已出表)的重复 commit 同样忽略
        val h = (logic.onRequest(7, "c.bin", 1, false) as InboxDecision.Answer).handle as FakeHandle
        logic.onCommit(7, true)
        logic.onCommit(7, true) // 迟到重复
        assertEquals(1, h.revealed)
        assertEquals(0, h.discarded)
    }

    @Test
    fun `onRequest 不做终态动作 fd 生命周期归调用方`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        val h = (logic.onRequest(8, "d.bin", 1, false) as InboxDecision.Answer).handle as FakeHandle
        // onRequest 只开暂存,不动 reveal/discard/release(fd 副本由调用方收)
        assertEquals(0, h.revealed + h.discarded + h.released)
    }
    @Test
    fun `在场查询走 store exists 且异常按不在场`() {
        val store = FakeStore()
        store.presentFor = { it == "在场件.bin" }
        val logic = InboxLogic(store)
        // 行在且 size 吻合 → true(daemon 对账 Skip 维持)
        assertTrue(logic.onQuery("在场件.bin", 100))
        // 行缺席(用户已删)/size 不符折算的 false → false(daemon FULL 重收)
        assertFalse(logic.onQuery("已删件.bin", 100))
        assertEquals(
            listOf("在场件.bin" to 100L, "已删件.bin" to 100L),
            store.existsCalls,
        )
        // 查询异常 → false(失败方向铁律:宁可重传,不可假完成;复审
        // Important B:异常经 InboxStoreException 收口后不再逸出——provider
        // 协程得以存活,appScope 无 CoroutineExceptionHandler,逸出即崩 App)
        store.failExistsFor = "坏件.bin"
        assertFalse(logic.onQuery("坏件.bin", 100))
        // 存活性:异常后同一 logic 继续可用(协程崩死的形态下走不到这里)
        store.failExistsFor = null
        assertTrue(logic.onQuery("在场件.bin", 100))
    }
}
