package com.mtdss.android.inbox

import com.mtdss.android.fdpass.FakeFdServer
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.FakeDaemon
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.test.TestKit.awaitUntil
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

/**
 * InboxProvider(发布器)决策单测(伪 daemon 事件注入 + 伪 fdpass server +
 * 伪 InboxStore):fs.publish → 发布 → 0x05 应答(ok/error/Skipped 亦 ok)、
 * policy 快照缝(命中透传/缺省 overwrite)、先订阅后动作闸、畸形帧不杀
 * 订阅、0x04 查询同通道照常。
 */
class InboxProviderTest {

    /** 伪 InboxStore:发布调用记录 + 应答面。 */
    private class FakeInboxStore : InboxStore {
        data class PublishCall(val rel: String, val stagingPath: String, val size: Long, val policy: TransferPolicy)

        val publishes = CopyOnWriteArrayList<PublishCall>()

        @Volatile
        var result: PublishResult = PublishResult.Published

        override fun exists(rel: String, size: Long): Boolean = false

        override fun publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult {
            publishes += PublishCall(rel, stagingPath, size, policy)
            return result
        }
    }

    private inner class Env(
        val store: FakeInboxStore = FakeInboxStore(),
        val policyFor: (String) -> TransferPolicy? = { null },
    ) {
        val fake = FakeDaemon()
        val fdServer = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val ipc = IpcClient(fake, scope, backoff = { 25L })
        val channel = FdPassChannel(fdServer, scope, ackTimeoutMs = 2_000L)
        val provider = InboxProvider(ipc, channel, scope, store, policyFor)

        /** 已回 ack 的 request_id 面(服务端受理记录)。 */
        val acked = CopyOnWriteArrayList<Long>()

        fun frames() = fdServer.connections.firstOrNull()?.frames ?: emptyList()

        suspend fun start() {
            runBlocking { withTimeout(5_000) { provider.awaitSubscribed() } }
            ipc.connect().getOrThrow()
            // 服务端受理面:0x04/0x05 到达即回 ack ok:true(镜像 daemon FdBridgeHandler)
            fdServer.onFrame = { conn, f ->
                val id = f.payload.opt("request_id")!!.asLong()
                acked += id
                conn.send(
                    FdWire.KIND_ACK,
                    JVal.Obj("ok" to JVal.bool(true), "request_id" to JVal.num(id)),
                )
            }
        }

        fun emitPublish(requestId: Long, rel: String = "small.txt", staging: String = "/staging/small.txt", size: Long = 2025) {
            fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"fs.publish",""" +
                    """"params":{"request_id":$requestId,"rel":"$rel","staging_path":"$staging","size":$size}}""",
            )
        }

        fun emitMalformedPublish(requestId: Long) {
            // 缺 staging_path/size:解析容错丢弃,不杀订阅协程
            fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"fs.publish","params":{"request_id":$requestId,"rel":"x.bin"}}""",
            )
        }

        fun emitQueryDest(requestId: Long, rel: String = "small.txt", size: Long = 2025) {
            fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"fs.query_dest",""" +
                    """"params":{"request_id":$requestId,"rel":"$rel","size":$size}}""",
            )
        }

        fun close() {
            channel.close()
            ipc.close()
            scope.cancel()
        }
    }

    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    @Test
    fun `fs publish 成功发布回 0x05 ok 且参数透传`() = runTest {
        val e = Env()
        try {
            e.start()
            e.emitPublish(1, rel = "docs/甲.bin", staging = "/staging/docs/甲.bin", size = 4096)
            awaitUntil { e.frames().size == 1 }

            val frame = e.frames().single()
            assertEquals(FdWire.KIND_PUBLISH_RESULT, frame.kind)
            assertEquals("""{"ok":true,"request_id":1}""", frame.payload.toCanonicalJson())
            assertTrue("0x05 无 fd", frame.fds.isEmpty())
            // 发布调用:rel/staging/size 原样;policy = 缺省 overwrite(快照无匹配)
            val call = e.store.publishes.single()
            assertEquals("docs/甲.bin", call.rel)
            assertEquals("/staging/docs/甲.bin", call.stagingPath)
            assertEquals(4096L, call.size)
            assertEquals(TransferPolicy(), call.policy)
            awaitUntil { 1L in e.acked }
        } finally {
            e.close()
        }
    }

    @Test
    fun `发布失败回 0x05 error 带因`() = runTest {
        val e = Env()
        try {
            e.start()
            e.store.result = PublishResult.Failed("MediaStore 插入失败: small.txt (insert clash)")
            e.emitPublish(2)
            awaitUntil { e.frames().size == 1 }

            val frame = e.frames().single()
            assertEquals(
                """{"error":"MediaStore 插入失败: small.txt (insert clash)","ok":false,"request_id":2}""",
                frame.payload.toCanonicalJson(),
            )
        } finally {
            e.close()
        }
    }

    @Test
    fun `Skipped 亦回 ok true`() = runTest {
        // skip 弃件 = 文件已在场,daemon 收尾(删暂存+打标)照常 → ok:true
        val e = Env()
        try {
            e.start()
            e.store.result = PublishResult.Skipped
            e.emitPublish(3)
            awaitUntil { e.frames().size == 1 }
            assertEquals("""{"ok":true,"request_id":3}""", e.frames().single().payload.toCanonicalJson())
        } finally {
            e.close()
        }
    }

    @Test
    fun `policy 快照命中透传 未命中缺省 overwrite`() = runTest {
        val e = Env(policyFor = { rel ->
            if (rel == "已选.bin") TransferPolicy(overwrite = listOf("已选.bin")) else null
        })
        try {
            e.start()
            e.emitPublish(4, rel = "已选.bin")
            e.emitPublish(5, rel = "陌生.bin")
            awaitUntil { e.store.publishes.size == 2 }

            // 逐发布一协程,落定序不定——按 rel 对齐断言(序无关)
            val byRel = e.store.publishes.associate { it.rel to it.policy }
            assertEquals(TransferPolicy(overwrite = listOf("已选.bin")), byRel["已选.bin"])
            assertEquals("快照无匹配 → 缺省 overwrite(类注释钉)", TransferPolicy(), byRel["陌生.bin"])
        } finally {
            e.close()
        }
    }

    @Test
    fun `先订阅后动作 连接后事件不丢`() = runTest {
        // 订阅闸:awaitSubscribed 先落位,connect 后立刻到达的 fs.publish
        // 必被处理(构造即订阅,SharedFlow no-replay 亦无丢帧窗)
        val e = Env()
        try {
            runBlocking { withTimeout(5_000) { e.provider.awaitSubscribed() } }
            e.ipc.connect().getOrThrow()
            e.emitPublish(6)
            awaitUntil { e.frames().size == 1 }
            assertEquals("""{"ok":true,"request_id":6}""", e.frames().single().payload.toCanonicalJson())
        } finally {
            e.close()
        }
    }

    @Test
    fun `畸形帧丢弃不杀订阅 后续发布照常`() = runTest {
        val e = Env()
        try {
            e.start()
            e.emitMalformedPublish(7)
            e.emitPublish(8)
            awaitUntil { e.frames().size == 1 }
            assertEquals(1, e.frames().size)
            assertEquals("""{"ok":true,"request_id":8}""", e.frames().single().payload.toCanonicalJson())
        } finally {
            e.close()
        }
    }

    @Test
    fun `fs query_dest 同通道照常应答 0x04`() = runTest {
        val e = Env()
        try {
            e.start()
            e.emitQueryDest(9)
            awaitUntil { e.frames().size == 1 }
            val frame = e.frames().single()
            assertEquals(FdWire.KIND_DEST_QUERY, frame.kind)
            // store.exists 恒 false → exists:false,size 恒 0
            assertEquals("""{"exists":false,"request_id":9,"size":0}""", frame.payload.toCanonicalJson())
        } finally {
            e.close()
        }
    }
}
