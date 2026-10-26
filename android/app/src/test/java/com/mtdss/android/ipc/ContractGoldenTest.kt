package com.mtdss.android.ipc

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

/**
 * IPC 契约 golden 双端锁定:crates/ipc/contract 目录下 *.json(daemon 契约
 * 测试同源,单一真源)逐一对齐 Kotlin 客户端——
 *
 * - `{op}.req.json`:类型化调用经伪 server 走完整客户端路径,线上请求帧与
 *   golden 文件**逐字节**比对(键序 = serde_json 字母序,含尾换行);
 * - `{op}.resp.json`:类型化解析吃下 golden 应答并核对字段值;错误形态核对
 *   RpcException(code/message/kind);
 * - `{event}.json`:事件帧经事件流发射并类型化解析核对;
 * - **孤儿 golden 防护**:目录里出现客户端面未绑定、也未声明不移植的形态
 *   → 立即红(T3 已知盲区:golden 增枚不声不响);全部文件必须被消费。
 */
class ContractGoldenTest {

    /** 客户端面不移植的 req 形态(spec §4.2 便携面之外;golden 仅解析校验)。 */
    private val notPortedForms = setOf("set_recv_dir", "transfer.start.local_copy", "transfer.start.remote_copy")

    /** android 变体 golden(T6 平台投影:status 收件箱哨兵 / set_recv_dir 拒绝)。 */
    private val androidVariants = setOf("status.resp.android.json", "set_recv_dir.resp.android.json")

