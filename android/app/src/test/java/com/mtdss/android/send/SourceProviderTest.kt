package com.mtdss.android.send

import com.mtdss.android.fdpass.FakeFdServer
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.FakeDaemon
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.test.TestKit
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
 * SourceProvider 决策单测(伪 daemon 事件注入 + 伪 fdpass server + 伪
 * SAF 面):rootRef/单文档 ref 应答 0x01+fd、打开失败/授权根移除/畸形
 * 引用拒开(0x01 无 fd + ok:false + error)、畸形事件帧不杀订阅。
 */
class SourceProviderTest {

    private val defaultRoots = listOf(StoredRoot("r1", "content://tree/1", "primary:DCIM", "DCIM"))

    /** DownloadsProvider 形授权根:纯数字 docId(MIUI 把 Download 目录挂它,设备场 F8)。 */
    private val downloadsRoots = listOf(StoredRoot("r2", "content://tree/dl", "12", "Download"))

    private inner class Env(
        val roots: List<StoredRoot> = defaultRoots,
        val fake: FakeDaemon = FakeDaemon(),
        val fdServer: FakeFdServer = FakeFdServer(),
        val saf: FakeSafFace = FakeSafFace(),
    ) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val ipc = IpcClient(fake, scope, backoff = { 25L })
        val channel = FdPassChannel(fdServer, scope, ackTimeoutMs = 2_000L)
        val provider = SourceProvider({ roots }, ipc, channel, saf, scope)

        /** 已回 ack 的 request_id 面(服务端受理记录)。 */
        val acked = CopyOnWriteArrayList<Long>()

        /** 首条 fdpass 连接的已收帧(连接未建时空表)。 */
        fun frames(): List<com.mtdss.android.fdpass.ReceivedFrame> =
            fdServer.connections.firstOrNull()?.frames ?: emptyList()

        suspend fun awaitFrames() = awaitUntil { frames().isNotEmpty() }

        suspend fun start() {
            runBlocking { withTimeout(5_000) { provider.awaitSubscribed() } }
            ipc.connect().getOrThrow()
            // 服务端受理面:0x01 到达即回 ack ok:true(镜像 daemon FdBridgeHandler)
            fdServer.onFrame = { conn, f ->
                if (f.kind == FdWire.KIND_SOURCE_FD) {
                    acked += f.payload.opt("request_id")!!.asLong()
                    conn.send(
                        FdWire.KIND_ACK,
                        JVal.Obj(
                            "ok" to JVal.bool(true),
                            "request_id" to JVal.num(f.payload.opt("request_id")!!.asLong()),
                        ),
                    )
                }
            }
        }

