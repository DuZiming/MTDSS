package com.mtdss.android.ui.tasks

import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.send.BridgeLimits
import com.mtdss.android.send.DocMeta
import com.mtdss.android.send.FakeSafFace
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.ui.tasks.ReinitiateRouter.Plan
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 重新发起路由 JVM 单测:分臂路由(bridge 上传 → App 侧重建;download/
 * remote_copy/路径形态 upload → daemon op)、ref 重建矩阵(授权根/单文档/
 * 同名撞号/源失效全形态)、dst 解析、条目数护栏。
 */
class ReinitiateRouterTest {

    private val root = StoredRoot(
        rootId = "r1",
        treeUri = "content://tree/r1",
        treeDocId = "doc-r1",
        displayName = "授权根",
    )

    private val saf = FakeSafFace()

    private fun setUpTreeFile(rel: String, size: Long, mtime: Long) {
        // 授权根子文档 URI = FakeSafFace.buildChildUri(treeUri, childDocumentId)
        val childUri = saf.buildChildUri(root.treeUri, "${root.treeDocId}/$rel")
        saf.metas[childUri] = DocMeta(rel.substringAfterLast('/'), size, mtime)
    }

    private fun job(
        kind: String,
        src: String?,
        dst: String? = "dev-pc:/golden/peer-inbox",
        state: String = "failed",
    ) = TransferJob(
        id = "b1", kind = kind, name = "样本", state = state,
        filesDone = 0, filesTotal = 1, bytesDone = 0, bytesTotal = 100,
        speedBps = 0, skipped = 0, relay = false, src = src, dst = dst, error = null,
    )

    private fun plan(job: TransferJob, roots: List<StoredRoot> = listOf(root)): Plan =
        ReinitiateRouter.plan(job, RootLookup { roots }, saf)

    // ── 分臂路由 ────────────────────────────────────────────────────────────

    @Test
    fun `bridge 上传走 App 侧重建`() {
        setUpTreeFile("docs/a.bin", size = 100, mtime = 555)
        saf.metas["content://pick/x.bin"] = DocMeta("x.bin", 200, 666)

        val p = plan(
            job("upload", src = "bridge://roottree://r1/docs/a.bin, bridge://content://pick/x.bin"),
        )
        assertTrue(p is Plan.Rebuild)
        p as Plan.Rebuild
        assertEquals("dev-pc", p.to)
        assertEquals("/golden/peer-inbox", p.dir)
        // 授权根条目:rel 逐字取自 ref(断点续传的落位锚);单文档条目:rel = 显示名
        assertEquals(
            listOf(
                com.mtdss.android.ipc.BridgeItem("roottree://r1/docs/a.bin", "docs/a.bin", 100, 555),
                com.mtdss.android.ipc.BridgeItem("content://pick/x.bin", "x.bin", 200, 666),
            ),
            p.items,
        )
    }

    @Test
    fun `download 与 remote_copy 走 daemon op`() {
        assertEquals(Plan.Daemon("b1"), plan(job("download", src = "dev-pc:/远端/文档.pdf")))
        assertEquals(
            Plan.Daemon("b1"),
            plan(job("remote_copy", src = "dev-a:/s", dst = "dev-b:/d")),
        )
    }

    @Test
    fun `路径形态上传走 daemon op(非本 App 发起的防御面)`() {
        assertEquals(Plan.Daemon("b1"), plan(job("upload", src = "C:/a.bin, C:/b.bin")))
        assertEquals(Plan.Daemon("b1"), plan(job("upload", src = null)))
        assertEquals(Plan.Daemon("b1"), plan(job("upload", src = "")))
    }

    @Test
    fun `未知 kind 拒绝`() {
        val p = plan(job("weird", src = "bridge://x"))
        assertTrue(p is Plan.Reject)
        assertTrue((p as Plan.Reject).reason.contains("任务种类不可重新发起"))
    }

    // ── 源失效矩阵(spec 评审裁定③:「源已不存在」词面)─────────────────────

    @Test
    fun `分享件授权已失 → 源已不存在`() {
        // content:// URI 可解析但元数据查询失败(SecurityException → null)
        val p = plan(job("upload", src = "bridge://content://share/tmp.bin"))
        assertTrue(p is Plan.Reject)
        assertTrue((p as Plan.Reject).reason.startsWith("源已不存在"))
    }

