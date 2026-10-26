package com.mtdss.android.ipc

/**
 * 极简 JSON 值模型 + 递归下降解析器 + 规范化写出(零外部依赖,实机/JVM 单测
 * 同一份实现,不引入 org.json 双源口径差)。
 *
 * ## 规范写出 = serde_json 字节口径(契约关键,ContractGoldenTest 逐字节锁定)
 * daemon 侧 serde_json 默认 BTreeMap 键序 = **字母序**(UTF-8 字节序);请求帧
 * 必须与之逐字节一致,故 [toCanonicalJson] 递归按键排序写出。转义面与
 * serde_json 对齐:仅 `"`、`\` 与 <0x20 控制字符(`\b\f\n\r\t` 快捷式,其余
 * `\u00xx` 小写十六进制);非 ASCII(含 CJK)原样直出、不转义 `/`。
 *
 * ## 数值以原文([JVal.Num.raw])保留
 * 解析往返无损(应答值不再上线的场景);构造侧整型经 [num] 生成十进制原文,
 * 与 Rust i64 Display 一致。浮点字段不在契约面(全字段整型/字符串/布尔)。
 */
sealed class JVal {
    object Null : JVal()
    data class Bool(val v: Boolean) : JVal()

    /** 数值原文(如 "1"、"1760000000000"、"2.5");整型取值经 [asLong]/[asInt]。 */
    data class Num(val raw: String) : JVal()
    data class Str(val v: String) : JVal()
    data class Arr(val items: List<JVal>) : JVal()

    /** JSON 对象;插入序保留(构造便利),规范化写出时统一按字母序。 */
    class Obj(val entries: LinkedHashMap<String, JVal>) : JVal() {
        constructor(vararg pairs: Pair<String, JVal>) : this(linkedMapOf(*pairs))
        operator fun get(key: String): JVal? = entries[key]
        val size: Int get() = entries.size
        override fun equals(other: Any?): Boolean = other is Obj && other.entries == entries
        override fun hashCode(): Int = entries.hashCode()
    }

    companion object {
        fun str(v: String): JVal = Str(v)
        fun bool(v: Boolean): JVal = Bool(v)
        fun num(v: Long): JVal = Num(v.toString())
        fun num(v: Int): JVal = Num(v.toString())

        /** 解析标准 JSON 文本;非法输入抛 [IllegalArgumentException]。 */
        fun parse(text: String): JVal = Parser(text).parseRoot()
    }
}

/** 规范化写出:对象键递归按字母序(serde_json BTreeMap 口径),转义见类注释。 */
fun JVal.toCanonicalJson(): String {
    val sb = StringBuilder()
    writeCanonicalTo(sb)
    return sb.toString()
}

private fun JVal.writeCanonicalTo(sb: StringBuilder) {
    when (this) {
        is JVal.Null -> sb.append("null")
        is JVal.Bool -> sb.append(if (v) "true" else "false")
        is JVal.Num -> sb.append(raw)
        is JVal.Str -> writeJsonString(sb, v)
        is JVal.Arr -> {
            sb.append('[')
            items.forEachIndexed { i, item ->
                if (i > 0) sb.append(',')
                item.writeCanonicalTo(sb)
            }
            sb.append(']')
        }
        is JVal.Obj -> {
            sb.append('{')
            var first = true
            for (key in entries.keys.sorted()) {
                if (!first) sb.append(',')
                first = false
                writeJsonString(sb, key)
                sb.append(':')
                entries.getValue(key).writeCanonicalTo(sb)
            }
            sb.append('}')
        }
    }
}

/** serde_json 兼容字符串转义:仅 " \ 与 <0x20 控制;非 ASCII 原样直出。 */
private fun writeJsonString(sb: StringBuilder, v: String) {
    sb.append('"')
    for (c in v) {
        when {
            c == '"' -> sb.append("\\\"")
            c == '\\' -> sb.append("\\\\")
            c == '\b' -> sb.append("\\b")
            c == '\u000C' -> sb.append("\\f")
            c == '\n' -> sb.append("\\n")
            c == '\r' -> sb.append("\\r")
            c == '\t' -> sb.append("\\t")
            c.code < 0x20 -> sb.append("\\u").append(String.format("%04x", c.code))
            else -> sb.append(c)
        }
    }
    sb.append('"')
}

/** 递归下降解析器(索引推进;不支持注释/尾逗号,标准 JSON 严格口径)。 */
private class Parser(private val s: String) {
    private var i = 0

    fun parseRoot(): JVal {
        ws()
        val v = value()
        ws()
        require(i >= s.length) { "JSON 尾部多余内容 @$i" }
        return v
    }

    private fun ws() {
        while (i < s.length && s[i].isWhitespace()) i++
    }

    private fun value(): JVal {
        require(i < s.length) { "JSON 意外结束" }
        return when (s[i]) {
            '{' -> obj()
            '[' -> arr()
            '"' -> JVal.Str(string())
            't' -> lit("true", JVal.Bool(true))
            'f' -> lit("false", JVal.Bool(false))
            'n' -> lit("null", JVal.Null)
            else -> number()
        }
    }

    private fun lit(word: String, v: JVal): JVal {
        require(s.regionMatches(i, word, 0, word.length)) { "非法字面量 @$i" }
        i += word.length
        return v
    }

