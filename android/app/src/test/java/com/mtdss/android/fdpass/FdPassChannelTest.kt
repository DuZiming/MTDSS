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
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.onSubscription
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
 * 见 [FakeFdServer]):0x02 交付/拒开、**ack 与 0x03 交错容忍**(T6 关键
 * 语义:0x03 经同一连接事后推送,读循环不假设应答序)、ack 超时语义分流、
 * 断连/协议异常收口、重连。
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

    /** 订阅落位闸(commits 流先订阅再触发;镜像 IpcClientTest 的闸模式)。 */
    private fun subscribeCommits(
        channel: FdPassChannel,
        into: MutableList<FdCommit>,
        scope: CoroutineScope,
    ): CompletableDeferred<Unit> {
        val gate = CompletableDeferred<Unit>()
        scope.launch {
            channel.commits.onSubscription { gate.complete(Unit) }.collect { into.add(it) }
        }
        return gate
    }

    @Test
    fun `0x02 交付收 ack 成功`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        val fd = FileDescriptor()
        try {
            server.onFrame = { conn, f ->
                // 服务端受理:0x02 + 1 fd → 回 ack ok:true
                assertEquals(FdWire.KIND_DEST_FD, f.kind)
                assertEquals(1, f.fds.size)
                conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
            }
            val ack = channel.sendDestFd(7, fd).getOrThrow()
            assertEquals(FdAck(7, true, null), ack)
            // 服务端帧面:kind/payload/fd 齐核对
            val frame = server.connections[0].frames.single()
            assertEquals(FdWire.KIND_DEST_FD, frame.kind)
            assertEquals("""{"request_id":7}""", frame.payload.toCanonicalJson())
            assertEquals(listOf(fd), frame.fds)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `ack 之前交错 0x03 容忍且 commit 归流`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        val commits = mutableListOf<FdCommit>()
        try {
            val gate = subscribeCommits(channel, commits, scope)
            withTimeout(10_000) { gate.await() }
            server.onFrame = { conn, f ->
                // 先推 0x03(早前请求的落定),再回本请求 ack——同连接交错
                conn.send(
                    FdWire.KIND_DEST_COMMIT,
                    JVal.Obj("done" to JVal.bool(true), "request_id" to JVal.num(5)),
                )
                conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
            }
            val ack = withTimeout(5_000) { channel.sendDestFd(9, FileDescriptor()).getOrThrow() }
            assertEquals(FdAck(9, true, null), ack)
            // 交错的 0x03 不丢:进 commits 流,request_id 与 ack 无关
            withTimeout(5_000) { while (commits.size < 1) kotlinx.coroutines.delay(10) }
            assertEquals(listOf(FdCommit(5, true)), commits)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `拒开应答无 fd 且 payload 带 error`() = runTest {
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val channel = newChannel(server, scope = scope)
        try {
            server.onFrame = { conn, f ->
                assertTrue("拒开帧不得带 fd", f.fds.isEmpty())
                conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
            }
            val ack = channel.sendDestFd(3, null, "NOT_FOUND: 暂存已失").getOrThrow()
            assertEquals(FdAck(3, true, null), ack)
            val frame = server.connections[0].frames.single()
            assertEquals(
                """{"error":"NOT_FOUND: 暂存已失","ok":false,"request_id":3}""",
                frame.payload.toCanonicalJson(),
            )
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
                        "error" to JVal.str("目的地请求已结束(超时或已交付)"),
                        "ok" to JVal.bool(false),
                        "request_id" to JVal.num(4),
                    ),
                )
            }
            val r = channel.sendDestFd(4, FileDescriptor())
            assertTrue("ack 本体(含 ok:false)不是传输失败", r.isSuccess)
            assertEquals(FdAck(4, false, "目的地请求已结束(超时或已交付)"), r.getOrThrow())
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
            val r = withTimeout(5_000) { channel.sendDestFd(21, FileDescriptor()) }
            assertTrue(r.isFailure)
            assertTrue("超时应近 ackTimeoutMs,实际 ${System.currentTimeMillis() - t0}ms",
                System.currentTimeMillis() - t0 in 90..3_000)

            // 迟到 ack:请求已超时摘除 → 忽略(不断连、不误关联)
            server.connections[0].send(
                FdWire.KIND_ACK,
                JVal.Obj("ok" to JVal.bool(true), "request_id" to JVal.num(21)),
            )
            kotlinx.coroutines.delay(150) // 读循环消化迟到帧

            // 通道仍可用:下一请求照常完成
            server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true)) }
            val ack = withTimeout(5_000) { channel.sendDestFd(22, FileDescriptor()).getOrThrow() }
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
            channel.sendDestFd(1, FileDescriptor()).getOrThrow()

            // 请求 2 悬挂等 ack(服务端不答)→ 断连:在飞应答等待快速失败
            server.onFrame = { _, _ -> }
            val inFlight = async { channel.sendDestFd(2, FileDescriptor()) }
            kotlinx.coroutines.delay(100) // 请求 2 已上连接
            server.connections[0].close()
            val e = withTimeout(5_000) { inFlight.await().exceptionOrNull() }
            assertTrue("断连路径应失败:${e?.message}", e is IOException || e is FdWireException)

            // 第二条连接:照常受理(按需重连,无需自愈循环)
            server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true)) }
            val ack = withTimeout(5_000) { channel.sendDestFd(3, FileDescriptor()).getOrThrow() }
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
            val inFlight = async { channel.sendDestFd(6, FileDescriptor()) }
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
        channel.sendDestFd(1, FileDescriptor()).getOrThrow()
        channel.close()

        val r = withTimeout(5_000) { channel.sendDestFd(2, FileDescriptor()) }
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
            val r = channel.sendDestFd(1, FileDescriptor())
            assertTrue(r.isFailure)
            assertTrue(r.exceptionOrNull() is IOException)
        } finally {
            channel.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    @Test
    fun `0x02 与 0x04 独立通道同 id 并发不串扰`() = runTest {
        // F7 复审 Important A:daemon 侧 DestResolver(0x02)与 DestQueries
        // (0x04)的 request_id 空间独立——撞号(同为 7)时必须靠「各用一条
        // 通道实例」隔离。单通道形态下 pendingAcks 同键覆盖写,后登记者
        // 吃掉先登记者的应答(或一方只能超时)。伪 server 按连接回异号
        // ack:0x02 受理 ok:true,0x04 受理 ok:false——两通道并发各自拿到
        // 自己连接的应答,即实例纪律成立的钉子。
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob())
        val dest = newChannel(server, scope = scope)
        val query = newChannel(server, scope = scope)
        val fd = FileDescriptor()
        try {
            server.onFrame = { conn, f ->
                when (f.kind) {
                    FdWire.KIND_DEST_FD -> conn.send(FdWire.KIND_ACK, ackPayload(f, ok = true))
                    FdWire.KIND_DEST_QUERY -> conn.send(
                        FdWire.KIND_ACK,
                        JVal.Obj(
                            "error" to JVal.str("查询请求已结束(超时或已应答)"),
                            "ok" to JVal.bool(false),
                            "request_id" to JVal.num(7),
                        ),
                    )
                    else -> error("非预期 kind: ${f.kind}")
                }
            }
            val destAck = async { dest.sendDestFd(7, fd).getOrThrow() }
            val queryAck = async { query.sendDestQuery(7, exists = true, size = 42).getOrThrow() }
            assertEquals(FdAck(7, true, null), destAck.await())
            assertEquals(FdAck(7, false, "查询请求已结束(超时或已应答)"), queryAck.await())
            // 各连接各收恰一帧,kind 与本侧一致(0x02 带 fd,0x04 无 fd)
            val all = server.connections.flatMap { it.frames }
            assertEquals(2, all.size)
            val qf = all.single { it.kind == FdWire.KIND_DEST_QUERY }
            assertTrue(qf.fds.isEmpty())
            assertEquals(1, all.single { it.kind == FdWire.KIND_DEST_FD }.fds.size)
        } finally {
            dest.close()
            query.close()
            scope.coroutineContext.cancelChildren()
        }
    }

    private fun ackPayload(frame: ReceivedFrame, ok: Boolean): JVal.Obj =
        JVal.Obj(
            "ok" to JVal.bool(ok),
            "request_id" to JVal.num((frame.payload.opt("request_id") ?: JVal.Null).asLong()),
        )
}
