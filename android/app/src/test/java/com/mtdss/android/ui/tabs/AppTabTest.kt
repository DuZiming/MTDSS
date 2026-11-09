package com.mtdss.android.ui.tabs

import com.mtdss.android.ui.common.CapsuleTabGlyph
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 四标签壳枚举 JVM 单测(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-1;brief T5 Step 1):
 * - 四枚枚举序 = 胶囊底栏自左向右 = iOS AppTab.swift CaseIterable 序;
 * - 标题词面与 iOS 逐字一致(快捷目录/设备/传输列表/我的;spec「中文文案
 *   与 iOS 端逐字一致」约束);
 * - 图标形映射 CapsuleTabGlyph(胶囊底栏同序)。
 */
class AppTabTest {

    @Test
    fun `枚举序与 iOS AppTab 一致`() {
        assertEquals(
            listOf(AppTab.FAVORITES, AppTab.DEVICES, AppTab.TRANSFERS, AppTab.PROFILE),
            AppTab.entries,
        )
    }

    @Test
    fun `标题词面逐字`() {
        assertEquals("快捷目录", AppTab.FAVORITES.title)
        assertEquals("设备", AppTab.DEVICES.title)
        assertEquals("传输列表", AppTab.TRANSFERS.title)
        assertEquals("我的", AppTab.PROFILE.title)
    }

    @Test
    fun `图标形映射同序`() {
        assertEquals(CapsuleTabGlyph.FAVORITES, AppTab.FAVORITES.glyph)
        assertEquals(CapsuleTabGlyph.DEVICES, AppTab.DEVICES.glyph)
        assertEquals(CapsuleTabGlyph.TRANSFERS, AppTab.TRANSFERS.glyph)
        assertEquals(CapsuleTabGlyph.PROFILE, AppTab.PROFILE.glyph)
    }
}
