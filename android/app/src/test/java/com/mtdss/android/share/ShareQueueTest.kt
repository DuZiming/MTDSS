package com.mtdss.android.share

import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.test.TestKit.awaitUntil
import com.mtdss.android.test.TestKit.runTest
import com.mtdss.android.ui.tasks.ShareFailureRow
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.File
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList

/**
 * 分享暂存队列单测(V3-4 §4.4 T15,iOS ShareQueueTests 语义镜像;brief 步骤 1
 * 四场景全钉):拷贝中断(半件)不 finish(宽限窗内不动/窗外折失败)、盘损
 * 截短被尺寸守卫拒(不发「文件可能已损坏」)、重试换目标设备重发、消费幂等
 * (登录前不动队列保等待态;sent 不重发)。另钉 schema 蛇形键往返、孤儿
 * 目录折失败、发送失败不静默丢、失败行投影(ShareFailureController)。
 *
 * 队列与消费均为纯 JVM 面(root 临时目录注入;send 闭包注入,不依赖活引擎)。
 */
class ShareQueueTest {

    private val roots = CopyOnWriteArrayList<File>()

    @After
    fun tearDown() {
        roots.forEach { it.deleteRecursively() }
    }

    // ── 脚手架 ────────────────────────────────────────────────────────────

    private fun newStore(): ShareQueueStore =
        ShareQueueStore(Files.createTempDirectory("mtdss-t15-").toFile().also { roots.add(it) })

    private fun bytes(s: String): ByteArray = s.toByteArray(Charsets.UTF_8)

    private fun write(path: File, content: ByteArray) {
        path.parentFile.mkdirs()
        path.writeBytes(content)
    }

    private fun seedSrc(store: ShareQueueStore, name: String, content: String): File {
        val src = File(store.queueDir, "src-$name")
        write(src, bytes(content))
        return src
    }

    /** 两文件就绪项(IMG + 报告,内容各异;iOS seedReadyItem 同构)。 */
    private fun seedReady(
        store: ShareQueueStore,
        id: String = "item-1",
        target: String = "dev-1",
        nowMs: Long = 1_760_000_000_000L,
    ): List<Pair<ShareQueueFile, String>> {
        val files = listOf(
            ShareQueueFile(name = "IMG_0421.HEIC", size = 6),
            ShareQueueFile(name = "年度报告.pdf", size = 4),
        )
        val contents = listOf("photo1", "doc2")
        store.beginItem(id = id, targetDeviceId = target, files = files, nowMs = nowMs)
        for ((file, content) in files.zip(contents)) {
            store.copyStream(id, file, ByteArrayInputStream(bytes(content)))
        }
        store.finishItem(id, nowMs)
        return files.zip(contents)
    }

    /** 记录型伪发送(可编程首 N 次抛错)。 */
    private class FakeSend {
        val calls = CopyOnWriteArrayList<Pair<List<BridgeItem>, String>>()
        var failuresLeft = 0

        suspend fun send(items: List<BridgeItem>, target: String): String {
            calls += items to target
            if (failuresLeft > 0) {
                failuresLeft--
                throw IllegalStateException("设备离线")
            }
            return "job-1"
        }
    }

    private fun consumer(
        store: ShareQueueStore,
        nowMs: () -> Long = { 1_760_000_100_000L },
        send: suspend (List<BridgeItem>, String) -> String,
    ): ShareQueueConsumer = ShareQueueConsumer(queue = store, send = send, now = nowMs)

    // ── 队列写入→读出等价 + schema 钉面 ──────────────────────────────────

