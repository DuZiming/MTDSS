package com.mtdss.android.ipc

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.system.measureTimeMillis

/**
 * IpcClient 状态机 JVM 单测(伪 server = 内存管道,见 [FakeDaemon]):
 * id 路由并发安全、错误帧 → RpcException、事件 → SharedFlow、断连自愈
 * (在飞快速失败 + 重连 + 补拉 transfer.list + 事件续流)、超限帧拒绝、
 * 连接失败快速返回、close 后不再复活。字节级 golden 对齐见 ContractGoldenTest。
 */
class IpcClientTest {

    /** 测试口径退避:25ms/次(无热自旋,重连数百 ms 内可见)。 */
    private val fastBackoff: (Int) -> Long = { 25L }

    private fun newClient(fake: FakeDaemon): IpcClient =
        IpcClient(fake, CoroutineScope(SupervisorJob()), fastBackoff)

    /** 条件轮询直至成立(默认 5s 上界,届满抛 IllegalStateException)。 */
    private suspend fun awaitUntil(timeoutMs: Long = 5_000, cond: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (!cond()) {
            check(System.currentTimeMillis() < deadline) { "等待条件超时(${timeoutMs}ms)" }
            delay(25)
        }
    }

    /** 伪 server 应答脚本助手:应答 golden 帧(id 重写为请求 id)。 */
    private fun replyWithGolden(name: String): (FakeDaemon.ServerSide, ByteArray) -> Unit =
        { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(Goldens.withId(Goldens.read(name), id))
        }

