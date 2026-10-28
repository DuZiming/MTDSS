package com.mtdss.android.inbox

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** 内存授权账本缝(SafInboxStoreTest 共用)。 */
internal class MemInboxGrantPersistence : InboxGrantPersistence {
    var value: String? = null
    override fun load(): String? = value
    override fun save(uri: String) {
        value = uri
    }
    override fun clear() {
        value = null
    }
}

/**
 * 收件箱 SAF 授权账本 JVM 单测(伪 DocumentsFace):未授权 → null(发布
 * 按无授权 Failed、查询 false 的前提);非法树 URI 不落账不持久;成功授权
 * 落账 + takePersistableReadWrite(读写双旗——收件须写,区别发送根的只读);
 * 撤销连释放。真实持久授权行为实机验证。
 */
class InboxGrantStoreTest {

    private fun env(face: FakeDocumentsFace = FakeDocumentsFace()) =
        Pair(InboxGrantStore(MemInboxGrantPersistence(), face), face)

    @Test
    fun `未授权 treeUri 为空`() {
        val (store, _) = env()
        assertNull(store.treeUri())
    }

    @Test
    fun `授权成功 落账并持久授权`() {
        val (store, face) = env()
        assertEquals(face.treeUri, store.grant(face.treeUri))
        assertEquals(face.treeUri, store.treeUri())
        assertEquals("takePersistableReadWrite 读写双旗收授权", listOf(face.treeUri), face.taken)
    }

    @Test
    fun `非法树 URI 不落账不持久`() {
        val (store, face) = env()
        assertNull(store.grant("content://not-a-tree"))
        assertNull(store.treeUri())
        assertTrue("非法授权不得触发持久授权", face.taken.isEmpty())
    }

    @Test
    fun `重复授权幂等覆盖`() {
        val (store, face) = env()
        store.grant(face.treeUri)
        store.grant(face.treeUri)
        assertEquals(face.treeUri, store.treeUri())
    }

    @Test
    fun `撤销清账并释放授权`() {
        val (store, face) = env()
        store.grant(face.treeUri)
        store.revoke()
        assertNull(store.treeUri())
        assertEquals(listOf(face.treeUri), face.released)
    }
}
