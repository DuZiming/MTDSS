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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelChildren
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
 * 缩略图桥应答器 JVM 单测(V4-1 T9;daemon 侧 jobs.rs ThumbBridge /
 * fdpass.rs 0x0B 契约为真源):`fs.request_thumb {request_id, path, w}` 事件
 * → SAF/MediaStore 两源解析 → 生成 JPEG(MediaMetadataRetriever/BitmapFactory
 * 实机面,此处注入伪编码器)落缩略图临时目录 → 0x0B thumb_data 帧
 * (`{ok, path, request_id}` 字母序线上形;拒答 = 无 path +
 * `{error, ok:false, request_id}`)。降采样参数面:w 原样(钳制后)到达
 * 编码器;畸形事件帧可拒答的拒答、无路由键的静默丢弃。
 */
class ThumbResponderTest {

    // ── 假面装配 ───────────────────────────────────────────────────────────

    companion object {
        private const val INBOX_TREE_URI = "content://tree/inbox/tree"
        private const val INBOX_TREE_DOC_ID = "primary:Download"

        /** 伪 JPEG 字节(内容任意——编码器注入,解码面归实机冒烟)。 */
        private val FAKE_JPEG = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0x01, 0x02, 0x03)
    }

    /** 伪授权根表(RootsStore 同形缝)。 */
    private class FakeRoots : RootLookup {
        var items: List<StoredRoot> = emptyList()
        override fun roots(): List<StoredRoot> = items
    }

    /** 伪相册库(openSeekable 注入缝:photoId → 带 release 旗的源)。 */
    private class FakePhotoLibrary : PhotoLibraryFace {
        var albumRows: List<PhotoAlbum>? = emptyList()
        val photoRows = LinkedHashMap<String, List<PhotoAsset>>()
        val seekable = LinkedHashMap<String, OpenedSource>()
        var seekableErrors = false
        val released = mutableListOf<String>()

        override fun albums(): List<PhotoAlbum>? = albumRows
        override fun photos(albumName: String): List<PhotoAsset>? = photoRows[albumName]
        override fun openExport(photoId: String): PhotoExportSource? = null
        override fun openSeekable(photoId: String): OpenedSource? {
            if (seekableErrors) return null
            return seekable[photoId] ?: OpenedSource(FileDescriptor()) { released += photoId }
        }
    }

    /** 伪编码器:记录 (displayName, w) 调用,回注入字节;null 注入 = 生成失败面。 */
    private class FakeEncoder : ThumbEncoder {
        val calls = mutableListOf<Pair<String, Int>>()
        var result: ByteArray? = FAKE_JPEG
        override fun jpeg(source: OpenedSource, displayName: String, w: Int): ByteArray? {
            calls += displayName to w
            return result
        }
    }

    /**
     * 标准收件箱树(与 SeekResponderTest 同款布局):授权树根名 Download ≠
     * MTDSS → 收件根 = 树内 MTDSS 子目录;`MTDSS/子目录/图.png` 下钻面。
     */
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
            TreeChild("$inboxDoc/子目录/图.png", "图.png", isDir = false, size = 24, mtimeMs = 333),
            TreeChild("$inboxDoc/子目录/深层", "深层", isDir = true, size = 0, mtimeMs = 0),
        )
    }

    /** SAF 面(openRead 换装带 release 旗的源;其余沿用 FakeSafFace 记录面)。 */
    private class TrackingSafFace : FakeSafFace() {
        var releases = 0
        override fun openRead(uri: String): OpenedSource {
            opened += uri
            return OpenedSource(FileDescriptor()) { releases += 1 }
        }
    }

    private class Env(preFill: (File) -> Unit = {}) {
        val saf = TrackingSafFace()
        val roots = FakeRoots()
        var granted: String? = INBOX_TREE_URI
        val inboxGrant = InboxGrantLookup { granted }
        val photos = FakePhotoLibrary()
        val encoder = FakeEncoder()
        val server = FakeFdServer()
        val thumbDir: File = Files.createTempDirectory("mtdss-t9-thumb-serve-").toFile().also { dir ->
            dir.mkdirs()
            preFill(dir)
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

        val channel = FdPassChannel(server, scope)
        val ipc = IpcClient(object : com.mtdss.android.ipc.SocketFactory {
            override fun connect(): com.mtdss.android.ipc.SocketConnection = throw IOException("测试不连 IPC")
        }, scope)

        val responder = ThumbResponder(
            saf = saf,
            inboxGrant = inboxGrant,
            roots = roots,
            photos = photos,
            thumbDir = thumbDir,
            channel = channel,
            ipc = ipc,
            scope = scope,
            ioDispatcher = Dispatchers.Unconfined,
            encoder = encoder,
        )

        fun close() {
            channel.close()
            ipc.close()
            scope.coroutineContext.cancelChildren()
            thumbDir.deleteRecursively()
        }
    }

    private fun env(): Env = Env().also { givenInboxTree(it.saf) }

    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    /** 伪 daemon ack 回执(server.onFrame 里同步回 0x81)。 */
    private fun ackAll(server: FakeFdServer) {
        server.onFrame = { conn, f ->
            conn.send(FdWire.KIND_ACK, JVal.Obj("request_id" to JVal.num(f.payload.opt("request_id")!!.asLong()), "ok" to JVal.bool(true)))
        }
    }

    private fun event(method: String, params: String): EventFrame = EventFrame(method, JVal.parse(params))

    // ── 事件处置 → 0x0B 应答帧(线上形)────────────────────────────────────

    @Test
    fun `thumb 事件 收件箱 vpath——0x0B 交付形 canonical 落盘编码器参数`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.request_thumb", """{"request_id":9,"path":"/收件箱/子目录/图.png","w":64}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_THUMB_DATA, frame.kind)
            assertEquals("交付形三键字母序(镜像 0x07/0x09 导出形)", true, frame.payload.opt("ok")!!.asBoolean())
            assertEquals(9L, frame.payload.opt("request_id")!!.asLong())
            val path = frame.payload.opt("path")!!.asStringOrNull()!!
            assertTrue("path 落注入的缩略图临时目录", path.startsWith(env.thumbDir.absolutePath))
            assertEquals("请求级唯一名", "9-图.png.jpg", File(path).name)
            assertEquals("应答无 fd(JPEG 走临时件)", 0, frame.fds.size)
            // 生成件落盘,字节 = 编码器产物
            assertEquals(FAKE_JPEG.toList(), File(path).readBytes().toList())
            // 降采样参数面:w 原样到达编码器 + 显示名(视频/图片分型凭据)
            assertEquals(listOf("图.png" to 64), env.encoder.calls)
            // 生成完源即释放(fd 副本生命周期 = 解码窗)
            assertEquals("SAF 源用毕释放", 1, env.saf.releases)
        } finally {
            env.close()
        }
    }

    @Test
    fun `thumb 事件 照片 vpath——MediaStore 臂批内唯一名重放`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.photos.photoRows["Camera"] = listOf(
                PhotoAsset(photoId = "img:1", name = "IMG_0001.jpg", size = 10, mtimeMs = 1),
                PhotoAsset(photoId = "img:2", name = "IMG_0001.jpg", size = 20, mtimeMs = 2),
            )
            env.responder.handle(event("fs.request_thumb", """{"request_id":5,"path":"/照片图库/Camera/IMG_0001 (1).jpg","w":128}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_THUMB_DATA, frame.kind)
            assertEquals(true, frame.payload.opt("ok")!!.asBoolean())
            assertEquals(
                "5-IMG_0001.jpg.jpg",
                File(frame.payload.opt("path")!!.asStringOrNull()!!).name,
            )
            assertEquals("编码器收到资产原始名(分型凭据=真实文件名,非树侧顺延名)", listOf("IMG_0001.jpg" to 128), env.encoder.calls)
            assertEquals("命中的资产源用毕释放", listOf("img:2"), env.photos.released)
        } finally {
            env.close()
        }
    }

    @Test
    fun `thumb 降采样参数——w 钳制 1 至 1024 后到达编码器`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            // 巨 w 钳到上界(与 daemon 侧 THUMB_MAX_WIDTH 同口径,防御旧端透传)
            env.responder.handle(event("fs.request_thumb", """{"request_id":1,"path":"/收件箱/子目录/图.png","w":5000}"""))
            // 下界:0/负无意义,钳到 1
            env.responder.handle(event("fs.request_thumb", """{"request_id":2,"path":"/收件箱/子目录/图.png","w":0}"""))
            assertEquals(
                "w 钳制面(巨 w→1024,0→1)",
                listOf("图.png" to 1024, "图.png" to 1),
                env.encoder.calls,
            )
        } finally {
            env.close()
        }
    }

    @Test
    fun `thumb 拒形矩阵 缺席文件 未注册根 相册缺席 目录 不可解 根自身 资产不可开`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.request_thumb", """{"request_id":1,"path":"/收件箱/子目录/缺席.png","w":64}"""))
            env.responder.handle(event("fs.request_thumb", """{"request_id":2,"path":"/未注册根/x.bin","w":64}"""))
            env.responder.handle(event("fs.request_thumb", """{"request_id":3,"path":"/照片图库/无此相册/x.jpg","w":64}"""))
            env.responder.handle(event("fs.request_thumb", """{"request_id":4,"path":"/收件箱/子目录/深层","w":64}"""))
            env.encoder.result = null // 生成失败面(不可解)
            env.responder.handle(event("fs.request_thumb", """{"request_id":5,"path":"/收件箱/子目录/图.png","w":64}"""))
            env.responder.handle(event("fs.request_thumb", """{"request_id":6,"path":"/照片图库","w":64}"""))
            env.photos.seekableErrors = true
            env.photos.photoRows["Camera"] = listOf(PhotoAsset("img:1", "a.jpg", 1, 1))
            env.responder.handle(event("fs.request_thumb", """{"request_id":7,"path":"/照片图库/Camera/a.jpg","w":64}"""))

            val conn = env.server.connections[0]
            assertEquals(7, conn.frames.size)
            for (f in conn.frames) {
                assertEquals("拒形全部 0x0B", FdWire.KIND_THUMB_DATA, f.kind)
                assertEquals("拒形 ok:false(引擎落既有回退)", false, f.payload.opt("ok")!!.asBoolean())
                assertEquals("拒形无 path 键", null, f.payload.opt("path"))
                assertEquals("拒形无 fd", 0, f.fds.size)
            }
            val canonical = conn.frames.map { it.payload.toCanonicalJson() }
            assertTrue("缺席文件词面", canonical[0].contains("源已不存在"))
            assertTrue("未注册根词面", canonical[1].contains("无此根"))
            assertTrue("相册缺席词面", canonical[2].contains("无此相册"))
            assertTrue("目录不可出图词面", canonical[3].contains("目录"))
            assertTrue("生成失败词面", canonical[4].contains("无法生成"))
            assertTrue("照片根词面", canonical[5].contains("照片"))
            assertTrue("资产不可开词面", canonical[6].contains("资产不可用"))
            assertTrue("生成失败零落盘", env.thumbDir.listFiles().isNullOrEmpty())
            assertTrue("生成失败不误放未开源", env.photos.released.isEmpty())
        } finally {
            env.close()
        }
    }

    @Test
    fun `thumb 畸形事件——缺 path 缺 w 拒答形 坏 request_id 静默丢弃`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            // 缺 path / 缺 w:可拒答面(有 id)→ 拒答帧(引擎落回退)
            env.responder.handle(event("fs.request_thumb", """{"request_id":9,"w":64}"""))
            env.responder.handle(event("fs.request_thumb", """{"request_id":10,"path":"/收件箱/子目录/图.png"}"""))
            // 缺 request_id / request_id 非数:无路由键 → 静默丢弃零帧
            env.responder.handle(event("fs.request_thumb", """{"path":"/收件箱/子目录/图.png","w":64}"""))
            env.responder.handle(event("fs.request_thumb", """{"request_id":"bad","path":"/收件箱/子目录/图.png","w":64}"""))
            val frames = env.server.connections[0].frames
            assertEquals("恰两枚拒答帧", 2, frames.size)
            for (f in frames) {
                assertEquals(FdWire.KIND_THUMB_DATA, f.kind)
                assertEquals(false, f.payload.opt("ok")!!.asBoolean())
                assertEquals(0, f.fds.size)
            }
            assertEquals("""{"error":"事件缺 path","ok":false,"request_id":9}""", frames[0].payload.toCanonicalJson())
            assertEquals("""{"error":"事件缺 w","ok":false,"request_id":10}""", frames[1].payload.toCanonicalJson())
            assertTrue("畸形帧不触编码器", env.encoder.calls.isEmpty())
        } finally {
            env.close()
        }
    }

    @Test
    fun `thumb 挂载清扫——目录内残件清空 目录保留`() = runTest {
        // 残件先于 Env 构造(挂载清扫发生在 responder 构造期)
        val env = Env(preFill = { dir ->
            File(dir, "777-stale.jpg").writeBytes(byteArrayOf(1))
            File(dir, "子目录").mkdirs()
        })
        try {
            assertFalse("残件应被清扫", File(env.thumbDir, "777-stale.jpg").exists())
            assertFalse("子目录应被清扫", File(env.thumbDir, "子目录").exists())
            assertTrue("目录本体保留", env.thumbDir.isDirectory)
        } finally {
            env.close()
        }
    }
}
