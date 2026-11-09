package com.mtdss.android.serve

import com.mtdss.android.fdpass.FakeFdServer
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.CopyOnWriteArrayList

/**
 * 照片桥应答器 JVM 单测(V3-4 §4.3 T14;iOS PhotoServe.swift 契约为真源,
 * daemon photo.rs/0x06/0x07 消费):权限即数据面(未授权**不应答** → 引擎
 * 侧根隐藏,实时判定每次应答前)、根 = 相册列表(children = 照片数)、相册
 * = 照片列表(photo_id 导出凭据/批内唯一名 ` (n)` 顺延)、截断帽、导出原始
 * 字节到 photo-serve 请求级唯一名 + 尺寸守卫、拒答形(授权而相册缺席/查询
 * 失败)、启动清扫。
 */
class PhotoResponderTest {

    /** 伪相册库:相册表 + 相册内照片表 + 导出源表。 */
    private class FakePhotoLibrary : PhotoLibraryFace {
        var albumRows: List<PhotoAlbum>? = emptyList()
        val photoRows = LinkedHashMap<String, List<PhotoAsset>>()
        val exportSources = LinkedHashMap<String, ByteArray>()
        val exportNames = LinkedHashMap<String, String>()
        var exportSizes = LinkedHashMap<String, Long>()
        var sizeMismatch = false

        override fun albums(): List<PhotoAlbum>? = albumRows
        override fun photos(albumName: String): List<PhotoAsset>? = photoRows[albumName]
        override fun openExport(photoId: String): PhotoExportSource? {
            val data = exportSources[photoId] ?: return null
            val size = exportSizes[photoId] ?: data.size.toLong()
            return PhotoExportSource(
                input = object : InputStream() {
                    var pos = 0
                    override fun read(): Int =
                        if (pos >= data.size) -1 else data[pos++].toInt() and 0xFF
                },
                size = size,
                displayName = exportNames[photoId] ?: "asset.bin",
            )
        }
    }

