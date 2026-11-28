package com.mtdss.android.serve

import com.mtdss.android.fdpass.FakeFdServer
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.send.FakeSafFace
import com.mtdss.android.send.OpenedSource
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.send.TreeChild
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.io.FileDescriptor
import java.io.IOException
import java.nio.file.Files

/**
 * 转码桥应答器 JVM 单测(V4-1 T13;daemon 侧 jobs.rs RenditionBridge /
 * fdpass 0x0C 契约为真源):`fs.rendition_open {path, request_id, start_at}`
 * → 源解析(BridgeSources 单点)→ 会话目录 → **回执先行** → 注入伪管线
 * 产段落盘 + 0x0C `{index, ok, path, request_id}` 段回告;`fs.rendition_cancel`
 * → 取消旗 + 清段目录 + `{ok, request_id}`(未知会话幂等)。拒形矩阵、
 * 畸形帧、回执未受理(不开管线)、挂载清扫。
 */
class RenditionResponderTest {

    // ── 假面装配 ───────────────────────────────────────────────────────────

    companion object {
        private const val INBOX_TREE_URI = "content://tree/inbox/tree"
        private const val INBOX_TREE_DOC_ID = "primary:Download"
    }

    /** 伪授权根表(RootsStore 同形缝)。 */
    private class FakeRoots : RootLookup {
        var items: List<StoredRoot> = emptyList()
        override fun roots(): List<StoredRoot> = items
    }

    /** 伪相册库(openSeekable 注入缝)。 */
    private class FakePhotoLibrary : PhotoLibraryFace {
        var albumRows: List<PhotoAlbum>? = emptyList()
        val photoRows = LinkedHashMap<String, List<PhotoAsset>>()
        val seekable = LinkedHashMap<String, OpenedSource>()
        val released = mutableListOf<String>()

        override fun albums(): List<PhotoAlbum>? = albumRows
        override fun photos(albumName: String): List<PhotoAsset>? = photoRows[albumName]
        override fun openExport(photoId: String): PhotoExportSource? = null
        override fun openSeekable(photoId: String): OpenedSource? = seekable[photoId]
    }

    /**
     * 伪转码管线:记录调用面(displayName/startAt/sessionDir/isCancelled);
     * 可编程:onStart 挂起(取消测试)或即产段(段回告测试)或抛错(失败
     * 回告测试)。fd 释放契约照走(响应器收尾 release)。
     */
    private class FakeFactory : TranscoderFactory {
        class Call(
            val displayName: String,
            val startAtMs: Long,
            val sessionDir: File,
            val isCancelled: () -> Boolean,
        )

        val calls = mutableListOf<Call>()
        var mode: Mode = Mode.Produce
        var started = CompletableDeferred<Unit>()
        var finish = CompletableDeferred<Unit>()

        enum class Mode { Produce, Hang, Fail }

        override fun transcode(
            source: OpenedSource,
            displayName: String,
            startAtMs: Long,
            sessionDir: File,
            notifier: SegmentNotifier,
            isCancelled: () -> Boolean,
        ) {
            calls += Call(displayName, startAtMs, sessionDir, isCancelled)
            started.complete(Unit)
            when (mode) {
                Mode.Produce -> {
                    for (i in 0..1) {
                        val f = File(sessionDir, "$i.m4s")
                        f.writeBytes(byteArrayOf(i.toByte(), 9, 9))
                        notifier.onSegment(i, f)
                    }
                }
                Mode.Hang -> {
                    // 挂到 cancel/收尾(cancel 测试经 finish.complete 解挂)
                    runBlocking { finish.await() }
                }
                Mode.Fail -> throw IOException("编码器初始化失败")
            }
        }
    }

    /** SAF 面(openRead 换装带 release 旗的源)。 */
    private class TrackingSafFace : FakeSafFace() {
        var releases = 0
        override fun openRead(uri: String): OpenedSource {
            opened += uri
            return OpenedSource(FileDescriptor()) { releases += 1 }
        }
    }

