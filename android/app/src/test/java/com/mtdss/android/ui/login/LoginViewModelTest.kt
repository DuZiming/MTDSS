package com.mtdss.android.ui.login

import com.mtdss.android.ipc.Goldens
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import com.mtdss.android.ui.login.LoginViewModel.LoginPhase
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
 * 演练钩子(spec §7 自签信令)注入/省略的请求参数面;V3-1 登录中间态状态机
 * (Form/Approval/Limit/Done:verify 步进分派、审批码直输、轮询收口与
 * denied/expired 终态、踢选收口、取消回退)。
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

    // ── V3-1 登录中间态状态机(Form/Approval/Limit/Done)─────────────────────

    /** 测试节拍的 VM(轮询 10ms,收口即时可见;产线 3s 由缺省给出)。 */
    private fun vm(e: TestEnv) = LoginViewModel(e.repo, e.hub, e.scope, pollDelayMs = 10)

    /** Form 面填单(设备名走屏面预填路径,VM 测不涉)。 */
    private fun fillForm(vm: LoginViewModel) {
        vm.setServer("http://127.0.0.1:39731")
        vm.setEmail("a@x.y")
        vm.setCode("654321")
    }

    /** 步进应答脚本(op → result JSON 字面量;恒循环应答,状态机终态自停)。 */
    private fun stepScript(vararg routes: Pair<String, String>): (com.mtdss.android.ipc.FakeDaemon.ServerSide, ByteArray) -> Unit {
        val map = routes.toMap()
        return { conn, line ->
            val req = JVal.parse(String(line, Charsets.UTF_8))
            val id = req.opt("id")!!.asLong()
            val result = map[req.opt("method")?.asString()] ?: "{}"
            conn.sendLine("""{"id":$id,"result":$result}""")
        }
    }

    @Test
    fun `verify 落审批态且轮询收口 ok 至 Done`() = TestKit.runTest {
        val e = TestEnv()
        try {
            var statusCalls = 0
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "login.verify" -> conn.sendLine(Goldens.withId(Goldens.read("login.verify.resp.approval.json"), id))
                    "login.status" -> {
                        statusCalls++
                        val name = if (statusCalls >= 2) "login.status.resp.json" else "login.verify.resp.approval.json"
                        conn.sendLine(Goldens.withId(Goldens.read(name), id))
                    }
                    else -> conn.sendLine("""{"id":$id,"result":{}}""")
                }
            }
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Approval }
            assertEquals("sess-contract-a", v.state.value.sessionId)
            assertEquals(1760000200L, v.state.value.expiresAt)
            // respond-settle:受信端应答后轮询领到 ok → Done(loggedIn 只是
            // 屏内信号,导航权威态在 SessionViewModel 的 status)
            awaitUntil { v.state.value.phase == LoginPhase.Done }
            assertTrue(v.state.value.loggedIn)
            assertNull(v.state.value.error)
        } finally {
            e.close()
        }
    }

    @Test
    fun `审批轮询瞬态失败不终止轮询`() = TestKit.runTest {
        val e = TestEnv()
        try {
            var statusCalls = 0
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "login.verify" -> conn.sendLine(Goldens.withId(Goldens.read("login.verify.resp.approval.json"), id))
                    "login.status" -> {
                        statusCalls++
                        if (statusCalls == 1) {
                            conn.sendLine("""{"id":$id,"error":{"code":1,"message":"信令抖动"}}""")
                        } else {
                            conn.sendLine(Goldens.withId(Goldens.read("login.status.resp.json"), id))
                        }
                    }
                    else -> conn.sendLine("""{"id":$id,"result":{}}""")
                }
            }
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Approval }
            awaitUntil { v.state.value.phase == LoginPhase.Done }
            assertTrue("首拍失败后仍续轮询收口", statusCalls >= 2)
        } finally {
            e.close()
        }
    }

    @Test
    fun `审批轮询 denied 终态回 Form 文案`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = stepScript(
                "login.verify" to """{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}""",
                "login.status" to """{"status":"denied"}""",
            )
            val v = vm(e)
            fillForm(v)
            v.verify()
            // Approval 相位是瞬态(轮询 10ms 即落终态,逐帧断言必漏帧),
            // 直接等终态文案;Form 相位与中间态凭据清场由断言收口
            awaitUntil { v.state.value.terminal != null }
            assertEquals(LoginPhase.Form, v.state.value.phase)
            assertEquals("登录请求已被拒绝", v.state.value.terminal)
            assertFalse(v.state.value.loggedIn)
            assertNull(v.state.value.sessionId)
        } finally {
            e.close()
        }
    }

    @Test
    fun `审批轮询 expired 终态回 Form 文案`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = stepScript(
                "login.verify" to """{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}""",
                "login.status" to """{"status":"expired"}""",
            )
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.terminal != null }
            assertEquals(LoginPhase.Form, v.state.value.phase)
            assertEquals("登录已超时,请重新发起", v.state.value.terminal)
            assertFalse(v.state.value.loggedIn)
        } finally {
            e.close()
        }
    }

    @Test
    fun `verify 落 limit 直入踢选相位`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine(Goldens.withId(Goldens.read("login.verify.resp.limit.json"), id))
            }
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Limit }
            assertEquals("enroll-contract-fixed", v.state.value.enrollToken)
            assertEquals(2, v.state.value.evictDevices.size)
            assertEquals("契约本机", v.state.value.evictDevices[0].name)
            assertFalse(v.state.value.loggedIn)
        } finally {
            e.close()
        }
    }

    @Test
    fun `踢选成功收口 Done 且请求面带凭据`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "login.verify" -> conn.sendLine(Goldens.withId(Goldens.read("login.verify.resp.limit.json"), id))
                    "login.evict" -> conn.sendLine(Goldens.withId(Goldens.read("login.evict.resp.json"), id))
                    else -> conn.sendLine("""{"id":$id,"result":{}}""")
                }
            }
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Limit }
            v.evict("dev-peer-fixed")
            awaitUntil { v.state.value.phase == LoginPhase.Done }
            assertTrue(v.state.value.loggedIn)
            // 请求参数面:enroll_token + device_id 逐字对齐踢选凭据
            val evictReq = e.fake.connections[0].received
                .map { JVal.parse(String(it, Charsets.UTF_8)) }
                .first { it.opt("method")?.asString() == "login.evict" }
            val p = evictReq.opt("params")!!
            assertEquals("enroll-contract-fixed", p.opt("enroll_token")!!.asString())
            assertEquals("dev-peer-fixed", p.opt("device_id")!!.asString())
        } finally {
            e.close()
        }
    }

    @Test
    fun `审批相位提交审批码收口 Done`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "login.verify" -> conn.sendLine(Goldens.withId(Goldens.read("login.verify.resp.approval.json"), id))
                    "login.submit_code" -> conn.sendLine(Goldens.withId(Goldens.read("login.submit_code.resp.json"), id))
                    else -> conn.sendLine("""{"id":$id,"result":{}}""")
                }
            }
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Approval }
            // 空码本地校验(不触 RPC)
            v.submitApprovalCode()
            assertEquals("请填写审批码", v.state.value.error)
            v.setApprovalCode("123456")
            v.submitApprovalCode()
            awaitUntil { v.state.value.phase == LoginPhase.Done }
            assertTrue(v.state.value.loggedIn)
            val submit = e.fake.connections[0].received
                .map { JVal.parse(String(it, Charsets.UTF_8)) }
                .first { it.opt("method")?.asString() == "login.submit_code" }
            assertEquals("sess-contract-a", submit.opt("params")!!.opt("session_id")!!.asString())
            assertEquals("123456", submit.opt("params")!!.opt("code")!!.asString())
        } finally {
            e.close()
        }
    }

    @Test
    fun `审批轮询撞 limit 转踢选相位`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = stepScript(
                "login.verify" to """{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}""",
                "login.status" to
                    """{"status":"limit","enroll_token":"enroll-poll","devices":[""" +
                    """{"device_id":"dev-peer-fixed","name":"契约对端","online":false,"platform":"linux"}]}""",
            )
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Limit }
            assertEquals("enroll-poll", v.state.value.enrollToken)
            assertEquals(1, v.state.value.evictDevices.size)
        } finally {
            e.close()
        }
    }

    @Test
    fun `取消登录回 Form 停轮询且发 login cancel`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = stepScript(
                "login.verify" to """{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}""",
                "login.status" to """{"status":"approval","session_id":"sess-contract-a","expires_at":1760000200}""",
            )
            val v = vm(e)
            fillForm(v)
            v.verify()
            awaitUntil { v.state.value.phase == LoginPhase.Approval }
            v.cancel()
            assertEquals(LoginPhase.Form, v.state.value.phase)
            assertNull(v.state.value.terminal)
            assertNull(v.state.value.sessionId)
            delay(100) // 迟到的轮询步进不得把相位拖回 Approval(Form 期守卫)
            assertEquals(LoginPhase.Form, v.state.value.phase)
            awaitUntil {
                e.fake.connections[0].received.any {
                    JVal.parse(String(it, Charsets.UTF_8)).opt("method")?.asString() == "login.cancel"
                }
            }
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
