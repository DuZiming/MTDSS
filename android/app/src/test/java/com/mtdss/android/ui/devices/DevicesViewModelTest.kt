package com.mtdss.android.ui.devices

import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.Goldens
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import com.mtdss.android.ui.common.deviceSubtitle
import com.mtdss.android.ui.common.deviceTappable
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 设备列表 VM JVM 单测:op 全量拉取 + event.devices 快照替换(先订阅后
 * 动作:事件与应答同连接回流,闸保证不丢)、错误态、手动刷新对账。
 */
class DevicesViewModelTest {

    private fun replyDevicesGolden(): (com.mtdss.android.ipc.FakeDaemon.ServerSide, ByteArray) -> Unit =
        { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("devices.resp.json"), id))
        }

    @Test
    fun `初始拉取设备列表`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = replyDevicesGolden()
            val vm = DevicesViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.devices.size == 2 }
            assertEquals("10ba682c8ad13513971e8b56881aab8b", vm.state.value.devices[0].deviceId)
            assertEquals("契约对端", vm.state.value.devices[1].name)
            assertFalse(vm.state.value.stale)
            assertFalse(vm.state.value.loading)
            assertNull(vm.state.value.error)
        } finally {
            e.close()
        }
    }

    @Test
    fun `event devices 快照替换列表`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = replyDevicesGolden()
            val vm = DevicesViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.devices.size == 2 } // 初始拉取完成

            // 事件(单设备在线形态)随后到达:快照全量替换(hub 订阅闸保证
            // VM 的事件收集先于拉取动作落位)
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.devices","params":{"devices":[""" +
                    """{"device_id":"dev-peer-fixed","name":"契约对端","platform":"linux","online":true}]}}""",
            )
            awaitUntil { vm.state.value.devices.size == 1 && vm.state.value.devices[0].online }
            assertEquals("dev-peer-fixed", vm.state.value.devices[0].deviceId)
        } finally {
            e.close()
        }
    }

    @Test
    fun `devices 错误转错误态`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine("""{"id":$id,"error":{"code":3,"message":"未登录,请先 login.verify"}}""")
            }
            val vm = DevicesViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.error != null }
            assertTrue(vm.state.value.error!!.contains("未登录"))
            assertFalse(vm.state.value.loading)
        } finally {
            e.close()
        }
    }

    @Test
    fun `手动 refresh 对账重拉`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = replyDevicesGolden()
            val vm = DevicesViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.devices.size == 2 }

            // 事件把列表换成单设备
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.devices","params":{"devices":[""" +
                    """{"device_id":"dev-peer-fixed","name":"契约对端","platform":"linux","online":false}]}}""",
            )
            awaitUntil { vm.state.value.devices.size == 1 }

            // 手动刷新:golden 全量回来(事件面与拉取面的对账路径)
            vm.refresh()
            awaitUntil { vm.state.value.devices.size == 2 }
            assertFalse(vm.state.value.loading)
        } finally {
            e.close()
        }
    }

    /** 本机固定 id(golden 首行 契约本机;「本机在列」投影测试用)。 */
    private val goldenSelfId = "10ba682c8ad13513971e8b56881aab8b"

    /**
     * 行语言数据面(spec §3-3/V3-4;iOS DevicesRowFormat 逐字镜像):
     * 本机在列——副文「Android · 本机」禁进;离线行禁点;在线对端可进。
     * 纯函数宿主可测,composable 面(DevicesScreen/DevicePickerSheet)消费。
     */
    @Test
    fun `本机行在列 副文与禁进 数据面`() = TestKit.runTest {
        // 副文逐字:「平台 · 在线/离线/本机」;平台名 iOS DeviceIcon.displayName 同款
        assertEquals("Windows · 在线", deviceSubtitle(platform = "windows", isSelf = false, online = true))
        assertEquals("Windows · 离线", deviceSubtitle(platform = "windows", isSelf = false, online = false))
        assertEquals("Linux · 在线", deviceSubtitle(platform = "linux", isSelf = false, online = true))
        assertEquals("macOS · 离线", deviceSubtitle(platform = "macos", isSelf = false, online = false))
        assertEquals("iOS · 在线", deviceSubtitle(platform = "ios", isSelf = false, online = true))
        assertEquals("Android · 本机", deviceSubtitle(platform = "android", isSelf = true, online = true))
        assertEquals("未知 · 离线", deviceSubtitle(platform = null, isSelf = false, online = false))

        // 门:在线对端可进;离线/本机禁点(iOS:本机行不可进 + 离线置灰不可选)
        val self = DirectoryDevice("dev-self", "本机", "android", online = true)
        val peerOnline = DirectoryDevice("dev-peer", "对端", "windows", online = true)
        val peerOffline = DirectoryDevice("dev-off", "离线机", "windows", online = false)
        assertTrue(deviceTappable(peerOnline, selfDeviceId = "dev-self"))
        assertFalse(deviceTappable(peerOffline, selfDeviceId = "dev-self"))
        assertFalse(deviceTappable(self, selfDeviceId = "dev-self"))
        // selfDeviceId 未就绪(AppModel 投影晚到短窗):行暂按对端样(iOS 同构)
        assertTrue(deviceTappable(peerOnline, selfDeviceId = null))
    }

    /**
     * 行序 = daemon created_at 定序既有(spec「VM 行序 created_at 定序既有」;
     * 安卓不做 iOS R12 在线沉底重排,目录投影如实呈现)+ 本机在列投影。
     */
    @Test
    fun `行序 created_at 既有 本机行投影`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = replyDevicesGolden()
            val vm = DevicesViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.devices.size == 2 }
            // created_at 升序既有:契约本机(1730000000)在前,契约对端(1730086400)在后
            assertEquals(goldenSelfId, vm.state.value.devices[0].deviceId)
            assertEquals("dev-peer-fixed", vm.state.value.devices[1].deviceId)

            // 本机行(第一行):副文「Windows · 本机」+ 禁进;对端离线行禁点
            assertEquals("Windows · 本机", deviceSubtitle(vm.state.value.devices[0].platform, isSelf = true, online = vm.state.value.devices[0].online))
            assertFalse(deviceTappable(vm.state.value.devices[0], selfDeviceId = goldenSelfId))
            assertFalse(deviceTappable(vm.state.value.devices[1], selfDeviceId = goldenSelfId))

            // 事件把对端翻在线 → 数据面翻转可进(设备页点行进远端浏览的门)
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.devices","params":{"devices":[""" +
                    """{"device_id":"dev-peer-fixed","name":"契约对端","platform":"linux","online":true}]}}""",
            )
            awaitUntil { vm.state.value.devices.first { it.deviceId == "dev-peer-fixed" }.online }
            assertTrue(deviceTappable(vm.state.value.devices.first { it.deviceId == "dev-peer-fixed" }, selfDeviceId = goldenSelfId))
        } finally {
            e.close()
        }
    }

}
