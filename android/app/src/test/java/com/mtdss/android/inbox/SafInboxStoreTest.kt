package com.mtdss.android.inbox

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * SAF 授权树发布决策矩阵 JVM 单测(P4a-r2,spec §7;伪件 FakeDocumentsFace
 * ——DocumentsContract 在 JVM 不可伪,收进缝后编排在此钉死):授权归一化
 * (根=MTDSS/根=Download 自建)、发布编排(.mtdss 隐藏目录点文件临时名 →
 * 拷贝 → rename 落正式名)、rename 碰撞词面、policy skip/overwrite/驱逐、
 * 拷贝与改名失败删临时、在场查询矩阵、同 rel 残留清扫(成功终局清同指纹,
 * 异 rel/旧格式不溯及——用户裁定 2026-10-28)。真实 provider 行为(含
 * MIUI 点名改写 `_.mtdss` 形态、rename 的 "(N)" 自动改名)实机验证
 * (spec §7.7)。
 */
class SafInboxStoreTest {

    /** 测试台:伪 SAF 树 + 授权账本 + 伪拷贝缝(记录调用/注入失败)。 */
    private class Env(
        rootName: String = "Download",
        granted: Boolean = true,
        build: FakeDocumentsFace.Node.() -> Unit = {},
    ) {
        val face = FakeDocumentsFace(rootName).apply { build(root) }
        val grant = InboxGrantStore(MemInboxGrantPersistence(), face)
        val copyCalls = mutableListOf<Triple<String, String, Long>>() // (docId, stagingPath, size)
        var copyFail: RuntimeException? = null
        val log = mutableListOf<String>()
        val store: SafInboxStore = SafInboxStore(face, grant, { log += it }) { _, docId, path, size ->
            copyFail?.let { throw it }
            copyCalls += Triple(docId, path, size)
        }

        init {
            if (granted) checkNotNull(grant.grant(face.treeUri))
        }
    }

    // ── 授权归一化(spec §7.2)──────────────────────────────────────────────

    @Test
    fun `授权根为 Download 树内自建 MTDSS 与点 mtdss`() {
        val e = Env()
        assertEquals(PublishResult.Published, e.store.publish("a.bin", "/staging/a.bin", 8, TransferPolicy()))
        val rootId = e.face.docIdAt("")!!
        val mtssId = e.face.docIdAt("MTDSS")!!
        assertEquals(listOf(rootId to "MTDSS", mtssId to ".mtdss"), e.face.createdDirs)
    }

    @Test
    fun `授权根即 MTDSS 直接用根 不再造 MTDSS`() {
        val e = Env(rootName = "MTDSS")
        assertEquals(PublishResult.Published, e.store.publish("a.bin", "/staging/a.bin", 8, TransferPolicy()))
        val rootId = e.face.docIdAt("")!!
        assertEquals("隐藏目录建在根内", listOf(rootId to ".mtdss"), e.face.createdDirs)
        assertNull("根内不得再嵌 MTDSS", e.face.root["MTDSS"])
    }

    @Test
    fun `MTDSS 已存在 复用不自建`() {
        val e = Env(build = { dir("MTDSS") })
        assertEquals(PublishResult.Published, e.store.publish("a.bin", "/staging/a.bin", 8, TransferPolicy()))
        assertEquals("只建 .mtdss 一处", listOf(e.face.docIdAt("MTDSS")!! to ".mtdss"), e.face.createdDirs)
    }

    @Test
    fun `MIUI 改写形下划线 mtdss 命中 不重复建目录`() {
        // spec §7.7:MIUI 若把点名改写为 `_.mtdss`,定位须双形态兼容——
        // 否则每次发布都新建一个暂存目录
        val e = Env(build = { dir("MTDSS") { dir("_.mtdss") } })
        assertEquals(PublishResult.Published, e.store.publish("a.bin", "/staging/a.bin", 8, TransferPolicy()))
        assertTrue("不得再建 .mtdss", e.face.createdDirs.none { it.second == ".mtdss" })
        assertEquals("临时文件落既有改写形目录", e.face.docIdAt("MTDSS/_.mtdss")!!, e.face.createdDocs.single().first)
    }

