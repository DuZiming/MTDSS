package com.mtdss.android.favorites

import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.favorites.FavoritesTree.MemberRow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 快捷树成员行投影 JVM 单测(V3-4 T9,spec §4.1 成员行树 +
 * §4.10「devices 快照 join 设备名(未及退化 id)」;iOS 首页 FavoritesView
 * memberRow/deviceName/isMemberOnline 同构):
 * ①行标签 = 「设备名 - 目录名」(设备名 = 快照 join,目录名 = 路径末段显示名);
 * ②快照缺该设备 = 离线且设备名退化 id;③快照在场但 online=false 亦离线;
 * ④目录名末段双分隔符裁切;⑤行序 = 成员添加序;空成员快捷 = 空行集。
 */
class FavoritesTreeTest {

    private fun dev(id: String, name: String?, online: Boolean) =
        DirectoryDevice(deviceId = id, name = name, platform = null, online = online, createdAt = 10L)

    // ── ①行标签「设备名 - 目录名」+ 离线判定 ────────────────────────────────

    @Test
    fun `成员行标签设备名-目录名与在线判定`() {
        val favorite = Favorite(
            id = "f1",
            name = "项目",
            createdAt = 1,
            members = listOf(FavMember("d1", "C:\\go"), FavMember("d2", "/home/u/照片")),
        )
        val devices = listOf(
            dev("d1", "设备甲", online = true),
            dev("d2", "设备乙", online = false),
            dev("d3", "设备丙", online = true),
        )
        val rows = FavoritesTree.members(favorite, devices)
        assertEquals(2, rows.size)
        assertEquals(MemberRow(displayName = "设备甲 - go", deviceId = "d1", path = "C:\\go", offline = false), rows[0])
        // 快照在场但 online=false:同样离线(iOS contains { online } 同象)
        assertEquals(MemberRow(displayName = "设备乙 - 照片", deviceId = "d2", path = "/home/u/照片", offline = true), rows[1])
    }

    // ── ②快照缺该设备:离线 + 设备名退化 id ─────────────────────────────────

    @Test
    fun `快照缺设备退化id且按离线`() {
        val favorite = Favorite("f1", "项目", 1, listOf(FavMember("dev-x", "C:\\go")))
        val rows = FavoritesTree.members(favorite, emptyList())
        assertEquals(1, rows.size)
        assertTrue("快照未及窗恒保守离线", rows[0].offline)
        assertEquals("dev-x - go", rows[0].displayName)
        assertEquals("dev-x", rows[0].deviceId)
        assertEquals("C:\\go", rows[0].path)
    }

    // ── ③目录名 = 路径末段显示名(双分隔符,严禁单 '/' 切分)───────────────

    @Test
    fun `目录名取路径末段`() {
        assertEquals("proj", FavoritesTree.dirName("C:\\go\\proj"))
        assertEquals("照片", FavoritesTree.dirName("/home/u/照片"))
        assertEquals("go", FavoritesTree.dirName("C:\\go"))
        assertEquals("go", FavoritesTree.dirName("/go"))
        assertEquals("C:", FavoritesTree.dirName("C:\\")) // 盘根 = 盘符段(iOS split 同象)
        assertEquals("/", FavoritesTree.dirName("/")) // POSIX 根无段 → 整路径
        assertEquals("", FavoritesTree.dirName(""))
    }

    // ── ④行序 = 成员添加序;空成员 = 空行集 ─────────────────────────────────

    @Test
    fun `行序随成员添加序且空成员空集`() {
        val favorite = Favorite(
            "f1", "项目", 1,
            listOf(FavMember("d2", "/b"), FavMember("d1", "/a"), FavMember("d2", "/c")),
        )
        val devices = listOf(dev("d1", "甲", true), dev("d2", "乙", true))
        val rows = FavoritesTree.members(favorite, devices)
        // 不重排不合并:成员序即行序,同设备两成员两行
        assertEquals(listOf("/b", "/a", "/c"), rows.map { it.path })
        assertEquals(listOf("乙 - b", "甲 - a", "乙 - c"), rows.map { it.displayName })
        assertTrue(rows.all { !it.offline })

        assertEquals(emptyList<MemberRow>(), FavoritesTree.members(Favorite("f2", "空", 1, emptyList()), devices))
    }

    // ── ⑤同名设备快照取首个(iOS first 同象)────────────────────────────────

    @Test
    fun `设备名join取快照首个`() {
        val favorite = Favorite("f1", "项目", 1, listOf(FavMember("d1", "/a")))
        val devices = listOf(dev("d1", "旧名", true), dev("d1", "新名", true))
        assertEquals("旧名 - a", FavoritesTree.members(favorite, devices)[0].displayName)
        assertFalse(FavoritesTree.members(favorite, devices)[0].offline)
    }
}
