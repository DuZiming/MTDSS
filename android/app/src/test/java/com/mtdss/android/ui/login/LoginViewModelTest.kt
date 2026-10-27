package com.mtdss.android.ui.login

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
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * 登录 VM JVM 单测(FakeDaemon 驱动):dev 验证码回填、本地校验、错误帧
 * 文案、verify 成功 + event.login 捕获(先订阅后动作的协议闸验证);dev CA
 * 演练钩子(spec §7 自签信令)注入/省略的请求参数面。
 */
class LoginViewModelTest {

    /** 应答 golden(重写 id)助手。 */
    private fun replyGolden(name: String): (com.mtdss.android.ipc.FakeDaemon.ServerSide, ByteArray) -> Unit =
        { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read(name), id))
        }

    @Test
    fun `request_code 成功回填 dev 验证码`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = replyGolden("login.request_code.resp.json")
            val vm = LoginViewModel(e.repo, e.hub, e.scope)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.requestCode()
            awaitUntil { vm.state.value.devCode != null }
            assertEquals("654321", vm.state.value.devCode)
            assertEquals("654321", vm.state.value.code)
            assertFalse(vm.state.value.busy)
            assertNull(vm.state.value.error)
        } finally {
            e.close()
        }
    }

    @Test
    fun `request_code 空字段本地校验不触 RPC`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = replyGolden("login.request_code.resp.json")
            val vm = LoginViewModel(e.repo, e.hub, e.scope)
            vm.setEmail("a@x.y") // server 空
            vm.requestCode()
            assertEquals("请填写服务器地址与邮箱", vm.state.value.error)
            delay(100)
            assertTrue("本地校验失败不得触 RPC", e.fake.connections.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `request_code 错误帧转错误文案`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine("""{"id":$id,"error":{"code":1,"message":"服务器不可达"}}""")
            }
            val vm = LoginViewModel(e.repo, e.hub, e.scope)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.requestCode()
            awaitUntil { vm.state.value.error != null }
            assertTrue(vm.state.value.error!!.contains("服务器不可达"))
            assertFalse(vm.state.value.busy)
        } finally {
            e.close()
        }
    }

    @Test
    fun `verify 成功置屏内信号且 event login 被捕获`() = TestKit.runTest {
        val e = TestEnv()
        try {
            // verify 应答 + 同连接补发 event.login(事件由本动作触发的口径)
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                if (req.opt("method")!!.asString() == "login.verify") {
                    conn.sendLine(Goldens.read("event.login.json"))
                    conn.sendLine(Goldens.withId(Goldens.read("login.verify.resp.json"), id))
                }
            }
            val vm = LoginViewModel(e.repo, e.hub, e.scope)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.setName("MIX 3")
            vm.setCode("654321")
            vm.verify()
            awaitUntil { vm.state.value.loggedIn }
            // 订阅闸:no-replay 事件流未丢帧(hub 订阅先于 verify 动作)
            awaitUntil { e.hub.login.value == true }
            assertFalse(vm.state.value.busy)
            assertNull(vm.state.value.error)
        } finally {
            e.close()
        }
    }

    @Test
    fun `verify 空验证码本地校验`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val vm = LoginViewModel(e.repo, e.hub, e.scope)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.verify()
            assertEquals("请填写验证码", vm.state.value.error)
            delay(100)
            assertTrue(e.fake.connections.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `verify 错误帧转错误文案`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine("""{"id":$id,"error":{"code":1,"message":"验证码错误"}}""")
            }
            val vm = LoginViewModel(e.repo, e.hub, e.scope)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.setCode("000000")
            vm.verify()
            awaitUntil { vm.state.value.error != null }
            assertTrue(vm.state.value.error!!.contains("验证码错误"))
            assertFalse(vm.state.value.loggedIn)
        } finally {
            e.close()
        }
    }

    /** login.request_code 请求参数捕获 + 应答 golden(重写 id;按 method 过滤)。 */
    private fun captureRequestCodeParams(sink: (JVal) -> Unit): (com.mtdss.android.ipc.FakeDaemon.ServerSide, ByteArray) -> Unit =
        { conn, line ->
            val req = JVal.parse(String(line, Charsets.UTF_8))
            if (req.opt("method")?.asString() == "login.request_code") sink(req.opt("params")!!)
            conn.sendLine(Goldens.withId(Goldens.read("login.request_code.resp.json"), req.opt("id")!!.asLong()))
        }

    // ── dev CA 演练钩子(spec §7;参数面在 FakeDaemon 线上断言)────────────

    @Test
    fun `request_code dev CA 文件在则 ca 随请求注入`() = TestKit.runTest {
        val e = TestEnv()
        val ca = File.createTempFile("dev-ca", ".pem")
        try {
            var params: JVal? = null
            e.fake.onLine = captureRequestCodeParams { params = it }
            val vm = LoginViewModel(e.repo, e.hub, e.scope, devCaPath = ca.absolutePath)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.requestCode()
            awaitUntil { vm.state.value.devCode != null }
            val p = params!!
            assertEquals(ca.absolutePath, p.opt("ca")?.asString())
            assertEquals("http://127.0.0.1:39731", p.opt("server")?.asString())
            assertEquals("a@x.y", p.opt("email")?.asString())
        } finally {
            ca.delete()
            e.close()
        }
    }

    @Test
    fun `request_code devCaPath 缺省不带 ca 键`() = TestKit.runTest {
        val e = TestEnv()
        try {
            var params: JVal? = null
            e.fake.onLine = captureRequestCodeParams { params = it }
            val vm = LoginViewModel(e.repo, e.hub, e.scope) // devCaPath 缺省 null
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.requestCode()
            awaitUntil { vm.state.value.devCode != null }
            assertNull(params!!.opt("ca"))
        } finally {
            e.close()
        }
    }

    @Test
    fun `request_code dev CA 文件不在不注入`() = TestKit.runTest {
        val e = TestEnv()
        // 候选路径在、文件无(存在性闸的否定臂;release 的 DEBUG 闸是构建期常量,不在此测)
        val missing = File.createTempFile("dev-ca", ".pem").apply { delete() }
        try {
            var params: JVal? = null
            e.fake.onLine = captureRequestCodeParams { params = it }
            val vm = LoginViewModel(e.repo, e.hub, e.scope, devCaPath = missing.absolutePath)
            vm.setServer("http://127.0.0.1:39731")
            vm.setEmail("a@x.y")
            vm.requestCode()
            awaitUntil { vm.state.value.devCode != null }
            assertNull(params!!.opt("ca"))
        } finally {
            e.close()
        }
    }
}
