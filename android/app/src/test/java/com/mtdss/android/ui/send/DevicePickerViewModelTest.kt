package com.mtdss.android.ui.send

import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 选设备弹层 VM JVM 单测:行序在线在前稳定投影(iOS DeviceListOrder.
 * onlineFirst 镜像,T7 收口 T6 deferred minor ②)——在线在前、组内保序
 * (daemon 目录投影序),离线沉底;op 拉取与 event 快照两路同投影。
 */
class DevicePickerViewModelTest {

    @Test
    fun `弹层行序在线在前 组内保序 离线沉底`() = TestKit.runTest {
        val e = TestEnv()
        try {
            // daemon 目录投影序:B(离线)→ A(在线)→ C(在线)→ D(离线)
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                conn.sendLine(
                    """{"id":$id,"result":{"devices":[""" +
                        """{"device_id":"dev-b","name":"B 机","platform":"windows","online":false},""" +
                        """{"device_id":"dev-a","name":"A 机","platform":"linux","online":true},""" +
                        """{"device_id":"dev-c","name":"C 机","platform":"macos","online":true},""" +
                        """{"device_id":"dev-d","name":"D 机","platform":"ios","online":false}],"stale":false}}""",
                )
            }
            val vm = DevicePickerViewModel(e.repo, e.hub, e.scope)
            awaitUntil { !vm.state.value.loading && vm.state.value.devices.size == 4 }
            // 在线在前(A、C 保 daemon 序),离线沉底(B、D 保 daemon 序)
            assertEquals(
                listOf("dev-a", "dev-c", "dev-b", "dev-d"),
                vm.state.value.devices.map { it.deviceId },
            )

            // event 快照路径同投影(事件翻 D 在线 → D 进在线组、保序在 A、C 后)
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.devices","params":{"devices":[""" +
                    """{"device_id":"dev-b","name":"B 机","platform":"windows","online":false},""" +
                    """{"device_id":"dev-a","name":"A 机","platform":"linux","online":true},""" +
                    """{"device_id":"dev-c","name":"C 机","platform":"macos","online":true},""" +
                    """{"device_id":"dev-d","name":"D 机","platform":"ios","online":true}]}}""",
            )
            awaitUntil { vm.state.value.devices.last { it.deviceId == "dev-d" }.online }
            assertEquals(
                listOf("dev-a", "dev-c", "dev-d", "dev-b"),
                vm.state.value.devices.map { it.deviceId },
            )
        } finally {
            e.close()
        }
    }

    @Test
    fun `纯函数 在线在前稳定投影 边界`() {
        // 纯函数面:空集/全离线/全在线恒保序;混排组内保序
        org.junit.Assert.assertTrue(onlineFirst(emptyList()).isEmpty())
        val d1 = DirectoryDevice("1", "一", "windows", online = false)
        val d2 = DirectoryDevice("2", "二", "linux", online = false)
        assertEquals(listOf("1", "2"), onlineFirst(listOf(d1, d2)).map { it.deviceId })
    }
}
