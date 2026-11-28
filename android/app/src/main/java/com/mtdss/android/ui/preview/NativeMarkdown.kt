package com.mtdss.android.ui.preview

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * r70k:原生 Markdown 渲染(零依赖手写,配合 [NativeDocViewers] 文本族——
 * WebView 在老 WebView 设备物理屏不上屏的终局解,md 同走原生)。
 *
 * 覆盖面(常见工作日志/文档的主干):
 * - 块级:`#`-`######` 标题(分级字号)/ 无序有序列表(2 空格缩进嵌套)/
 *   `>` 引用(竖线 + 静默色)/ ``` 代码围栏(等宽 + 浅底)/ `---` 分隔线 /
 *   段落(空行分段);
 * - 行内:`**粗**` / `*斜*` / `` `码` `` / `[文](链)`(样式化,暂不点击)/
 *   任务勾选 `- [ ]`→☐、`- [x]`→☑;
 * - 表格暂按原文段落等宽降级(工作日志场景少见;后续票)。
 */
// ── 数据面 ────────────────────────────────────────────────────────────────

/** 行内样式段。 */
internal data class MdSpan(
    val text: String,
    val bold: Boolean = false,
    val italic: Boolean = false,
    val code: Boolean = false,
    val link: Boolean = false,
)

/** 块级元素。 */
internal sealed class MdBlock {
    data class Heading(val level: Int, val spans: List<MdSpan>) : MdBlock()
    data class Para(val spans: List<MdSpan>) : MdBlock()
    data class ListItem(val indent: Int, val marker: String, val spans: List<MdSpan>) : MdBlock()
    data class Quote(val spans: List<MdSpan>) : MdBlock()
    data class Code(val lines: List<String>) : MdBlock()
    data object Rule : MdBlock()
}

// ── 解析面 ────────────────────────────────────────────────────────────────

/** 行内解析:顺序扫描粗/码/链接/斜;不闭合标记按原文容错。 */
internal fun parseInline(src: String): List<MdSpan> {
    val out = mutableListOf<MdSpan>()
    val plain = StringBuilder()
    var i = 0

    fun flush() {
        if (plain.isNotEmpty()) {
            out.add(MdSpan(plain.toString()))
            plain.setLength(0)
        }
    }

    while (i < src.length) {
        val c = src[i]
        when {
            src.startsWith("**", i) -> {
                val end = src.indexOf("**", i + 2)
                if (end > 0) {
                    flush(); out.add(MdSpan(src.substring(i + 2, end), bold = true)); i = end + 2
                } else {
                    plain.append(c); i += 1
                }
            }
            c == '`' -> {
                val end = src.indexOf('`', i + 1)
                if (end > 0) {
                    flush(); out.add(MdSpan(src.substring(i + 1, end), code = true)); i = end + 1
                } else {
                    plain.append(c); i += 1
                }
            }
            c == '[' -> {
                val close = src.indexOf(']', i + 1)
                if (close > 0 && close + 1 < src.length && src[close + 1] == '(') {
                    val end = src.indexOf(')', close + 2)
                    if (end > 0) {
                        flush(); out.add(MdSpan(src.substring(i + 1, close), link = true))
                        i = end + 1
                    } else {
                        plain.append(c); i += 1
                    }
                } else {
                    plain.append(c); i += 1
                }
            }
            c == '*' && (i + 1 >= src.length || src[i + 1] != '*') -> {
                // 斜体闭合不能是 ** 的首星(内容非空且闭合星后不再跟星)
                var end = src.indexOf('*', i + 1)
                while (end > 0 && end + 1 < src.length && src[end + 1] == '*') {
                    end = src.indexOf('*', end + 1)
                }
                val content = if (end > i + 1) src.substring(i + 1, end) else ""
                if (end > i + 1 && content.isNotBlank() && !content.startsWith(" ") && !content.endsWith(" ")) {
                    flush(); out.add(MdSpan(content, italic = true)); i = end + 1
                } else {
                    plain.append(c); i += 1
                }
            }
            else -> {
                plain.append(c); i += 1
            }
        }
    }
    flush()
    return out
}

/** 块级解析(行扫描;代码围栏优先,标题/引用/列表/分隔线/段落)。 */
internal fun parseMarkdown(src: String): List<MdBlock> {
    val blocks = mutableListOf<MdBlock>()
    val lines = src.split('\n')
    var para = mutableListOf<String>()

    fun flushPara() {
        if (para.isNotEmpty()) {
            blocks.add(MdBlock.Para(parseInline(para.joinToString(" "))))
            para = mutableListOf()
        }
    }

    var i = 0
    while (i < lines.size) {
        val line = lines[i].trimEnd()
        when {
            line.startsWith("```") -> {
                flushPara()
                val fence = mutableListOf<String>()
                i += 1
                while (i < lines.size && !lines[i].trimEnd().startsWith("```")) {
                    fence.add(lines[i])
                    i += 1
                }
                blocks.add(MdBlock.Code(fence))
            }
            line.isEmpty() -> flushPara()
            line.startsWith("#") -> {
                flushPara()
                val level = line.takeWhile { it == '#' }.length.coerceAtMost(6)
                blocks.add(MdBlock.Heading(level, parseInline(line.drop(level).trim())))
            }
            line.startsWith(">") -> {
                flushPara()
                blocks.add(MdBlock.Quote(parseInline(line.drop(1).trim())))
            }
            line.trim() == "---" || line.trim() == "***" -> {
                flushPara()
                blocks.add(MdBlock.Rule)
            }
            else -> {
                val t = line.trimStart()
                val indent = (line.length - t.length) / 2
                val bullet = when {
                    t.startsWith("- ") || t.startsWith("* ") || t.startsWith("+ ") -> t.take(1)
                    else -> null
                }
                val ordered = Regex("^\\d+\\. ").find(t)?.value
                when {
                    bullet != null -> {
                        flushPara()
                        blocks.add(MdBlock.ListItem(indent, bullet, parseInline(t.drop(2).trim())))
                    }
                    ordered != null -> {
                        flushPara()
                        blocks.add(
                            MdBlock.ListItem(
                                indent,
                                ordered.trimEnd(),
                                parseInline(t.drop(ordered.length).trim()),
                            ),
                        )
                    }
                    else -> para.add(line.trim())
                }
            }
        }
        i += 1
    }
    flushPara()
    return blocks
}