    private fun obj(): JVal.Obj {
        i++ // '{'
        val map = LinkedHashMap<String, JVal>()
        ws()
        if (i < s.length && s[i] == '}') {
            i++
            return JVal.Obj(map)
        }
        while (true) {
            ws()
            require(i < s.length && s[i] == '"') { "对象键须为字符串 @$i" }
            val key = string()
            ws()
            require(i < s.length && s[i] == ':') { "缺 ':' @$i" }
            i++
            ws()
            map[key] = value()
            ws()
            require(i < s.length) { "对象未闭合" }
            when (s[i]) {
                ',' -> i++
                '}' -> {
                    i++
                    return JVal.Obj(map)
                }
                else -> throw IllegalArgumentException("对象内非法字符 '${s[i]}' @$i")
            }
        }
    }

    private fun arr(): JVal.Arr {
        i++ // '['
        val items = ArrayList<JVal>()
        ws()
        if (i < s.length && s[i] == ']') {
            i++
            return JVal.Arr(items)
        }
        while (true) {
            ws()
            items.add(value())
            ws()
            require(i < s.length) { "数组未闭合" }
            when (s[i]) {
                ',' -> i++
                ']' -> {
                    i++
                    return JVal.Arr(items)
                }
                else -> throw IllegalArgumentException("数组内非法字符 '${s[i]}' @$i")
            }
        }
    }

    /** 字符串字面量(调用时 i 指向开引号);含 \uXXXX(代理对按两码元原样拼接)。 */
    private fun string(): String {
        i++ // '"'
        val sb = StringBuilder()
        while (true) {
            require(i < s.length) { "字符串未闭合" }
            when (val c = s[i]) {
                '"' -> {
                    i++
                    return sb.toString()
                }
                '\\' -> {
                    i++
                    require(i < s.length) { "转义未闭合" }
                    when (val e = s[i]) {
                        '"' -> sb.append('"')
                        '\\' -> sb.append('\\')
                        '/' -> sb.append('/')
                        'b' -> sb.append('\b')
                        'f' -> sb.append('\u000C')
                        'n' -> sb.append('\n')
                        'r' -> sb.append('\r')
                        't' -> sb.append('\t')
                        'u' -> {
                            require(i + 5 <= s.length) { "\\u 转义不完整 @$i" }
                            val hex = s.substring(i + 1, i + 5)
                            val code = hex.toIntOrNull(16)
                                ?: throw IllegalArgumentException("非法 \\u 转义 \\u$hex @$i")
                            sb.append(code.toChar())
                            i += 4
                        }
                        else -> throw IllegalArgumentException("非法转义 '\\$e' @$i")
                    }
                    i++
                }
                else -> {
                    require(c.code >= 0x20) { "字符串含未转义控制字符 @$i" }
                    sb.append(c)
                    i++
                }
            }
        }
    }

    /** 数值字面量:按原文捕获([-]?digits[.digits][[eE][+-]?digits])。 */
    private fun number(): JVal.Num {
        val start = i
        if (i < s.length && s[i] == '-') i++
        require(i < s.length && s[i].isDigit()) { "非法数值 @$i" }
        while (i < s.length && s[i].isDigit()) i++
        if (i < s.length && s[i] == '.') {
            i++
            require(i < s.length && s[i].isDigit()) { "小数部分缺数字 @$i" }
            while (i < s.length && s[i].isDigit()) i++
        }
        if (i < s.length && (s[i] == 'e' || s[i] == 'E')) {
            i++
            if (i < s.length && (s[i] == '+' || s[i] == '-')) i++
            require(i < s.length && s[i].isDigit()) { "指数部分缺数字 @$i" }
            while (i < s.length && s[i].isDigit()) i++
        }
        return JVal.Num(s.substring(start, i))
    }
}

// ── 类型化访问助手(解析侧;与键序无关)──────────────────────────────────

/** 取对象字段;非对象或缺键 → null。 */
fun JVal.opt(key: String): JVal? = (this as? JVal.Obj)?.entries?.get(key)

/** 取必填字段;缺失即契约违规。 */
fun JVal.req(key: String): JVal = opt(key) ?: throw ContractViolationException("缺少字段: $key")

fun JVal.asString(): String = (this as? JVal.Str)?.v
    ?: throw ContractViolationException("非字符串值: ${toCanonicalJson()}")

/** 可空投影:接收者 null(键缺失)或 JSON null → null;其余类型错即违规。 */
fun JVal?.asStringOrNull(): String? = when (this) {
    is JVal.Str -> v
    is JVal.Null, null -> null
    else -> throw ContractViolationException("非字符串/null 值: ${toCanonicalJson()}")
}

fun JVal.asLong(): Long = (this as? JVal.Num)?.raw?.toLongOrNull()
    ?: throw ContractViolationException("非整数值: ${toCanonicalJson()}")

fun JVal.asInt(): Int = asLong().toInt()

fun JVal.asBoolean(): Boolean = (this as? JVal.Bool)?.v
    ?: throw ContractViolationException("非布尔值: ${toCanonicalJson()}")

fun JVal.asArray(): List<JVal> = (this as? JVal.Arr)?.items
    ?: throw ContractViolationException("非数组值: ${toCanonicalJson()}")

fun JVal.asObject(): JVal.Obj = this as? JVal.Obj
    ?: throw ContractViolationException("非对象值: ${toCanonicalJson()}")
