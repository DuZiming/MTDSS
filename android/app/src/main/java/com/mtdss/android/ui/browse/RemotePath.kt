package com.mtdss.android.ui.browse

/**
 * 远端路径工具(纯函数,移植桌面 ui/src/lib/nav.ts 的 splitPath/parentPath/
 * pathSegments——DirEntry.path 由 daemon 按目标平台原生分隔符下发,Windows
 * 反斜杠、POSIX 正斜杠,严禁以 '/' 单独切分)。
 *
 * 边界语义(Android 三层导航适配):根路径(POSIX "/"、Windows 盘根
 * "C:\"/卷条目 "C:")的上级是**根卷列表**([parentOf] 返回 "");卷列表的
 * 上级才是 null(设备层由 AppNav 路由栈承担,不进路径模型)。桌面侧
 * 盘根的上级直接是设备层——Android 多一层卷列表,故根形态映射到 ""。
 * 尾分隔符与无尾分隔等价("C:\a\" ≡ "C:\a")。
 */
object RemotePath {

    /** 父目录路径;卷列表("")的上级返回 null,根路径返回 ""(卷列表)。 */
    fun parentOf(path: String): String? {
        if (path.isEmpty()) return null
        val (sep, segs) = split(path)
        // 根形态:POSIX 根(剥空段后仅前导空段)与 Windows 盘根/卷条目(单段,
        // 含 "C:" 无尾分隔的卷条目形态——分隔符探测对它失效,按盘符形特判)
        val isRoot = when (sep) {
            '/' -> path == "/" || DRIVE_FORM.matches(path)
            else -> segs.size == 1 // "C:" / "C:\"
        }
        if (isRoot) return ""
        if (segs.size <= 1) return null
        // Windows 恰两段("C:\a")父为盘根,规范形带尾分隔符("C:\")
        if (sep == '\\' && segs.size == 2) return segs[0] + "\\"
        val parent = segs.dropLast(1).joinToString(sep.toString())
        // POSIX 绝对路径剥到只剩前导空段时,父即根 "/"
        if (parent.isEmpty() && path.startsWith(sep)) return sep.toString()
        return parent
    }

    /** 路径段列表(面包屑数据源;POSIX 前导空段呈现为独立的 "/" 段)。 */
    fun segments(path: String): List<String> {
        if (path.isEmpty()) return emptyList()
        val (sep, segs) = split(path)
        if (sep == '/' && segs.isNotEmpty() && segs[0].isEmpty()) return listOf("/") + segs.drop(1)
        return segs
    }

    /** 分解:按含 '\' 探测分隔符;剥尾部分隔符产生的空段。 */
    private fun split(path: String): Pair<Char, List<String>> {
        val sep: Char = if (path.contains('\\')) '\\' else '/'
        var segs = path.split(sep)
        while (segs.size > 1 && segs.last().isEmpty()) segs = segs.dropLast(1)
        return sep to segs
    }

    /** Windows 卷条目形(单盘符段,无尾分隔:"C:")。 */
    private val DRIVE_FORM = Regex("^[A-Za-z]:$")
}
