package com.mtdss.android.send

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * ref 方案单测(**持久化契约的往返锁定**):编码/解析往返、malformed 防御、
 * 重解析矩阵(授权根查表/单文档透传/不可解析)。
 */
class SourceRefsTest {

    // ── 编码/解析往返 ─────────────────────────────────────────────────────

    @Test
    fun `rootRef 编码解析往返`() {
        val ref = SourceRefs.rootRef("r1", "photos/2024/img.jpg")
        assertEquals("roottree://r1/photos/2024/img.jpg", ref)
        assertEquals(SourceRef.RootTree("r1", "photos/2024/img.jpg"), SourceRefs.parse(ref))
    }

    @Test
    fun `根下单文件 rel 无目录段`() {
        val ref = SourceRefs.rootRef("rabc123", "a.bin")
        assertEquals("roottree://rabc123/a.bin", ref)
        assertEquals(SourceRef.RootTree("rabc123", "a.bin"), SourceRefs.parse(ref))
    }

    @Test
    fun `单文档 ref 原样透传`() {
        val uri = "content://com.android.providers.downloads.documents/document/msf:42"
        assertEquals(SourceRef.Document(uri), SourceRefs.parse(uri))
    }

    // ── malformed 防御 ────────────────────────────────────────────────────

    @Test
    fun `malformed ref 全拒`() {
        assertNull(SourceRefs.parse(""))
        assertNull(SourceRefs.parse("roottree://")) // 无 rootId 无 rel
        assertNull(SourceRefs.parse("roottree://r1")) // 无 rel
        assertNull(SourceRefs.parse("roottree://r1/")) // rel 空
        assertNull(SourceRefs.parse("roottree:///a")) // rootId 空
        assertNull(SourceRefs.parse("roottree://r1//a")) // rel 含空段
        assertNull(SourceRefs.parse("roottree://r1/a/")) // rel 尾 '/'
    }

    @Test
    fun `编码侧违规即抛`() {
        assertThrows { SourceRefs.rootRef("", "a") }
        assertThrows { SourceRefs.rootRef("r/1", "a") } // rootId 含 '/'
        assertThrows { SourceRefs.rootRef("r1", "") }
        assertThrows { SourceRefs.rootRef("r1", "/a") }
        assertThrows { SourceRefs.rootRef("r1", "a//b") }
    }

    // ── 重解析矩阵 ───────────────────────────────────────────────────────

    private val roots = listOf(
        StoredRoot("r1", "content://tree/one", "primary:DCIM", "DCIM"),
        StoredRoot("r2", "content://tree/two", "primary:Download", "下载"),
    )

    @Test
    fun `rootRef 重解析为树子文档`() {
        val r = SourceRefs.resolve("roottree://r1/cam/x.jpg", roots)
        assertEquals(
            ResolvedSource.TreeChild(
                treeUri = "content://tree/one",
                documentId = "primary:DCIM/cam/x.jpg",
                treeDocId = "primary:DCIM",
                rel = "cam/x.jpg",
            ),
            r,
        )
    }

    @Test
    fun `childDocumentId 累积编码`() {
        assertEquals("primary:DCIM/cam/x.jpg", SourceRefs.childDocumentId("primary:DCIM", "cam/x.jpg"))
    }

    // ── 回退下钻(drillChildDocumentId;设备场 F8,DownloadsProvider 数字 docId)──

    /** 造一层子项表:parent → entries(缺项 = children 闭包返回 null)。 */
    private fun childTable(vararg entries: Pair<String, List<TreeChild>>) = entries.toMap()

    private fun dir(id: String, name: String) = TreeChild(id, name, isDir = true, size = 0, mtimeMs = 0)

    private fun file(id: String, name: String) = TreeChild(id, name, isDir = false, size = 1, mtimeMs = 0)

    @Test
    fun `下钻多段嵌套 rel 逐层命中真实 docId`() {
        // DownloadsProvider 形:纯数字 docId,与显示名无拼接关系
        val children = childTable(
            "12" to listOf(dir("100", "docs"), dir("13", "MTDSS"), file("99", "别的.txt")),
            "13" to listOf(file("14", "small.txt")),
        )
        assertEquals("14", SourceRefs.drillChildDocumentId("12", "MTDSS/small.txt") { children[it] })
    }

    @Test
    fun `下钻段名无匹配返回 null`() {
        val children = childTable("12" to listOf(file("99", "别的.txt")))
        assertNull(SourceRefs.drillChildDocumentId("12", "MTDSS/small.txt") { children[it] })
    }

    @Test
    fun `下钻中途目录查询失败返回 null`() {
        val children = childTable("12" to listOf(dir("13", "MTDSS"))) // "13" 缺项 = 查询失败
        assertNull(SourceRefs.drillChildDocumentId("12", "MTDSS/small.txt") { children[it] })
    }

    @Test
    fun `下钻按段名精确匹配`() {
        val children = childTable("12" to listOf(dir("13", "MTDSS2"))) // 前缀同名不命中
        assertNull(SourceRefs.drillChildDocumentId("12", "MTDSS") { children[it] })
    }

    @Test
    fun `授权根已移除拒开`() {
        val r = SourceRefs.resolve("roottree://gone/a.bin", roots)
        assertTrue(r is ResolvedSource.Unresolvable)
        assertTrue((r as ResolvedSource.Unresolvable).reason.contains("授权根已移除"))
    }

    @Test
    fun `单文档重解析原样可开`() {
        val uri = "content://picked/1"
        assertEquals(ResolvedSource.Document(uri), SourceRefs.resolve(uri, roots))
    }

    @Test
    fun `malformed 引用拒开`() {
        val r = SourceRefs.resolve("roottree://r1/", roots)
        assertTrue(r is ResolvedSource.Unresolvable)
        assertTrue((r as ResolvedSource.Unresolvable).reason.contains("引用格式非法"))
    }

    private fun assertThrows(body: () -> Unit) {
        try {
            body()
            fail("应抛 IllegalArgumentException")
        } catch (_: IllegalArgumentException) {
        }
    }
}
