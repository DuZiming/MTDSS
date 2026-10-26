package com.mtdss.android.ui.send

import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.send.BridgeLimits
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 发送控制器单测(伪 daemon):条目数护栏前置(超限不出 RPC)、
 * transfer.start bridge 形态线格式、daemon 桥接错误透传、dir 记忆。
 */
class SendControllerTest {

    private class MemoryDir : SendController.DirMemory {
        override var last: String = ""
        override fun remember(dir: String) {
            last = dir
        }
    }

    private fun items(n: Int): List<BridgeItem> =
        (1..n).map { BridgeItem(ref = "content://p/$it", rel = "f$it.bin", size = it.toLong(), mtime = 100L + it) }

    private fun scriptStart(e: TestEnv, respond: (id: Long) -> String) {
        e.fake.onLine = { conn, line ->
            val req = JVal.parse(String(line, Charsets.UTF_8))
            val id = req.opt("id")!!.let { (it as JVal.Num).raw.toLong() }
            conn.sendLine(respond(id))
        }
    }

    @Test
    fun `超限护栏前置不出 RPC`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptStart(e) { id -> """{"id":$id,"result":{"job_id":"9"}}""" }
            val controller = SendController(e.repo, e.scope)
            controller.send(items(BridgeLimits.MAX_BRIDGE_ITEMS + 1), "dev-1", "C:\\recv", overwrite = false)
            // 词面 = BridgeLimits 源头(与 daemon 逐字一致);弹层渲染此 State.error
            assertEquals(
                BridgeLimits.itemCountError(BridgeLimits.MAX_BRIDGE_ITEMS + 1),
                controller.state.value.error,
            )
            assertEquals(
                "桥接条目过多(513 项,上限 512):请分批发送",
                controller.state.value.error,
            )
            assertTrue("超限不得发出 RPC(不建连接)", e.fake.connections.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `发送走 bridge 形态并记忆 dir`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptStart(e) { id -> """{"id":$id,"result":{"job_id":"42"}}""" }
            val memory = MemoryDir()
            val controller = SendController(e.repo, e.scope, memory)
            assertEquals("", controller.lastDir)

            controller.send(items(2), "dev-1", "C:/Users/me/Downloads", overwrite = true)
            // 应答到达即请求已上线(sentJobId 由应答揭开)
            awaitUntil { controller.state.value.sentJobId == "42" }
            // 线格式:bridge 形态 + 覆盖名单 = 全部 rel + cut 恒 false
            val req = JVal.parse(String(e.fake.connections[0].received[0], Charsets.UTF_8))
            assertEquals("transfer.start", req.opt("method")!!.asString())
            val params = req.opt("params")!!
            assertEquals("upload", params.opt("kind")!!.asString())
            assertEquals("bridge", params.opt("source")!!.asString())
            assertEquals("dev-1", params.opt("to")!!.asString())
            assertEquals("C:/Users/me/Downloads", params.opt("dir")!!.asString())
            assertEquals(false, params.opt("cut")!!.asBoolean())
            val items = params.opt("items")!!.asArray()
            assertEquals(2, items.size)
            assertEquals("content://p/1", items[0].opt("ref")!!.asString())
            assertEquals("f1.bin", items[0].opt("rel")!!.asString())
            assertEquals(1L, (items[0].opt("size") as JVal.Num).raw.toLong())
            assertEquals(listOf("f1.bin", "f2.bin"), params.opt("overwrite")!!.asArray().map { (it as JVal.Str).v })
            // 成功后 dir 入记忆
            assertEquals("C:/Users/me/Downloads", memory.last)
            assertEquals("C:/Users/me/Downloads", controller.lastDir)
        } finally {
            e.close()
        }
    }

    @Test
    fun `daemon 桥接错误透传`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptStart(e) { id ->
                """{"id":$id,"error":{"code":1,"message":"桥接条目过多(600 项,上限 512):请分批发送"}}"""
            }
            val controller = SendController(e.repo, e.scope)
            controller.send(items(2), "dev-1", "C:/recv", overwrite = false)
            awaitUntil { controller.state.value.error != null }
            assertEquals("桥接条目过多(600 项,上限 512):请分批发送", controller.state.value.error)
            assertNull(controller.state.value.sentJobId)
        } finally {
            e.close()
        }
    }
}
