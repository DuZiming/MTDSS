package com.mtdss.android.fdpass

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.FileDescriptor
import java.io.IOException

/**
 * FdPassChannel 状态机 JVM 单测(伪 server = 内存管道 + 偏移挂载 fd 模拟,
 * 见 [FakeFdServer]):0x05 发布应答、0x04 在场查询(T10 的 0x01 源交付由
 * SourceProviderTest 覆盖)、ack 超时语义、断连/协议异常收口、重连。
 *
 * P4a 通道纪律改二:0x02 dest_fd/0x03 dest_commit 已随收件 fd 桥退役;
 * 0x04 与 0x05 共用**同一通道实例**(daemon 侧 DestQueries 单 resolver 单
 * 计数器,spec §6.5——App 侧单通道单表,同号撞不出错路)。
 */
class FdPassChannelTest {

    /** 限时测试体:30s 上界防 RED 桩/协议回归把测试 JVM 挂死(协程级取消,runBlocking 可正常收尾)。 */
    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    private fun newChannel(
        server: FakeFdServer,
        ackTimeoutMs: Long = 5_000L,
        scope: CoroutineScope = CoroutineScope(SupervisorJob()),
    ): FdPassChannel = FdPassChannel(server, scope, ackTimeoutMs)

    @Test
    fun `0x05 发布应答无 fd 收 ack 成功`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            server.onFrame = { conn, f ->
                assertEquals(FdWire.KIND_PUBLISH_RESULT, f.kind)
                conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
            }
            val ack = channel.sendPublishResult(7, ok = true).getOrThrow()
            assertEquals(FdAck(7, true, null), ack)
            // 服务端帧面:kind/payload 核对;0x05 契约无 fd
            val frame = server.connections[0].frames.single()
            assertEquals(FdWire.KIND_PUBLISH_RESULT, frame.kind)
            assertEquals("""{"ok":true,"request_id":7}""", frame.payload.toCanonicalJson())
            assertTrue("发布应答帧不得带 fd", frame.fds.isEmpty())
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `0x05 失败形态 payload 带 error 且无 fd`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            server.onFrame = { conn, f ->
                assertTrue("失败应答帧不得带 fd", f.fds.isEmpty())
                conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
            }
            val ack = channel.sendPublishResult(3, ok = false, error = "发布拷贝失败: 磁盘满").getOrThrow()
            assertEquals(FdAck(3, true, null), ack)
            val frame = server.connections[0].frames.single()
            assertEquals(
                """{"error":"发布拷贝失败: 磁盘满","ok":false,"request_id":3}""",
                frame.payload.toCanonicalJson(),
            )
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `0x04 与 0x05 同通道同连接不串扰`() = runTest {
        // spec §6.5:daemon 侧 DestQueries 单 resolver 单计数器(0x04/0x05 的
        // request_id 全序单调)——App 侧单通道单表,两类应答在同一条连接上
        // 各自按 id 落答,无 P3 时代「ack 不携 kind 撞号串扰」面(那是 0x02
        // 独立计数器时代的 Instance 纪律;0x02 已退役)。
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            server.onFrame = { conn, f ->
                when (f.kind) {
                    FdWire.KIND_DEST_QUERY -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
                    FdWire.KIND_PUBLISH_RESULT -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
                    else -> error("非预期 kind: ${f.kind}")
                }
            }
            val q = async { channel.sendDestQuery(1, exists = true, size = 64).getOrThrow() }
            val p = async { channel.sendPublishResult(2, ok = false, error = "x").getOrThrow() }
            assertEquals(FdAck(1, true, null), q.await())
            assertEquals(FdAck(2, true, null), p.await())
            // 恰一条连接承载两类帧(单通道;连接按需建立后复用);两请求并发
            // 上帧序不定,按集合断言
            assertEquals(1, server.connections.size)
            val kinds = server.connections[0].frames.map { it.kind }.toSet()
            assertEquals(setOf(FdWire.KIND_DEST_QUERY, FdWire.KIND_PUBLISH_RESULT), kinds)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `ack ok false 走 success 语义分流`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            server.onFrame = { conn, f ->
                conn.send(
                    FdWire.KIND_ACK,
                    JVal.Obj(
                        "error" to JVal.str("发布请求已结束(超时或已应答)"),
                        "ok" to JVal.bool(false),
                        "request_id" to JVal.num(4),
                    ),
                )
            }
            val r = channel.sendPublishResult(4, ok = true)
            assertTrue("ack 本体(含 ok:false)不是传输失败", r.isSuccess)
            assertEquals(FdAck(4, false, "发布请求已结束(超时或已应答)"), r.getOrThrow())
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `ack 超时失败且迟到 ack 被忽略`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, ackTimeoutMs = 100L, scope = scope)
        try {
            server.onFrame = { _, _ -> /* 不应答:超时路径 */ }
            val t0 = System.currentTimeMillis()
            val r = withTimeout(5_000) { channel.sendPublishResult(21, ok = true) }
            assertTrue(r.isFailure)
            assertTrue(
                "超时应近 ackTimeoutMs,实际 ${System.currentTimeMillis() - t0}ms",
                System.currentTimeMillis() - t0 in 90..3_000,
            )

            // 迟到 ack:请求已超时摘除 → 忽略(不断连、不误关联)
            server.connections[0].send(
                FdWire.KIND_ACK,
                JVal.Obj("ok" to JVal.bool(true), "request_id" to JVal.num(21)),
            )
            kotlinx.coroutines.delay(150) // 读循环消化迟到帧

            // 通道仍可用:下一请求照常完成
            server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true)) }
            val ack = withTimeout(5_000) { channel.sendPublishResult(22, ok = true).getOrThrow() }
            assertEquals(FdAck(22, true, null), ack)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `服务端断连后在飞失败且下次发送重连`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            // 第一条连接:请求 1 正常受理
            server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true)) }
            channel.sendPublishResult(1, ok = true).getOrThrow()

            // 请求 2 悬挂等 ack(服务端不答)→ 断连:在飞应答等待快速失败
            server.onFrame = { _, _ -> }
            val inFlight = async { channel.sendPublishResult(2, ok = true) }
            kotlinx.coroutines.delay(100) // 请求 2 已上连接
            server.connections[0].close()
            val e = withTimeout(5_000) { inFlight.await().exceptionOrNull() }
            assertTrue("断连路径应失败:${e?.message}", e is IOException || e is FdWireException)

            // 第二条连接:照常受理(按需重连,无需自愈循环)
            server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true)) }
            val ack = withTimeout(5_000) { channel.sendPublishResult(3, ok = true).getOrThrow() }
            assertEquals(FdAck(3, true, null), ack)
            assertEquals(2, server.connections.size)
            assertEquals(2, server.connections[0].frames.size) // 请求 1 + 请求 2(死连接上)
            assertEquals(1, server.connections[1].frames.size) // 请求 3(重连后)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `协议异常帧随附超量 fd 断连且 fd 被关闭`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        val extra1 = FileDescriptor()
        val extra2 = FileDescriptor()
        try {
            val inFlight = async { channel.sendPublishResult(6, ok = true) }
            // 服务端推一帧带 2 个 fd 的 ack:协议允许 0/1,超量即协议异常
            kotlinx.coroutines.delay(50) // 让在飞请求先建连
            server.connections[0].send(
                FdWire.KIND_ACK,
                JVal.Obj("ok" to JVal.bool(true), "request_id" to JVal.num(6)),
                fds = listOf(extra1, extra2),
            )
            val e = withTimeout(5_000) { inFlight.await().exceptionOrNull() }
            assertTrue(e is IOException || e is FdWireException)
            // 防御收口:超量 fd 必须被关闭(镜像 fdpass.rs recv_some 的 drop 口径)
            assertEquals(listOf(extra1, extra2), server.connections[0].closedInbound)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `close 后发送失败且不再建连`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true)) }
        channel.sendPublishResult(1, ok = true).getOrThrow()
        channel.close()

        val r = withTimeout(5_000) { channel.sendPublishResult(2, ok = true) }
        assertTrue(r.isFailure)
        assertNull("close 后不得新建连接", server.connections.getOrNull(1))
        scope.coroutineContext.cancelChildren()
    }

    @Test
    fun `连接失败即失败`() = runTest {
        val server = FakeFdServer()
        server.connectFailure = IOException("fds.sock 不可达")
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            val r = channel.sendPublishResult(1, ok = true)
            assertTrue(r.isFailure)
            assertTrue(r.exceptionOrNull() is IOException)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    private fun ackPayload(frame: ReceivedFrame, ok: Boolean): JVal.Obj =
        JVal.Obj(
            "ok" to JVal.bool(ok),
            "request_id" to JVal.num((frame.payload.opt("request_id") ?: JVal.Null).asLong()),
        )
}
