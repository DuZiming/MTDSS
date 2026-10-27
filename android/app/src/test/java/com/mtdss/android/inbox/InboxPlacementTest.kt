package com.mtdss.android.inbox

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 收件箱落位映射 JVM 单测(spec §5.1「嵌套目录传输目录结构保真」):
 * 顶层/嵌套映射、段消毒拒开口径(对照 daemon validate_rel_path +
 * MediaStore 约束,见 InboxPlacement 类注释)、**不同 rel 不互撞**性质
 * (旧平铺映射的非单射缺陷——`docs/a.bin` 撞 `docs__a.bin` ——随多段
 * RELATIVE_PATH 修复,此处钉死不回归)。
 */
class InboxPlacementTest {

    @Test
    fun `顶层文件落位收件箱根`() {
        assertEquals(
            InboxPlacement.Placement("Download/MTDSS/", "a.bin"),
            InboxPlacement.place("a.bin"),
        )
        // CJK / 空格 / 多点原名样透传
        assertEquals(
            InboxPlacement.Placement("Download/MTDSS/", "桥接 收件 甲.bin"),
            InboxPlacement.place("桥接 收件 甲.bin"),
        )
        assertEquals(
            InboxPlacement.Placement("Download/MTDSS/", "archive.tar.gz"),
            InboxPlacement.place("archive.tar.gz"),
        )
    }

    @Test
    fun `嵌套 rel 映射多段 RELATIVE_PATH 目录结构保真`() {
        // 单层:docs/a.bin → Download/MTDSS/docs/ + a.bin
        assertEquals(
            InboxPlacement.Placement("Download/MTDSS/docs/", "a.bin"),
            InboxPlacement.place("docs/a.bin"),
        )
        // 深层:a/b/c.bin → Download/MTDSS/a/b/ + c.bin
        assertEquals(
            InboxPlacement.Placement("Download/MTDSS/a/b/", "c.bin"),
            InboxPlacement.place("a/b/c.bin"),
        )
        // CJK 目录段
        assertEquals(
            InboxPlacement.Placement("Download/MTDSS/嵌套/", "乙.bin"),
            InboxPlacement.place("嵌套/乙.bin"),
        )
    }

    @Test
    fun `落位串恒带尾斜杠 MediaProvider 归一化形态`() {
        // F12 根因钉死:MediaProvider 插入侧把 RELATIVE_PATH 归一化为恒带
        // 尾 '/';落位串不带尾 '/' 时一切 `RELATIVE_PATH=?` 等值查询落空
        // (pending 重查/预清理/在场查询/验尸全盲,统一 F3/F4/F10/F11)。
        // 性质:任何合法 rel 的 relativePath 以 '/' 收尾且无连续 '/'。
        listOf("a.bin", "docs/a.bin", "a/b/c.bin", "嵌套/乙.bin", "docs__a.bin").forEach { rel ->
            val p = InboxPlacement.place(rel)!!
            assertTrue("须以 '/' 收尾:$rel → ${p.relativePath}", p.relativePath.endsWith("/"))
            assertFalse("不得出现连续 '/'(无空段):$rel → ${p.relativePath}", p.relativePath.contains("//"))
        }
        assertEquals("Download/MTDSS/", InboxPlacement.place("a.bin")!!.relativePath)
        assertEquals("Download/MTDSS/docs/", InboxPlacement.place("docs/a.bin")!!.relativePath)
    }

    @Test
    fun `段消毒拒开口径`() {
        // 空/超长
        assertNull(InboxPlacement.place(""))
        assertNull(InboxPlacement.place("a".repeat(4097)))
        // 空段:首 '/'、尾 '/'、连续 '/'
        assertNull(InboxPlacement.place("/a.bin"))
        assertNull(InboxPlacement.place("a.bin/"))
        assertNull(InboxPlacement.place("a//b.bin"))
        // '.'/'..' 段(含非逃逸 "a/../b" —— 严于 daemon,见类注释)
        assertNull(InboxPlacement.place("."))
        assertNull(InboxPlacement.place(".."))
        assertNull(InboxPlacement.place("a/.."))
        assertNull(InboxPlacement.place("a/../b.bin"))
        // 段内分隔符/盘符/NUL(镜像 daemon validate_rel_path + 防御)
        assertNull(InboxPlacement.place("a\\b.bin"))
        assertNull(InboxPlacement.place("C:a.bin"))
        assertNull(InboxPlacement.place("a\u0000b.bin"))
        // 保留首段 .mtdss(大小写不敏感,daemon 同口径)
        assertNull(InboxPlacement.place(".mtdss/x.bin"))
        assertNull(InboxPlacement.place(".MTDSS/x.bin"))
    }

    @Test
    fun `不同 rel 不互撞(旧平铺映射的非单射缺陷不回归)`() {
        // 旧映射把 '/' 归一 "__":docs/a.bin 与顶层 docs__a.bin 同名落位,
        // create 前的同名清理会误删另一 rel 的在途暂存。多段 RELATIVE_PATH
        // 后落位键 = (RELATIVE_PATH, DISPLAY_NAME) 二元组,互异合法 rel 两两
        // 落位互异。
        val a = InboxPlacement.place("docs/a.bin")
        val b = InboxPlacement.place("docs__a.bin")
        assertNotNull(a)
        assertNotNull(b)
        assertNotEquals(a, b)
        assertEquals("Download/MTDSS/docs/", a!!.relativePath)
        assertEquals("Download/MTDSS/", b!!.relativePath)

        val c = InboxPlacement.place("a/b/c.bin")
        val d = InboxPlacement.place("a/b__c.bin")
        assertNotNull(c)
        assertNotNull(d)
        assertNotEquals(c, d)

        // 性质面:一组互异合法 rel 的落位两两互异
        val rels = listOf(
            "a.bin", "docs/a.bin", "docs__a.bin", "a/b/c.bin", "a/b__c.bin",
            "docs/b.bin", "a/b", "嵌套/乙.bin",
        )
        val placements = rels.map { InboxPlacement.place(it)!! }
        assertEquals(rels.size, placements.toSet().size)
    }
}
