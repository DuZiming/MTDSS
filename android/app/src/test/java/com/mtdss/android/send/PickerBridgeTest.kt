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

    // ── 分享取名链(R5/J1:itemsFromShareUris;OpenableColumns 主通道 →
    // MediaStore 直查 → DocumentsContract,逐级垃圾名下沉)─────────────────

    /** media 形 URI(相册分享的典型形态)。 */
    private val mediaUri = "content://media/external/images/media/5"

    @Test
    fun `分享取名链 openable 真名直收`() {
        val saf = FakeSafFace()
        saf.openableMetas[mediaUri] = DocMeta("IMG_20261108_093000.jpg", size = 4096, mtimeMs = 0)
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri))
        assertEquals(1, picked.items.size)
        assertEquals(0, picked.skipped)
        assertEquals("IMG_20261108_093000.jpg", picked.items[0].rel)
        assertEquals(4096, picked.items[0].size)
    }

    @Test
    fun `分享取名链 openable 纯数字垃圾下沉 MediaStore 真名`() {
        // 用户实机形:DocumentsContract/OpenableColumns 回「1」;MediaStore
        // 直查回真名(链 ② 收)
        val saf = FakeSafFace()
        saf.openableMetas[mediaUri] = DocMeta("1", size = 8192, mtimeMs = 0)
        saf.metas[mediaUri] = DocMeta("1", size = 8192, mtimeMs = 0)
        saf.mediaMetas[mediaUri] = MediaMeta("IMG_20261107_1.jpg", "image/jpeg", 8192)
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri))
        assertEquals(listOf("IMG_20261107_1.jpg"), picked.items.map { it.rel })
        assertEquals(0, picked.skipped)
        assertEquals(listOf(mediaUri), saf.mediaQueries)
    }

    @Test
    fun `分享取名链 纯数字且 MIME 可知补扩展名`() {
        // 链 ①② 全垃圾/缺席,MIME 可知 → 按 MIME 补扩展(唯一允许的「改名」)
        val saf = FakeSafFace()
        saf.openableMetas["content://photos/x/1"] = DocMeta("1", size = 1, mtimeMs = 0)
        saf.mimes["content://photos/x/1"] = "image/jpeg"
        saf.mimeExtensions["image/jpeg"] = "jpg"
        val picked = bridge(saf).itemsFromShareUris(listOf("content://photos/x/1"))
        assertEquals(listOf("1.jpg"), picked.items.map { it.rel })
    }

    @Test
    fun `分享取名链 纯数字 MIME 不可知全链垃圾 跳过带实得词面`() {
        val saf = FakeSafFace()
        saf.openableMetas[mediaUri] = DocMeta("2", size = 1, mtimeMs = 0)
        saf.metas[mediaUri] = DocMeta("2", size = 1, mtimeMs = 0)
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri))
        assertEquals(0, picked.items.size)
        assertEquals(1, picked.skipped)
        assertEquals(listOf("垃圾名(实得:2):$mediaUri"), picked.skipReasons)
    }

    @Test
    fun `分享取名链 全空跳过既有词面`() {
        val saf = FakeSafFace()
        val picked = bridge(saf).itemsFromShareUris(listOf("content://picked/9"))
        assertEquals(0, picked.items.size)
        assertEquals(1, picked.skipped)
        assertEquals(listOf("无显示名:content://picked/9"), picked.skipReasons)
    }

    @Test
    fun `分享取名链 非 media 形 URI 不查 MediaStore`() {
        // ② 通道 gating:仅 content://media 形直查;其余 URI 走 ③ DocumentsContract
        val saf = FakeSafFace()
        saf.openableMetas["content://photos/x/2"] = DocMeta("2", size = 1, mtimeMs = 0)
        saf.mediaMetas["content://photos/x/2"] = MediaMeta("IMG_补.jpg", "image/jpeg", 2)
        saf.metas["content://photos/x/2"] = DocMeta("IMG_真名.jpg", 2, 2)
        val picked = bridge(saf).itemsFromShareUris(listOf("content://photos/x/2"))
        assertEquals(listOf("IMG_真名.jpg"), picked.items.map { it.rel })
        assertTrue(saf.mediaQueries.isEmpty())
    }

    @Test
    fun `分享取名链 无扩展但 MIME 可知补扩展名`() {
        // 真名无扩展(README 形)+ MIME 可知 → 补扩展名收(不下沉)
        val saf = FakeSafFace()
        saf.openableMetas[mediaUri] = DocMeta("IMG_20261108_093000", size = 99, mtimeMs = 0)
        saf.mimes[mediaUri] = "image/jpeg"
        saf.mimeExtensions["image/jpeg"] = "jpg"
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri))
        assertEquals(listOf("IMG_20261108_093000.jpg"), picked.items.map { it.rel })
    }

    @Test
    fun `分享取名链 有扩展即收 数字主干不判垃圾`() {
        // 「1.jpg」有扩展即真名(数字主干是真实场景),不下沉不改名
        val saf = FakeSafFace()
        saf.openableMetas[mediaUri] = DocMeta("1.jpg", size = 1, mtimeMs = 0)
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri))
        assertEquals(listOf("1.jpg"), picked.items.map { it.rel })
    }

    @Test
    fun `分享取名链 同名撞号序号不变`() {
        // uniqueRel 撞号既有语义不变(两条真名同名的分享件)
        val saf = FakeSafFace()
        saf.openableMetas[mediaUri] = DocMeta("IMG_1.jpg", size = 1, mtimeMs = 0)
        saf.openableMetas["content://media/external/images/media/6"] =
            DocMeta("IMG_1.jpg", size = 2, mtimeMs = 0)
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri, "content://media/external/images/media/6"))
        assertEquals(listOf("IMG_1.jpg", "IMG_1.jpg (1)"), picked.items.map { it.rel })
    }

    @Test
    fun `分享取名链 provider 异常词面带出`() {
        // ③ 通道 metaFailure 词面旁路(全链垃圾/缺席时带出,既有 skipReasons 语义)
        val saf = FakeSafFace()
        saf.metaFailures[mediaUri] = "查询失败(SecurityException: 拒绝访问)"
        val picked = bridge(saf).itemsFromShareUris(listOf(mediaUri))
        assertEquals(0, picked.items.size)
        assertEquals(1, picked.skipped)
        assertEquals(
            listOf("查询失败(SecurityException: 拒绝访问):$mediaUri"),
            picked.skipReasons,
        )
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
