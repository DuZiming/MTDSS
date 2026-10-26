package com.mtdss.android.send

import com.mtdss.android.ipc.BridgeItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 授权根树展开单测:rel 语义(相对授权根,树根名不进 rel)、目录递归、
 * 环/空名防御、条目数护栏提前停走、查询失败收口。
 */
class RootTreeTest {

    private fun file(id: String, name: String, size: Long = 1, mtime: Long = 2) =
        TreeChild(id, name, isDir = false, size = size, mtimeMs = mtime)

    private fun dir(id: String, name: String) = TreeChild(id, name, isDir = true, size = 0, mtimeMs = 0)

    private fun sel(rel: String, entry: TreeChild) = RootTree.TreeSelection(rel, entry)

    private fun expandOf(
        selections: List<RootTree.TreeSelection>,
        children: Map<String, List<TreeChild>> = emptyMap(),
        maxItems: Int = BridgeLimits.MAX_BRIDGE_ITEMS,
    ) = RootTree.expand("r1", selections, maxItems) { children[it] }

    // ── rel 语义与顺序 ────────────────────────────────────────────────────

    @Test
    fun `文件选择直出条目`() {
        val r = expandOf(listOf(sel("a.bin", file("d1", "a.bin", size = 100, mtime = 7))))
        val items = (r as RootTree.ExpandResult.Items).items
        assertEquals(
            listOf(BridgeItem(ref = "roottree://r1/a.bin", rel = "a.bin", size = 100, mtime = 7)),
            items,
        )
    }

    @Test
    fun `目录选择递归且目录名进 rel 前缀`() {
        val r = expandOf(
            selections = listOf(sel("docs", dir("d:docs", "docs"))),
            children = mapOf(
                "d:docs" to listOf(file("d:docs/x", "x.txt"), dir("d:docs/sub", "sub")),
                "d:docs/sub" to listOf(file("d:docs/sub/y", "y.txt")),
            ),
        )
        val items = (r as RootTree.ExpandResult.Items).items.map { it.rel }
        assertEquals(listOf("docs/x.txt", "docs/sub/y.txt"), items)
    }

    @Test
    fun `文件与目录混合按选择序`() {
        val r = expandOf(
            selections = listOf(sel("b.txt", file("d:b", "b.txt")), sel("a", dir("d:a", "a"))),
            children = mapOf("d:a" to listOf(file("d:a/x", "x"))),
        )
        val rels = (r as RootTree.ExpandResult.Items).items.map { it.rel }
        assertEquals(listOf("b.txt", "a/x"), rels)
    }

    @Test
    fun `重复 rel 去重`() {
        val r = expandOf(listOf(sel("a.txt", file("d1", "a.txt")), sel("a.txt", file("d2", "a.txt"))))
        assertEquals(1, (r as RootTree.ExpandResult.Items).items.size)
    }

    // ── 展开本目录 ────────────────────────────────────────────────────────

    @Test
    fun `展开树根本体 rel 无前导斜杠`() {
        val r = RootTree.expandDirectory(
            rootId = "r1",
            dirRel = "",
            dirDocumentId = "root",
            children = { id ->
                when (id) {
                    "root" -> listOf(file("root/a", "a.txt"), dir("root/d", "d"))
                    else -> listOf(file("root/d/y", "y.txt"))
                }
            },
        )
        val rels = (r as RootTree.ExpandResult.Items).items.map { it.rel }
        assertEquals(listOf("a.txt", "d/y.txt"), rels)
    }

    @Test
    fun `展开子目录沿用父前缀`() {
        val r = RootTree.expandDirectory(
            rootId = "r1",
            dirRel = "docs/2024",
            dirDocumentId = "d:2024",
            children = { listOf(file("d:2024/a", "a.txt")) },
        )
        val rels = (r as RootTree.ExpandResult.Items).items.map { it.rel }
        assertEquals(listOf("docs/2024/a.txt"), rels)
    }

    // ── 环与退化防御 ──────────────────────────────────────────────────────

    @Test
    fun `空名子项跳过(不进 rel 也不入队)`() {
        // 空名目录若入队会以空 rel 循环展开(退化 provider 防御);空名文件同理跳过
        val r = RootTree.expandDirectory(
            rootId = "r1",
            dirRel = "",
            dirDocumentId = "root",
            children = { listOf(dir("x", ""), file("y", "ok.txt")) },
        )
        val rels = (r as RootTree.ExpandResult.Items).items.map { it.rel }
        assertEquals(listOf("ok.txt"), rels)
    }

    @Test
    fun `provider 自含环被 visited 守卫截断`() {
        // 退化 provider:每个目录的子目录指回自身(rel 逐级变长永不重复、
        // 无 visited 守卫即无限 BFS——此形态曾烧满 CPU 被线程转储查证)
        val r = RootTree.expandDirectory(
            rootId = "r1",
            dirRel = "",
            dirDocumentId = "root",
            children = { id -> listOf(file("f-$id", "f.txt"), dir(id, "self")) },
        )
        val rels = (r as RootTree.ExpandResult.Items).items.map { it.rel }
        assertEquals(listOf("f.txt"), rels)
    }

    // ── 护栏与失败 ────────────────────────────────────────────────────────

    @Test
    fun `超限提前停走不再查询后续目录`() {
        val bigDir1 = (1..600).map { file("d1f$it", "f$it") }
        val bigDir2 = (1..600).map { file("d2f$it", "g$it") }
        val queried = mutableListOf<String>()
        val r = RootTree.expand(
            rootId = "r1",
            selections = listOf(sel("d1", dir("d1", "d1")), sel("d2", dir("d2", "d2"))),
        ) { id ->
            queried += id
            when (id) {
                "d1" -> bigDir1
                "d2" -> bigDir2
                else -> emptyList()
            }
        }
        val e = r as RootTree.ExpandResult.Error
        assertTrue("词面应与 daemon 一致:${e.message}", e.message!!.startsWith("桥接条目过多(513 项,上限 512)"))
        assertEquals(listOf("d1"), queried) // d2 未被查询(提前停走)
    }

    @Test
    fun `子目录查询失败收口`() {
        // children 表缺 "d:docs" → 查询失败(null)
        val r = expandOf(selections = listOf(sel("docs", dir("d:docs", "docs"))))
        val e = r as RootTree.ExpandResult.Error
        assertTrue(e.message!!.contains("目录读取失败"))
        assertTrue(e.message!!.contains("docs"))
    }
}
