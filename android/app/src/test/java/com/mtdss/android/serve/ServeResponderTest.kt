package com.mtdss.android.serve

import com.mtdss.android.fdpass.FakeFdServer
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdWire
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.ListResult
import com.mtdss.android.ipc.RootSpec
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.asStringOrNull
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import com.mtdss.android.send.DocMeta
import com.mtdss.android.send.FakeSafFace
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
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.io.IOException
import java.util.concurrent.CopyOnWriteArrayList

/**
 * 服务桥应答器 JVM 单测(V3-4 §4.1 T14;daemon 侧 serve.rs/fdpass.rs 0x08/09/0A
 * 契约为真源):rootsSpecs 形(收件箱恒在 + 授权根 vpath 映射)、SAF list 假面
 * 矩阵(docId → vpath 子路径下钻/收件根归一化/.mtdss 私有区剔除)、导出临时件
 * 路径与尺寸守卫(实拷不符拒 + 无残件)、删除一次调用整树语义、三帧应答线上
 * 形(逐字节 canonical)与拒答形、启动清扫(挂载清空导出残件)。
 */
class ServeResponderTest {

    // ── 假面装配 ───────────────────────────────────────────────────────────

    companion object {
        /** 收件箱授权树 URI/树根 docId(FakeSafFace 查表键;任意串)。 */
        private const val INBOX_TREE_URI = "content://tree/inbox/tree"
        private const val INBOX_TREE_DOC_ID = "primary:Download"
    }

    private class RecordingDeletion : DocumentDeletion {
        val calls = CopyOnWriteArrayList<Pair<String, String>>()
        var result = true
        override fun deleteDocument(treeUri: String, documentId: String): Boolean {
            calls += treeUri to documentId
            return result
        }
    }

    /** 导出拷贝假面:uri → 原始字节;实拷恒等长(可注入尺寸不符)。 */
    private class FakeExportCopy : ExportCopy {
        val sources = LinkedHashMap<String, ByteArray>()
        var sizeMismatch: Boolean = false
        val copied = CopyOnWriteArrayList<String>()
        override fun copy(uri: String, dest: File, expectedSize: Long) {
            val data = sources[uri] ?: throw IOException("无此源: $uri")
            copied += uri
            if (sizeMismatch) {
                // 少写一字节 = 实拷不符(残件;调用方须删临时收口)
                dest.writeBytes(data.copyOfRange(0, (data.size - 1).coerceAtLeast(0)))
                throw IOException("拷贝字节数不符:实拷 ${data.size - 1} 预期 $expectedSize")
            }
            dest.writeBytes(data)
        }
    }

    /** 可通告变更的伪授权根表(RootsStore 同形缝)。 */
    private class FakeRoots : RootLookup {
        var onChange: (() -> Unit)? = null
        var items: List<StoredRoot> = emptyList()
        override fun roots(): List<StoredRoot> = items
    }

    /**
     * 标准收件箱树(授权树根名 = Download ≠ MTDSS → 收件根 = 树内 MTDSS 子目录):
     * - `MTDSS/报告.pdf`(16B,mtime 222)/ `MTDSS/子目录`(2 直接子项)/
     *   `MTDSS/.mtdss`(App 私有记账区 → 服务面剔除);
     * - `MTDSS/子目录/图.png` + `MTDSS/子目录/深层/嵌套.bin`(两级下钻面);
     * - `野.pdf`(树根直下,收件根之外——收件根=MTDSS 口径不得出现)。
     */
    private fun givenInboxTree(saf: FakeSafFace, sources: FakeExportCopy) {
        val rootUri = "fake://child/$INBOX_TREE_URI/$INBOX_TREE_DOC_ID"
        saf.treeDocIds[INBOX_TREE_URI] = INBOX_TREE_DOC_ID
        saf.metas[rootUri] = DocMeta(name = "Download", size = 0, mtimeMs = 0)
        saf.children[INBOX_TREE_DOC_ID] = listOf(
            TreeChild("primary:Download/MTDSS", "MTDSS", isDir = true, size = 0, mtimeMs = 0),
            TreeChild("primary:Download/野.pdf", "野.pdf", isDir = false, size = 3, mtimeMs = 1),
        )
        val inboxDoc = "primary:Download/MTDSS"
        saf.children[inboxDoc] = listOf(
            TreeChild("$inboxDoc/报告.pdf", "报告.pdf", isDir = false, size = 16, mtimeMs = 222),
            TreeChild("$inboxDoc/子目录", "子目录", isDir = true, size = 0, mtimeMs = 0),
            TreeChild("$inboxDoc/.mtdss", ".mtdss", isDir = true, size = 0, mtimeMs = 0),
        )
        saf.children["$inboxDoc/子目录"] = listOf(
            TreeChild("$inboxDoc/子目录/图.png", "图.png", isDir = false, size = 24, mtimeMs = 333),
            TreeChild("$inboxDoc/子目录/深层", "深层", isDir = true, size = 0, mtimeMs = 0),
        )
        saf.children["$inboxDoc/子目录/深层"] = listOf(
            TreeChild("$inboxDoc/子目录/深层/嵌套.bin", "嵌套.bin", isDir = false, size = 8, mtimeMs = 444),
        )
        saf.children["$inboxDoc/.mtdss"] = listOf(
            TreeChild("$inboxDoc/.mtdss/tmp.bin", "tmp.bin", isDir = false, size = 9, mtimeMs = 0),
        )
        sources.sources["fake://child/$INBOX_TREE_URI/$inboxDoc/报告.pdf"] = ByteArray(16) { it.toByte() } // 与 hit.size 对账
        sources.sources["fake://child/$INBOX_TREE_URI/$inboxDoc/子目录/图.png"] = ByteArray(24) { it.toByte() }
    }