    @Test
    fun `授权根被移除 → 源已不存在`() {
        val p = plan(
            job("upload", src = "bridge://roottree://rGone/a.bin"),
            roots = emptyList(),
        )
        assertTrue(p is Plan.Reject)
        val reason = (p as Plan.Reject).reason
        assertTrue(reason.startsWith("源已不存在"))
        assertTrue(reason.contains("授权根已移除"))
    }

    @Test
    fun `树内文件被删 → 源已不存在`() {
        // 根仍在,子文档元数据查不到(文档消失/授权被撤)
        val p = plan(job("upload", src = "bridge://roottree://r1/gone.bin"))
        assertTrue(p is Plan.Reject)
        assertTrue((p as Plan.Reject).reason.startsWith("源已不存在"))
    }

    @Test
    fun `malformed ref → 源已不存在`() {
        val p = plan(job("upload", src = "bridge://roottree://noRel"))
        assertTrue(p is Plan.Reject)
        assertTrue((p as Plan.Reject).reason.startsWith("源已不存在"))
    }

    @Test
    fun `多文件批次任一源失效整批拒`() {
        setUpTreeFile("a.bin", 1, 1)
        // 第二个 ref 的元数据缺失
        val p = plan(job("upload", src = "bridge://roottree://r1/a.bin, bridge://content://dead.bin"))
        assertTrue(p is Plan.Reject) // 不做半批重发(隐性丢件)
    }

    // ── 重建细节 ────────────────────────────────────────────────────────────

    @Test
    fun `单文档同名撞号加序号(与 PickerBridge 方案一致)`() {
        saf.metas["content://p/1"] = DocMeta("x.bin", 10, 1)
        saf.metas["content://p/2"] = DocMeta("x.bin", 20, 2)
        val p = plan(job("upload", src = "bridge://content://p/1, bridge://content://p/2")) as Plan.Rebuild
        assertEquals(listOf("x.bin", "x.bin (1)"), p.items.map { it.rel })
    }

    @Test
    fun `条目数超限前置拒绝(词面与 BridgeLimits 一致)`() {
        repeat(BridgeLimits.MAX_BRIDGE_ITEMS + 1) { i ->
            saf.metas["content://p/$i"] = DocMeta("f$i.bin", 1, 1)
        }
        val src = (0..BridgeLimits.MAX_BRIDGE_ITEMS).joinToString(", ") { "bridge://content://p/$it" }
        val p = plan(job("upload", src = src))
        assertTrue(p is Plan.Reject)
        assertEquals(BridgeLimits.itemCountError(BridgeLimits.MAX_BRIDGE_ITEMS + 1), (p as Plan.Reject).reason)
    }

    @Test
    fun `dst 缺失或畸形拒绝`() {
        setUpTreeFile("a.bin", 1, 1)
        assertTrue(plan(job("upload", src = "bridge://roottree://r1/a.bin", dst = null)) is Plan.Reject)
        assertTrue(plan(job("upload", src = "bridge://roottree://r1/a.bin", dst = "")) is Plan.Reject)
        assertTrue(plan(job("upload", src = "bridge://roottree://r1/a.bin", dst = "dev-pc:")) is Plan.Reject)
        assertTrue(plan(job("upload", src = "bridge://roottree://r1/a.bin", dst = ":/d")) is Plan.Reject)
        // 正常形态:首个 ':' 切分(dir 内含 ':' 不受影响)
        val ok = plan(job("upload", src = "bridge://roottree://r1/a.bin", dst = "dev-pc:C:/x:y")) as Plan.Rebuild
        assertEquals("dev-pc", ok.to)
        assertEquals("C:/x:y", ok.dir)
    }

    // ── bridgeRefs 解析 ─────────────────────────────────────────────────────

    @Test
    fun `bridgeRefs 形态判定`() {
        assertEquals(
            listOf("a", "b"),
            ReinitiateRouter.bridgeRefs("bridge://a, bridge://b"),
        )
        assertNull(ReinitiateRouter.bridgeRefs(null))
        assertNull(ReinitiateRouter.bridgeRefs("  "))
        assertNull(ReinitiateRouter.bridgeRefs("C:/a, C:/b")) // 路径形态
        assertNull(ReinitiateRouter.bridgeRefs("bridge://a, C:/b")) // 混形
        assertNull(ReinitiateRouter.bridgeRefs("bridge://")) // 空 ref
    }
}