    private class Env(
        val listingCap: Int = 200,
        val serveDir: File = File(java.lang.System.getProperty("java.io.tmpdir"), "mtdss-photo-serve-test-${System.nanoTime()}"),
    ) {
        val photos = FakePhotoLibrary()
        var authorized = true
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

        val channel = FdPassChannel(server, scope)
        val ipc = IpcClient(object : com.mtdss.android.ipc.SocketFactory {
            override fun connect(): com.mtdss.android.ipc.SocketConnection = throw IOException("测试不连 IPC")
        }, scope)

        val responder = PhotoResponder(
            photos = photos,
            serveDir = serveDir,
            channel = channel,
            ipc = ipc,
            scope = scope,
            authorized = { authorized },
            listingCap = listingCap,
            ioDispatcher = Dispatchers.Unconfined,
        )

        fun close() {
            channel.close()
            ipc.close()
            scope.coroutineContext.cancelChildren()
            serveDir.deleteRecursively()
        }
    }

    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    private fun ackAll(server: FakeFdServer) {
        server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, JVal.Obj("request_id" to JVal.num(f.payload.opt("request_id")!!.asLong()), "ok" to JVal.bool(true))) }
    }

    private fun event(method: String, params: String): EventFrame =
        EventFrame(method, JVal.parse(params))

    // ── 权限即数据面 ───────────────────────────────────────────────────────

    @Test
    fun `未授权不应答——列表与导出零帧(引擎侧根隐藏)`() = runTest {
        val env = Env()
        try {
            env.authorized = false
            ackAll(env.server)
            env.responder.handle(event("fs.photo_list", """{"request_id":1,"path":""}"""))
            env.responder.handle(event("fs.photo_export", """{"photo_id":"img:9","request_id":2}"""))
            assertTrue("未授权零应答帧(超时 → 引擎根隐藏)", env.server.connections.isEmpty())
        } finally {
            env.close()
        }
    }

    @Test
    fun `授权判定实时——先拒后准同一应答器翻面`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            env.photos.albumRows = listOf(PhotoAlbum("相机", 1))
            env.authorized = false
            env.responder.handle(event("fs.photo_list", """{"request_id":3,"path":""}"""))
            assertEquals("未授权窗零帧", 0, env.server.connections.size)
            env.authorized = true
            env.responder.handle(event("fs.photo_list", """{"request_id":4,"path":""}"""))
            assertEquals("授权即答(无需重注册)", 1, env.server.connections.size)
        } finally {
            env.close()
        }
    }

    // ── 列表(0x06)────────────────────────────────────────────────────────

    @Test
    fun `根列表——相册目录项 children 照片数 canonical 线上形`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            env.photos.albumRows = listOf(PhotoAlbum("相机", 3), PhotoAlbum("截屏", 1))
            env.responder.handle(event("fs.photo_list", """{"request_id":5,"path":""}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_PHOTO_LIST, frame.kind)
            assertTrue(frame.fds.isEmpty())
            assertEquals(
                """{"entries":[{"children":3,"is_dir":true,"mtime_ms":0,"name":"相机","size":0},""" +
                    """{"children":1,"is_dir":true,"mtime_ms":0,"name":"截屏","size":0}],"ok":true,"request_id":5,"truncated":false}""",
                frame.payload.toCanonicalJson(),
            )
        } finally {
            env.close()
        }
    }

    @Test
    fun `相册列表——照片文件项 photo_id 导出凭据且批内撞名顺延`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            env.photos.photoRows["相机"] = listOf(
                PhotoAsset("img:1", "IMG.png", 100, 111),
                PhotoAsset("img:2", "IMG.png", 200, 222),
                PhotoAsset("vid:3", "VID.mp4", 300, 333),
            )
            env.responder.handle(event("fs.photo_list", """{"request_id":6,"path":"相机"}"""))
            val frame = env.server.connections[0].frames.single()
            val entries = frame.payload.opt("entries")!!.asArray()
            assertEquals(3, entries.size)
            assertEquals("批内撞名 ` (n)` 顺延(R8 I-1 同语言)", "IMG (1).png", entries[1].opt("name")!!.asString())
            assertEquals("photo_id 原样(导出凭据)", "img:2", entries[1].opt("photo_id")!!.asString())
            assertEquals(200L, entries[1].opt("size")!!.asLong())
            assertEquals(222L, entries[1].opt("mtime_ms")!!.asLong())
            assertNull("文件项无 children", entries[1].opt("children"))
            assertEquals(
                """{"is_dir":false,"mtime_ms":333,"name":"VID.mp4","photo_id":"vid:3","size":300}""",
                entries[2].toCanonicalJson(),
            )
        } finally {
            env.close()
        }
    }

    @Test
    fun `列表截断——超帽保留前缀且 truncated true`() = runTest {
        val env = Env(listingCap = 3)
        try {
            ackAll(env.server)
            env.photos.albumRows = (1..5).map { PhotoAlbum("相册$it", it) }
            env.responder.handle(event("fs.photo_list", """{"request_id":7,"path":""}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(3, frame.payload.opt("entries")!!.asArray().size)
            assertEquals(true, frame.payload.opt("truncated")!!.asBoolean())
        } finally {
            env.close()
        }
    }

    @Test
    fun `列表拒答——无此相册与查询失败(ok false 词面)`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.photo_list", """{"request_id":8,"path":"无此相册"}"""))
            val f1 = env.server.connections[0].frames[0]
            assertEquals(false, f1.payload.opt("ok")!!.asBoolean())
            assertTrue(f1.payload.opt("error")!!.asStringOrNull()!!.contains("无此相册"))

            env.photos.albumRows = null // 查询失败
            env.responder.handle(event("fs.photo_list", """{"request_id":9,"path":""}"""))
            val f2 = env.server.connections[0].frames[1]
            assertEquals(false, f2.payload.opt("ok")!!.asBoolean())
            assertEquals(9L, f2.payload.opt("request_id")!!.asLong())
        } finally {
            env.close()
        }
    }

    // ── 导出(0x07)────────────────────────────────────────────────────────

    @Test
    fun `导出——原始字节落 photo-serve 请求级唯一名并回路径`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            val data = ByteArray(128) { (it * 3).toByte() }
            env.photos.exportSources["img:9"] = data
            env.photos.exportNames["img:9"] = "IMG_0001.heic"
            env.photos.exportSizes["img:9"] = data.size.toLong()
            env.responder.handle(event("fs.photo_export", """{"photo_id":"img:9","request_id":21}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_PHOTO_EXPORT, frame.kind)
            assertEquals(true, frame.payload.opt("ok")!!.asBoolean())
            assertEquals(21L, frame.payload.opt("request_id")!!.asLong())
            val path = frame.payload.opt("path")!!.asString()
            assertEquals("请求级唯一名 <id>-<名>", "21-IMG_0001.heic", File(path).name)
            assertTrue("落 photo-serve", path.contains("photo-serve"))
            assertTrue(data.contentEquals(File(path).readBytes()))
        } finally {
            env.close()
        }
    }

    @Test
    fun `导出尺寸守卫——流实读不符拒导且无残件`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            val data = ByteArray(64) { it.toByte() }
            env.photos.exportSources["img:9"] = data
            env.photos.exportSizes["img:9"] = 100L // 元数据宣称 100B,流只有 64B
            env.responder.handle(event("fs.photo_export", """{"photo_id":"img:9","request_id":22}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals("尺寸不符 = 拒导(ok:false)", false, frame.payload.opt("ok")!!.asBoolean())
            assertEquals("无残件", 0, env.serveDir.listFiles()?.size ?: 0)
        } finally {
            env.close()
        }
    }

    @Test
    fun `导出拒答——资产不可用(ok false)`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.photo_export", """{"photo_id":"img:缺席","request_id":23}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(false, frame.payload.opt("ok")!!.asBoolean())
        } finally {
            env.close()
        }
    }

    // ── 启动清扫 ──────────────────────────────────────────────────────────

    @Test
    fun `挂载清扫——构造即清空 photo-serve 残件`() {
        val dir = File(java.lang.System.getProperty("java.io.tmpdir"), "mtdss-photo-serve-sweep-${System.nanoTime()}")
        dir.mkdirs()
        File(dir, "5-IMG.heic").writeBytes(byteArrayOf(1))
        val env = Env(serveDir = dir)
        try {
            assertTrue(dir.isDirectory)
            assertEquals(0, dir.listFiles()?.size ?: 0)
        } finally {
            env.close()
        }
    }

    // ── 畸形帧 ────────────────────────────────────────────────────────────

    @Test
    fun `畸形事件帧——缺 request_id 静默丢弃不炸订阅`() = runTest {
        val env = Env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.photo_list", """{"path":""}"""))
            env.responder.handle(event("fs.photo_其他", """{"request_id":1}"""))
            assertTrue(env.server.connections.isEmpty())
        } finally {
            env.close()
        }
    }
}
