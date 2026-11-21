package com.mtdss.android.serve

import com.mtdss.android.fdpass.FakeFdServer
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
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
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.FileDescriptor
import java.io.IOException

/**
 * 可 seek fd 源桥应答器 JVM 单测(V4-1 T8;daemon 侧 jobs.rs SeekBridge /
 * fdpass.rs 0x0D 契约为真源):`fs.request_seek {request_id, path}` 事件 →
 * SAF/MediaStore 两源解析 → 0x0D seek_fd 帧(fd 随帧交付,`{request_id}`
 * 线上形;拒开 = 无 fd + `{error, ok:false, request_id}` 字母序)。
 * 畸形事件帧静默丢弃;不可 seek 的探测在 daemon 侧 fd 落点(from_seek_fd),
 * App 侧只管开 fd 与拒开词面。
 */
class SeekResponderTest {

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

    /** 伪相册库(含 openSeekable 注入缝:photoId → fd;null = 不可开)。 */
    private class FakePhotoLibrary : PhotoLibraryFace {
        var albumRows: List<PhotoAlbum>? = emptyList()
        val photoRows = LinkedHashMap<String, List<PhotoAsset>>()
        val seekable = LinkedHashMap<String, OpenedSource>()
        var seekableErrors = false

        override fun albums(): List<PhotoAlbum>? = albumRows
        override fun photos(albumName: String): List<PhotoAsset>? = photoRows[albumName]
        override fun openExport(photoId: String): PhotoExportSource? = null
        override fun openSeekable(photoId: String): OpenedSource? {
            if (seekableErrors) return null
            return seekable[photoId] ?: OpenedSource(FileDescriptor()) { }
        }
    }