    @Test
    fun `hello 请求帧与 golden 逐字节一致`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = replyWithGolden("hello.resp.json")
        val client = newClient(fake)
        try {
            val hello = client.hello().getOrThrow()
            assertEquals("1.0.0", hello.version)
            assertTrue(hello.loggedIn)
            assertEquals("10ba682c8ad13513971e8b56881aab8b", hello.device?.deviceId)
            // golden 文件内容即线上帧(含尾换行);客户端首调 id=1 与 golden 同
            assertEquals(1, fake.connections.single().received.size)
            assertArrayEquals(Goldens.read("hello.req.json"), fake.connections.single().received[0])
        } finally {
            client.close()
        }
    }

    @Test
    fun `并发调用按 id 路由互不串扰`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            // 睡 20ms 让 32 个请求全部入队在飞,应答集中回 —— 最大化路由竞态窗口
            Thread.sleep(20)
            val req = JVal.parse(String(line, Charsets.UTF_8))
            val id = req.opt("id")!!.asLong()
            val marker = req.opt("params")!!.opt("marker")!!.asString()
            conn.sendLine("""{"id":$id,"result":{"marker":"$marker","n":$id}}""")
        }
        val client = newClient(fake)
        try {
            val n = 32
            val results = (1..n).map { k ->
                async {
                    client.call(Op("echo"), JVal.Obj("marker" to JVal.str("m$k"))) { v ->
                        v.opt("marker")!!.asString() to v.opt("n")!!.asLong()
                    }.getOrThrow()
                }
            }.map { it.await() }
            // 各调用收到自己的 marker 回显;id 恰好覆盖 1..n(无重复无丢失)
            (1..n).forEach { k -> assertEquals("m$k", results[k - 1].first) }
            assertEquals((1L..n.toLong()).toSet(), results.map { it.second }.toSet())
        } finally {
            client.close()
        }
    }

    @Test
    fun `错误应答转 RpcException 且 kind 取自 data kind`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            conn.sendLine(
                """{"id":$id,"error":{"code":7,"message":"没找到","data":{"kind":"NOT_FOUND"}}}"""
            )
        }
        val client = newClient(fake)
        try {
            val e1 = client.call(Op("op1")).exceptionOrNull() as RpcException
            assertEquals(7L, e1.code)
            assertEquals("没找到", e1.message)
            assertEquals("NOT_FOUND", e1.kind)

            fake.onLine = { conn, line ->
                val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                conn.sendLine("""{"id":$id,"error":{"code":3,"message":"设备离线: x"}}""")
            }
            val e2 = client.call(Op("op2")).exceptionOrNull() as RpcException
            assertEquals(3L, e2.code)
            assertEquals("设备离线: x", e2.message)
            assertNull(e2.kind)
        } finally {
            client.close()
        }
    }

    @Test
    fun `事件帧进事件流且与响应路由互不干扰`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
            // 先塞两个事件帧再回应答:验证读循环按帧分拣
            conn.sendLine(Goldens.read("event.login.json"))
            conn.sendLine(Goldens.read("event.notify.json"))
            conn.sendLine("""{"id":$id,"result":{"ok":true}}""")
        }
        val client = newClient(fake)
        val frames = CopyOnWriteArrayList<EventFrame>()
        // 订阅落位闸(见 ContractGoldenTest 同款注释):应答携带的事件帧
        // 会在请求写出的瞬间回流,不能依赖 call 的挂起点替订阅让路。
        val subscribed = CompletableDeferred<Unit>()
        val collector = launch {
            subscribed.complete(Unit)
            client.events.collect { frames.add(it) }
        }
        try {
            subscribed.await()
            val r = client.call(Op("x")).getOrThrow()
            assertTrue(r.opt("ok")!!.asBoolean())
            awaitUntil { frames.size == 2 }
            assertEquals("event.login", frames[0].method)
            assertTrue(frames[0].params.opt("logged_in")!!.asBoolean())
            assertEquals("event.notify", frames[1].method)
            assertEquals("MTDSS", frames[1].params.opt("title")!!.asString())
        } finally {
            collector.cancel()
            client.close()
        }
    }

    @Test
    fun `断连在飞快速失败 自愈重连并补拉 transfer list 事件续流`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, line ->
            when (JVal.parse(String(line, Charsets.UTF_8)).opt("method")!!.asString()) {
                "hello" -> conn.sendLine(Goldens.withId(Goldens.read("hello.resp.json"), 1))
                "transfer.wait" -> Unit // 悬挂不答 → 断连时它在飞
                "transfer.list" -> {
                    val id = JVal.parse(String(line, Charsets.UTF_8)).opt("id")!!.asLong()
                    conn.sendLine(Goldens.withId(Goldens.read("transfer.list.resp.json"), id))
                }
                else -> error("意外的请求")
            }
        }
        val client = newClient(fake)
        val refreshes = CopyOnWriteArrayList<Result<TransferListResult>>()
        val refreshCollector = launch { client.transferListRefresh.collect { refreshes.add(it) } }
        val events = CopyOnWriteArrayList<EventFrame>()
        val eventCollector = launch { client.events.collect { events.add(it) } }
        try {
            assertEquals("1.0.0", client.hello().getOrThrow().version)
            // 首连(非重连)不补拉
            assertTrue(refreshes.isEmpty())

            val inFlight = async { client.transferWait("3") }
            awaitUntil { fake.connections[0].received.size >= 2 } // hello + wait 已上管道
            fake.connections[0].close() // 服务端断连(慢消费者同象:读侧 EOF)

            val e = withTimeout(5_000) { inFlight.await().exceptionOrNull() as RpcException }
            assertEquals(IpcClient.CODE_TRANSPORT, e.code)

            awaitUntil { fake.connections.size == 2 } // 自愈重连
            awaitUntil { refreshes.isNotEmpty() } // 重连后补拉 transfer.list
            assertEquals(3, refreshes[0].getOrThrow().jobs.size)

            fake.connections[1].sendLine(Goldens.read("event.login.json")) // 事件续流
            awaitUntil { events.isNotEmpty() }
            assertEquals("event.login", events[0].method)
        } finally {
            refreshCollector.cancel()
            eventCollector.cancel()
            client.close()
        }
    }

    @Test
    fun `超限帧拒绝断连且在飞调用失败`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = { conn, _ ->
            // 8 MiB + 1 字节无换行:超单帧上限 → 客户端须断连(镜像 daemon 口径)
            conn.sendLine("x".repeat(8 * 1024 * 1024 + 1))
        }
        val client = newClient(fake)
        try {
            val inFlight = async { client.call(Op("op")) }
            val e = withTimeout(10_000) { inFlight.await().exceptionOrNull() as RpcException }
            assertEquals(IpcClient.CODE_TRANSPORT, e.code)
            awaitUntil { fake.connections.size == 2 } // 断连后自愈另起连接
        } finally {
            client.close()
        }
    }

    @Test
    fun `connect 失败快速返回 恢复后自愈接通`() = runBlocking {
        val fake = FakeDaemon()
        fake.connectFailure = IOException("daemon 未就绪")
        val client = newClient(fake)
        try {
            val elapsed = measureTimeMillis {
                val r = client.connect()
                assertTrue(r.isFailure)
                assertTrue(r.exceptionOrNull() is IpcConnectException)
            }
            assertTrue("connect 须快速失败,实际 ${elapsed}ms", elapsed < 2_000)

            fake.connectFailure = null // daemon 起来了
            awaitUntil(10_000) { client.connectionState.value == ConnState.Connected }
            assertEquals(1, fake.connections.size)

            fake.onLine = replyWithGolden("hello.resp.json")
            assertEquals("1.0.0", client.hello().getOrThrow().version)
        } finally {
            client.close()
        }
    }

    @Test
    fun `close 后调用失败且不再重连`() = runBlocking {
        val fake = FakeDaemon()
        fake.onLine = replyWithGolden("hello.resp.json")
        val client = newClient(fake)
        assertEquals("1.0.0", client.hello().getOrThrow().version)
        client.close()

        val r = client.call(Op("x"))
        assertTrue(r.isFailure)
        assertTrue(r.exceptionOrNull() is IpcConnectException)
        val after = fake.connections.size
        delay(300) // 若 close 后仍在自愈,这里会新增连接
        assertEquals(after, fake.connections.size)
        assertEquals(ConnState.Disconnected, client.connectionState.value)
    }
}
