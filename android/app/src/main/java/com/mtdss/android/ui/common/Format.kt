package com.mtdss.android.ui.common

import java.util.Calendar

/**
 * 展示格式化(纯函数,镜像桌面 ui/src/lib/browse.ts 的 fmtSize/fmtMtime,
 * 双端口径一致):
 * - size:目录 → "";0 → "0 B";其余 1024 进制、一位小数不剪尾("2.0 MB"),
 *   字节段整数,超出表尾单位(PB)封顶;
 * - mtime:0 → "";否则本地时区 `Y/M/D H:MM`(月/日/时不补零,分钟两位)。
 */
object Format {

    private val UNITS = arrayOf("B", "KB", "MB", "GB", "TB", "PB")

    fun size(bytes: Long, isDir: Boolean): String {
        if (isDir) return ""
        if (bytes == 0L) return "0 B"
        var value = bytes.toDouble()
        var i = 0
        while (i < UNITS.size - 1 && value >= 1024) {
            value /= 1024
            i++
        }
        // 字节段恒整数(daemon 的 size 是 u64;<1024 才停在本段)
        return if (i == 0) "$bytes ${UNITS[0]}" else String.format(java.util.Locale.ROOT, "%.1f %s", value, UNITS[i])
    }

    fun mtime(ms: Long): String {
        if (ms == 0L) return ""
        val cal = Calendar.getInstance()
        cal.timeInMillis = ms
        return buildString {
            append(cal.get(Calendar.YEAR)).append('/')
            append(cal.get(Calendar.MONTH) + 1).append('/')
            append(cal.get(Calendar.DAY_OF_MONTH)).append(' ')
            append(cal.get(Calendar.HOUR_OF_DAY)).append(':')
            append(String.format(java.util.Locale.ROOT, "%02d", cal.get(Calendar.MINUTE)))
        }
    }
}