    @Test
    fun `入队读出往返 蛇形键 schema 逐字`() {
        val store = newStore()
        val nowMs = 1_760_000_000_000L
        val seeded = seedReady(store, id = "item-1", target = "dev-abc", nowMs = nowMs)

        val entries = store.scan(nowMs = nowMs + 1_000)
        assertEquals(1, entries.size)
        val entry = entries.first()
        assertEquals("item-1", entry.id)
        assertEquals("dev-abc", entry.meta.targetDeviceId)
        assertEquals("IMG_0421.HEIC", entry.meta.sourceName)
        assertEquals(10L, entry.meta.size)
        assertEquals(nowMs, entry.meta.enqueuedAt)
        assertEquals(ShareQueueStatus.READY, entry.meta.status)
        assertEquals(
            listOf(
                ShareQueueFile("IMG_0421.HEIC", 6),
                ShareQueueFile("年度报告.pdf", 4),
            ),
            entry.meta.files,
        )
        assertNull(entry.meta.error)
        // 两步拷贝落定形态:最终名在场、字节一致、无 .part 残留
        for ((file, content) in seeded) {
            val f = File(store.filesDir("item-1"), file.name)
            assertEquals(bytes(content).toList(), f.readBytes().toList())
            assertFalse(File(store.filesDir("item-1"), file.name + ".part").exists())
        }
        // meta.json 键面钉死(蛇形;与 iOS ShareQueueMeta schema 同语言)
        val meta = store.readMeta("item-1")
        assertNotNull(meta)
        val json = store.metaFile("item-1").readText()
        for (key in listOf(
            "target_device_id", "source_name", "size", "enqueued_at", "status", "files",
        )) {
            assertTrue("meta.json 应含键 $key:$json", "\"$key\":" in json)
        }
    }

    @Test
    fun `文件名净化`() {
        assertEquals("b_c.txt", ShareQueueStore.sanitizedName("a/b:c.txt"))
        assertEquals("ok.txt", ShareQueueStore.sanitizedName("ok.txt"))
        assertEquals("反斜_杠", ShareQueueStore.sanitizedName("反斜\\杠"))
        // 相对路径分量净化(暂存区目录穿越防)
        assertEquals("_", ShareQueueStore.sanitizedName("."))
        assertEquals("_", ShareQueueStore.sanitizedName(".."))
    }

    @Test
    fun `暂存件解析拒越名`() {
        val store = newStore()
        store.beginItem(
            id = "x", targetDeviceId = "dev-1",
            files = listOf(ShareQueueFile("a.bin", 1)), nowMs = 0,
        )
        // 合法名在场才解析
        assertNull(store.stagedFile("x", "a.bin"))
        store.copyStream("x", ShareQueueFile("a.bin", 1), ByteArrayInputStream(bytes("z")))
        assertNotNull(store.stagedFile("x", "a.bin"))
        // 越名/缺件拒(null = 源不可达;目录穿越防)
        assertNull(store.stagedFile("x", ".."))
        assertNull(store.stagedFile("x", "sub/a.bin"))
        assertNull(store.stagedFile("x", "missing.bin"))
        assertNull(store.stagedFile("no-such", "a.bin"))
    }

    @Test
    fun `meta 损坏按缺失收口`() {
        val store = newStore()
        write(File(store.itemDir("x"), "meta.json"), bytes("{not json"))
        assertNull(store.readMeta("x"))
        // scan:损坏 meta = 孤儿目录同判(折失败不静默丢)
        val entries = store.scan(nowMs = 1_000)
        assertEquals(1, entries.size)
        assertEquals(ShareQueueStatus.FAILED, entries[0].meta.status)
    }

    // ── Step1-① 拷贝中断(半件)不 finish ────────────────────────────────

