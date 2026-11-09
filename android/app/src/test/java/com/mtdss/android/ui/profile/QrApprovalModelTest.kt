package com.mtdss.android.ui.profile

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.Ops
import com.mtdss.android.ipc.RpcException
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit.runTest
import com.mtdss.android.ui.profile.QrApprovalModel.Stage
import kotlinx.coroutines.delay
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 扫码授权闭环状态机 JVM 单测(V3-4 T12;iOS QrApprovalModelTests 语义镜像,
 * FakeDaemon 步骤脚本同形):
 * - claim:候选面 confirming 上帧(弹层数据源)/ 死码 typed not_found →
 *   failed 文案直出 / 超时 → 请求超时 / 连接失败 → 连接中断 / 失败后可再
 *   claim(重试面);
 * - approve:同意/拒绝结果行文案(ok/denied/expired/limit 全映射)+ 请求
 *   帧参数面(token/allow 逐字)+ 相位守卫(仅自 confirming 接受);
 * - dismiss 清场:迟到裁决不外发。
 */
class QrApprovalModelTest {

    /** 按 method 应答脚本(静态 result / `@error:` 前缀 = 错误帧;留痕供断言)。 */
    private fun script(
        fake: com.mtdss.android.ipc.FakeDaemon,
        rules: Map<String, String>,
    ): MutableList<String> {
        val received = mutableListOf<String>()
        fake.onLine = { conn, line ->
            val text = String(line, Charsets.UTF_8)
            synchronized(received) { received.add(text) }
            val frame = JVal.parse(text)
            val id = frame.opt("id")!!.asLong()
            val method = frame.opt("method")!!.asString()
            val rule = rules[method] ?: "{}"
            conn.sendLine(
                if (rule.startsWith("@error:")) {
                    """{"id":$id,"error":{"code":1,"message":"${rule.removePrefix("@error:")}","data":{"kind":"not_found"}}}"""
                } else {
                    """{"id":$id,"result":$rule}"""
                },
            )
        }
        return received
    }

    private val claimOk =
        """{"candidate_name":"契约新机","candidate_platform":"windows","expires_at":1760000300}"""

    // ── claim(候选确认面)──────────────────────────────────────────────────

    @Test
    fun `claim 成功落 confirming 候选面与 token 上帧`() = runTest {
        val e = TestEnv()
        try {
            val received = script(e.fake, mapOf(Ops.QR_CLAIM to claimOk))
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-contract-fixed")
            val stage = model.stage.value
            assertTrue("应落 confirming: $stage", stage is Stage.Confirming)
            val c = (stage as Stage.Confirming).candidate
            assertEquals("契约新机", c.candidateName)
            assertEquals("windows", c.candidatePlatform)
            assertEquals(1_760_000_300L, c.expiresAt)
            assertEquals("裁决随路 token 上帧", "qrt-contract-fixed", model.token.value)
            assertFalse(model.busy.value)

            // 请求参数面:token 逐字上帧
            val line = received.first { it.contains(Ops.QR_CLAIM) }
            assertTrue(line, line.contains("\"token\":\"qrt-contract-fixed\""))
        } finally {
            e.close()
        }
    }

    @Test
    fun `claim 死码 typed 错误文案直出`() = runTest {
        val e = TestEnv()
        try {
            script(e.fake, mapOf(Ops.QR_CLAIM to "@error:二维码已失效,请重新扫码"))
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-contract-dead")
            val stage = model.stage.value
            assertTrue("应落 failed: $stage", stage is Stage.Failed)
            assertEquals("二维码已失效,请重新扫码", (stage as Stage.Failed).text)
            assertNull("失败不记 token", model.token.value)
        } finally {
            e.close()
        }
    }

    @Test
    fun `claim 超时落请求超时文案`() = runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { _, _ -> } // 静默吞掉 qr.claim(无应答)
            val model = QrApprovalModel(e.repo, callTimeoutMs = 200)