    /**
     * 标准收件箱树(与 ServeResponderTest 同款布局):授权树根名 Download ≠
     * MTDSS → 收件根 = 树内 MTDSS 子目录;`MTDSS/子目录/图.png` 两级下钻面。
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

    private class Env {
        val saf = FakeSafFace()
        val roots = FakeRoots()
        var granted: String? = INBOX_TREE_URI
        val inboxGrant = InboxGrantLookup { granted }
        val photos = FakePhotoLibrary()
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

        val channel = FdPassChannel(server, scope)
        val ipc = IpcClient(object : com.mtdss.android.ipc.SocketFactory {
            override fun connect(): com.mtdss.android.ipc.SocketConnection = throw IOException("测试不连 IPC")
        }, scope)

        val responder = SeekResponder(
            saf = saf,
            inboxGrant = inboxGrant,
            roots = roots,
            photos = photos,
            channel = channel,
            ipc = ipc,
            scope = scope,
            ioDispatcher = Dispatchers.Unconfined,
        )

        fun close() {
            channel.close()
            ipc.close()
            scope.coroutineContext.cancelChildren()
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

    // ── 事件处置 → 0x0D 应答帧(线上形)────────────────────────────────────

    @Test
    fun `seek 事件 收件箱 vpath——0x0D 交付 fd 线上形 canonical 单 fd`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.request_seek", """{"request_id":9,"path":"/收件箱/子目录/图.png"}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SEEK_FD, frame.kind)
            assertEquals("交付形 payload 仅 request_id(镜像 0x01 交付形)", """{"request_id":9}""", frame.payload.toCanonicalJson())
            assertEquals("fd 随帧交付(恰一枚)", 1, frame.fds.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `seek 事件 授权根 vpath——按显示名解析交付`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.roots.items = listOf(
                StoredRoot(rootId = "r1", treeUri = "content://tree/pics", treeDocId = "primary:Pics", displayName = "相册根"),
            )
            env.saf.treeDocIds["content://tree/pics"] = "primary:Pics"
            env.saf.children["primary:Pics"] = listOf(
                TreeChild("primary:Pics/a.jpg", "a.jpg", isDir = false, size = 5, mtimeMs = 7),
            )
            env.responder.handle(event("fs.request_seek", """{"request_id":3,"path":"/相册根/a.jpg"}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SEEK_FD, frame.kind)
            assertEquals("""{"request_id":3}""", frame.payload.toCanonicalJson())
            assertEquals(1, frame.fds.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `seek 事件 照片 vpath——MediaStore 臂批内唯一名重放解析 photoId`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            // 相册 Camera 两张同名 IMG_0001.jpg → 列表面名顺延 ` (1)`(与
            // daemon 照片树一致);daemon 侧 vpath 携的唯一名 = 顺延名
            env.photos.photoRows["Camera"] = listOf(
                PhotoAsset(photoId = "img:1", name = "IMG_0001.jpg", size = 10, mtimeMs = 1),
                PhotoAsset(photoId = "img:2", name = "IMG_0001.jpg", size = 20, mtimeMs = 2),
            )
            val marked = OpenedSource(FileDescriptor()) { }
            env.photos.seekable["img:2"] = marked
            env.responder.handle(event("fs.request_seek", """{"request_id":5,"path":"/照片图库/Camera/IMG_0001 (1).jpg"}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SEEK_FD, frame.kind)
            assertEquals("""{"request_id":5}""", frame.payload.toCanonicalJson())
            assertEquals("fd = 顺延名命中的那枚资产", marked.fd, frame.fds.single())
        } finally {
            env.close()
        }
    }

    @Test
    fun `seek 拒形矩阵 缺席文件 未注册根 相册缺席 目录不可 seek 资产不可开`() = runTest {
        val env = env()
        try {
            ackAll(env.server)

            env.responder.handle(event("fs.request_seek", """{"request_id":1,"path":"/收件箱/子目录/缺席.png"}"""))
            env.responder.handle(event("fs.request_seek", """{"request_id":2,"path":"/未注册根/x.bin"}"""))
            env.responder.handle(event("fs.request_seek", """{"request_id":3,"path":"/照片图库/无此相册/x.jpg"}"""))
            env.responder.handle(event("fs.request_seek", """{"request_id":4,"path":"/收件箱/子目录/深层"}"""))
            env.responder.handle(event("fs.request_seek", """{"request_id":5,"path":"/照片图库"}"""))
            env.photos.seekableErrors = true
            env.photos.photoRows["Camera"] = listOf(PhotoAsset("img:1", "a.jpg", 1, 1))
            env.responder.handle(event("fs.request_seek", """{"request_id":6,"path":"/照片图库/Camera/a.jpg"}"""))

            val conn = env.server.connections[0]
            assertEquals(6, conn.frames.size)
            for (f in conn.frames) {
                assertEquals("拒形全部无 fd(触发引擎导出回退)", 0, f.fds.size)
                assertEquals(FdWire.KIND_SEEK_FD, f.kind)
                assertEquals(false, f.payload.opt("ok")!!.asBoolean())
            }
            val canonical = conn.frames.map { it.payload.toCanonicalJson() }
            assertTrue(canonical[0].contains(""""request_id":1"""))
            assertTrue("缺席文件词面", canonical[0].contains("源已不存在"))
            assertTrue("未注册根词面", canonical[1].contains("无此根"))
            assertTrue("相册缺席词面", canonical[2].contains("无此相册"))
            assertTrue("目录不可 seek 词面", canonical[3].contains("目录"))
            assertTrue("照片根不可 seek 词面", canonical[4].contains("照片"))
            assertTrue("openSeekable 失败词面", canonical[5].contains("资产不可用"))
        } finally {
            env.close()
        }
    }

    @Test
    fun `seek 畸形事件——缺 path 拒开形 缺或坏 request_id 静默丢弃`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            // 缺 path:可拒答面(有 id)→ 拒开帧(引擎落导出回退)
            env.responder.handle(event("fs.request_seek", """{"request_id":9}"""))
            // 缺 request_id / request_id 非数:无路由键 → 静默丢弃零帧
            env.responder.handle(event("fs.request_seek", """{"path":"/收件箱/x.bin"}"""))
            env.responder.handle(event("fs.request_seek", """{"request_id":"bad","path":"/收件箱/x.bin"}"""))
            val frames = env.server.connections[0].frames
            assertEquals("恰一枚拒开帧", 1, frames.size)
            assertEquals(FdWire.KIND_SEEK_FD, frames[0].kind)
            assertEquals(0, frames[0].fds.size)
            assertEquals("""{"error":"事件缺 path","ok":false,"request_id":9}""", frames[0].payload.toCanonicalJson())
        } finally {
            env.close()
        }
    }
}
