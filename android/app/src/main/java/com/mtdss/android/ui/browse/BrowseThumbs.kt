package com.mtdss.android.ui.browse

import com.mtdss.android.ipc.DirEntry

/**
 * 浏览缩略图纯函数面(V4-1 T22;钦定 mockup mobile-tiles-v2/mobile-browse
 * .html):缩略图资格矩阵 / 回环 URL 组装 / 时长角标格式化。零 Android/
 * Compose 依赖,宿主 JVM 单测直驱([BrowseThumbsTest])。
 */
object BrowseThumbs {

    /**
     * 缩略图请求宽(brief 钉死 `?w=256`;daemon 侧 loopback /thumb 的
     * THUMB_DEFAULT_WIDTH 同值锚,两处不得漂移)。
     */
    const val THUMB_WIDTH = 256

    /** 平铺列数(钦定 mockup:360dp 屏宽一行 4 枚 ≈76dp 磁贴,严格内收)。 */
    const val GRID_COLUMNS = 4

    /** 图片扩展名词表(与桌面 ui/src/lib/filetype.ts img 类同源,小写存取)。 */
    private val IMAGE_EXTS =
        setOf("jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif")

    /** 视频扩展名词表(与桌面 filetype.ts vid 类同源)。 */
    private val VIDEO_EXTS =
        setOf("mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp")

    /** 该条目是否出缩略图(图片/视频文件;目录/其余 = 类型图标占位)。 */
    fun thumbEligible(name: String, isDir: Boolean): Boolean =
        !isDir && (extOf(name).let { it in IMAGE_EXTS || it in VIDEO_EXTS })

    /** 该条目是否视频(时长角标只落视频,图片有 secs 也不显,mockup 同)。 */
    fun isVideoName(name: String): Boolean = extOf(name) in VIDEO_EXTS

    /**
     * 时长角标串(桌面 ui/src/lib/duration.ts fmtDuration 逐字镜像):
     * `m:ss`(不满 1 小时,分位不补零)/ `h:mm:ss`(满 1 小时,时分位补零);
     * 小数向下取整;null/非正/非数 → 空串(有则显无则不显,角标不落垃圾)。
     */
    fun durationBadge(secs: Double?): String {
        if (secs == null || !secs.isFinite() || secs <= 0.0) return ""
        val total = secs.toLong() // 正数 = floor
        val ss = (total % 60).toString().padStart(2, '0')
        val mins = total / 60 % 60
        val hours = total / 3600
        return if (hours == 0L) "$mins:$ss" else "$hours:${mins.toString().padStart(2, '0')}:$ss"
    }

    /** 条目时长角标(视频名 + secs 在场才显;其余空串)。 */
    fun durationBadgeFor(name: String, secs: Double?): String =
        if (isVideoName(name)) durationBadge(secs) else ""

    /**
     * 缩略图回环 URL:`http://127.0.0.1:{port}/f/{handle}/thumb?w={w}&t={token}`
     * (?t= 查询令牌与 Bearer 双缝等价,T20 面;Coil `<img>` 等直连不带头的
     * 消费面靠它,与桌面 previewer bundle 同一凭据缝)。
     */
    fun thumbUrl(port: Long, handle: String, token: String, w: Int = THUMB_WIDTH): String =
        "http://127.0.0.1:$port/f/$handle/thumb?w=$w&t=$token"

    /** 取小写扩展名(与桌面 filetype.ts extOf 同口径):无点/点在首位(点
     *  文件)/点在末尾 → 空串(视为无扩展名)。 */
    private fun extOf(name: String): String {
        val i = name.lastIndexOf('.')
        if (i <= 0 || i == name.length - 1) return ""
        return name.substring(i + 1).lowercase()
    }
}
