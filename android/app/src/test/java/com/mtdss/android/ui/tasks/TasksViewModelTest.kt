package com.mtdss.android.ui.tasks

import com.mtdss.android.ipc.Goldens
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.send.DocMeta
import com.mtdss.android.send.FakeSafFace
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.delay
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 任务列表 VM JVM 单测(伪 daemon):初拉 golden + 事件增量收敛(终态粘滞/
 * 会话行终态即消失)、操作线格式与 busy、操作错误透传、重发路由核心契约
 * (bridge 上传 = App 侧重建发新 transfer.start;download = daemon op;
 * 分享件源已失 = 零 RPC 拒绝)、断连自愈补拉重置。
 */
class TasksViewModelTest {

    private val root = StoredRoot("r1", "content://tree/r1", "doc-r1", "授权根")

    /** 伪 daemon 应答脚本:按 method 分发;transfer.list 用注入的 jobs JSON。 */
    private fun script(e: TestEnv, jobsJson: () -> String) {
        e.fake.onLine = { conn, line ->
            val req = JVal.parse(String(line, Charsets.UTF_8))
            val id = (req.opt("id") as? JVal.Num)?.raw?.toLong()
            if (id != null) {
                when (req.opt("method")?.asString()) {
                    "transfer.list" -> conn.sendLine("""{"id":$id,"result":{"jobs":${jobsJson()}}}""")
                    "transfer.start" -> conn.sendLine("""{"id":$id,"result":{"job_id":"new-1"}}""")
                    "transfer.pause", "transfer.resume", "transfer.cancel",
                    "transfer.reinitiate", "transfer.clear",
                    -> conn.sendLine("""{"id":$id,"result":{}}""")
                }
            }
        }
    }

    /** golden transfer.list(3 行:upload failed / download cancelled / upload cancelled)。 */
    private fun goldenJobs(): String {
        val frame = JVal.parse(String(Goldens.read("transfer.list.resp.json"), Charsets.UTF_8)) as JVal.Obj
        return ((frame["result"] as JVal.Obj)["jobs"] as JVal.Arr).toCanonicalJson()
    }

    /** bridge 形态上传失败记录(src = 任务 src_paths 序列化,重建原料)。 */
    private fun bridgeJobJson(): String =
        """{"id":"b1","kind":"upload","name":"a.bin","state":"failed","files_done":0,"files_total":2,""" +
            """"bytes_done":0,"bytes_total":300,"speed_bps":0,"skipped":0,"relay":false,""" +
            """"src":"bridge://roottree://r1/docs/a.bin, bridge://content://pick/x.bin",""" +
            """"dst":"dev-pc:/golden/peer-inbox","error":"发送失败(共尝试 6 轮)"}"""

    private fun downloadJobJson(): String =
        """{"id":"d1","kind":"download","name":"远端文档","state":"interrupted","files_done":0,"files_total":1,""" +
            """"bytes_done":0,"bytes_total":100,"speed_bps":0,"skipped":0,"relay":false,""" +
            """"src":"dev-pc:/远端/文档.pdf","dst":"inbox","error":null}"""

    private fun makeVm(e: TestEnv, saf: FakeSafFace, roots: List<StoredRoot> = listOf(root)): TasksViewModel =
        TasksViewModel(e.ipc, e.repo, RootLookup { roots }, saf, e.scope)

    /** 全部连接已上线的请求 method 序列(时序断言用)。 */
    private fun sentMethods(e: TestEnv): List<String> =
        e.fake.connections.flatMap { c ->
            c.received.mapNotNull { line ->
                try {
                    JVal.parse(String(line, Charsets.UTF_8)).opt("method")?.asString()
                } catch (_: Exception) {
                    null
                }
            }
        }

