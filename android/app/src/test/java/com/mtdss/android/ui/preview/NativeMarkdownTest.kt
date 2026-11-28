package com.mtdss.android.ui.preview

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** r70k:原生 md 解析器单测(块级结构 + 行内样式主干面)。 */
class NativeMarkdownTest {

    @Test
    fun headingsAndParas() {
        val blocks = parseMarkdown("# 标题一\n\n正文段落。\n\n### 三级\n")
        assertEquals(3, blocks.size)
        assertTrue(blocks[0] is MdBlock.Heading && (blocks[0] as MdBlock.Heading).level == 1)
        assertTrue(blocks[1] is MdBlock.Para)
        assertTrue(blocks[2] is MdBlock.Heading && (blocks[2] as MdBlock.Heading).level == 3)
    }

    @Test
    fun listsWithNestingAndOrder() {
        val blocks = parseMarkdown("- 甲\n- 乙\n  - 乙一\n1. 第一\n2. 第二\n")
        val items = blocks.filterIsInstance<MdBlock.ListItem>()
        assertEquals(5, items.size)
        assertEquals(1, items[2].indent)
        assertEquals("1.", items[3].marker)
    }

    @Test
    fun codeFenceSwallowsMarkers() {
        val blocks = parseMarkdown("前文\n```\n# 不是标题\n- 不是列表\n```\n后文\n")
        assertEquals(3, blocks.size)
        val code = blocks[1] as MdBlock.Code
        assertEquals(listOf("# 不是标题", "- 不是列表"), code.lines)
    }

    @Test
    fun inlineStyles() {
        val spans = parseInline("粗 **甲** 码 `乙` 链 [文](http://x) 斜 *丙*")
        // 样式段天然分割纯文本:粗|甲(bold)|码|乙(code)|链|文(link)|斜|丙(italic)
        assertEquals(
            listOf(false, true, false, false, false, false, false, false),
            spans.map { it.bold },
        )
        assertTrue(spans.any { it.code && it.text == "乙" })
        assertTrue(spans.any { it.link && it.text == "文" })
        assertTrue(spans.any { it.italic && it.text == "丙" })
    }

    @Test
    fun unclosedMarkersStayLiteral() {
        val spans = parseInline("孤立 * 星与 ** 双星")
        assertEquals(1, spans.size)
        assertEquals("孤立 * 星与 ** 双星", spans[0].text)
    }

    @Test
    fun ruleAndQuote() {
        val blocks = parseMarkdown("> 引用一句\n\n---\n")
        assertTrue(blocks[0] is MdBlock.Quote)
        assertTrue(blocks[1] is MdBlock.Rule)
    }
}