    @Test
    fun `目录创建竞争败者回查复用 create-or-get`() {
        // join_all 并发发布:两协程同见目录缺席 → 各自 createDirectory →
        // 败者返 null 但对手已建(目录 bootstrap 的 TOCTOU)——回查一次
        // 复用既有,不伪 Failed(§7.2 幂等;评审 Important-1)
        val e = Env().apply { face.createDirLosesRace = true }
        val r = e.store.publish("docs/a.bin", "/staging/a.bin", 8, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertTrue("终件落位(三处 create 败臂全自愈:MTDSS/docs/.mtdss)", e.face.nodeAt("MTDSS/docs/a.bin") != null)
        assertEquals("各目录恰建一次,败臂不重复建", 3, e.face.createdDirs.size)
    }

    // ── 发布编排(spec §7.3)────────────────────────────────────────────────

    @Test
    fun `发布成功 临时落隐藏目录 拷贝改名落终名`() {
        val e = Env()
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        // 临时文档:落在 MTDSS/.mtdss 内,点文件临时名,MIME 按名兜(JVM 桩
        // 面无 MimeTypeMap → octet-stream)
        val stagingId = e.face.docIdAt("MTDSS/.mtdss")!!
        val (parent, name, mime) = e.face.createdDocs.single()
        assertEquals(stagingId, parent)
        assertTrue("临时名形态:$name", name.matches(TEMP_NAME_SHAPE))
        assertEquals("application/octet-stream", mime)
        // 拷贝:临时 docId + 暂存路径 + 期望尺寸原样
        val finalId = e.face.docIdAt("MTDSS/small.txt")!! // 同 id move/rename:终件即临时件
        assertEquals(listOf(Triple(finalId, "/staging/small.txt", 2025L)), e.copyCalls)
        // 移位:临时件(仍以临时名)从 .mtdss 挪入落位父目录
        assertEquals(
            listOf(Triple(finalId, stagingId, e.face.docIdAt("MTDSS")!!)),
            e.face.movedCalls,
        )
        // 改名落请求终名;无删除(无撞名 + 同 id 变更无旧件清理)
        assertEquals(listOf(finalId to "small.txt"), e.face.renamedCalls)
        assertTrue(e.face.deleted.isEmpty())
        assertEquals("成功后隐藏目录零残留", 0, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
        assertTrue("终件真实落位", e.face.nodeAt("MTDSS/small.txt") != null)
    }

    @Test
    fun `临时名恒点文件 uuid 形态且每次新`() {
        val e = Env()
        e.store.publish("a.bin", "/staging/a", 1, TransferPolicy())
        e.store.publish("b.bin", "/staging/b", 2, TransferPolicy())
        val names = e.face.createdDocs.map { it.second }
        assertEquals(2, names.size)
        names.forEach { assertTrue("临时名须 dot+8hex+.tmp 形态:$it", it.matches(TEMP_NAME_SHAPE)) }
        assertNotEquals("uuid 每次新(残留件恒异名,不成碰撞源)", names[0], names[1])
    }

    @Test
    fun `rename 碰撞 实际名词面 Failed 文件保留`() {
        // 终名路径被强杀残留等占用而 provider 自动改 "(N)":内容已完整落盘
        // 于实际名——诚实告知词面,改名后的正式文件保留(= 行保留等价物)
        val e = Env().apply { face.renameCollisionTo = "small (1).txt" }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须点名碰撞:${failed.reason}", failed.reason.contains("目标名被残留文件占用"))
        assertTrue("词面须带实际名:${failed.reason}", failed.reason.contains("small (1).txt"))
        assertTrue("词面须告知手动改名出口:${failed.reason}", failed.reason.contains("文件管理器手动改名"))
        assertEquals("拷贝已走到(内容安全落盘)", 1, e.copyCalls.size)
        assertTrue("碰撞件保留", e.face.nodeAt("MTDSS/small (1).txt") != null)
        assertTrue(e.face.deleted.isEmpty())
    }

    @Test
    fun `rename 返回新 id 时旧临时显式清理且触发扫描`() {
        // 路径形 provider 改名即换 docId(移动语义):旧 id 若仍活(复制式
        // 改名 provider)须显式删;移动式则删除无害失败。新 id 为路径形时
        // 推导扫描路径触发索引(spec §7.6)
        val e = Env().apply { face.renameDocIdOverride = "primary:Download/MTDSS/small.txt" }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertEquals("旧临时清理恰一次", 1, e.face.deleted.size)
        assertTrue("终件仍在(旧 id 删除无害失败)", e.face.nodeAt("MTDSS/small.txt") != null)
        assertEquals(listOf("/storage/emulated/0/Download/MTDSS/small.txt"), e.face.scanned)
    }

    @Test
    fun `move 返新 id 且 rename 保 id 旧临时仍被清理`() {
        // 复制式 move provider:move 换 id、rename 保 id——rename 旧 id
        // 清理不触发,move 旧 id(.mtdss 内原临时件)须独立清,不留残件
        // (评审 Minor-2)
        val e = Env().apply { face.moveDocIdOverride = "d-move-new" }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertEquals("move 旧临时恰清一次", 1, e.face.deleted.size)
        assertTrue("终件真实落位", e.face.nodeAt("MTDSS/small.txt") != null)
        assertEquals("隐藏目录零残留", 0, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
    }

    @Test
    fun `拷贝失败删临时 收口 Failed`() {
        val e = Env().apply { copyFail = RuntimeException("EIO: 磁盘满") }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须带底层因:${failed.reason}", failed.reason.contains("发布拷贝失败"))
        assertTrue(failed.reason.contains("磁盘满"))
        assertEquals("临时必删(SAF 全权)", 1, e.face.deleted.size)
        assertEquals("隐藏目录内零残留", 0, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
        assertTrue("失败不得落终名", e.face.renamedCalls.isEmpty())
    }

    @Test
    fun `移位失败删临时 收口 Failed`() {
        // moveDocument 失败(provider 不支持/目标父目录消失):临时仍在
        // 隐藏目录内,SAF 全权删之——零残留
        val e = Env().apply { face.failMove = true }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("发布落位失败"))
        assertEquals(1, e.face.movedCalls.size)
        assertEquals("临时必删", 1, e.face.deleted.size)
        assertEquals("隐藏目录内零残留", 0, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
        assertTrue("不得改名", e.face.renamedCalls.isEmpty())
    }

    @Test
    fun `改名失败删件 收口 Failed`() {
        // 改名义失败时件已在落位父目录内(点文件临时名):删除收口,不留
        // 可见区残留;旧临时 id(移动式已失效)一并尽力清
        val e = Env().apply { face.failRename = true }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue(failed.reason.contains("发布改名失败"))
        assertEquals(1, e.face.renamedCalls.size)
        assertEquals("移位后的件必删", 1, e.face.deleted.size)
        assertNull("失败不得落终名", e.face.nodeAt("MTDSS/small.txt"))
        assertEquals(0, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
    }

    @Test
    fun `临时文件创建失败带因 Failed`() {
        val e = Env().apply { face.failCreateDoc = true }
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertTrue((r as PublishResult.Failed).reason.contains("临时文件创建失败"))
        assertTrue(e.face.deleted.isEmpty()) // 无临时可清
        assertTrue(e.copyCalls.isEmpty())
    }

    @Test
    fun `嵌套 rel 目录段自建 结构保真`() {
        val e = Env()
        val r = e.store.publish("docs/sub/a.bin", "/staging/x", 1, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        val mtssId = e.face.docIdAt("MTDSS")!!
        val docsId = e.face.docIdAt("MTDSS/docs")!!
        assertEquals(
            listOf(e.face.docIdAt("")!! to "MTDSS", mtssId to "docs", docsId to "sub", mtssId to ".mtdss"),
            e.face.createdDirs,
        )
        assertTrue("终件落嵌套位", e.face.nodeAt("MTDSS/docs/sub/a.bin") != null)
    }

    // ── 授权闸与消毒 ────────────────────────────────────────────────────────

    @Test
    fun `无授权 Failed 词面指向授权入口`() {
        val e = Env(granted = false)
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        val failed = r as PublishResult.Failed
        assertTrue("词面须点名未授权:${failed.reason}", failed.reason.contains("收件箱未授权"))
        assertTrue(failed.reason.contains("目录授权"))
        assertTrue("不触任何树操作", e.face.createdDirs.isEmpty())
        assertTrue(e.face.createdDocs.isEmpty())
        assertTrue(e.copyCalls.isEmpty())
    }

    @Test
    fun `非法 rel 拒发布 不触面`() {
        val e = Env()
        val r = e.store.publish("../bad", "/staging/x", 1, TransferPolicy())
        assertTrue((r as PublishResult.Failed).reason.contains("非法相对路径"))
        assertTrue(e.face.createdDirs.isEmpty() && e.face.createdDocs.isEmpty() && e.face.deleted.isEmpty())
    }

    @Test
    fun `收件根归一化不可判时诚实失败`() {
        // 根名查询失败:盲建会在 MTDSS 根内再造 MTDSS/MTDSS 层级——收口失败
        val e = Env().apply { face.failDocumentName = true }
        val r = e.store.publish("a.bin", "/staging/a", 1, TransferPolicy())
        assertTrue((r as PublishResult.Failed).reason.contains("收件箱根定位失败"))
        assertTrue(e.face.createdDirs.isEmpty())
    }

    // ── 撞名沿 policy(spec §7.5)───────────────────────────────────────────

    @Test
    fun `撞名真文件在场 skip 命中弃件记 Skipped`() {
        val e = Env(build = { dir("MTDSS") { file("small.txt", 42) } })
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy(skip = listOf("small.txt")))
        assertEquals(PublishResult.Skipped, r)
        assertEquals("既有件保留不动", 42L, e.face.nodeAt("MTDSS/small.txt")!!.size)
        assertTrue("不得建临时", e.face.createdDocs.isEmpty())
        assertTrue("不得拷贝", e.copyCalls.isEmpty())
    }

    @Test
    fun `撞名真文件在场 缺省策略驱逐后发布`() {
        val e = Env(build = { dir("MTDSS") { file("small.txt", 42) } })
        val oldId = e.face.docIdAt("MTDSS/small.txt")!!
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals(PublishResult.Published, r)
        assertEquals("驱逐在场件", listOf(oldId), e.face.deleted)
        assertEquals(1, e.copyCalls.size)
    }

    @Test
    fun `撞名真文件在场 overwrite 命中驱逐后发布`() {
        val e = Env(build = { dir("MTDSS") { file("small.txt", 42) } })
        val oldId = e.face.docIdAt("MTDSS/small.txt")!!
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy(overwrite = listOf("small.txt")))
        assertEquals(PublishResult.Published, r)
        assertEquals(listOf(oldId), e.face.deleted)
    }

    @Test
    fun `驱逐失败收口 Failed 不落临时`() {
        val e = Env(build = { dir("MTDSS") { file("small.txt", 42) } }, )
        e.face.failDelete = true
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertTrue((r as PublishResult.Failed).reason.contains("落位驱逐失败"))
        assertTrue("既有件未被盲删(删除失败=保留)", e.face.nodeAt("MTDSS/small.txt") != null)
        assertTrue(e.face.createdDocs.isEmpty())
    }

    @Test
    fun `同名目录占用 拒发布且绝不递归删`() {
        val e = Env(build = { dir("MTDSS") { dir("small.txt") { file("内件.bin", 1) } } })
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertTrue((r as PublishResult.Failed).reason.contains("同名目录占用"))
        assertTrue("目录与其内容原样保留", e.face.nodeAt("MTDSS/small.txt/内件.bin") != null)
        assertTrue(e.face.deleted.isEmpty())
        assertTrue(e.face.createdDocs.isEmpty())
    }

    // ── 在场查询矩阵(spec §7.4)────────────────────────────────────────────

    @Test
    fun `在场查询矩阵 在且尺寸符才真`() {
        val e = Env(build = {
            dir("MTDSS") {
                file("a.bin", 100)
                dir("docs") { file("n.bin", 5) }
                dir("d.bin")
            }
        })
        assertTrue("在 + 尺寸符", e.store.exists("a.bin", 100))
        assertFalse("在 + 尺寸不符", e.store.exists("a.bin", 101))
        assertFalse("不在", e.store.exists("b.bin", 100))
        assertTrue("嵌套在 + 尺寸符", e.store.exists("docs/n.bin", 5))
        assertFalse("嵌套终件不在", e.store.exists("docs/miss.bin", 5))
        assertFalse("同名目录不算在场", e.store.exists("d.bin", 0))
        assertFalse("消毒不过", e.store.exists(".mtdss/x", 1))
        // 下钻断链(children → null)→ false(F10 铁律:宁可重传)
        e.face.failChildrenOf += e.face.docIdAt("MTDSS")!!
        assertFalse(e.store.exists("docs/n.bin", 5))
    }

    @Test
    fun `在场查询未授权与收件根未建皆 false 且不建目录`() {
        val ungranted = Env(granted = false)
        assertFalse(ungranted.store.exists("a.bin", 1))
        // MTDSS 未落位 = 一切终件均不在场;纯查询不得自建目录
        val empty = Env()
        assertFalse(empty.store.exists("a.bin", 1))
        assertTrue("查询不得自建收件根", empty.face.createdDirs.isEmpty())
    }

    // ── 扫描路径推导(spec §7.6,纯函数)────────────────────────────────────

    @Test
    fun `扫描路径推导 主卷路径形 才可推导`() {
        assertEquals(
            "/storage/emulated/0/Download/MTDSS/docs/a.bin",
            SafInboxStore.scanPathOf("primary:Download/MTDSS/docs/a.bin"),
        )
        assertNull("纯数字 docId(DownloadsProvider)推导不出", SafInboxStore.scanPathOf("12345"))
        assertNull("非主卷(SD 卡等)推导不出,交扫描器自然索引", SafInboxStore.scanPathOf("other:Download/a.bin"))
    }

    // ── 同 rel 中间态清扫(P4a-r2 收口:任务归位语义,用户裁定)────────────

    @Test
    fun `发布成功清同指纹残留`() {
        // 用户裁定语义:中间态按 rel 归属——发布某 rel 成功时,该 rel 被杀
        // 发布留下的同指纹历史残留随成功消失(杀于发布→重发完成→残留清)
        val fp = SafInboxStore.relFingerprint("small.txt")
        val e = Env(build = {
            dir("MTDSS") {
                dir(".mtdss") {
                    file(".mtdss-pub-$fp-dead00.tmp", 5)
                    // MIUI 扩展名改写形(provider 按 MIME 追加 .txt,p9 实机观察)
                    file(".mtdss-pub-$fp-cafe00.tmp.txt", 6)
                }
            }
        })
        assertEquals(PublishResult.Published, e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy()))
        assertNull("同指纹历史残留被清", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fp-dead00.tmp"))
        assertNull("MIUI 改写形(.tmp.txt)同指纹残留同清", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fp-cafe00.tmp.txt"))
        assertTrue("终件落位", e.face.nodeAt("MTDSS/small.txt") != null)
        assertEquals("清扫后隐藏目录零残留", 0, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
        assertEquals("本次临时名编指纹", 1, e.face.createdDocs.size)
        assertTrue(
            "临时名形态 .mtdss-pub-<指纹8>-<6hex>.tmp:${e.face.createdDocs[0].second}",
            e.face.createdDocs[0].second.matches(TEMP_NAME_SHAPE),
        )
    }

    @Test
    fun `异 rel 残留不溯及 旧格式孤儿同保留`() {
        // 关键断言(用户语义的直接钉):别的 rel 的残留、无主历史残留——
        // 不做处理;旧格式临时名(无指纹段,设备现存形态)天然不命中
        val fpOther = SafInboxStore.relFingerprint("other.bin")
        val e = Env(build = {
            dir("MTDSS") {
                dir(".mtdss") {
                    file(".mtdss-pub-$fpOther-beef00.tmp", 5) // 异 rel 残留(别任务的中间态)
                    file(".mtdss-pub-faaa5c0a.tmp", 7) // 旧格式无指纹孤儿(不溯及)
                }
            }
        })
        assertEquals(PublishResult.Published, e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy()))
        assertNotNull("异 rel 残留保留(不溯及)", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fpOther-beef00.tmp"))
        assertNotNull("旧格式无指纹孤儿保留(不溯及)", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-faaa5c0a.tmp"))
        assertEquals("仅清本 rel,他物原样", 2, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
    }

    @Test
    fun `清扫只清同指纹 不误删他物且本次件已走`() {
        // 复制式 move provider(move 换 id):本次临时件随 move 离开 .mtdss;
        // 成功清扫只命中同指纹形态——异指纹/普通文件分毫不动
        val fp = SafInboxStore.relFingerprint("small.txt")
        val fpOther = SafInboxStore.relFingerprint("z.bin")
        val e = Env(build = {
            dir("MTDSS") {
                dir(".mtdss") {
                    file(".mtdss-pub-$fp-cafe00.tmp", 5) // 同指纹历史残留
                    file(".mtdss-pub-$fpOther-f00d00.tmp", 6) // 异指纹残留
                    file("user-file.txt", 9) // 普通文件
                }
            }
        }).apply { face.moveDocIdOverride = "d-move-new" }
        assertEquals(PublishResult.Published, e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy()))
        assertNull("同指纹残留被清", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fp-cafe00.tmp"))
        assertNotNull("异指纹保留", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fpOther-f00d00.tmp"))
        assertNotNull("普通文件保留", e.face.nodeAt("MTDSS/.mtdss/user-file.txt"))
        assertEquals("隐藏目录仅余两件他物", 2, e.face.nodeAt("MTDSS/.mtdss")!!.children.size)
        assertTrue("终件真实落位", e.face.nodeAt("MTDSS/small.txt") != null)
    }

    @Test
    fun `清扫单项删除失败 发布不受影响 其余仍清`() {
        // best-effort 钉:逐项失败 log 不中断,发布照常 Published
        val fp = SafInboxStore.relFingerprint("small.txt")
        val e = Env(build = {
            dir("MTDSS") {
                dir(".mtdss") {
                    file(".mtdss-pub-$fp-aaaa00.tmp", 3)
                    file(".mtdss-pub-$fp-bbbb00.tmp", 5)
                }
            }
        })
        e.face.failDeleteOf += e.face.docIdAt("MTDSS/.mtdss/.mtdss-pub-$fp-aaaa00.tmp")!!
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy())
        assertEquals("清扫失败不影响发布结果", PublishResult.Published, r)
        assertNotNull("失败项保留", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fp-aaaa00.tmp"))
        assertNull("其余同指纹残留仍清", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fp-bbbb00.tmp"))
        assertTrue("终件照常落位", e.face.nodeAt("MTDSS/small.txt") != null)
    }

    @Test
    fun `skip 弃件终局同清同指纹`() {
        // Skipped 亦成功终局(0x05 ok:true)——本 rel 中间态随任务完成清;
        // 异 rel 残留仍不溯及;暂存目录只查不建(弃件路径零建副作用)
        val fp = SafInboxStore.relFingerprint("small.txt")
        val fpOther = SafInboxStore.relFingerprint("z.bin")
        val e = Env(build = {
            dir("MTDSS") {
                file("small.txt", 42)
                dir(".mtdss") {
                    file(".mtdss-pub-$fp-cccc00.tmp", 5)
                    file(".mtdss-pub-$fpOther-dddd00.tmp", 6)
                }
            }
        })
        val r = e.store.publish("small.txt", "/staging/small.txt", 2025, TransferPolicy(skip = listOf("small.txt")))
        assertEquals(PublishResult.Skipped, r)
        assertNull("弃件终局同清同指纹残留", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fp-cccc00.tmp"))
        assertNotNull("异 rel 残留仍保留", e.face.nodeAt("MTDSS/.mtdss/.mtdss-pub-$fpOther-dddd00.tmp"))
        assertEquals("既有件保留不动", 42L, e.face.nodeAt("MTDSS/small.txt")!!.size)
        assertEquals("弃件路径不建目录", 0, e.face.createdDirs.size)
    }

    private companion object {
        /** 点文件临时名形态:dot 开头 + `.mtdss-pub-` + 8 位指纹 hex + `-` + 6 位 hex + `.tmp`。 */
        private val TEMP_NAME_SHAPE = Regex("""^\.mtdss-pub-[0-9a-f]{8}-[0-9a-f]{6}\.tmp$""")
    }
}