// ── 渲染面 ────────────────────────────────────────────────────────────────

private val MutedColor = androidx.compose.ui.graphics.Color(0xFF8A8A85)
private val CodeBgColor = androidx.compose.ui.graphics.Color(0xFFEDEDE8)
private val LinkColor = androidx.compose.ui.graphics.Color(0xFF0B62C4)

/** md 渲染体:块级 Column。 */
@Composable
internal fun MarkdownBody(src: String, modifier: Modifier = Modifier) {
    val blocks = remember(src) { parseMarkdown(src) }
    Column(modifier) {
        blocks.forEachIndexed { index, block ->
            when (block) {
                is MdBlock.Heading -> Text(
                    remember(block) { buildAnnotated(block.spans) },
                    Modifier.padding(top = if (index == 0) 0.dp else 12.dp, bottom = 4.dp),
                    fontSize = when (block.level) {
                        1 -> 22.sp
                        2 -> 19.sp
                        3 -> 17.sp
                        else -> 15.sp
                    },
                    lineHeight = when (block.level) {
                        1 -> 28.sp
                        2 -> 25.sp
                        else -> 23.sp
                    },
                    color = InkColor,
                    fontWeight = FontWeight.Bold,
                )

                is MdBlock.Para -> Text(
                    remember(block) { buildAnnotated(block.spans) },
                    Modifier.padding(vertical = 4.dp),
                    fontSize = 14.sp,
                    lineHeight = 22.sp,
                )

                is MdBlock.ListItem -> Row(
                    Modifier.padding(
                        start = (block.indent * 16).dp,
                        top = 3.dp,
                        bottom = 3.dp,
                    ),
                ) {
                    Text(
                        block.marker,
                        Modifier.padding(end = 8.dp),
                        fontSize = 14.sp,
                        lineHeight = 22.sp,
                        color = MutedColor,
                    )
                    Text(
                        remember(block) { buildAnnotated(block.spans) },
                        Modifier.weight(1f),
                        fontSize = 14.sp,
                        lineHeight = 22.sp,
                    )
                }

                is MdBlock.Quote -> Row(Modifier.padding(vertical = 4.dp)) {
                    Box(
                        Modifier
                            .width(3.dp)
                            .height(20.dp)
                            .background(MutedColor)
                            .align(Alignment.Top),
                    )
                    Text(
                        remember(block) { buildAnnotated(block.spans) },
                        Modifier.padding(start = 10.dp),
                        fontSize = 14.sp,
                        lineHeight = 22.sp,
                        color = MutedColor,
                    )
                }

                is MdBlock.Code -> Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 6.dp)
                        .background(CodeBgColor, RoundedCornerShape(6.dp))
                        .padding(10.dp),
                ) {
                    block.lines.forEach { ln ->
                        Text(
                            ln,
                            fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                            fontSize = 12.sp,
                            lineHeight = 18.sp,
                            color = InkColor,
                        )
                    }
                }

                MdBlock.Rule -> Box(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 10.dp)
                        .height(1.dp)
                        .background(CodeBgColor),
                )
            }
        }
    }
}

/** 行内段 → AnnotatedString(粗/斜/码/链接样式;勾选框 ☐/☑ 预处理)。 */
private fun buildAnnotated(spans: List<MdSpan>): AnnotatedString {
    val builder = AnnotatedString.Builder()
    for (span in spans) {
        var text = span.text
        if (text == "[ ]") text = "☐"
        if (text.equals("[x]", ignoreCase = true)) text = "☑"
        if (span.code) builder.pushStyle(
            SpanStyle(
                fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                background = CodeBgColor,
            ),
        )
        if (span.bold) builder.pushStyle(SpanStyle(fontWeight = FontWeight.Bold))
        if (span.italic) builder.pushStyle(SpanStyle(fontStyle = FontStyle.Italic))
        if (span.link) builder.pushStyle(
            SpanStyle(color = LinkColor, textDecoration = TextDecoration.Underline),
        )
        builder.append(text)
        if (span.link) builder.pop()
        if (span.italic) builder.pop()
        if (span.bold) builder.pop()
        if (span.code) builder.pop()
    }
    return builder.toAnnotatedString()
}
