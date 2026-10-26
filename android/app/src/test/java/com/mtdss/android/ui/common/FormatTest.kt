package com.mtdss.android.ui.common

import com.mtdss.android.test.TestKit
import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.Calendar

/**
 * 展示格式化 JVM 单测(口径与桌面 ui/src/lib/browse.ts 的 vitest 用例同源:
 * 1024 进制一位小数、字节段整数、目录空串、mtime 本地时区不补零形态)。
 */
class FormatTest {

    @Test
    fun `size 目录与零`() = TestKit.runTest {
        assertEquals("", Format.size(123456, isDir = true))
        assertEquals("0 B", Format.size(0, isDir = false))
    }

    @Test
    fun `size 1024 进制一位小数不剪尾`() = TestKit.runTest {
        assertEquals("1 B", Format.size(1, isDir = false))
        assertEquals("999 B", Format.size(999, isDir = false))
        assertEquals("1.0 KB", Format.size(1024, isDir = false))
        assertEquals("1.5 KB", Format.size(1536, isDir = false))
        assertEquals("2.0 MB", Format.size(2 * 1024 * 1024, isDir = false))
        assertEquals("1.0 GB", Format.size(1024L * 1024 * 1024, isDir = false))
        assertEquals("1.0 TB", Format.size(1024L * 1024 * 1024 * 1024, isDir = false))
        assertEquals("1.0 PB", Format.size(1024L * 1024 * 1024 * 1024 * 1024, isDir = false))
        // 超出表尾单位(PB)封顶不再递进
        assertEquals("8.0 PB", Format.size(8 * 1024L * 1024 * 1024 * 1024 * 1024, isDir = false))
    }

    @Test
    fun `mtime 零为空串其余本地时区不补零`() = TestKit.runTest {
        assertEquals("", Format.mtime(0))
        fun at(y: Int, month: Int, day: Int, h: Int, min: Int): Long {
            val cal = Calendar.getInstance()
            cal.clear()
            cal.set(y, month, day, h, min)
            return cal.timeInMillis
        }

        assertEquals("2026/10/18 9:05", Format.mtime(at(2026, Calendar.OCTOBER, 18, 9, 5)))
        assertEquals("2026/12/31 23:59", Format.mtime(at(2026, Calendar.DECEMBER, 31, 23, 59)))
        assertEquals("2026/1/2 10:05", Format.mtime(at(2026, Calendar.JANUARY, 2, 10, 5)))
    }
}