    private class Env(val serveDir: File = File(java.lang.System.getProperty("java.io.tmpdir"), "mtdss-serve-tmp-test-${System.nanoTime()}")) {
        val saf = FakeSafFace()
        val sources = FakeExportCopy()
        val deletion = RecordingDeletion()
        val roots = FakeRoots()
        var granted: String? = INBOX_TREE_URI
        val inboxGrant = InboxGrantLookup { granted }
        val server = FakeFdServer()
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

        val channel = FdPassChannel(server, scope)
        val ipc = IpcClient(object : com.mtdss.android.ipc.SocketFactory {
            override fun connect(): com.mtdss.android.ipc.SocketConnection = throw IOException("测试不连 IPC")
        }, scope)

        val responder = ServeResponder(
            saf = saf,
            inboxGrant = inboxGrant,
            roots = roots,
            serveDir = serveDir,
            deletion = deletion,
            exportCopy = sources,
            channel = channel,
            ipc = ipc,
            scope = scope,
            ioDispatcher = Dispatchers.Unconfined,
        )

        fun close() {
            channel.close()
            ipc.close()
            scope.coroutineContext.cancelChildren()
            serveDir.deleteRecursively()
        }
    }

    private fun env(): Env = Env().also { givenInboxTree(it.saf, it.sources) }

    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    // ── rootsSpecs 形 ──────────────────────────────────────────────────────

    @Test
    fun `rootsSpecs——收件箱恒在首项且授权根映射为斜杠根 vpath`() {
        val roots = listOf(
            StoredRoot(rootId = "r1", treeUri = "u1", treeDocId = "d1", displayName = "相册根"),
            StoredRoot(rootId = "r2", treeUri = "u2", treeDocId = "d2", displayName = "文档"),
        )
        val specs = rootsSpecs(authorizedRoots = roots)
        assertEquals(3, specs.size)
        // 收件箱恒在首项(bridge + 可见)
        assertEquals(RootSpec("收件箱", "/收件箱", "bridge", true), specs[0])
        // 授权根按显示名映射(bridge + 可见,顺序保授权表序)
        assertEquals(RootSpec("相册根", "/相册根", "bridge", true), specs[1])
        assertEquals(RootSpec("文档", "/文档", "bridge", true), specs[2])
        // 空授权根 = 单收件箱(直通语义不出现——App 恒注册收件箱面)
        assertEquals(listOf(RootSpec("收件箱", "/收件箱", "bridge", true)), rootsSpecs(authorizedRoots = emptyList()))
    }

    // ── SAF list 假面矩阵(docId → vpath 子路径)─────────────────────────────

