package com.mtdss.android.ui

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.ipc.Goldens
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.MtdssEvent
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 会话 VM JVM 单测:连接建立即拉取 status(权威登录态)、event.login 翻转
 * 触发全量刷新(桌面同款)、event.online 只翻单字段;V3-1 事件面——
 * event.approval 路由审批流、event.device_evicted 置被移除旗并对账回登录态。
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

    @Test
    fun `event approval 路由审批流且 device_evicted 置被移除旗`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line -> statusReply(conn, line) }
            fakeLogged = false
            val vm = SessionViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.loggedIn == false }
            assertTrue("未移除时旗位恒假", !vm.state.value.evicted)

            // event.approval → hub 审批流(受信终端弹窗数据面)
            var approval: MtdssEvent.Approval? = null
            val job = e.scope.launch { e.hub.approval.collect { approval = it } }
            e.fake.connections[0].sendLine(Goldens.read("event.approval.json"))
            awaitUntil { approval != null }
            assertEquals("sess-contract-a", approval!!.sessionId)
            assertEquals("契约申请端", approval!!.candidateName)
            assertEquals("windows", approval!!.candidatePlatform)
            assertEquals("123456", approval!!.code)
            job.cancel()

            // event.device_evicted → 置旗 + 权威对账(daemon 已清会话)
            e.fake.connections[0].sendLine(Goldens.read("event.device_evicted.json"))
            awaitUntil { vm.state.value.evicted }
            assertEquals(false, vm.state.value.loggedIn)
        } finally {
            e.close()
        }
    }

    @Test
    fun `被移除旗随重登成功清除`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line -> statusReply(conn, line) }
            fakeLogged = false
            val vm = SessionViewModel(e.repo, e.hub, e.scope)
            awaitUntil { vm.state.value.loggedIn == false }
            e.fake.connections[0].sendLine(Goldens.read("event.device_evicted.json"))
            awaitUntil { vm.state.value.evicted }

            // 重新登录:status 拉到 loggedIn=true → 旗位清零(警示条退场)
            fakeLogged = true
            e.fake.connections[0].sendLine(Goldens.read("event.login.json"))
            awaitUntil { vm.state.value.loggedIn == true }
            assertEquals(false, vm.state.value.evicted)
        } finally {
            e.close()
        }
    }
}
