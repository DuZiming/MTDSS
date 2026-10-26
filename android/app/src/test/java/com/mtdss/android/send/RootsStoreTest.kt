package com.mtdss.android.send

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 授权根持久化单测(伪存储 + 伪 SAF):编解码往返、损坏账本弃置、
 * 增删语义(移除连带释放授权)、重复 treeUri 幂等、rootId 形态。
 */
class RootsStoreTest {

    private class MemoryPersistence : RootsPersistence {
        var value: String? = null
        override fun load(): String? = value
        override fun save(json: String) {
            value = json
        }
    }

    private fun newStore(persistence: RootsPersistence = MemoryPersistence(), saf: FakeSafFace = FakeSafFace()): RootsStore =
        RootsStore(persistence, saf)

    // ── 编解码 ────────────────────────────────────────────────────────────

    @Test
    fun `编码解码往返`() {
        val roots = listOf(
            StoredRoot("r1", "content://tree/1", "primary:DCIM", "DCIM"),
            StoredRoot("r2", "content://tree/2", "primary:Download/x", "深 目录"),
        )
        assertEquals(roots, RootsCodec.decode(RootsCodec.encode(roots)))
    }

    @Test
    fun `空表往返`() {
        assertEquals(emptyList<StoredRoot>(), RootsCodec.decode(RootsCodec.encode(emptyList())))
        assertEquals(emptyList<StoredRoot>(), RootsCodec.decode("[]"))
    }

    @Test
    fun `损坏账本整表弃置`() {
        assertNull(RootsCodec.decode("不是 json"))
        assertNull(RootsCodec.decode("{}")) // 非数组
        assertNull(RootsCodec.decode("[{\"id\":\"r1\"}]")) // 缺字段
        assertNull(RootsCodec.decode("[{\"id\":1,\"name\":\"a\",\"tree_doc_id\":\"d\",\"tree_uri\":\"u\"}]")) // 字段非字符串
    }

    @Test
    fun `rootId 形态与去重`() {
        val id = RootsCodec.newRootId()
        assertTrue("应为 r+12hex:$id", Regex("^r[0-9a-f]{12}$").matches(id))
        // 与既有 id 撞号时必出集合外新值
        val existing = (0 until 64).map { RootsCodec.newRootId() }.toSet()
        assertTrue(RootsCodec.newRootId(existing) !in existing)
    }

    // ── 账本操作 ──────────────────────────────────────────────────────────

    @Test
    fun `add 登记并持久化`() {
        val persistence = MemoryPersistence()
        val saf = FakeSafFace()
        saf.treeDocIds["content://tree/1"] = "primary:DCIM"
        val store = newStore(persistence, saf)

        val root = store.add("content://tree/1", "DCIM")
        assertNotNull(root)
        assertEquals("primary:DCIM", root!!.treeDocId)
        assertEquals(listOf("content://tree/1"), saf.taken) // 持久授权已尝试
        assertEquals(listOf(root), store.roots())
        assertEquals(listOf(root), RootsCodec.decode(persistence.value ?: "")) // 已落账
    }

    @Test
    fun `add 空名兜底`() {
        val saf = FakeSafFace()
        saf.treeDocIds["content://tree/1"] = "d1"
        val root = newStore(saf = saf).add("content://tree/1", "")
        assertEquals("未命名根", root!!.displayName)
    }

    @Test
    fun `add 非法树 URI 拒绝`() {
        val persistence = MemoryPersistence()
        val store = newStore(persistence) // saf.treeDocIds 无该 uri
        assertNull(store.add("content://tree/none", "X"))
        assertTrue(store.roots().isEmpty())
        assertNull(persistence.value) // 未落账
    }

    @Test
    fun `add 重复 treeUri 幂等`() {
        val saf = FakeSafFace()
        saf.treeDocIds["content://tree/1"] = "d1"
        val store = newStore(saf = saf)
        val first = store.add("content://tree/1", "A")!!
        val second = store.add("content://tree/1", "B")!!
        assertEquals(first, second)
        assertEquals(1, store.roots().size)
    }

    @Test
    fun `remove 连带释放授权`() {
        val saf = FakeSafFace()
        saf.treeDocIds["content://tree/1"] = "d1"
        val persistence = MemoryPersistence()
        val store = newStore(persistence, saf)
        val root = store.add("content://tree/1", "A")!!

        assertTrue(store.remove(root.rootId))
        assertTrue(store.roots().isEmpty())
        assertEquals(listOf("content://tree/1"), saf.released) // 授权已释放
        assertEquals("[]", persistence.value)

        assertFalse(store.remove(root.rootId)) // 再删 = 不存在
    }

    @Test
    fun `损坏持久化 JSON 读取为空表`() {
        val persistence = MemoryPersistence().apply { value = "垃圾数据" }
        assertTrue(newStore(persistence).roots().isEmpty())
    }
}