    @Test
    fun `拷贝中断半件 宽限窗内不动 窗外折失败`() {
        val store = newStore()
        val nowMs = 1_760_000_000_000L
        val files = listOf(
            ShareQueueFile(name = "done.bin", size = 4),
            ShareQueueFile(name = "killed.bin", size = 9),
        )
        store.beginItem(id = "kill-1", targetDeviceId = "dev-1", files = files, nowMs = nowMs)
        // 文件一完整(拷完 rename);文件二只落 .part 临时名(拷一半进程被杀)
        store.copyStream("kill-1", files[0], ByteArrayInputStream(bytes("abcd")))
        write(File(store.filesDir("kill-1"), "killed.bin.part"), bytes("12345"))

        // 宽限窗内:扩展面(分享 Activity)可能仍在拷——不动(不 finish 不失败)
        val young = store.scan(nowMs = nowMs + 1_000)
        assertEquals(1, young.size)
        assertEquals(ShareQueueStatus.COPYING, young[0].meta.status)
        // 半件不 finish:最终名不在场(只有 .part 临时名)
        assertFalse(File(store.filesDir("kill-1"), "killed.bin").exists())

        // 宽限窗外(进程已死,主 App 下次启动)→ failed「文件拷贝中断」,不静默丢
        val stale = store.scan(nowMs = nowMs + ShareQueueStore.DEFAULT_COPY_GRACE_MS + 1_000)
        assertEquals(1, stale.size)
        assertEquals(ShareQueueStatus.FAILED, stale[0].meta.status)
        assertEquals(ShareQueueConsumer.ERROR_COPY_INTERRUPTED, stale[0].meta.error)
        // 折算已持久化(失败行跨会话不丢)
        assertEquals(ShareQueueStatus.FAILED, store.readMeta("kill-1")?.status)
    }

    @Test
    fun `孤儿目录折失败`() {
        val store = newStore()
        write(File(store.filesDir("orphan"), "stray.bin"), bytes("x"))
        val entries = store.scan(nowMs = 1_000)
        assertEquals(1, entries.size)
        assertEquals("orphan", entries[0].id)
        assertEquals(ShareQueueStatus.FAILED, entries[0].meta.status)
        assertEquals(ShareQueueConsumer.ERROR_COPY_INTERRUPTED, entries[0].meta.error)
    }

    // ── Step1-② 盘损截短被尺寸守卫拒 ─────────────────────────────────────

    @Test
    fun `盘损截短拒发 文件可能已损坏`() = runTest {
        val store = newStore()
        seedReady(store, id = "m-1", target = "dev-1")
        // 完成后盘上字节被截(盘损/篡改形态)——消费端逐文件复验须拦下
        val target = File(store.filesDir("m-1"), "IMG_0421.HEIC")
        target.writeBytes(bytes("phot1"))

        val send = FakeSend()
        val outcome = consumer(store) { i, t -> send.send(i, t) }.consumeOnce(loggedIn = true)
        assertEquals(0, outcome.consumed)
        assertEquals(0, send.calls.size) // 校验不过不发(坏字节不上线)
        val entry = store.scan(nowMs = 1).single()
        assertEquals(ShareQueueStatus.FAILED, entry.meta.status)
        assertEquals(ShareQueueConsumer.ERROR_SIZE_MISMATCH, entry.meta.error)
    }

    @Test
    fun `缺件同拒`() = runTest {
        val store = newStore()
        val nowMs = 1_760_000_000_000L
        store.beginItem(
            id = "g-1", targetDeviceId = "dev-1",
            files = listOf(ShareQueueFile("gone.bin", 3)), nowMs = nowMs,
        )
        store.finishItem("g-1", nowMs) // 声明了文件但从未落盘

        val send = FakeSend()
        val outcome = consumer(store) { i, t -> send.send(i, t) }.consumeOnce(loggedIn = true)
        assertEquals(0, outcome.consumed)
        assertEquals(0, send.calls.size)
        assertEquals(ShareQueueConsumer.ERROR_SIZE_MISMATCH, store.scan(nowMs = 1).single().meta.error)
    }

    // ── Step1-③ 重试换目标设备 ───────────────────────────────────────────

