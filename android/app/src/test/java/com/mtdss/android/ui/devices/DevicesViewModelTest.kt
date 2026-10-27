package com.mtdss.android.ui.devices

import com.mtdss.android.ipc.Goldens
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
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

}