    /** 首个匹配 method 的请求 params(结构断言用)。 */
    private fun sentRequest(e: TestEnv, method: String): JVal.Obj? =
        e.fake.connections.asSequence()
            .flatMap { it.received.asSequence() }
            .mapNotNull { line ->
                try {
                    JVal.parse(String(line, Charsets.UTF_8)) as? JVal.Obj
                } catch (_: Exception) {
                    null
                }
            }
            .firstOrNull { it.opt("method")?.asString() == method }
            ?.opt("params") as? JVal.Obj

    private fun sendEvent(e: TestEnv, params: String) {
        e.fake.connections[0].sendLine("""{"jsonrpc":"2.0","method":"event.transfer","params":$params}""")
    }

    // ── 初拉与事件收敛 ──────────────────────────────────────────────────────

    @Test
    fun `初拉 transfer list 全量入列`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val jobs = goldenJobs()
            script(e) { jobs }
            val vm = makeVm(e, FakeSafFace())
            awaitUntil { vm.state.value.rows.size == 3 }
            val s = vm.state.value
            assertFalse(s.loading)
            assertNull(s.error)
            // golden 字段逐项(契约样本.bin 失败行)
            val r1 = s.rows.first { it.id == "1" }
            assertEquals("upload", r1.kind)
            assertEquals("failed", r1.state)
            assertEquals("契约样本.bin", r1.name)
            assertEquals("contract-fixture/契约样本.bin", r1.src)
            assertEquals("dev-peer-fixed:/golden/peer-inbox", r1.dst)
            assertTrue(r1.error!!.contains("信令未在线"))
            // 全终态 → 不占屏
            assertFalse(s.keepScreenOn)
        } finally {
            e.close()
        }
    }

    @Test
    fun `事件增量收敛,粘滞,会话行终态即消失`() = TestKit.runTest {
        val e = TestEnv()
        try {
            // 活行(id=5 running)+ 终行(id=2 cancelled):增量与粘滞两面各占一行
            val jobs =
                """[{"id":"5","kind":"upload","name":"大文件.iso","state":"running","files_done":0,"files_total":1,""" +
                    """"bytes_done":0,"bytes_total":4096,"speed_bps":0,"skipped":0,"relay":false,""" +
                    """"src":"bridge://roottree://r1/大文件.iso","dst":"dev-pc:/inbox","error":null},""" +
                    """{"id":"2","kind":"download","name":"契约 文档","state":"cancelled","files_done":0,"files_total":0,""" +
                    """"bytes_done":0,"bytes_total":0,"speed_bps":0,"skipped":0,"relay":false,""" +
                    """"src":"dev-peer-fixed:/远端/契约 文档.pdf","dst":"/golden/inbox","error":null}]"""
            script(e) { jobs }
            val vm = makeVm(e, FakeSafFace())
            awaitUntil { vm.state.value.rows.size == 2 }

            // 增量:id=5 进度推进(计数/速率/relay 落,name/src/dst 保留前值)
            sendEvent(
                e,
                """{"id":"5","kind":"upload","state":"running","files_done":0,"files_total":1,""" +
                    """"bytes_done":1024,"bytes_total":4096,"speed_bps":2048,"skipped":0,""" +
                    """"relay":true,"error":null}""",
            )
            awaitUntil { vm.state.value.rows.first { it.id == "5" }.bytesDone == 1024L }
            val r5 = vm.state.value.rows.first { it.id == "5" }
            assertTrue(r5.relay)
            assertEquals("大文件.iso", r5.name) // 事件帧不带 name,保留前值
            assertEquals("bridge://roottree://r1/大文件.iso", r5.src)
            assertTrue(vm.state.value.keepScreenOn) // 活任务占屏

            // 终态粘滞:id=2 已 cancelled,迟到 running 帧忽略(以同流后发事件
            // 的可见效果作序证明:事件按序收敛,id5 暂停可见时 id2 帧已处理)
            sendEvent(
                e,
                """{"id":"2","kind":"download","state":"running","files_done":0,"files_total":1,""" +
                    """"bytes_done":5,"bytes_total":10,"speed_bps":1,"skipped":0,""" +
                    """"relay":false,"error":null}""",
            )
            sendEvent(
                e,
                """{"id":"5","kind":"upload","state":"paused","files_done":0,"files_total":1,""" +
                    """"bytes_done":1024,"bytes_total":4096,"speed_bps":0,"skipped":0,""" +
                    """"relay":true,"error":null}""",
            )
            awaitUntil { vm.state.value.rows.first { it.id == "5" }.state == "paused" }
            assertEquals("cancelled", vm.state.value.rows.first { it.id == "2" }.state)
            assertEquals(0, vm.state.value.rows.first { it.id == "2" }.bytesDone)

            // 会话行:活态建行(显示名兜底 id)→ 终态摘行
            sendEvent(
                e,
                """{"id":"9","kind":"incoming","state":"running","files_done":0,"files_total":0,""" +
                    """"bytes_done":10,"bytes_total":0,"speed_bps":5,"skipped":0,""" +
                    """"relay":false,"error":null}""",
            )
            awaitUntil { vm.state.value.rows.any { it.id == "9" } }
            assertEquals("9", vm.state.value.rows.first { it.id == "9" }.name)
            sendEvent(
                e,
                """{"id":"9","kind":"incoming","state":"done","files_done":1,"files_total":1,""" +
                    """"bytes_done":10,"bytes_total":10,"speed_bps":0,"skipped":0,""" +
                    """"relay":false,"error":null}""",
            )
            awaitUntil { vm.state.value.rows.none { it.id == "9" } }
        } finally {
            e.close()
        }
    }

    // ── 操作:线格式与 busy ─────────────────────────────────────────────────

    @Test
    fun `pause 操作出线且 busy 收尾`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val jobs = goldenJobs()
            script(e) { jobs }
            val vm = makeVm(e, FakeSafFace())
            awaitUntil { vm.state.value.rows.size == 3 }

            vm.pause("3")
            awaitUntil { "transfer.pause" in sentMethods(e) }
            val params = sentRequest(e, "transfer.pause")!!
            assertEquals("3", params.opt("id")!!.asString())
            awaitUntil { vm.state.value.busy.isEmpty() }
        } finally {
            e.close()
        }
    }

    @Test
    fun `操作失败透传 opError`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = (req.opt("id") as? JVal.Num)?.raw?.toLong()
                if (id != null) {
                    if (req.opt("method")?.asString() == "transfer.clear") {
                        conn.sendLine("""{"id":$id,"error":{"code":3,"message":"对端不在线,无法清除"}}""")
                    } else {
                        conn.sendLine("""{"id":$id,"result":{"jobs":[]}}""")
                    }
                }
            }
            val vm = makeVm(e, FakeSafFace())
            awaitUntil { "transfer.list" in sentMethods(e) && !vm.state.value.loading }

            vm.clear("3")
            awaitUntil { vm.state.value.opError != null }
            assertEquals("对端不在线,无法清除", vm.state.value.opError)
            awaitUntil { vm.state.value.busy.isEmpty() }
            vm.clearOpError()
            assertNull(vm.state.value.opError)
        } finally {
            e.close()
        }
    }

    // ── 重新发起路由(核心契约)──────────────────────────────────────────────

    @Test
    fun `bridge 上传重发走 App 侧重建,发新 transfer start`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val jobs = "[${bridgeJobJson()}]"
            script(e) { jobs }
            val saf = FakeSafFace()
            saf.metas[saf.buildChildUri(root.treeUri, "${root.treeDocId}/docs/a.bin")] = DocMeta("a.bin", 100, 555)
            saf.metas["content://pick/x.bin"] = DocMeta("x.bin", 200, 666)
            val vm = makeVm(e, saf)
            awaitUntil { vm.state.value.rows.size == 1 }

            vm.reinitiate("b1")
            awaitUntil { "transfer.start" in sentMethods(e) }
            // 路由正确:不经 transfer.reinitiate(daemon 对 bridge 记录恒「无可传内容」)
            assertFalse("transfer.reinitiate" in sentMethods(e))
            // 新 start 的 items = App 侧重建(ref 重解析 + 现查元数据;rel 逐字保真)
            val params = sentRequest(e, "transfer.start")!!
            assertEquals("upload", params.opt("kind")!!.asString())
            assertEquals("bridge", params.opt("source")!!.asString())
            assertEquals("dev-pc", params.opt("to")!!.asString())
            assertEquals("/golden/peer-inbox", params.opt("dir")!!.asString())
            val items = params.opt("items")!!.asArray()
            assertEquals(2, items.size)
            assertEquals("roottree://r1/docs/a.bin", items[0].opt("ref")!!.asString())
            assertEquals("docs/a.bin", items[0].opt("rel")!!.asString())
            assertEquals(100L, (items[0].opt("size") as JVal.Num).raw.toLong())
            assertEquals("content://pick/x.bin", items[1].opt("ref")!!.asString())
            assertEquals("x.bin", items[1].opt("rel")!!.asString())
            // 断点续传:skip/overwrite 双空 + cut 恒 false
            assertTrue(params.opt("skip")!!.asArray().isEmpty())
            assertTrue(params.opt("overwrite")!!.asArray().isEmpty())
            assertEquals(false, params.opt("cut")!!.asBoolean())
            // 成功后重拉对账 + busy 收尾 + opError 空
            awaitUntil { vm.state.value.busy.isEmpty() }
            assertNull(vm.state.value.opError)
            assertTrue(sentMethods(e).count { it == "transfer.list" } >= 2)
        } finally {
            e.close()
        }
    }

    @Test
    fun `download 重发走 daemon op`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val jobs = "[${downloadJobJson()}]"
            script(e) { jobs }
            val vm = makeVm(e, FakeSafFace())
            awaitUntil { vm.state.value.rows.size == 1 }

            vm.reinitiate("d1")
            awaitUntil { "transfer.reinitiate" in sentMethods(e) }
            assertFalse("transfer.start" in sentMethods(e)) // 不走 App 侧重建
            assertEquals("d1", sentRequest(e, "transfer.reinitiate")!!.opt("id")!!.asString())
            awaitUntil { vm.state.value.busy.isEmpty() }
        } finally {
            e.close()
        }
    }

    @Test
    fun `分享件源已失重发拒绝,零 RPC 零重发`() = TestKit.runTest {
        val e = TestEnv()
        try {
            // 记录的 src 是分享件 ref(临时授权已随进程死亡)→ 元数据查不到
            val deadShare =
                """{"id":"s1","kind":"upload","name":"share.bin","state":"failed","files_done":0,"files_total":1,""" +
                    """"bytes_done":0,"bytes_total":10,"speed_bps":0,"skipped":0,"relay":false,""" +
                    """"src":"bridge://content://share/dead.bin","dst":"dev-pc:/in","error":"发送失败"}"""
            val jobs = "[$deadShare]"
            script(e) { jobs }
            val vm = makeVm(e, FakeSafFace()) // 无元数据表 → 拒
            awaitUntil { vm.state.value.rows.size == 1 }

            vm.reinitiate("s1")
            awaitUntil { vm.state.value.opError != null }
            assertTrue(vm.state.value.opError!!.startsWith("源已不存在"))
            assertFalse("transfer.start" in sentMethods(e))
            assertFalse("transfer.reinitiate" in sentMethods(e))
            awaitUntil { vm.state.value.busy.isEmpty() }
        } finally {
            e.close()
        }
    }

    // ── 签名级在途防连点(双终态同签名行的双发窗口)────────────────────────

    @Test
    fun `同签名双终态行,重建窗口内 B 行不可双发,窗口结束恢复`() = TestKit.runTest {
        val e = TestEnv()
        try {
            // 双终态同签名行(原始失败行 + 重发后再失败行):headline
            // 「熄屏中断→重发→对端离线失败」的自然态
            fun dupFail(id: String) =
                """{"id":"$id","kind":"upload","name":"a.bin","state":"failed","files_done":0,"files_total":1,""" +
                    """"bytes_done":0,"bytes_total":100,"speed_bps":0,"skipped":0,"relay":false,""" +
                    """"src":"bridge://content://pick/x.bin","dst":"dev-pc:/in","error":"发送失败"}"""
            val jobs = "[${dupFail("f1")}, ${dupFail("f2")}]"

            // transfer.start 应答扣住:重建窗口保持打开(逐 ref SAF 现查 +
            // start + 行内 reseed 的全程);放行由测试显式送应答
            val heldStartIds = CopyOnWriteArrayList<Long>()
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = (req.opt("id") as? JVal.Num)?.raw?.toLong()
                if (id != null) {
                    when (req.opt("method")?.asString()) {
                        "transfer.list" -> conn.sendLine("""{"id":$id,"result":{"jobs":$jobs}}""")
                        "transfer.start" -> heldStartIds += id // 扣住不答
                        else -> conn.sendLine("""{"id":$id,"result":{}}""")
                    }
                }
            }
            val saf = FakeSafFace()
            saf.metas["content://pick/x.bin"] = DocMeta("x.bin", 200, 666)
            val vm = makeVm(e, saf)
            awaitUntil { vm.state.value.rows.size == 2 }

            // 点 A 行:f1 重建窗口打开(请求已上线、应答未归)
            vm.reinitiate("f1")
            awaitUntil { heldStartIds.size == 1 }
            // 窗口内:同签名(= B 行签名)已入在途集——屏面 B 行按钮因此置灰
            val sig = TasksReducer.signatureOf(vm.state.value.rows.first { it.id == "f2" })
            assertTrue(sig in vm.state.value.busySigs)
            // 窗口内点 B 行:no-op(无第二个 transfer.start 出线)
            vm.reinitiate("f2")
            delay(300)
            assertEquals("同签名在途集必须拦下 B 行重发", 1, heldStartIds.size)

            // 放行 A 的应答:窗口收尾(行内 reseed + busy/busySigs 清空)
            e.fake.connections[0].sendLine("""{"id":${heldStartIds[0]},"result":{"job_id":"n1"}}""")
            awaitUntil { vm.state.value.busySigs.isEmpty() && vm.state.value.busy.isEmpty() }
            // 窗口结束:B 行恢复可点(第二个 transfer.start 出线 = 修复语义)
            vm.reinitiate("f2")
            awaitUntil { heldStartIds.size == 2 }
        } finally {
            e.close()
        }
    }

    // ── 重连补拉重置(transferListRefresh)──────────────────────────────────

    @Test
    fun `断连自愈后补拉结果重置行集`() = TestKit.runTest {
        val e = TestEnv()
        try {
            var jobs = goldenJobs()
            script(e) { jobs }
            val vm = makeVm(e, FakeSafFace())
            awaitUntil { vm.state.value.rows.size == 3 }

            // 服务端换内容(daemon 已换实例的等价面)后断连:自愈重连 → 补拉 → 重置
            jobs =
                """[{"id":"7","kind":"upload","name":"新实例任务","state":"running","files_done":0,"files_total":1,""" +
                    """"bytes_done":1,"bytes_total":10,"speed_bps":1,"skipped":0,"relay":false,""" +
                    """"src":"bridge://a","dst":"dev:/d","error":null}]"""
            e.fake.connections[0].close()
            awaitUntil { vm.state.value.rows.size == 1 && vm.state.value.rows[0].id == "7" }
            assertTrue(vm.state.value.keepScreenOn) // running 行占屏
        } finally {
            e.close()
        }
    }
}
