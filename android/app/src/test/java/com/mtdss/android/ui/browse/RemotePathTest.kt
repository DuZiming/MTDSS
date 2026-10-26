package com.mtdss.android.ui.browse

import com.mtdss.android.test.TestKit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 远端路径工具 JVM 单测(移植桌面 nav.ts 的 vitest 口径;Android 三层
 * 导航适配:根路径的上级是卷列表 "" 而非设备层——设备层由导航栈承担)。
 */
class RemotePathTest {

    @Test
    fun `posix 父路径`() = TestKit.runTest {
        assertEquals("/a", RemotePath.parentOf("/a/b"))
        assertEquals("/", RemotePath.parentOf("/a"))
        assertNull(RemotePath.parentOf(""))
        // 尾分隔符等价
        assertEquals("/a", RemotePath.parentOf("/a/b/"))
    }

    @Test
    fun `windows 父路径`() = TestKit.runTest {
        assertEquals("""C:\Users""", RemotePath.parentOf("""C:\Users\x"""))
        assertEquals("""C:\""", RemotePath.parentOf("""C:\Users"""))
        assertEquals("""C:\a\b""", RemotePath.parentOf("""C:\a\b\c"""))
        assertEquals("""C:\""", RemotePath.parentOf("""C:\Users\"""))
    }

    @Test
    fun `根形态的上级是卷列表`() = TestKit.runTest {
        // Android 适配:根路径(POSIX "/"、Windows 盘根/卷条目)的上级 =
        // 根卷列表(桌面是设备层,这里由 AppNav 的路由栈承担设备层)
        assertEquals("", RemotePath.parentOf("/"))
        assertEquals("", RemotePath.parentOf("C:\\"))
        assertEquals("", RemotePath.parentOf("C:"))
        assertNull(RemotePath.parentOf(""))
    }

    @Test
    fun `segments 面包屑`() = TestKit.runTest {
        assertEquals(emptyList<String>(), RemotePath.segments(""))
        assertEquals(listOf("/", "a", "b"), RemotePath.segments("/a/b"))
        assertEquals(listOf("C:", "Users", "x"), RemotePath.segments("""C:\Users\x"""))
        assertEquals(listOf("C:"), RemotePath.segments("C:\\"))
        assertEquals(listOf("/"), RemotePath.segments("/"))
    }
}
