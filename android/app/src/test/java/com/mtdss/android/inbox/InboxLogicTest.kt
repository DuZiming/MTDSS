package com.mtdss.android.inbox

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * InboxLogic 决策核心 JVM 单测(伪 InboxStore):P4a 起剩两面——
 * - 在场查询(fs.query_dest/0x04):store.exists 异常按不在场收口;
 * - 发布(fs.publish/0x05):store 透传 + 契约外异常兜网为
 *   [PublishResult.Failed](appScope 无 CoroutineExceptionHandler,逸出即
 *   崩 App——镜像 onQuery 的收口纪律)。
 * (fd 桥时代的请求分诊/在途表/终态收尾测试已随 0x02/0x03 退役。)
 */
class InboxLogicTest {

    private data class PublishCall(
        val rel: String,
        val stagingPath: String,
        val size: Long,
        val policy: TransferPolicy,
    )

    private class FakeStore : InboxStore {
        val existsCalls = mutableListOf<Pair<String, Long>>()
        var presentFor: (String) -> Boolean = { false }
        var failExistsFor: String? = null

        val publishCalls = mutableListOf<PublishCall>()

        /** publish 抛出的消息(非 null = store 契约外异常,协议上不该发生)。 */
        var failPublish: String? = null

        /** publish 应答面(缺省成功)。 */
        @Volatile
        var publishResult: PublishResult = PublishResult.Published

        override fun exists(rel: String, size: Long): Boolean {
            failExistsFor?.takeIf { it == rel }?.let { throw InboxStoreException(it) }
            existsCalls += rel to size
            return presentFor(rel)
        }

        override fun publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult {
            failPublish?.let { throw InboxStoreException(it) }
            publishCalls += PublishCall(rel, stagingPath, size, policy)
            return publishResult
        }
    }

    // ── 在场查询(0x04,保留面)────────────────────────────────────────────

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

    @Test
    fun `在场查询以真开 fd 为准 僵尸行折算不在场`() {
        // F10:MIUI 文件管理器裸删后 MediaStore 行残留(SIZE 字段照旧)——
        // 行查询撒谎成假完成。store 契约改为「真开 fd 为准,行仅定位」,
        // 本测试的 presentFor 即 store 侧 fd 验证后的应答面(MediaStore
        // 行为实机验证,fd 尺寸判据另见 MediaStoreInboxStoreTest)。
        val store = FakeStore()
        val logic = InboxLogic(store)
        // 行在、fd 真开且 statSize 吻合 → 在场(daemon Skip 维持)
        store.presentFor = { true }
        assertTrue(logic.onQuery("small.txt", 2025))
        // 行在但 fd 开抛(stale 行 FileNotFoundException)→ store 折算
        // false → daemon 对账不 Skip、引擎 FULL 重收
        store.presentFor = { false }
        assertFalse(logic.onQuery("small.txt", 2025))
        // 行定位查询本身的未归类异常 → InboxStoreException → 同样 false
        store.failExistsFor = "small.txt"
        assertFalse(logic.onQuery("small.txt", 2025))
    }

    // ── 发布(fs.publish/0x05,新增面)────────────────────────────────────

    @Test
    fun `发布透传 store 且参数原样`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        val policy = TransferPolicy(overwrite = listOf("small.txt"))
        val r = logic.onPublish("small.txt", "/staging/small.txt", 2025, policy)
        assertEquals(PublishResult.Published, r)
        assertEquals(
            listOf(PublishCall("small.txt", "/staging/small.txt", 2025, policy)),
            store.publishCalls,
        )
    }

    @Test
    fun `发布结果 Skipped 与 Failed 原样上浮`() {
        val store = FakeStore()
        val logic = InboxLogic(store)
        store.publishResult = PublishResult.Skipped
        assertEquals(PublishResult.Skipped, logic.onPublish("a.bin", "/s/a", 1, TransferPolicy()))
        store.publishResult = PublishResult.Failed("MediaStore 插入失败")
        assertEquals(
            PublishResult.Failed("MediaStore 插入失败"),
            logic.onPublish("a.bin", "/s/a", 1, TransferPolicy()),
        )
        // 决策面无在途状态:失败后继续发布照常
        store.publishResult = PublishResult.Published
        assertEquals(PublishResult.Published, logic.onPublish("b.bin", "/s/b", 2, TransferPolicy()))
    }

    @Test
    fun `store 契约外异常兜网为 Failed 不逸出且存活`() {
        val store = FakeStore()
        store.failPublish = "磁盘蒸发"
        val logic = InboxLogic(store)
        val r = logic.onPublish("a.bin", "/s/a", 1, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须带底层因:${failed.reason}", failed.reason.contains("磁盘蒸发"))
        // 存活性:兜网后同一 logic 继续可用
        store.failPublish = null
        assertEquals(PublishResult.Published, logic.onPublish("b.bin", "/s/b", 2, TransferPolicy()))
    }
}