    @Test
    fun `重试换目标设备重发`() = runTest {
        val store = newStore()
        seedReady(store, id = "item-1", target = "dev-1")
        val inboxCheck = File(store.filesDir("item-1"), "IMG_0421.HEIC").readBytes()

        val send = FakeSend().apply { failuresLeft = 1 }
        val c = consumer(store) { i, t -> send.send(i, t) }
        // 第一轮:发送失败(设备瞬断)→ failed「设备离线」;常规消费不自动重发
        assertEquals(0, c.consumeOnce(loggedIn = true).consumed)
        assertEquals(ShareQueueConsumer.errorText(IllegalStateException("设备离线")), store.scan(nowMs = 1).single().meta.error)
        // 暂存文件留队(重试不重拷)
        assertEquals(inboxCheck.toList(), File(store.filesDir("item-1"), "IMG_0421.HEIC").readBytes().toList())

        // 重试(重选目标):换 dev-2 重发;条目 ref = stage:// 暂存件形态
        assertTrue(c.retryOnce("item-1", overrideTargetDeviceId = "dev-2"))
        assertEquals(2, send.calls.size) // 首轮失败 + 重试各一次
        val call = send.calls.last()
        assertEquals("dev-2", call.second)
        assertEquals(listOf("stage://item-1/IMG_0421.HEIC", "stage://item-1/年度报告.pdf"), call.first.map { it.ref })
        assertEquals(listOf("IMG_0421.HEIC", "年度报告.pdf"), call.first.map { it.rel })
        // 成功即 sent(引擎 request_source 拉取期文件保暂存;清扫另有窗口)
        assertEquals(ShareQueueStatus.SENT, store.readMeta("item-1")?.status)
        assertTrue(File(store.filesDir("item-1"), "IMG_0421.HEIC").exists())
    }

    @Test
    fun `重试只属失败行`() = runTest {
        val store = newStore()
        seedReady(store, id = "item-1", target = "dev-1")
        val send = FakeSend()
        val c = consumer(store) { i, t -> send.send(i, t) }
        // ready 项不属重试面(消费面管);无此 id 同拒
        assertFalse(c.retryOnce("item-1", overrideTargetDeviceId = null))
        assertFalse(c.retryOnce("no-such", overrideTargetDeviceId = null))
        assertEquals(0, send.calls.size)
    }

    // ── Step1-④ 消费幂等(登录前不动队列)────────────────────────────────

    @Test
    fun `登录前不动队列`() = runTest {
        val store = newStore()
        seedReady(store, id = "item-1", target = "dev-1")
        val send = FakeSend()
        val outcome = consumer(store) { i, t -> send.send(i, t) }.consumeOnce(loggedIn = false)
        assertEquals(0, outcome.consumed)
        assertEquals(0, send.calls.size) // 登录前零发送
        // 等待态保真:队列原样(仍 ready,登录后照常消费)
        assertEquals(ShareQueueStatus.READY, store.readMeta("item-1")?.status)
    }

    @Test
    fun `消费幂等 sent 不重发`() = runTest {
        val store = newStore()
        seedReady(store, id = "item-1", target = "dev-1")
        val send = FakeSend()
        val c = consumer(store) { i, t -> send.send(i, t) }
        assertEquals(1, c.consumeOnce(loggedIn = true).consumed)
        assertEquals(1, send.calls.size)
        // 第二轮:sent 项不再进发送面(受理成功即出队;进度归引擎任务列表)
        assertEquals(0, c.consumeOnce(loggedIn = true).consumed)
        assertEquals(1, send.calls.size)
    }

    // ── sent 清扫(暂存件生命周期)─────────────────────────────────────────

    @Test
    fun `sent 项按 sent_at 超窗清扫 窗内保留`() {
        val store = newStore()
        val enqueuedMs = 1_760_000_000_000L
        val sentMs = enqueuedMs + 2_000L // 受理刻 = 清扫锚(评审 fix-1)
        seedReady(store, id = "item-1", target = "dev-1", nowMs = enqueuedMs)
        store.markSent("item-1", sentMs)
        // 窗内(sent_at 锚):保留(request_source 拉取期文件必须在场)
        assertEquals(1, store.scan(nowMs = sentMs + 1_000).size)
        assertTrue(File(store.filesDir("item-1"), "IMG_0421.HEIC").exists())
        // 超窗(锚 sent_at):整项清扫(暂存目录有界;数据已归引擎任务/对端管)
        store.scan(nowMs = sentMs + ShareQueueStore.DEFAULT_SENT_SWEEP_MS + 1_000)
        assertFalse(store.itemDir("item-1").exists())
    }

