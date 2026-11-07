package com.mtdss.android.ui.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import com.mtdss.android.ui.common.entrySubtitle
import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.Calendar

/**
 * Synshu 主题 token 层 JVM 单测(spec 2026-11-08-mtdss-v3p4 §2 逐值钉子):
 * - 浅/深配色与语义色常量逐值断言(spec §2.1 全表;语义色不随主题漂移);
 * - 浅/深 M3 ColorScheme 挂接(全 token 覆写后的关键槽位);
 * - 字阶(spec §2.2 规格值);
 * - `entrySubtitle` 行副文格式(spec §2.3「日期 · N 项」「日期 · 大小」;
 *   视觉稿 06-detail-screens 逐字面:今天/昨天/M月d日 + 「 · 」)。
 */
class SynshuThemeTest {

    // ── 断言工具 ─────────────────────────────────────────────────────────────

    /** rgba 分量断言(发丝线/玻璃底等带 alpha 的 token)。 */
    private fun assertRgba(c: Color, r: Int, g: Int, b: Int, a: Float) {
        assertEquals("red", r / 255f, c.red, 0.004f)
        assertEquals("green", g / 255f, c.green, 0.004f)
        assertEquals("blue", b / 255f, c.blue, 0.004f)
        assertEquals("alpha", a, c.alpha, 0.004f)
    }

    /** 本地时区定点时刻(与实现同用 Calendar,任一时区下自洽)。 */
    private fun at(y: Int, month: Int, day: Int, h: Int, min: Int): Long {
        val cal = Calendar.getInstance()
        cal.clear()
        cal.set(y, month, day, h, min, 0)
        return cal.timeInMillis
    }

    // ── 浅色 token(spec §2.1 浅色表逐值)────────────────────────────────────

    @Test
    fun `浅色 token 逐值`() {
        assertEquals(Color(0xFF007AFF), SynshuColors.Accent)
        assertEquals(Color(0xFFFFFFFF), SynshuColors.Background)
        assertEquals(Color(0xFFF0F0F3), SynshuColors.InputField)
        assertEquals(Color(0xFFF2F2F7), SynshuColors.SelectedRow)
        assertEquals(Color(0xFFF2F2F7), SynshuColors.GroupCard)
        assertEquals(Color(0xFF1C1C1E), SynshuColors.TextPrimary)
        assertEquals(Color(0xFF8E8E93), SynshuColors.TextSecondary)
        // 发丝线 rgba(60,60,67,.2)
        assertRgba(SynshuColors.Hairline, 60, 60, 67, 0.2f)
        assertEquals(Color(0xFFB4B4B9), SynshuColors.Disabled)
        assertEquals(Color(0xFFFF3B30), SynshuColors.DestructiveRed)
        assertEquals(Color(0xFF34A853), SynshuColors.OnlineDot)
        // 胶囊毛玻璃:白 72%(spec §2.2)
        assertRgba(SynshuColors.CapsuleGlass, 255, 255, 255, 0.72f)
    }

    // ── 深色 token(spec §2.1 深色表逐值)────────────────────────────────────

    @Test
    fun `深色 token 逐值`() {
        // 背景纵向渐变两端
        assertEquals(Color(0xFF0D0F15), SynshuColorsDark.BackgroundStart)
        assertEquals(Color(0xFF12141C), SynshuColorsDark.BackgroundEnd)
        assertEquals(Color(0xFF4E9AFF), SynshuColorsDark.Accent)
        assertEquals(Color(0xFFE5E6EB), SynshuColorsDark.TextPrimary)
        assertEquals(Color(0xFF989AA6), SynshuColorsDark.TextSecondary)
        // 分割线 rgba(255,255,255,.09);选中行 rgba(255,255,255,.06)
        assertRgba(SynshuColorsDark.Hairline, 255, 255, 255, 0.09f)
        assertRgba(SynshuColorsDark.SelectedRow, 255, 255, 255, 0.06f)
        // 胶囊深色毛玻璃 rgba(28,30,40,.74)
        assertRgba(SynshuColorsDark.CapsuleGlass, 28, 30, 40, 0.74f)
    }

    // ── 语义色恒定(spec §2.1:不随主题/品牌漂移)────────────────────────────

    @Test
    fun `语义色恒定`() {
        assertEquals(Color(0xFF007AFF), SynshuSemantic.Receive) // 接收=蓝
        assertEquals(Color(0xFF34A853), SynshuSemantic.Send) // 发送=绿
        assertEquals(Color(0xFF8E8AA0), SynshuSemantic.Schedule) // 调度=紫灰
        assertEquals(Color(0xFF007AFF), SynshuSemantic.InProgress) // 进行中=蓝
        assertEquals(Color(0xFF34A853), SynshuSemantic.Done) // 完成=绿
        assertEquals(Color(0xFFFF3B30), SynshuSemantic.Fail) // 失败=红
    }

    // ── 浅/深 ColorScheme 挂接(M3 全 token 覆写后的关键槽位)────────────────

    @Test
    fun `浅色 ColorScheme 挂接`() {
        assertEquals(SynshuColors.Accent, SynshuLightScheme.primary)
        assertEquals(SynshuColors.Background, SynshuLightScheme.background)
        assertEquals(SynshuColors.Background, SynshuLightScheme.surface)
        assertEquals(SynshuColors.TextPrimary, SynshuLightScheme.onSurface)
        assertEquals(SynshuColors.TextSecondary, SynshuLightScheme.onSurfaceVariant)
        assertEquals(SynshuColors.InputField, SynshuLightScheme.surfaceVariant)
        assertEquals(SynshuColors.SelectedRow, SynshuLightScheme.surfaceContainer)
        assertEquals(SynshuColors.Hairline, SynshuLightScheme.outlineVariant)
        assertEquals(SynshuColors.DestructiveRed, SynshuLightScheme.error)
        assertEquals(SynshuColors.OnlineDot, SynshuLightScheme.secondary)
    }

