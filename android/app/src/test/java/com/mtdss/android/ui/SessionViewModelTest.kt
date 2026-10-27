package com.mtdss.android.ui

import com.mtdss.android.ipc.Goldens
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import kotlinx.coroutines.delay
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 会话 VM JVM 单测:连接建立即拉取 status(权威登录态)、event.login 翻转
 * 触发全量刷新(桌面同款)、event.online 只翻单字段。
 */
class SessionViewModelTest {

    /** 伪 daemon 的可变登录态(模拟 daemon 侧 session 翻转)。 */
    private var fakeLogged: Boolean = false

    private fun statusReply(conn: com.mtdss.android.ipc.FakeDaemon.ServerSide, line: ByteArray) {
        val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
        conn.sendLine(
            """{"id":$id,"result":{""" +
                """"device":{"device_id":"dev-self","name":"MIX 3"},"logged_in":$fakeLogged,""" +
                """"online":true,"recv_dir":"inbox","server":"http://127.0.0.1:39731"}}""",
        )
    }

    @Test
    fun `连接建立即拉取 status 得权威登录态`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line -> statusReply(conn, line) }
            fakeLogged = false
            val vm = SessionViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.conn == com.mtdss.android.ipc.ConnState.Connected }
            awaitUntil { vm.state.value.loggedIn == false }
            assertEquals("dev-self", vm.state.value.deviceId)
            assertEquals("MIX 3", vm.state.value.deviceName)
            assertEquals(true, vm.state.value.selfOnline)
        } finally {
            e.close()
        }
    }

    @Test
    fun `event login 翻转触发全量刷新`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line -> statusReply(conn, line) }
            fakeLogged = false
            val vm = SessionViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.loggedIn == false }

            // daemon 侧登录完成 → 事件推送;hub 路由 → VM 全量刷新
            fakeLogged = true
            e.fake.connections[0].sendLine(Goldens.read("event.login.json"))
            awaitUntil { vm.state.value.loggedIn == true }

            // 登出同理(桌面:登出回引导态)
            fakeLogged = false
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.login","params":{"logged_in":false}}""",
            )
            awaitUntil { vm.state.value.loggedIn == false }
        } finally {
            e.close()
        }
    }

    @Test
    fun `event online 只翻单字段不重拉 status`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line -> statusReply(conn, line) }
            fakeLogged = false
            val vm = SessionViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.loggedIn == false }
            val statusCalls = e.fake.connections[0].received.count {
                JVal.parse(String(it, Charsets.UTF_8)).opt("method")?.let { m -> m.asString() == "status" } == true
            }

            e.fake.connections[0].sendLine(Goldens.read("event.online.json"))
            awaitUntil { vm.state.value.selfOnline == true }
            delay(150) // 若误触发刷新,这里会新增 status 调用
            val statusCallsAfter = e.fake.connections[0].received.count {
                JVal.parse(String(it, Charsets.UTF_8)).opt("method")?.let { m -> m.asString() == "status" } == true
            }
            assertEquals("event.online 不得触发 status 重拉", statusCalls, statusCallsAfter)
            assertEquals("MIX 3", vm.state.value.deviceName) // 设备身份不因在线事件丢失
        } finally {
            e.close()
        }
    }
}
