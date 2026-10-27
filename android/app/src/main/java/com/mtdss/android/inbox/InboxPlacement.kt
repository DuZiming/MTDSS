package com.mtdss.android.inbox

/**
 * 收件箱落位映射(spec §5.1「收件箱相对路径语义不变——嵌套目录传输
 * 目录结构保真」;纯函数,JVM 钉死):rel 的段映射进**多段
 * RELATIVE_PATH(恒带尾 '/',见下「尾斜杠」)**,终名只取末段——
 *
 * - 顶层文件 `a.bin` → RELATIVE_PATH `Download/MTDSS/` + DISPLAY_NAME `a.bin`
 * - 嵌套 `docs/a.bin` → RELATIVE_PATH `Download/MTDSS/docs/` + DISPLAY_NAME
 *   `a.bin`(目录结构保真)
 *
 * 由此消除旧平铺映射(rel 的 '/' 归一 "__")的两类缺陷:嵌套结构丢失/
 * 文件名改写,以及**非单射碰撞**(`docs/a.bin` 撞顶层 `docs__a.bin`、
 * `a/b/c.bin` 撞 `a/b__c.bin`)——碰撞会使 create 前的同名 pending 清理
 * 误删另一 rel 的在途暂存(daemon 向死 Uri 的 fd 写,reveal 静默失败)。
 * 落位键 = (RELATIVE_PATH, DISPLAY_NAME) 二元组,合法 rel 两两互异。
 *
 * ## 尾斜杠(F12 根因,统一 F3/F4/F10/F11)
 * MediaProvider(MIUI/AOSP 同)插入时把 RELATIVE_PATH 归一化为**恒带
 * 尾 '/'**(content query 实证:`relative_path=Download/MTDSS/`);本仓
 * 旧版落位串不带尾 '/',一切 `RELATIVE_PATH=?` 等值查询(既有件的
 * pending 重查/预清理/在场查询/验尸)全数落空——单字符统一了整案:
 * F3 pending 失联(行未失,重查看不见 → NOT_FOUND 拒开 → FULL)、F4
 * 孤儿、F10/F11 验尸无行,以及插入撞名(provider 按归一化路径匹配
 * 重名,撞 done 行不自动改名)。落位串自此恒用归一化形态,插查同言。
 *
 * ## 段消毒(拒开口径)
 * daemon 侧 manifest 已过 `validate_rel_path`(拒 .mtdss 首段/反斜杠/
 * 盘符冒号/空段/越界 ".."),此处是第二道防御(同口径 + MediaStore 约束):
 * - 拒空 rel、超长(>4096)、空段、"." 段;
 * - 拒任意位置的 ".."(**严于 daemon**:daemon 的深度计允许非逃逸 "a/../b",
 *   但 MediaStore RELATIVE_PATH 无法表达 ".." 段——此类 rel 一律拒开,
 *   daemon 侧任务失败带因,不产生静默错位);
 * - 拒段内反斜杠、冒号(镜像 daemon)、NUL(MediaStore/SQLite 面纯防御)。
 * 消毒不过 → [place] 返回 null,调用方按拒开处理。
 */
object InboxPlacement {

    /** 收件箱根:Downloads 集合下的 MTDSS 子树基段(**不含**尾 '/';
     *  落位串由 [place] 拼装并恒带尾 '/'——MediaProvider 归一化形态,F12)。 */
    const val BASE_RELATIVE_PATH: String = "Download/MTDSS"

    /** rel 在收件箱内的落位(查询/插入/清理的统一键)。 */
    data class Placement(
        /** MediaStore RELATIVE_PATH(多段,目录结构保真)。 */
        val relativePath: String,
        /** 终段文件名(DISPLAY_NAME)。 */
        val displayName: String,
    )

    /** rel → 落位;消毒不过返回 null(拒开)。 */
    fun place(rel: String): Placement? {
        // 与 daemon validate_rel_path 同口径的防御(见类注释「段消毒」)
        if (rel.isEmpty() || rel.length > MAX_REL_BYTES) return null
        if (rel.contains('\\') || rel.contains(':') || rel.contains('\u0000')) return null
        val segments = rel.split('/')
        if (segments.first().equals(".mtdss", ignoreCase = true)) return null
        for (seg in segments) {
            if (seg.isEmpty()) return null // 含首 '/'/尾 '/'/连续 '/'
            if (seg == "." || seg == "..") return null
        }
        val display = segments.last()
        val dirs = segments.dropLast(1)
        // 落位串恒带尾 '/'(MediaProvider 插入侧的归一化形态,F12 根因):
        // 等值查询必须与归一化后的存储同言,否则查无此行(插得进、查不见)
        val relativePath = buildString {
            append(BASE_RELATIVE_PATH).append('/')
            if (dirs.isNotEmpty()) append(dirs.joinToString("/")).append('/')
        }
        return Placement(relativePath, display)
    }

    private const val MAX_REL_BYTES = 4096
}