    @Test
    fun `深色 ColorScheme 挂接`() {
        assertEquals(SynshuColorsDark.Accent, SynshuDarkScheme.primary)
        assertEquals(SynshuColorsDark.BackgroundEnd, SynshuDarkScheme.background)
        assertEquals(SynshuColorsDark.TextPrimary, SynshuDarkScheme.onSurface)
        assertEquals(SynshuColorsDark.TextSecondary, SynshuDarkScheme.onSurfaceVariant)
        assertEquals(SynshuColorsDark.Hairline, SynshuDarkScheme.outlineVariant)
        assertEquals(SynshuColorsDark.SelectedRow, SynshuDarkScheme.surfaceContainer)
        assertEquals(SynshuSemantic.Fail, SynshuDarkScheme.error)
    }

    // ── 字阶(spec §2.2 规格值)───────────────────────────────────────────────

    @Test
    fun `字阶规格值`() {
        // 行名 17sp
        assertEquals(17.sp, SynshuTypography.bodyLarge.fontSize)
        // 副文 12.5sp
        assertEquals(12.5.sp, SynshuTypography.bodySmall.fontSize)
        // 顶栏标题 17sp·600
        assertEquals(17.sp, SynshuTypography.titleMedium.fontSize)
        assertEquals(FontWeight.SemiBold, SynshuTypography.titleMedium.fontWeight)
        // 尾注/弹窗正文 13sp
        assertEquals(13.sp, SynshuTypography.bodyMedium.fontSize)
        // 胶囊标签 11sp
        assertEquals(11.sp, SynshuTypography.labelSmall.fontSize)
        // 搜索框文本 15.5sp
        assertEquals(15.5.sp, SynshuTypography.labelLarge.fontSize)
        // 弹窗标题 16.5sp·600
        assertEquals(16.5.sp, SynshuTypography.titleLarge.fontSize)
        assertEquals(FontWeight.SemiBold, SynshuTypography.titleLarge.fontWeight)
    }

    // ── entrySubtitle 三分支(spec §2.3;视觉稿逐字面)────────────────────────

    @Test
    fun `entrySubtitle 三分支`() {
        val now = at(2026, Calendar.NOVEMBER, 8, 12, 0)
        // 目录已知子项数 →「日期 · N 项」(mockup 06 逐字面:11月8日 · 23 项)
        assertEquals("今天 · 12 项", entrySubtitle(at(2026, Calendar.NOVEMBER, 8, 8, 30), children = 12, size = 0, nowMs = now))
        assertEquals("11月2日 · 23 项", entrySubtitle(at(2026, Calendar.NOVEMBER, 2, 9, 0), children = 23, size = 0, nowMs = now))
        // 文件 →「日期 · 大小」(大小段与 iOS BrowseFormat.size 同口径:一位
        // 小数不剪尾,88KB 呈 88.0 KB;视觉稿「88 KB」为示意)
        assertEquals("昨天 · 2.0 MB", entrySubtitle(at(2026, Calendar.NOVEMBER, 7, 21, 0), children = null, size = 2 * 1024 * 1024, nowMs = now))
        assertEquals("今天 · 88.0 KB", entrySubtitle(at(2026, Calendar.NOVEMBER, 8, 9, 40), children = null, size = 88 * 1024, nowMs = now))
        // children 未知(目录 stat 未及,size 恒 0)→ 退化纯日期
        assertEquals("11月5日", entrySubtitle(at(2026, Calendar.NOVEMBER, 5, 10, 0), children = null, size = 0, nowMs = now))
    }

    @Test
    fun `entrySubtitle 日期措辞今天昨天与跨年`() {
        val now = at(2026, Calendar.NOVEMBER, 8, 12, 0)
        assertEquals("今天", entrySubtitle(at(2026, Calendar.NOVEMBER, 8, 0, 0), children = null, size = 0, nowMs = now))
        assertEquals("昨天", entrySubtitle(at(2026, Calendar.NOVEMBER, 7, 23, 59), children = null, size = 0, nowMs = now))
        // 跨年日期 → yyyy年M月d日
        assertEquals("2025年6月1日", entrySubtitle(at(2025, Calendar.JUNE, 1, 8, 0), children = null, size = 0, nowMs = now))
        // 年界昨天:now = 1/1,mtime = 12/31 → 仍「昨天」(日历日比较,非 24h)
        val newYear = at(2027, Calendar.JANUARY, 1, 0, 30)
        assertEquals("昨天", entrySubtitle(at(2026, Calendar.DECEMBER, 31, 22, 0), children = null, size = 0, nowMs = newYear))
        // 不补零形态:M月d日 个位月日
        assertEquals("3月5日", entrySubtitle(at(2026, Calendar.MARCH, 5, 9, 0), children = null, size = 0, nowMs = now))
    }

    @Test
    fun `entrySubtitle mtime 零时段剔除`() {
        val now = at(2026, Calendar.NOVEMBER, 8, 12, 0)
        // daemon 无 mtime 语义(0)→ 日期段剔除,余段照常
        assertEquals("12 项", entrySubtitle(0, children = 12, size = 0, nowMs = now))
        assertEquals("500 B", entrySubtitle(0, children = null, size = 500, nowMs = now))
        assertEquals("", entrySubtitle(0, children = null, size = 0, nowMs = now))
    }
}