    private class Env(preFill: (File) -> Unit = {}, ackTimeoutMs: Long = 5_000L) {
        val saf = TrackingSafFace()
        val roots = FakeRoots()
        var granted: String? = INBOX_TREE_URI
        val inboxGrant = InboxGrantLookup { granted }
        val photos = FakePhotoLibrary()
        val factory = FakeFactory()
        val server = FakeFdServer()
        val renditionDir: File = Files.createTempDirectory("mtdss-t13-rendition-serve-").toFile().also { dir ->
            dir.mkdirs()
            preFill(dir)
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

        val channel = FdPassChannel(server, scope, ackTimeoutMs = ackTimeoutMs)
        val ipc = IpcClient(object : com.mtdss.android.ipc.SocketFactory {
            override fun connect(): com.mtdss.android.ipc.SocketConnection = throw IOException("测试不连 IPC")
        }, scope)

        val responder = RenditionResponder(
            saf = saf,
            inboxGrant = inboxGrant,
            roots = roots,
            photos = photos,
            renditionDir = renditionDir,
            channel = channel,
            ipc = ipc,
            scope = scope,
            // 管线在真线程池(Hang 模式 runBlocking 挂面不可占 runBlocking
            // 测试线程——Unconfined 会把阻塞工厂内联进测试栈,死锁)
            ioDispatcher = Dispatchers.Default,
            factory = factory,
        )

        fun close() {
            channel.close()
            ipc.close()
            scope.coroutineContext.cancelChildren()
            renditionDir.deleteRecursively()
        }

        /** 等伪 server 收满 n 帧(有界;产段回告异步面)。 */
        suspend fun untilFrames(n: Int) {
            val deadline = System.currentTimeMillis() + 5_000
            while (server.connections.isEmpty() || server.connections[0].frames.size < n) {
                assertTrue("帧未在窗内到齐: ${server.connections.size}", System.currentTimeMillis() < deadline)
                delay(10)
            }
        }
    }

    private fun env(): Env = Env().also { givenInboxTree(it.saf) }

    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    /** 伪 daemon ack 回执(server.onFrame 里同步回 0x81)。 */
    private fun ackAll(server: FakeFdServer) {
        server.onFrame = { conn, f ->
            conn.send(
                FdWire.KIND_ACK,
                JVal.Obj("request_id" to JVal.num(f.payload.opt("request_id")!!.asLong()), "ok" to JVal.bool(true)),
            )
        }
    }

    private fun event(method: String, params: String): EventFrame = EventFrame(method, JVal.parse(params))

    /** 标准收件箱树(ThumbResponderTest 同款布局)。 */
    private fun givenInboxTree(saf: FakeSafFace) {
        val rootUri = "fake://child/$INBOX_TREE_URI/$INBOX_TREE_DOC_ID"
        saf.treeDocIds[INBOX_TREE_URI] = INBOX_TREE_DOC_ID
        saf.metas[rootUri] = com.mtdss.android.send.DocMeta(name = "Download", size = 0, mtimeMs = 0)
        val inboxDoc = "primary:Download/MTDSS"
        saf.children[INBOX_TREE_DOC_ID] = listOf(
            TreeChild("primary:Download/MTDSS", "MTDSS", isDir = true, size = 0, mtimeMs = 0),
        )
        saf.children[inboxDoc] = listOf(
            TreeChild("$inboxDoc/子目录", "子目录", isDir = true, size = 0, mtimeMs = 0),
        )
        saf.children["$inboxDoc/子目录"] = listOf(
            TreeChild("$inboxDoc/子目录/视频.mp4", "视频.mp4", isDir = false, size = 24, mtimeMs = 333),
        )
    }

    // ── open → 回执先行 + 段回告 ──────────────────────────────────────────

