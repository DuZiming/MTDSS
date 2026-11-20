package com.mtdss.android.ui.browse

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 浏览视图模式记忆 JVM 单测(V4-1 T22;钦定 mockup mobile-browse.html:
 * **列表默认** + 「⋮」切换平铺/列表,SharedPreferences 跨次进页记忆——键
 * `browse_view_mode`,值 `list|tiles` 逐字钉面):
 * - 缺省形:无档(null)/空串/垃圾值/大小写不符 → 一律 LIST(列表默认是
 *   基线形态,档面损坏只降级不炸面);
 * - 键/值字面常量锚(与 brief 逐字对齐,daemon/桌面无此面,App 私有档);
 * - 往返:save(TILES) → 重读 store = TILES(模拟下次进页);
 * - other() 切换面(菜单项词面 = 切往的另一形态)。
 */
class BrowseViewModeTest {

    /** 内存持久缝(实机 = SharedPreferences "browse";两方法同 RootsPersistence 形)。 */
    private class MemPrefs : BrowseViewPersistence {
        var v: String? = null
        override fun load(): String? = v
        override fun save(value: String) {
            v = value
        }
    }

    @Test
    fun `键值字面常量锚 list tiles`() {
        assertEquals("browse_view_mode", BrowseViewModeStore.KEY)
        assertEquals("browse", BrowseViewModeStore.PREFS)
        assertEquals("list", BrowseViewMode.LIST.key)
        assertEquals("tiles", BrowseViewMode.TILES.key)
    }

    @Test
    fun `缺省形 无档空串垃圾值一律列表`() {
        for (raw in listOf<String?>(null, "", "平铺", "TILES", "grid", "tiles ")) {
            assertNull(
                "垃圾值不猜: $raw",
                raw?.let { BrowseViewMode.entries.firstOrNull { m -> m.key == it } },
            )
            assertEquals(
                "档面 raw=[$raw] → 列表默认",
                BrowseViewMode.LIST,
                BrowseViewMode.fromKey(raw),
            )
        }
    }

    @Test
    fun `往返 save tiles 重读 store 即 tiles`() {
        val prefs = MemPrefs()
        val first = BrowseViewModeStore(prefs)
        assertEquals(BrowseViewMode.LIST, first.mode) // 初次进页 = 列表默认
        first.save(BrowseViewMode.TILES)
        // 新 store 实例 = 下次进页(构造即读档)
        assertEquals(BrowseViewMode.TILES, BrowseViewModeStore(prefs).mode)
        // 切回列表同面
        first.save(BrowseViewMode.LIST)
        assertEquals(BrowseViewMode.LIST, BrowseViewModeStore(prefs).mode)
    }

    @Test
    fun `other 切换面 两形态互翻`() {
        assertEquals(BrowseViewMode.TILES, BrowseViewMode.LIST.other())
        assertEquals(BrowseViewMode.LIST, BrowseViewMode.TILES.other())
    }
}