    @Test
    fun `休眠超窗受理后暂存件保真`() = runTest {
        // 评审 fix-1 钉面:入队 >24h 休眠(登出/进程死亡)→ 回来登录消费 →
        // 受理即随 scan 刷行——清扫若锚 enqueuedAt 此刻必误删拉源期暂存件;
        // 锚 sent_at 则保真。
        val store = newStore()
        val acceptMs = 1_760_000_100_000L // 消费面 now()(= 受理刻,sent_at 源)
        val enqueuedMs = acceptMs - ShareQueueStore.DEFAULT_SENT_SWEEP_MS - 3_600_000L
        seedReady(store, id = "item-1", target = "dev-1", nowMs = enqueuedMs)
        val send = FakeSend()
        assertEquals(1, consumer(store) { i, t -> send.send(i, t) }.consumeOnce(loggedIn = true).consumed)
        assertEquals(ShareQueueStatus.SENT, store.readMeta("item-1")?.status)
        assertEquals(acceptMs, store.readMeta("item-1")?.sentAt) // sent_at = 受理刻
        // 受理后多轮 scan(失败行刷新即走此路):暂存件必须在场(引擎拉源期)
        store.scan(nowMs = acceptMs + 1_000)
        store.scan(nowMs = acceptMs + 30_000) // 覆盖 25–35s 冷对端握手窗
        assertTrue(File(store.filesDir("item-1"), "IMG_0421.HEIC").exists())
        assertTrue(File(store.filesDir("item-1"), "年度报告.pdf").exists())
    }

    @Test
    fun `空条目行重试让路原因不覆写`() = runTest {
        // 孤儿折算行(无 meta → 折失败,files 空):重试不进发送面(「发送
        // (空条目)」错误不覆写「文件拷贝中断」);原失败行原样保留
        val store = newStore()
        write(File(store.filesDir("orphan"), "stray.bin"), bytes("x"))
        store.scan(nowMs = 1_000) // 折算落盘
        val send = FakeSend()
        val c = consumer(store) { i, t -> send.send(i, t) }
        assertFalse(c.retryOnce("orphan", overrideTargetDeviceId = "dev-2"))
        assertEquals(0, send.calls.size)
        val entry = store.scan(nowMs = 2_000).single()
        assertEquals(ShareQueueConsumer.ERROR_COPY_INTERRUPTED, entry.meta.error)
    }

    // ── 入队尺寸守卫(拷中校验,源面改短即拒)────────────────────────────

    @Test
    fun `拷贝流尺寸不符拒收`() {
        val store = newStore()
        val nowMs = 1_760_000_000_000L
        store.beginItem(
            id = "s-1", targetDeviceId = "dev-1",
            files = listOf(ShareQueueFile("a.bin", 10)), nowMs = nowMs,
        )
        try {
            store.copyStream("s-1", ShareQueueFile("a.bin", 10), ByteArrayInputStream(bytes("12345")))
            throw AssertionError("尺寸不符必须拒收")
        } catch (e: ShareQueueSizeMismatchException) {
            assertEquals("a.bin", e.name)
            assertEquals(10L, e.declared)
            assertEquals(5L, e.actual)
        }
        // 拒收后无 .part 残留(坏件不入队)
        assertFalse(File(store.filesDir("s-1"), "a.bin.part").exists())
    }

    // ── stage ref 编解码(SourceRefs 契约面)──────────────────────────────