        fun emitRequest(requestId: Long, ref: String?) {
            val refJson = if (ref == null) "" else ",\"ref\":\"$ref\""
            fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"fs.request_source","params":{"request_id":$requestId$refJson}}""",
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
    fun `rootRef 重解析并应答 0x01 加 fd`() = runTest {
        val e = Env()
        try {
            e.start()
            e.emitRequest(7, "roottree://r1/cam/x.jpg")
            e.awaitFrames()

            val frame = e.frames().single()
            assertEquals(FdWire.KIND_SOURCE_FD, frame.kind)
            assertEquals("""{"request_id":7}""", frame.payload.toCanonicalJson())
            assertEquals("交付帧恰 1 个 fd", 1, frame.fds.size)
            // 重解析面:授权根查表 → 子文档 id 累积编码 → URI 重建 → 打开
            assertEquals(
                listOf("content://tree/1" to "primary:DCIM/cam/x.jpg"),
                e.saf.childUris,
            )
            assertEquals(listOf("fake://child/content://tree/1/primary:DCIM/cam/x.jpg"), e.saf.opened)
            assertTrue("路径形 provider 快路径命中,不得触发子文档查询", e.saf.childQueries.isEmpty())
            awaitUntil { 7L in e.acked }
        } finally {
            e.close()
        }
    }

    @Test
    fun `单文档 ref 直开`() = runTest {
        val e = Env()
        try {
            e.start()
            e.emitRequest(8, "content://picked/1")
            e.awaitFrames()
            val frame = e.frames().single()
            assertEquals(FdWire.KIND_SOURCE_FD, frame.kind)
            assertEquals(1, frame.fds.size)
            assertEquals(listOf("content://picked/1"), e.saf.opened)
            assertTrue("树 URI 重建不应发生", e.saf.childUris.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `打开失败拒开`() = runTest {
        val e = Env()
        try {
            e.saf.openErrors["fake://child/content://tree/1/primary:DCIM/x.jpg"] =
                SourceOpenException("授权被撤或不可读(模拟 revoke)")
            e.start()
            e.emitRequest(9, "roottree://r1/x.jpg")
            e.awaitFrames()
            val frame = e.frames().single()
            assertEquals(FdWire.KIND_SOURCE_FD, frame.kind)
            assertTrue("拒开帧不得带 fd", frame.fds.isEmpty())
            assertEquals(
                """{"error":"授权被撤或不可读(模拟 revoke)","ok":false,"request_id":9}""",
                frame.payload.toCanonicalJson(),
            )
            awaitUntil { 9L in e.acked }
        } finally {
            e.close()
        }
    }

    // ── 两段式子文档解析(设备场 F8:DownloadsProvider 数字 docId)────────────

    @Test
    fun `数字 docId 根快开炸回退逐段下钻命中`() = runTest {
        val e = Env(roots = downloadsRoots)
        try {
            // 快路径路径形拼接在数字 docId provider 上必炸(镜像设备日志词面)
            e.saf.openErrors["fake://child/content://tree/dl/12/docs/MTDSS/small.txt"] =
                SourceOpenException("""文档不可开(For input string: "12/docs/MTDSS/small.txt")""")
            e.saf.givenChildren("12", TreeChild("100", "docs", isDir = true, size = 0, mtimeMs = 0))
            e.saf.givenChildren("100", TreeChild("13", "MTDSS", isDir = true, size = 0, mtimeMs = 0))
            e.saf.givenChildren("13", TreeChild("14", "small.txt", isDir = false, size = 11, mtimeMs = 22))
            e.start()
            e.emitRequest(21, "roottree://r2/docs/MTDSS/small.txt")
            e.awaitFrames()

            val frame = e.frames().single()
            assertEquals(FdWire.KIND_SOURCE_FD, frame.kind)
            assertEquals("回退命中后应答带 fd", 1, frame.fds.size)
            // 两次开、顺序执行:快路径(路径形拼接)→ 回退(下钻末段真实 docId 重建 URI)
            assertEquals(
                listOf(
                    "fake://child/content://tree/dl/12/docs/MTDSS/small.txt",
                    "fake://child/content://tree/dl/14",
                ),
                e.saf.opened,
            )
            // 逐段下钻查询链:树根 → docs → MTDSS(末段文件不再往下查)
            assertEquals(listOf("12", "100", "13"), e.saf.childQueries)
            awaitUntil { 21L in e.acked }
        } finally {
            e.close()
        }
    }

    @Test
    fun `回退也找不到段名则拒开`() = runTest {
        val e = Env(roots = downloadsRoots)
        try {
            e.saf.openErrors["fake://child/content://tree/dl/12/MTDSS/small.txt"] =
                SourceOpenException("""文档不可开(For input string: "12/MTDSS/small.txt")""")
            e.saf.givenChildren("12", TreeChild("100", "MTDSS", isDir = true, size = 0, mtimeMs = 0))
            // "100" 层子项表缺项 = 查询失败:回退在末段失手
            e.start()
            e.emitRequest(22, "roottree://r2/MTDSS/small.txt")
            e.awaitFrames()
            val frame = e.frames().single()
            assertEquals(FdWire.KIND_SOURCE_FD, frame.kind)
            assertTrue("拒开帧不得带 fd", frame.fds.isEmpty())
            val error = frame.payload.opt("error")!!.let { (it as JVal.Str).v }
            assertTrue("词面应承快开异常(文档不可开):$error", error.contains("文档不可开"))
            awaitUntil { 22L in e.acked }
        } finally {
            e.close()
        }
    }

    @Test
    fun `授权根已移除拒开`() = runTest {
        val e = Env()
        try {
            e.start()
            e.emitRequest(10, "roottree://gone/x.jpg")
            e.awaitFrames()
            val frame = e.frames().single()
            assertTrue(frame.fds.isEmpty())
            val error = frame.payload.opt("error")!!.let { (it as JVal.Str).v }
            assertTrue("词面应含移除提示:$error", error.contains("授权根已移除"))
            assertTrue(e.saf.opened.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `畸形事件帧不杀订阅`() = runTest {
        val e = Env()
        try {
            e.start()
            // 1) 缺 ref:丢弃,不应答
            e.emitRequest(11, null)
            kotlinx.coroutines.delay(300)
            assertEquals("缺 ref 帧不得产生应答", 0, e.frames().size)
            // 2) 后续合法事件照常应答(订阅未死)
            e.emitRequest(12, "roottree://r1/ok.jpg")
            e.awaitFrames()
            val frame = e.frames().single()
            assertEquals(1, frame.fds.size)
            awaitUntil { 12L in e.acked }
        } finally {
            e.close()
        }
    }
}