            model.claim("qrt-never-answered")
            val stage = model.stage.value
            assertTrue("应落 failed: $stage", stage is Stage.Failed)
            assertEquals("请求超时", (stage as Stage.Failed).text)
        } finally {
            e.close()
        }
    }

    @Test
    fun `claim 连接失败落连接中断文案`() = runTest {
        val e = TestEnv()
        try {
            e.fake.connectFailure = java.io.IOException("refused")
            val model = QrApprovalModel(e.repo, callTimeoutMs = 2_000)

            model.claim("qrt-no-conn")
            val stage = model.stage.value
            assertTrue("应落 failed: $stage", stage is Stage.Failed)
            assertEquals("请求失败:连接中断", (stage as Stage.Failed).text)
        } finally {
            e.close()
        }
    }

    @Test
    fun `claim 失败后可再 claim 重试收口 confirming`() = runTest {
        val e = TestEnv()
        try {
            var dead = true
            e.fake.onLine = { conn, line ->
                val frame = JVal.parse(String(line, Charsets.UTF_8))
                val id = frame.opt("id")!!.asLong()
                val method = frame.opt("method")!!.asString()
                conn.sendLine(
                    if (method == Ops.QR_CLAIM && dead) {
                        """{"id":$id,"error":{"code":1,"message":"二维码已失效,请重新扫码","data":{"kind":"not_found"}}}"""
                    } else {
                        """{"id":$id,"result":$claimOk}"""
                    },
                )
            }
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-dead-once")
            assertTrue(model.stage.value is Stage.Failed)
            // daemon 侧码复活后原入口再扫:failed 相位不拦 claim
            dead = false
            model.claim("qrt-retry")
            assertTrue("重试应落 confirming: ${model.stage.value}", model.stage.value is Stage.Confirming)
            assertEquals("qrt-retry", model.token.value)
        } finally {
            e.close()
        }
    }

    // ── approve(结果行)────────────────────────────────────────────────────

    @Test
    fun `approve 同意 ok 落结果行与线上参数`() = runTest {
        val e = TestEnv()
        try {
            val received = script(e.fake, mapOf(Ops.QR_CLAIM to claimOk, Ops.QR_APPROVE to """{"status":"ok"}"""))
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-contract-fixed")
            model.approve(allow = true)
            val stage = model.stage.value
            assertTrue("应落 result: $stage", stage is Stage.Result)
            assertEquals("已同意扫码登录", (stage as Stage.Result).line)
            assertFalse(model.busy.value)
            val line = received.first { it.contains(Ops.QR_APPROVE) }
            assertTrue(line, line.contains("\"token\":\"qrt-contract-fixed\""))
            assertTrue(line, line.contains("\"allow\":true"))
        } finally {
            e.close()
        }
    }

    @Test
    fun `approve 拒绝 denied 落结果行与 allow false`() = runTest {
        val e = TestEnv()
        try {
            val received = script(e.fake, mapOf(Ops.QR_CLAIM to claimOk, Ops.QR_APPROVE to """{"status":"denied"}"""))
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-contract-fixed")
            model.approve(allow = false)
            val stage = model.stage.value
            assertTrue("应落 result: $stage", stage is Stage.Result)
            assertEquals("已拒绝该设备登录", (stage as Stage.Result).line)
            val line = received.first { it.contains(Ops.QR_APPROVE) }
            assertTrue(line, line.contains("\"allow\":false"))
        } finally {
            e.close()
        }
    }

    @Test
    fun `approve expired 与 limit 状态行映射`() = runTest {
        for ((status, want) in listOf(
            "expired" to "二维码已过期,请重新扫码",
            "limit" to "对方需选择移除设备后完成登录",
        )) {
            val e = TestEnv()
            try {
                script(e.fake, mapOf(Ops.QR_CLAIM to claimOk, Ops.QR_APPROVE to """{"status":"$status"}"""))
                val model = QrApprovalModel(e.repo)

                model.claim("qrt-contract-fixed")
                model.approve(allow = true)
                val stage = model.stage.value
                assertTrue("应落 result: $stage", stage is Stage.Result)
                assertEquals(want, (stage as Stage.Result).line)
            } finally {
                e.close()
            }
        }
    }

    @Test
    fun `approve RPC 错误落 failed 文案`() = runTest {
        val e = TestEnv()
        try {
            script(
                e.fake,
                mapOf(Ops.QR_CLAIM to claimOk, Ops.QR_APPROVE to "@error:该二维码已由其他设备认领"),
            )
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-contract-fixed")
            model.approve(allow = true)
            val stage = model.stage.value
            assertTrue("应落 failed: $stage", stage is Stage.Failed)
            assertEquals("该二维码已由其他设备认领", (stage as Stage.Failed).text)
        } finally {
            e.close()
        }
    }

    // ── 相位守卫 / 清场 ─────────────────────────────────────────────────────

    @Test
    fun `非 confirming 相位裁决零外发`() = runTest {
        val e = TestEnv()
        try {
            val received = script(e.fake, mapOf(Ops.QR_APPROVE to """{"status":"ok"}"""))
            val model = QrApprovalModel(e.repo)

            // 初始态直接裁决:守卫拒,零外发
            model.approve(allow = true)
            delay(100)
            assertTrue("非 confirming 相位不外发裁决", received.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `dismiss 清场且迟到裁决零外发`() = runTest {
        val e = TestEnv()
        try {
            val received = script(e.fake, mapOf(Ops.QR_CLAIM to claimOk, Ops.QR_APPROVE to """{"status":"ok"}"""))
            val model = QrApprovalModel(e.repo)

            model.claim("qrt-contract-fixed")
            assertTrue(model.stage.value is Stage.Confirming)
            model.dismiss()
            assertEquals(Stage.Idle, model.stage.value)
            assertNull("清场后 token 撤帧", model.token.value)
            assertFalse(model.busy.value)

            // 迟到裁决(弹层已关):守卫拒,零外发
            model.approve(allow = true)
            delay(100)
            assertFalse("dismiss 后迟到裁决不外发", received.any { it.contains(Ops.QR_APPROVE) })
        } finally {
            e.close()
        }
    }

    // ── 文案映射表 ──────────────────────────────────────────────────────────

    @Test
    fun `resultLine 映射表`() {
        assertEquals("已同意扫码登录", QrApprovalModel.resultLine("ok"))
        assertEquals("已拒绝该设备登录", QrApprovalModel.resultLine("denied"))
        assertEquals("二维码已过期,请重新扫码", QrApprovalModel.resultLine("expired"))
        assertEquals("对方需选择移除设备后完成登录", QrApprovalModel.resultLine("limit"))
        assertEquals("二维码状态异常", QrApprovalModel.resultLine("mystery"))
    }

    @Test
    fun `userErrorText 分支表`() {
        assertEquals("死码文案", QrApprovalModel.userErrorText(RpcException(1, "死码文案")))
        assertEquals(
            "请求失败:连接中断",
            QrApprovalModel.userErrorText(java.io.IOException("boom")),
        )
        runTest {
            // 真实 TimeoutCancellationException(构造器 internal,借 withTimeout 铸)
            val timeout: kotlinx.coroutines.TimeoutCancellationException = try {
                kotlinx.coroutines.withTimeout(1) { kotlinx.coroutines.delay(200) }
                error("未按预期超时")
            } catch (e: kotlinx.coroutines.TimeoutCancellationException) {
                e
            }
            assertEquals("请求超时", QrApprovalModel.userErrorText(timeout))
        }
    }

    // ── QrLoginCode(纯解析面;iOS QrLoginCodeTests 同象)────────────────────

    @Test
    fun `parse 本家 URL 取 t 参数`() {
        assertEquals("qrt-ok", QrLoginCode.parse("https://www.mtdss.top/qr?t=qrt-ok"))
        assertEquals("空白首尾容忍", "qrt-ok", QrLoginCode.parse("  https://www.mtdss.top/qr?t=qrt-ok\n"))
    }

    @Test
    fun `parse 非本家形态拒绝`() {
        assertNull("他家 URL", QrLoginCode.parse("https://evil.example/qr?t=qrt-ok"))
        assertNull("大小写敏感防钓鱼", QrLoginCode.parse("https://WWW.MTDSS.TOP/qr?t=qrt-ok"))
        assertNull("纯文本", QrLoginCode.parse("随便什么文本"))
        assertNull("空 token", QrLoginCode.parse("https://www.mtdss.top/qr?t="))
    }

    @Test
    fun `parse 多参数取 t 段`() {
        assertEquals("qrt-ok", QrLoginCode.parse("https://www.mtdss.top/qr?t=qrt-ok&x=1"))
    }
}