    @Test
    fun `list 收件箱根——收件根归一化 MTDSS 且私有区剔除且目录携直接子项数`() {
        val env = env()
        try {
            val l = env.responder.listVpath("/收件箱", "")
            assertTrue("收件箱根应可列:${(l as? ServeListing.Refused)?.reason}", l is ServeListing.Ok)
            val entries = (l as ServeListing.Ok).entries
            // .mtdss 私有记账区剔除;收件根(MTDSS)即根,树根直下的野.pdf 不出现
            assertEquals(listOf("报告.pdf", "子目录"), entries.map { it.name })
            val file = entries[0]
            assertFalse(file.isDir)
            assertEquals(16L, file.size)
            assertEquals(222L, file.mtimeMs)
            assertNull("文件项无 children", file.children)
            val dir = entries[1]
            assertTrue(dir.isDir)
            assertEquals("目录项 children = 直接子项数(图.png + 深层)", 2L, dir.children)
        } finally {
            env.close()
        }
    }

    @Test
    fun `list 子路径下钻——vpath 逐段映射 docId 且两级可达`() {
        val env = env()
        try {
            val l1 = env.responder.listVpath("/收件箱", "子目录")
            assertTrue(l1 is ServeListing.Ok)
            assertEquals(listOf("图.png", "深层"), (l1 as ServeListing.Ok).entries.map { it.name })

            val l2 = env.responder.listVpath("/收件箱", "子目录/深层")
            assertTrue(l2 is ServeListing.Ok)
            val deep = (l2 as ServeListing.Ok).entries.single()
            assertEquals("嵌套.bin", deep.name)
            assertEquals(8L, deep.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `list 拒答矩阵 缺席子路径 未注册根 未授权收件箱`() {
        val env = env()
        try {
            assertTrue(env.responder.listVpath("/收件箱", "缺席") is ServeListing.Refused)
            assertTrue(env.responder.listVpath("/未注册", "") is ServeListing.Refused)
            env.granted = null
            val refused = env.responder.listVpath("/收件箱", "")
            assertTrue("未授权收件箱应拒答", refused is ServeListing.Refused)
            assertTrue((refused as ServeListing.Refused).reason.contains("未授权"))
        } finally {
            env.close()
        }
    }

    @Test
    fun `list 授权根——按显示名 vpath 命中且与收件箱互不串扰`() {
        val env = env()
        try {
            env.roots.items = listOf(
                StoredRoot(rootId = "r1", treeUri = "content://tree/pics", treeDocId = "primary:Pics", displayName = "相册根"),
            )
            env.saf.treeDocIds["content://tree/pics"] = "primary:Pics"
            env.saf.children["primary:Pics"] = listOf(
                TreeChild("primary:Pics/a.jpg", "a.jpg", isDir = false, size = 5, mtimeMs = 7),
            )
            val l = env.responder.listVpath("/相册根", "")
            assertTrue(l is ServeListing.Ok)
            assertEquals(listOf("a.jpg"), (l as ServeListing.Ok).entries.map { it.name })
            // 已移除的授权根 → 拒答
            val gone = env.responder.listVpath("/不在册", "")
            assertTrue(gone is ServeListing.Refused)
        } finally {
            env.close()
        }
    }

    // ── 导出(serve-tmp 临时件 + 尺寸守卫)───────────────────────────────────

    @Test
    fun `export——原始字节落 serve-tmp 请求级唯一名`() {
        val env = env()
        try {
            val out = env.responder.exportTo("/收件箱", "报告.pdf", requestId = 7)
            assertTrue("导出应成功:${out.exceptionOrNull()}", out.isSuccess)
            val f = out.getOrThrow()
            assertEquals("请求级唯一名 <id>-<名>", "7-报告.pdf", f.name)
            assertEquals(env.serveDir.absolutePath, f.parentFile!!.absolutePath)
            assertEquals("原始字节落盘 + 尺寸守卫双道(实落 = 元数据 16B)", 16L, f.length())
        } finally {
            env.close()
        }
    }

    @Test
    fun `export 尺寸守卫——实拷不符拒导且无临时残件`() {
        val env = env()
        try {
            env.sources.sizeMismatch = true
            val out = env.responder.exportTo("/收件箱", "报告.pdf", requestId = 8)
            assertTrue("尺寸不符应拒导", out.isFailure)
            assertEquals("serve-tmp 无残件(失败即删临时)", 0, env.serveDir.listFiles()?.size ?: 0)
        } finally {
            env.close()
        }
    }

    @Test
    fun `export 拒答矩阵 目录与缺席子路径`() {
        val env = env()
        try {
            assertTrue("目录不可导出", env.responder.exportTo("/收件箱", "子目录", 9).isFailure)
            assertTrue("缺席拒导", env.responder.exportTo("/收件箱", "无此件.pdf", 10).isFailure)
            assertEquals("失败路径零临时件", 0, env.serveDir.listFiles()?.size ?: 0)
        } finally {
            env.close()
        }
    }

    // ── 删除(一次调用 = 整树语义)────────────────────────────────────────────

    @Test
    fun `delete——目录子路径一次调用整树删且缺席为 false`() {
        val env = env()
        try {
            assertTrue(env.responder.deletePath("/收件箱", "子目录"))
            assertEquals("恰一次删除调用(SAF deleteDocument 整树语义)", 1, env.deletion.calls.size)
            assertEquals(INBOX_TREE_URI, env.deletion.calls[0].first)
            assertEquals("docId = 下钻命中的真实子文档 id", "primary:Download/MTDSS/子目录", env.deletion.calls[0].second)

            env.deletion.calls.clear()
            assertFalse("缺席目标 = deleted:false(NOT_FOUND 面)", env.responder.deletePath("/收件箱", "无此件"))
            assertEquals("缺席不得触删除", 0, env.deletion.calls.size)
        } finally {
            env.close()
        }
    }

    // ── 事件处置 → fdpass 应答帧(线上形)───────────────────────────────────

    /** 伪 daemon ack 回执(server.onFrame 里同步回 0x81)。 */
    private fun ackAll(server: FakeFdServer) {
        server.onFrame = { conn, f -> conn.send(FdWire.KIND_ACK, JVal.Obj("request_id" to JVal.num(f.payload.opt("request_id")!!.asLong()), "ok" to JVal.bool(true))) }
    }

    private fun event(method: String, params: String): EventFrame =
        EventFrame(method, JVal.parse(params))

    @Test
    fun `serve_list 事件——0x08 应答 entries 线上形 canonical`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.serve_list", """{"request_id":9,"root":"/收件箱","sub":""}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SERVE_LIST, frame.kind)
            assertTrue("0x08 契约无 fd", frame.fds.isEmpty())
            assertEquals(
                "canonical 逐字节(entries = SAF 面原序,引擎侧统一重排目录优先+名序;ok/truncated 在场)",
                """{"entries":[{"is_dir":false,"mtime_ms":222,"name":"报告.pdf","size":16},""" +
                    """{"children":2,"is_dir":true,"mtime_ms":0,"name":"子目录","size":0}],"ok":true,"request_id":9,"truncated":false}""",
                frame.payload.toCanonicalJson(),
            )
        } finally {
            env.close()
        }
    }