    /** 事件 golden 文件 → 期望断言。 */
    private val eventChecks: Map<String, (EventFrame) -> Unit> = linkedMapOf(
        "event.login.json" to { f ->
            assertEquals("event.login", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Login
            assertEquals(true, e.loggedIn)
        },
        "event.online.json" to { f ->
            assertEquals("event.online", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Online
            assertEquals(true, e.online)
        },
        "event.devices.json" to { f ->
            assertEquals("event.devices", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Devices
            assertEquals(2, e.devices.size)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", e.devices[0].deviceId)
            assertEquals("契约本机", e.devices[0].name)
            assertEquals("windows", e.devices[0].platform)
            assertEquals("dev-peer-fixed", e.devices[1].deviceId)
            assertEquals("linux", e.devices[1].platform)
        },
        "event.transfer.json" to { f ->
            assertEquals("event.transfer", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Transfer
            val p = e.progress
            assertEquals("3", p.id)
            assertEquals("upload", p.kind)
            assertEquals("queued", p.state)
            assertEquals(0L, p.bytesDone)
            assertEquals(4096L, p.bytesTotal)
            assertEquals(0L, p.filesDone)
            assertEquals(1L, p.filesTotal)
            assertEquals(0L, p.skipped)
            assertEquals(0L, p.speedBps)
            assertEquals(false, p.relay)
            assertNull(p.error)
        },
        "event.notify.json" to { f ->
            assertEquals("event.notify", f.method)
            val e = MtdssEvent.parse(f) as MtdssEvent.Notify
            assertEquals("MTDSS", e.title)
            assertEquals("«契约对端» 向 /mtdss/contract-inbox 写入 2 个文件", e.body)
        },
    )

    /**
     * 客户端面全形态注册表:golden 形态名 → 类型化调用 + 结果断言。
     * 形态名 = `{op}.req.json` 去后缀(transfer.start 的多形态各占一枚)。
     */
    private val forms: Map<String, suspend IpcClient.() -> Unit> = linkedMapOf(
        "hello" to {
            val v = hello().getOrThrow()
            assertEquals("1.0.0", v.version)
            assertEquals(true, v.loggedIn)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.device?.deviceId)
            assertNull(v.device?.name)
        },
        "status" to {
            val v = status().getOrThrow()
            assertEquals(true, v.loggedIn)
            assertEquals(false, v.online)
            assertEquals("http://127.0.0.1:39731", v.server)
            assertEquals("/mtdss/contract-inbox", v.recvDir)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.device?.deviceId)
        },
        "devices" to {
            val v = devices().getOrThrow()
            assertEquals(false, v.stale)
            assertEquals(2, v.devices.size)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.devices[0].deviceId)
            assertEquals("契约本机", v.devices[0].name)
            assertEquals("windows", v.devices[0].platform)
            assertEquals(false, v.devices[0].online)
            assertEquals("dev-peer-fixed", v.devices[1].deviceId)
            assertEquals("契约对端", v.devices[1].name)
            assertEquals("linux", v.devices[1].platform)
        },
        "login.request_code" to {
            val v = loginRequestCode("http://127.0.0.1:39731", "contract@example.com").getOrThrow()
            assertEquals("654321", v.code)
        },
        "login.verify" to {
            val v = loginVerify("654321", "contract@example.com", "契约本机").getOrThrow()
            assertEquals("10ba682c8ad13513971e8b56881aab8b", v.deviceId)
            assertEquals("契约本机", v.name)
        },
        "login.logout" to {
            assertEquals(Unit, loginLogout().getOrThrow())
        },
        "list" to {
            val e = list("dev-peer-fixed", "/远端/文档").exceptionOrNull() as RpcException
            assertEquals(3L, e.code)
            assertEquals("设备离线: dev-peer-fixed", e.message)
            assertNull(e.kind)
        },
        "stat" to {
            val e = stat("dev-peer-fixed", "/远端/契约 文档.pdf").exceptionOrNull() as RpcException
            assertEquals(3L, e.code)
            assertEquals("设备离线: dev-peer-fixed", e.message)
        },
        "transfer.start.upload" to {
            val v = transferStartUpload(
                files = listOf("contract-fixture/契约样本.bin"),
                to = "dev-peer-fixed",
                dir = "/golden/peer-inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).getOrThrow()
            assertEquals("3", v.jobId)
        },
        "transfer.start.upload-bridge" to {
            val v = transferStartUploadBridge(
                items = listOf(BridgeItem(ref = "bridge-sample-ref", rel = "契约桥接样本.bin", size = 4096, mtime = 1_760_000_000_000)),
                to = "dev-peer-fixed",
                dir = "/golden/peer-inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).getOrThrow()
            assertEquals("6", v.jobId)
        },
        "transfer.start.download" to {
            val e = transferStartDownload(
                device = "dev-peer-fixed",
                paths = listOf("/远端/契约 文档.pdf"),
                dir = "golden/inbox",
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).exceptionOrNull() as RpcException
            assertEquals(1L, e.code)
            assertEquals("dir 非法(须为绝对路径,Task 1 口径): golden/inbox", e.message)
        },
        "transfer.start.download-inbox" to {
            val e = transferStartDownload(
                device = "dev-peer-fixed",
                paths = listOf("/远端/契约 文档.pdf"),
                dir = "inbox", // 收件箱哨兵(T6):同一 builder,dir 取哨兵值
                skip = emptyList(),
                overwrite = emptyList(),
                cut = false,
            ).exceptionOrNull() as RpcException
            assertEquals(1L, e.code)
            assertEquals("dir 非法(须为绝对路径,Task 1 口径): inbox", e.message)
        },
        "transfer.list" to {
            val v = transferList().getOrThrow()
            assertEquals(3, v.jobs.size)
            val j1 = v.jobs[0]
            assertEquals("1", j1.id)
            assertEquals("upload", j1.kind)
            assertEquals("契约样本.bin", j1.name)
            assertEquals("failed", j1.state)
            assertEquals(0L, j1.filesDone)
            assertEquals(1L, j1.filesTotal)
            assertEquals(0L, j1.bytesDone)
            assertEquals(4096L, j1.bytesTotal)
            assertEquals("contract-fixture/契约样本.bin", j1.src)
            assertEquals("dev-peer-fixed:/golden/peer-inbox", j1.dst)
            assertEquals("发送失败(共尝试 6 轮,最后错误: 信令未在线,无法发起发送)", j1.error)
            val j2 = v.jobs[1]
            assertEquals("2", j2.id)
            assertEquals("download", j2.kind)
            assertEquals("cancelled", j2.state)
            assertEquals("dev-peer-fixed:/远端/契约 文档.pdf", j2.src)
            assertEquals("/golden/inbox", j2.dst)
            assertNull(j2.error)
            val j3 = v.jobs[2]
            assertEquals("3", j3.id)
            assertEquals("cancelled", j3.state)
            assertEquals(0L, j3.speedBps)
            assertEquals(false, j3.relay)
        },
        "transfer.pause" to {
            assertEquals(Unit, transferPause("3").getOrThrow())
        },
        "transfer.resume" to {
            assertEquals(Unit, transferResume("3").getOrThrow())
        },
        "transfer.cancel" to {
            assertEquals(Unit, transferCancel("3").getOrThrow())
        },
        "transfer.wait" to {
            val v = transferWait("3").getOrThrow()
            assertEquals("cancelled", v.state)
            assertNull(v.error)
            assertEquals(0L, v.filesDone)
            assertEquals(1L, v.filesTotal)
            assertEquals(4096L, v.bytesTotal)
            assertEquals(0L, v.skipped)
        },
        "transfer.reinitiate" to {
            assertEquals(Unit, transferReinitiate("1").getOrThrow())
        },
        "transfer.clear" to {
            assertEquals(Unit, transferClear("2").getOrThrow())
        },
    )

    @Test
    fun `全部 req 形态——请求帧逐字节对齐且应答类型化解析吃下 golden`() = runBlocking {
        val dir = Goldens.dir()
        val reqFiles = dir.listFiles { f: File -> f.name.endsWith(".req.json") }!!
            .sortedBy { it.name }
        assertTrue("golden 目录为空", reqFiles.isNotEmpty())

        // 孤儿 golden 防护:未绑定且未声明不移植的新形态 → 红
        val unbound = reqFiles.map { it.name.removeSuffix(".req.json") }.filter { it !in forms && it !in notPortedForms }
        assertTrue("golden 出现未绑定形态(客户端面缺绑定,或需声明不移植): $unbound", unbound.isEmpty())

        for (f in reqFiles) {
            val form = f.name.removeSuffix(".req.json")
            if (form in notPortedForms) continue // 解析校验在「全量文件无遗漏」用例
            val respBytes = dir.resolve("$form.resp.json").readBytes()
            val fake = FakeDaemon()
            fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine(Goldens.withId(respBytes, id))
            }
            val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
            try {
                withTimeout(10_000) { forms.getValue(form)(client) }
                // 字节级请求帧比对:golden 文件内容 = 线上帧(含尾换行)
                assertEquals("$form 恰好发出一个请求", 1, fake.connections.single().received.size)
                assertArrayEquals("$form 请求帧漂移", f.readBytes(), fake.connections.single().received[0])
            } catch (e: Throwable) {
                throw AssertionError("形态 $form 失败: ${e.message}", e)
            } finally {
                client.close()
            }
        }
    }

    @Test
    fun `android 变体——status 收件箱哨兵投影`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read("status.resp.android.json"), id))
        }
        val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
        try {
            val v = withTimeout(10_000) { client.status().getOrThrow() }
            assertEquals("inbox", v.recvDir) // INBOX_SENTINEL(T6 平台投影)
            assertEquals(true, v.loggedIn)
        } finally {
            client.close()
        }
    }

    @Test
    fun `事件 golden 全量——事件流发射且类型化解析核对`() = runBlocking {
        for ((name, check) in eventChecks) {
            val fake = FakeDaemon()
            val client = IpcClient(fake, CoroutineScope(SupervisorJob()), backoff = { 25L })
            val frames = CopyOnWriteArrayList<EventFrame>()
            // 订阅落位闸:collect 入口同步注册槽位(complete 与订阅间无挂起点),
            // await 之后主协程的续体必然晚于订阅被调度——杜绝「事件先到、
            // 订阅后到」的丢帧竞态(events 无 replay,无订阅者即丢,镜像 Rust)。
            val subscribed = CompletableDeferred<Unit>()
            val collector = launch {
                subscribed.complete(Unit)
                client.events.collect { frames.add(it) }
            }
            try {
                subscribed.await()
                withTimeout(10_000) { client.connect() }
                fake.connections.single().sendLine(Goldens.read(name)) // 原样字节 = 线上事件帧
                withTimeout(10_000) { while (frames.isEmpty()) delay(25) }
                check(frames[0])
            } catch (e: Throwable) {
                throw AssertionError("事件 $name 失败: ${e.message}", e)
            } finally {
                collector.cancel()
                client.close()
            }
        }
    }

    @Test
    fun `全量 golden 文件解析校验且无孤儿遗漏`() {
        val all = Goldens.allNames()
        // 每个文件必须可解析为 JSON(畸形 golden → 红)
        all.forEach { name ->
            val v = JVal.parse(Goldens.read(name).toString(Charsets.UTF_8))
            assertTrue("$name 须为 JSON 对象", v is JVal.Obj)
        }
        // 消费面:客户端形态 req/resp + android 变体 + 事件 + 不移植形态 req/resp
        val consumed = buildSet {
            forms.keys.forEach { add("$it.req.json"); add("$it.resp.json") }
            addAll(androidVariants)
            addAll(eventChecks.keys)
            notPortedForms.forEach { add("$it.req.json"); add("$it.resp.json") }
        }
        val leftover = all.filter { it !in consumed }
        assertTrue(
            "golden 目录出现测试未消费的新文件(孤儿 golden 盲区防护): $leftover",
            leftover.isEmpty(),
        )
        // 反向:声明的消费面文件必须真实存在(防声明与目录脱节)
        consumed.forEach { name ->
            assertTrue("声明的消费面文件缺失: $name", name in all)
        }
    }

    @Test
    fun `规范写出与 serde json 字节口径一致(键序与转义)`() {
        // 键字母序(含 CJK 键按 UTF-8 字节序)+ 转义:仅 " \ 与 <0x20 控制
        // 字符(\b\f\n\r\t 及 \u00xx 小写十六进制);非 ASCII 原样直出。
        // 值 "a" = 引号、反斜杠、换行、0x01 四个字符的转义面。
        // 转义压力输入:引号、反斜杠、换行、0x01(按码点拼装,避免字面量转义歧义)
        val tricky = buildString {
            append('"')
            append('\\')
            append(0x0A.toChar())
            append(0x01.toChar())
        }
        val value = JVal.Obj(
            "b" to JVal.Arr(listOf(JVal.num(1), JVal.Num("2.5"), JVal.Null, JVal.bool(true))),
            "a" to JVal.Str(tricky),
            "中" to JVal.Str("文"),
        )
        val expected =
            "{\"a\":\"\\\"\\\\\\n\\u0001\",\"b\":[1,2.5,null,true],\"中\":\"文\"}"
        assertEquals(expected, value.toCanonicalJson())
    }
}