    @Test
    fun `stageRef 编解码往返与拒畸形`() {
        assertEquals("stage://item-1/a.bin", com.mtdss.android.send.SourceRefs.stageRef("item-1", "a.bin"))
        val parsed = com.mtdss.android.send.SourceRefs.parse("stage://item-1/a.bin")
        assertEquals(com.mtdss.android.send.SourceRef.Staged("item-1", "a.bin"), parsed)
        // 畸形:无 itemId / name 带分隔 / 相对分量(防目录穿越,id 与 name 同守卫)
        assertNull(com.mtdss.android.send.SourceRefs.parse("stage://"))
        assertNull(com.mtdss.android.send.SourceRefs.parse("stage://only-item"))
        assertNull(com.mtdss.android.send.SourceRefs.parse("stage://../evil"))
        assertNull(com.mtdss.android.send.SourceRefs.parse("stage://item-1/../evil"))
        assertNull(com.mtdss.android.send.SourceRefs.parse("stage://item-1/.."))
        assertNull(com.mtdss.android.send.SourceRefs.parse("stage://item-1/a/b"))
    }

    // ── 失败行投影(ShareFailureController;iOS ShareCenter.refreshRows 同构)──

    @Test
    fun `失败行投影与重试清除`() = runTest {
        val store = newStore()
        val send = FakeSend().apply { failuresLeft = 1 }
        val loggedIn = MutableStateFlow<Boolean?>(null)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val controller = ShareFailureController(
                store = store,
                loggedIn = loggedIn,
                scope = scope,
                send = { i, t -> send.send(i, t) },
                now = { 1_760_000_100_000L },
            )
            seedReady(store, id = "item-1", target = "dev-1", nowMs = 1_760_000_000_000L)
            // 登录翻转触发消费(首值 true = 冷启动已登录也消费);首轮发送失败
            loggedIn.value = true
            awaitUntil(state = { controller.rows().value.toString() }) {
                controller.rows().value.isNotEmpty()
            }
            assertEquals(
                listOf(
                    ShareFailureRow(
                        id = "item-1", name = "IMG_0421.HEIC",
                        filesTotal = 2, reason = "设备离线",
                    ),
                ),
                controller.rows().value,
            )

            // 重试 = 重选设备:请求流出键 → retryWith 换目标重发 → 行消失
            controller.onRetry("item-1")
            assertEquals("item-1", controller.retryRequest().value)
            controller.retryWith("item-1", "dev-2")
            awaitUntil { controller.rows().value.isEmpty() }
            assertNull(controller.retryRequest().value)
            assertEquals("dev-2", send.calls.last().second)
            assertEquals(ShareQueueStatus.SENT, store.readMeta("item-1")?.status)

            // 清除:队列项整移除(失败行源零残留)
            store.beginItem(
                id = "gone", targetDeviceId = "dev-1",
                files = listOf(ShareQueueFile("x.bin", 1)), nowMs = 1_000,
            )
            store.markFailed("gone", "文件拷贝中断")
            controller.refreshRows()
            awaitUntil { controller.rows().value.isNotEmpty() }
            controller.onClear("gone")
            awaitUntil { controller.rows().value.isEmpty() }
            assertFalse(store.itemDir("gone").exists())
        } finally {
            scope.cancel()
        }
    }

    @Test
    fun `登录前消费触发不动队列`() = runTest {
        val store = newStore()
        seedReady(store, id = "item-1", target = "dev-1")
        val send = FakeSend()
        val loggedIn = MutableStateFlow<Boolean?>(false)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            val controller = ShareFailureController(
                store = store, loggedIn = loggedIn, scope = scope,
                send = { i, t -> send.send(i, t) },
            )
            controller.refresh()
            awaitUntil { controller.rows().value.isEmpty() } // 刷新跑过一轮
            assertEquals(0, send.calls.size) // 登录前零发送
            assertEquals(ShareQueueStatus.READY, store.readMeta("item-1")?.status)
        } finally {
            scope.cancel()
        }
    }
}