    @Test
    fun `serve_export 事件——0x09 应答 serve-tmp 绝对路径`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.serve_export", """{"request_id":11,"root":"/收件箱","sub":"报告.pdf"}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SERVE_EXPORT, frame.kind)
            assertEquals("ok", true, frame.payload.opt("ok")!!.asBoolean())
            val path = frame.payload.opt("path")!!.asString()
            assertTrue("path 须落在 serve-tmp 内", path.contains("serve-tmp"))
            assertNotNull(File(path).takeIf { it.exists() })
            assertEquals(11L, frame.payload.opt("request_id")!!.asLong())
        } finally {
            env.close()
        }
    }

    @Test
    fun `serve_delete 事件——0x0A 应答 deleted 形且缺席报 false`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            env.responder.handle(event("fs.serve_delete", """{"request_id":12,"root":"/收件箱","sub":"子目录"}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SERVE_DELETE, frame.kind)
            assertEquals(
                """{"deleted":true,"ok":true,"request_id":12}""",
                frame.payload.toCanonicalJson(),
            )
            // 缺席目标:ok:true + deleted:false(引擎收 NOT_FOUND)
            env.responder.handle(event("fs.serve_delete", """{"request_id":13,"root":"/收件箱","sub":"无此件"}"""))
            val miss = env.server.connections[0].frames.last()
            assertEquals("""{"deleted":false,"ok":true,"request_id":13}""", miss.payload.toCanonicalJson())
        } finally {
            env.close()
        }
    }

    @Test
    fun `拒答与畸形事件——ok false 应答与静默丢弃`() = runTest {
        val env = env()
        try {
            ackAll(env.server)
            // 拒答(未注册根):ok:false + error 词面
            env.responder.handle(event("fs.serve_list", """{"request_id":14,"root":"/未注册","sub":""}"""))
            val frame = env.server.connections[0].frames.single()
            assertEquals(FdWire.KIND_SERVE_LIST, frame.kind)
            assertEquals(false, frame.payload.opt("ok")!!.asBoolean())
            assertEquals("无此根", frame.payload.opt("error")!!.asStringOrNull())
            // 畸形帧(缺 request_id):静默丢弃,不炸订阅
            env.responder.handle(event("fs.serve_list", """{"root":"/收件箱","sub":""}"""))
            env.responder.handle(event("fs.serve_其他", """{"request_id":1}"""))
            assertEquals("畸形帧零应答", 1, env.server.connections[0].frames.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `列表截断——超帽条目 truncated true 且 payload 不超帧上限`() {
        val env = env()
        try {
            // 300 个子项(LISTING_CAP = 256)→ 应答 256 项 + truncated
            val many = (1..300).map {
                TreeChild("primary:Download/MTDSS/f$it", "f$it", isDir = false, size = 1, mtimeMs = 0)
            }
            env.saf.children["primary:Download/MTDSS"] = many
            val l = env.responder.listVpath("/收件箱", "")
            assertTrue(l is ServeListing.Ok)
            assertEquals(ServeResponder.LISTING_CAP.toLong(), (l as ServeListing.Ok).entries.size.toLong())
            // 帧化后 payload 不超 64 KiB(fdpass 单帧上限)
            val payload = FdWire.listReplyPayload(
                requestId = 1,
                entries = l.entries.map { env.responder.serveEntryPayload(it) },
                truncated = true,
            )
            assertTrue(
                "payload ${payload.toCanonicalJson().toByteArray().size} 字节须在帧上限内",
                payload.toCanonicalJson().toByteArray().size <= FdWire.MAX_PAYLOAD_BYTES,
            )
        } finally {
            env.close()
        }
    }

    // ── 本机收件箱 lister(T10 接缝)────────────────────────────────────────

    @Test
    fun `listInbox——vpath 分段映射 DirEntry 且非收件箱路径拒`() = runTest {
        val env = env()
        try {
            val root = env.responder.listInbox("/收件箱")
            assertTrue(root.isSuccess)
            val r = (root.getOrThrow() as ListResult)
            assertEquals(false, r.truncated)
            assertEquals(listOf("报告.pdf", "子目录"), r.entries.map { it.name })
            // 条目 path = 完整 vpath(可直接再下钻/推整页)
            assertEquals("/收件箱/报告.pdf", r.entries[0].path)
            assertEquals(DirEntry("报告.pdf", "/收件箱/报告.pdf", 16, 222, false, null), r.entries[0])
            assertEquals("/收件箱/子目录", r.entries[1].path)
            assertEquals(2L, r.entries[1].children)

            val sub = env.responder.listInbox("/收件箱/子目录")
            assertTrue(sub.isSuccess)
            assertEquals(listOf("图.png", "深层"), (sub.getOrThrow()).entries.map { it.name })

            // 非收件箱 vpath 拒(本机 lister 只服务收件箱子树)
            assertTrue(env.responder.listInbox("/别处").isFailure)
            // 前缀撞名不成根:/收件箱x 不是收件箱内
            assertTrue(env.responder.listInbox("/收件箱x").isFailure)
            // 未授权 → 失败
            env.granted = null
            assertTrue(env.responder.listInbox("/收件箱").isFailure)
        } finally {
            env.close()
        }
    }

    // ── 启动清扫(App 侧 unlink 义务)───────────────────────────────────────

    @Test
    fun `挂载清扫——构造即清空 serve-tmp 残件且目录保留`() {
        // 预置残件(上次进程崩溃遗留的导出中间态)→ 构造即清扫(Unconfined
        // 域:订阅协程启动即扫到首个挂起点)
        val dir = File(java.lang.System.getProperty("java.io.tmpdir"), "mtdss-serve-tmp-sweep-${System.nanoTime()}")
        dir.mkdirs()
        File(dir, "3-残件.pdf").writeBytes(byteArrayOf(1))
        File(dir, "4-残件.png").writeBytes(byteArrayOf(2))
        val env = Env(serveDir = dir)
        try {
            assertTrue("目录保留", dir.isDirectory)
            assertEquals("残件清空", 0, dir.listFiles()?.size ?: 0)
        } finally {
            env.close()
        }
    }
}