    @Test
    fun `open 回执先行 段回告有序 落盘 会话目录`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(
                event("fs.rendition_open", """{"request_id":9,"path":"/收件箱/子目录/视频.mp4","start_at":0}"""),
            )
            env.untilFrames(3)
            val conn = env.server.connections[0]
            // 帧 3:管线自然完毕通知(field 修 0x0C eof 形)——引擎记会话
            // 完毕,查看端 playlist 封 ENDLIST(等帧到 4 再断)
            env.untilFrames(4)
            assertEquals(4, conn.frames.size)
            // 帧 0:回执先行({ok, request_id};段回告必随其后)
            assertEquals(FdWire.KIND_RENDITION_DATA, conn.frames[0].kind)
            assertEquals(
                """{"ok":true,"request_id":9}""",
                conn.frames[0].payload.toCanonicalJson(),
            )
            // 帧 1/2:段回告(index 0 = init 段;路径落会话目录)
            for ((i, idx) in listOf(1 to 0, 2 to 1)) {
                assertEquals(FdWire.KIND_RENDITION_DATA, conn.frames[i].kind)
                assertEquals(idx.toLong(), conn.frames[i].payload.opt("index")!!.asLong())
                assertEquals(true, conn.frames[i].payload.opt("ok")!!.asBoolean())
                val path = conn.frames[i].payload.opt("path")!!.asStringOrNull()!!
                assertEquals(
                    "路径落会话目录({renditionDir}/{session}/{index}.m4s)",
                    File(File(env.renditionDir, "9"), "$idx.m4s").absolutePath,
                    File(path).absolutePath,
                )
                assertEquals(0, conn.frames[i].fds.size)
            }
            // 段文件真落盘(伪管线产物)
            assertEquals(
                listOf(0.toByte(), 9, 9).toList(),
                File(File(env.renditionDir, "9"), "0.m4s").readBytes().toList(),
            )
            // 管线调用面:start_at 透传 + 会话目录即会话键目录
            assertEquals(1, env.factory.calls.size)
            assertEquals(0L, env.factory.calls[0].startAtMs)
            assertEquals("视频.mp4", env.factory.calls[0].displayName)
            // 帧 3:0x0C eof 形钉面({eof, ok, request_id})
            assertEquals(FdWire.KIND_RENDITION_DATA, conn.frames[3].kind)
            assertEquals(
                """{"eof":true,"ok":true,"request_id":9}""",
                conn.frames[3].payload.toCanonicalJson(),
            )
            // fd 所有权:工厂返回即 release(响应器兜底)
            assertEquals("源用毕释放", 1, env.saf.releases)
        } finally {
            env.close()
        }
    }

    @Test
    fun `open start_at 透传非零 起点`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            // (回执须被受理管线才起——ackAll 装面必先;管线起于 Default
            // worker 异步面,started 闸对齐)
            env.responder.handle(
                event("fs.rendition_open", """{"request_id":1,"path":"/收件箱/子目录/视频.mp4","start_at":61000}"""),
            )
            env.factory.started.await()
            assertEquals(61000L, env.factory.calls[0].startAtMs)
        } finally {
            env.close()
        }
    }

    @Test
    fun `open 拒形矩阵 未注册根 缺席文件 相册缺席 目录`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.rendition_open", """{"request_id":1,"path":"/未注册根/x.mp4","start_at":0}"""))
            env.responder.handle(event("fs.rendition_open", """{"request_id":2,"path":"/收件箱/子目录/缺席.mp4","start_at":0}"""))
            env.responder.handle(event("fs.rendition_open", """{"request_id":3,"path":"/照片图库/无此相册/x.mp4","start_at":0}"""))
            env.responder.handle(event("fs.rendition_open", """{"request_id":4,"path":"/收件箱/子目录","start_at":0}"""))
            val frames = env.server.connections[0].frames
            assertEquals(4, frames.size)
            for (f in frames) {
                assertEquals("拒形全部 0x0C", FdWire.KIND_RENDITION_DATA, f.kind)
                assertEquals("拒形 ok:false(daemon 落 unsupported 回退原流)", false, f.payload.opt("ok")!!.asBoolean())
                assertEquals("拒形无 fd", 0, f.fds.size)
            }
            val canonical = frames.map { it.payload.toCanonicalJson() }
            assertTrue("未注册根词面", canonical[0].contains("无此根"))
            assertTrue("缺席文件词面", canonical[1].contains("源已不存在"))
            assertTrue("相册缺席词面", canonical[2].contains("无此相册"))
            assertTrue("目录词面", canonical[3].contains("目录"))
            // 拒形不开管线、不建会话目录
            assertTrue("拒形零管线", env.factory.calls.isEmpty())
            assertEquals("拒形零会话目录", 0, env.renditionDir.listFiles()!!.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `open 畸形事件 缺 path 缺 start_at 拒答 坏 request_id 静默`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.rendition_open", """{"request_id":9,"start_at":0}"""))
            env.responder.handle(event("fs.rendition_open", """{"request_id":10,"path":"/收件箱/子目录/视频.mp4"}"""))
            env.responder.handle(event("fs.rendition_open", """{"path":"/收件箱/子目录/视频.mp4","start_at":0}"""))
            env.responder.handle(event("fs.rendition_open", """{"request_id":"bad","path":"/收件箱/子目录/视频.mp4","start_at":0}"""))
            val frames = env.server.connections[0].frames
            assertEquals("恰两枚拒答帧", 2, frames.size)
            for (f in frames) {
                assertEquals(FdWire.KIND_RENDITION_DATA, f.kind)
                assertEquals(false, f.payload.opt("ok")!!.asBoolean())
            }
            assertEquals("""{"error":"事件缺 path","ok":false,"request_id":9}""", frames[0].payload.toCanonicalJson())
            assertEquals("""{"error":"事件缺 start_at","ok":false,"request_id":10}""", frames[1].payload.toCanonicalJson())
            assertTrue("畸形帧不触管线", env.factory.calls.isEmpty())
        } finally {
            env.close()
        }
    }

    @Test
    fun `open 回执未受理 不开管线 即收源与目录`() = runTest {
        val env = Env(ackTimeoutMs = 200).also { givenInboxTree(it.saf) }
        try {
            // server.onFrame 不装:ack 永不到(超时窗 200ms)
            env.responder.handle(
                event("fs.rendition_open", """{"request_id":5,"path":"/收件箱/子目录/视频.mp4","start_at":0}"""),
            )
            env.untilFrames(1) // 回执帧已发
            delay(400) // 过超时窗
            assertEquals("仅回执帧,零段回告", 1, env.server.connections[0].frames.size)
            assertTrue("不开管线", env.factory.calls.isEmpty())
            assertEquals("源即收(白烧电防线)", 1, env.saf.releases)
            assertFalse("会话目录即清", File(env.renditionDir, "5").exists())
        } finally {
            env.close()
        }
    }

    @Test
    fun `open 管线失败 失败回告 ok false`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.factory.mode = FakeFactory.Mode.Fail
            env.responder.handle(
                event("fs.rendition_open", """{"request_id":7,"path":"/收件箱/子目录/视频.mp4","start_at":0}"""),
            )
            env.untilFrames(2)
            val conn = env.server.connections[0]
            assertEquals(
                """{"ok":true,"request_id":7}""",
                conn.frames[0].payload.toCanonicalJson(),
            )
            assertEquals(FdWire.KIND_RENDITION_DATA, conn.frames[1].kind)
            assertEquals(false, conn.frames[1].payload.opt("ok")!!.asBoolean())
            val word = conn.frames[1].payload.opt("error")!!.asStringOrNull()!!
            assertTrue("失败词面透传", word.contains("编码器初始化失败"))
        } finally {
            env.close()
        }
    }

    // ── cancel ────────────────────────────────────────────────────────────

    @Test
    fun `cancel 取消旗可见 清段目录 回执 未知会话幂等`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.factory.mode = FakeFactory.Mode.Hang
            env.responder.handle(
                event("fs.rendition_open", """{"request_id":9,"path":"/收件箱/子目录/视频.mp4","start_at":0}"""),
            )
            env.factory.started.await()
            // cancel:置旗 → 清目录 → 回执
            env.responder.handle(event("fs.rendition_cancel", """{"request_id":100,"session":9}"""))
            assertTrue("取消旗对管线可见", env.factory.calls[0].isCancelled())
            assertFalse("段目录即清", File(env.renditionDir, "9").exists())
            val frames = env.server.connections[0].frames
            env.untilFrames(2) // hang 管线零段回告:回执×2(open+cancel)
            assertEquals(
                "cancel 回执 {ok, request_id}",
                """{"ok":true,"request_id":100}""",
                frames.last().payload.toCanonicalJson(),
            )
            // 未知会话幂等照成(残件自愈面)
            env.responder.handle(event("fs.rendition_cancel", """{"request_id":101,"session":55}"""))
            env.untilFrames(3)
            assertEquals(
                """{"ok":true,"request_id":101}""",
                frames.last().payload.toCanonicalJson(),
            )
            // 解挂管线(收尾面走完,无失败回告噪音——取消静默)
            env.factory.finish.complete(Unit)
        } finally {
            env.close()
        }
    }

    @Test
    fun `cancel 畸形 缺 session 静默零帧`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.rendition_cancel", """{"request_id":9}"""))
            delay(100)
            assertEquals("缺 session 静默零帧(拒回告会误伤失败标记面)", 0, env.server.connections.size)
        } finally {
            env.close()
        }
    }

    // ── 挂载清扫 ──────────────────────────────────────────────────────────

    @Test
    fun `挂载清扫 目录残件清空 目录保留`() {
        // 残件先于 Env 构造(挂载清扫发生在 responder 构造期)
        val env = Env(preFill = { dir ->
            File(dir, "9").mkdirs()
            File(dir, "9/0.m4s").writeBytes(byteArrayOf(1))
            File(dir, "99").writeBytes(byteArrayOf(2))
        })
        try {
            assertFalse("残件会话目录应被清扫", File(env.renditionDir, "9").exists())
            assertFalse("残件散件应被清扫", File(env.renditionDir, "99").exists())
            assertTrue("目录本体保留", env.renditionDir.isDirectory)
        } finally {
            env.close()
        }
    }
}
