package com.mtdss.android.send

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 选择器件条目化单测(伪 SAF):ref = URI 串本身、rel = 显示名、
 * 同名撞号加序号、不可读条目跳过计数、持久授权逐 URI 尝试。
 */
class PickerBridgeTest {

    private fun bridge(saf: FakeSafFace) = PickerBridge(saf)

    @Test
    fun `条目化 ref 为 URI rel 为显示名`() {
        val saf = FakeSafFace()
        saf.metas["content://picked/1"] = DocMeta("照片.jpg", size = 1024, mtimeMs = 1760000000000)
        saf.metas["content://picked/2"] = DocMeta("文档.pdf", size = 2048, mtimeMs = 1760000000001)
        val picked = bridge(saf).itemsFromUris(listOf("content://picked/1", "content://picked/2"))
        assertEquals(2, picked.items.size)
        assertEquals(0, picked.skipped)
        val first = picked.items[0]
        assertEquals("content://picked/1", first.ref)
        assertEquals("照片.jpg", first.rel)
        assertEquals(1024, first.size)
        assertEquals(1760000000000, first.mtime)
        assertEquals("文档.pdf", picked.items[1].rel)
        assertEquals(listOf("content://picked/1", "content://picked/2"), saf.taken)
    }

    @Test
    fun `同名撞号加序号`() {
        val saf = FakeSafFace()
        saf.metas["content://a/1"] = DocMeta("a.jpg", 1, 1)
        saf.metas["content://b/1"] = DocMeta("a.jpg", 2, 2)
        saf.metas["content://c/1"] = DocMeta("a.jpg", 3, 3)
        val picked = bridge(saf).itemsFromUris(listOf("content://a/1", "content://b/1", "content://c/1"))
        assertEquals(listOf("a.jpg", "a.jpg (1)", "a.jpg (2)"), picked.items.map { it.rel })
    }

    @Test
    fun `元数据缺失或无名跳过计数`() {
        val saf = FakeSafFace()
        saf.metas["content://picked/1"] = DocMeta("ok.bin", 1, 1)
        // picked/2:元数据查询失败;picked/3:有名无实(名称 null)
        saf.metas["content://picked/3"] = DocMeta(null, 5, 5)
        val picked = bridge(saf).itemsFromUris(
            listOf("content://picked/1", "content://picked/2", "content://picked/3"),
        )
        assertEquals(1, picked.items.size)
        assertEquals(2, picked.skipped)
        assertTrue(picked.items[0].ref == "content://picked/1")
    }

    @Test
    fun `跳过原因逐条带出 provider 异常词面`() {
        // F9:「文件不可读」须自带为什么——provider 异常经 metaFailure 旁路
        // 回收进 skipReasons,复现时从 UI 词面即可定位
        val saf = FakeSafFace()
        saf.metaFailures["content://media/15678"] = "查询失败(IllegalArgumentException: Invalid column last_modified)"
        val picked = bridge(saf).itemsFromUris(listOf("content://media/15678"))
        assertEquals(0, picked.items.size)
        assertEquals(1, picked.skipped)
        assertEquals(
            listOf("查询失败(IllegalArgumentException: Invalid column last_modified):content://media/15678"),
            picked.skipReasons,
        )
    }

    @Test
    fun `跳过原因无显示名`() {
        val saf = FakeSafFace()
        saf.metas["content://picked/1"] = DocMeta(null, 5, 5)
        saf.metas["content://picked/2"] = DocMeta("", 5, 5)
        val picked = bridge(saf).itemsFromUris(listOf("content://picked/1", "content://picked/2"))
        assertEquals(0, picked.items.size)
        assertEquals(2, picked.skipped)
        assertEquals(
            listOf("无显示名:content://picked/1", "无显示名:content://picked/2"),
            picked.skipReasons,
        )
    }

    @Test
    fun `空输入空输出`() {
        val picked = bridge(FakeSafFace()).itemsFromUris(emptyList())
        assertEquals(0, picked.items.size)
        assertEquals(0, picked.skipped)
        assertEquals(0, picked.skipReasons.size)
    }

    @Test
    fun `分享流并集 stream 与 clip 合并去重保序`() {
        // F9:MIUI 部分分享源只填 ClipData;EXTRA_STREAM × ClipData 并集兜底
        assertEquals(
            listOf("content://a/1", "content://b/2", "content://c/3"),
            PickerBridge.mergeShareUris(
                stream = listOf("content://a/1", "content://b/2"),
                clip = listOf("content://b/2", "content://c/3"),
            ),
        )
        // stream 缺席、clip 独撑(只填 ClipData 的分享源)
        assertEquals(
            listOf("content://clip/1", "content://clip/2"),
            PickerBridge.mergeShareUris(stream = emptyList(), clip = listOf("content://clip/1", "content://clip/2")),
        )
        // 双空
        assertEquals(emptyList<String>(), PickerBridge.mergeShareUris(emptyList(), emptyList()))
    }
}
