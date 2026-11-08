package com.mtdss.android.ui.browse

import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 浏览 VM JVM 单测(iOS MTDSSKit BrowseViewModel 语义镜像;brief T7 Step 1
 * 全列):确认语义(confirm = 只退出无副作用)、visibleRows 树展开序(父子
 * 插入位)、搜索/排序/隐藏过滤矩阵、浏览器式历史(open 截断前进分支)、
 * 删除确认文案三分支、远端档动作集(dirActions 含「快捷」,删除受门)、
 * seq 守卫、导航即时清列表(R12 ②)、删除/下载/转发请求面(force=true/
 * 收件箱哨兵/remote_copy from=浏览设备)、截断标志。
 */
class BrowseViewModelTest {

    private val device = DirectoryDevice(
        deviceId = "dev-peer-fixed",
        name = "契约对端",
        platform = "linux",
        online = true,
    )

    private fun entry(
        name: String,
        path: String,
        size: Long = 0L,
        mtimeMs: Long = 0L,
        isDir: Boolean,
        children: Long? = null,
    ): DirEntry = DirEntry(name, path, size, mtimeMs, isDir, children)

    /** 路径 → 列表内容脚本。JSON 恒单行(NDJSON 一行一帧)。 */
    private fun listingFor(path: String): Pair<String, Boolean> = when (path) {
        "" -> ("[" +
            "{\"name\":\"docs\",\"path\":\"/docs\",\"size\":0,\"mtime_ms\":0,\"is_dir\":true,\"children\":2}," +
            "{\"name\":\"a.bin\",\"path\":\"/a.bin\",\"size\":1024,\"mtime_ms\":1760000000000,\"is_dir\":false}," +
            "{\"name\":\".env\",\"path\":\"/.env\",\"size\":16,\"mtime_ms\":0,\"is_dir\":false}," +
            "{\"name\":\"z.log\",\"path\":\"/z.log\",\"size\":2048,\"mtime_ms\":1760000000001,\"is_dir\":false}" +
            "]") to false
        "/docs" -> ("[" +
            "{\"name\":\"sub\",\"path\":\"/docs/sub\",\"size\":0,\"mtime_ms\":0,\"is_dir\":true,\"children\":1}," +
            "{\"name\":\"deep.txt\",\"path\":\"/docs/deep.txt\",\"size\":4096,\"mtime_ms\":1760000000002,\"is_dir\":false}" +
            "]") to false
        "/docs/sub" -> ("[" +
            "{\"name\":\"leaf.bin\",\"path\":\"/docs/sub/leaf.bin\",\"size\":1,\"mtime_ms\":0,\"is_dir\":false}" +
            "]") to false
        "/docs/many" -> ("[" +
            "{\"name\":\"x\",\"path\":\"/docs/many/x\",\"size\":0,\"mtime_ms\":0,\"is_dir\":false}" +
            "]") to true
        "/slow" -> ("[" +
            "{\"name\":\"慢.bin\",\"path\":\"/slow/慢.bin\",\"size\":1,\"mtime_ms\":0,\"is_dir\":false}" +
            "]") to false
        else -> "[]" to false
    }

    /** 伪 daemon:list 按路径脚本应答;delete/transfer.start 记录后按脚本应答。 */
    private fun scriptList(e: TestEnv, slowPaths: Set<String> = emptySet()) {
        e.fake.onLine = { conn, line ->
            val req = JVal.parse(String(line, Charsets.UTF_8))
            val id = req.opt("id")!!.asLong()
            when (req.opt("method")!!.asString()) {
                "list" -> {
                    val path = req.opt("params")!!.opt("path")!!.asString()
                    if (path in slowPaths) Thread.sleep(200)
                    val (entries, truncated) = listingFor(path)
                    conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                }

                else -> error("意外的请求: ${req.opt("method")}")
            }
        }
    }

    private fun newVm(e: TestEnv): BrowseViewModel = BrowseViewModel(
        deviceId = device.deviceId,
        rootPath = "",
        rootTitle = "契约对端",
        deviceName = "契约对端",
        repo = e.repo,
        scope = e.scope,
    )

    // ── 纯函数面(排序/过滤矩阵;无需 daemon)────────────────────────────────

    @Test
    fun `排序矩阵 文件夹恒前 字段内主键名称兜底`() {
        val archive = entry("archive", "/archive", isDir = true, mtimeMs = 20)
        val docs = entry("docs", "/docs", isDir = true, mtimeMs = 1)
        val a = entry("a.txt", "/a.txt", size = 100, mtimeMs = 5, isDir = false)
        val big = entry("B.txt", "/B.txt", size = 300, mtimeMs = 9, isDir = false)
        val md = entry("c.md", "/c.md", size = 7, mtimeMs = 3, isDir = false)
        val items = listOf(docs, a, big, archive, md)

        // 名称:目录恒前(组内名称大小写不敏感),文件 a < B(大小写不敏感)
        assertEquals(
            listOf(archive, docs, a, big, md).map { it.path },
            BrowseViewModel.sortEntries(items, BrowseSort.NAME).map { it.path },
        )
        // 日期:最新在前;目录恒前
        assertEquals(
            listOf(archive, docs, big, a, md).map { it.path },
            BrowseViewModel.sortEntries(items, BrowseSort.DATE).map { it.path },
        )
        // 大小:大者在前;目录 size 恒 0 → 名称兜底
        assertEquals(
            listOf(archive, docs, big, a, md).map { it.path },
            BrowseViewModel.sortEntries(items, BrowseSort.SIZE).map { it.path },
        )
        // 种类:目录("")恒前;扩展名 md < txt,txt 组内 a < B
        assertEquals(
            listOf(archive, docs, md, a, big).map { it.path },
            BrowseViewModel.sortEntries(items, BrowseSort.KIND).map { it.path },
        )
    }

    @Test
    fun `搜索与隐藏过滤矩阵`() {
        val docs = entry("docs", "/docs", isDir = true)
        val hidden = entry(".secret", "/.secret", size = 1, isDir = false)
        val report = entry("报告.docx", "/报告.docx", size = 2, isDir = false)
        val items = listOf(docs, hidden, report)

        // 隐藏过滤:dotfiles 默认滤,showHidden 直通
        assertEquals(listOf("/docs", "/报告.docx"), BrowseViewModel.applyFilters(items, "", false, BrowseSort.NAME).map { it.path })
        assertEquals(
            listOf("/docs", "/.secret", "/报告.docx"),
            BrowseViewModel.applyFilters(items, "", true, BrowseSort.NAME).map { it.path },
        )
        // 搜索:名称大小写不敏感子串
        assertEquals(listOf("/docs"), BrowseViewModel.applyFilters(items, "DOCS", false, BrowseSort.NAME).map { it.path })
        assertEquals(listOf("/报告.docx"), BrowseViewModel.applyFilters(items, "报", false, BrowseSort.NAME).map { it.path })
        assertEquals(emptyList<String>(), BrowseViewModel.applyFilters(items, "无", false, BrowseSort.NAME).map { it.path })
    }

    @Test
    fun `标题组合与路径显示名`() {
        // 浏览根 = rootTitle 单段
        assertEquals("契约对端", BrowseViewModel.composeTitle("契约对端", "", "", "契约对端"))
        // 深于根(远端)= 「设备名 | 目录名」(裸 | 前后各一空格)
        assertEquals("契约对端 | sub", BrowseViewModel.composeTitle("契约对端", "/docs/sub", "", "契约对端"))
        // 本地档(deviceName null)= 目录名单段
        assertEquals("docs", BrowseViewModel.composeTitle("收件箱", "/docs", "/", null))
        // 显示名 = 末段(正反斜杠都切;卷条目原样)
        assertEquals("b", BrowseViewModel.displayName("/a/b"))
        assertEquals("b", BrowseViewModel.displayName("C:\\a\\b"))
        assertEquals("C:", BrowseViewModel.displayName("C:"))
    }

    @Test
    fun `删除确认文案三分支`() {
        // 单目录形:标题特称,副文点明整树
        val singleDir = DeleteRequest(paths = listOf("/docs"), folders = 1)
        assertEquals("删除文件夹及内容?", singleDir.alertTitle)
        assertEquals("所选文件夹及其全部内容将被永久删除,不可恢复。", singleDir.alertMessage)
        // 多选含目录:标题按条数,副文仍点明文件夹连同内容
        val mixed = DeleteRequest(paths = listOf("/docs", "/a.bin"), folders = 1)
        assertEquals("删除 2 项?", mixed.alertTitle)
        assertEquals("所选文件夹及其全部内容将被永久删除,不可恢复。", mixed.alertMessage)
        // 纯文件形
        val files = DeleteRequest(paths = listOf("/a.bin", "/b.bin"))
        assertEquals("删除 2 项?", files.alertTitle)
        assertEquals("所选项目将被永久删除,不可恢复。", files.alertMessage)
    }

    @Test
    fun `远端档动作集矩阵 dirActions 含快捷 删除受门`() {
        val dir = entry("docs", "/docs", isDir = true)
        val file = entry("a.bin", "/a.bin", isDir = false)

        // 行左滑:远端 = [下载, 转发],门开追加 删除;本地 = [发送, 删除] 与门无涉
        assertEquals(
            listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD),
            BrowseSwipeAction.actions(isRemote = true, allowRemoteDelete = false),
        )
        assertEquals(
            listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD, BrowseSwipeAction.DELETE),
            BrowseSwipeAction.actions(isRemote = true, allowRemoteDelete = true),
        )
        assertEquals(
            listOf(BrowseSwipeAction.SEND, BrowseSwipeAction.DELETE),
            BrowseSwipeAction.actions(isRemote = false, allowRemoteDelete = true),
        )
        // 目录行追加「快捷」(恒簇末);文件行不追加
        assertEquals(
            listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD, BrowseSwipeAction.FAVORITE),
            BrowseSwipeAction.dirActions(dir, isRemote = true, allowRemoteDelete = false),
        )
        assertEquals(
            listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD),
            BrowseSwipeAction.dirActions(file, isRemote = true, allowRemoteDelete = false),
        )
        assertEquals(
            listOf(BrowseSwipeAction.SEND, BrowseSwipeAction.DELETE, BrowseSwipeAction.FAVORITE),
            BrowseSwipeAction.dirActions(dir, isRemote = false, allowRemoteDelete = false),
        )

        // 选择底栏:远端 = [下载, 转发](+删除受门);本地 = [分享, 删除]
        assertEquals(
            listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD),
            BrowseSelectionAction.actions(isRemote = true, allowRemoteDelete = false),
        )
        assertEquals(
            listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD, BrowseSelectionAction.DELETE),
            BrowseSelectionAction.actions(isRemote = true, allowRemoteDelete = true),
        )
        assertEquals(
            listOf(BrowseSelectionAction.SHARE, BrowseSelectionAction.DELETE),
            BrowseSelectionAction.actions(isRemote = false, allowRemoteDelete = false),
        )
        // 纯目录多选追加「快捷」;含文件/混合不出
        assertEquals(
            listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD, BrowseSelectionAction.FAVORITE),
            BrowseSelectionAction.dirSelectionActions(isRemote = true, allDirs = true, allowRemoteDelete = false),
        )
        assertEquals(
            listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD),
            BrowseSelectionAction.dirSelectionActions(isRemote = true, allDirs = false, allowRemoteDelete = false),
        )
        assertEquals(
            listOf(BrowseSelectionAction.SHARE, BrowseSelectionAction.DELETE, BrowseSelectionAction.FAVORITE),
            BrowseSelectionAction.dirSelectionActions(isRemote = false, allDirs = true, allowRemoteDelete = false),
        )

        // 门单源(BrowseGates):默认全关;门关时远端删除动作不出集,门开时在
        // (本地删除不设门恒在)
        assertEquals(BrowseGates.DEFAULT, BrowseGates())
        assertFalse(BrowseGates.DEFAULT.showHidden)
        assertFalse(BrowseGates.DEFAULT.allowRemoteDelete)
        assertEquals(
            listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD),
            BrowseSwipeAction.actions(isRemote = true, allowRemoteDelete = BrowseGates.DEFAULT.allowRemoteDelete),
        )
        assertEquals(
            listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD, BrowseSelectionAction.DELETE),
            BrowseSelectionAction.actions(
                isRemote = true,
                allowRemoteDelete = BrowseGates(showHidden = false, allowRemoteDelete = true).allowRemoteDelete,
            ),
        )
        assertEquals(
            listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD),
            BrowseSelectionAction.actions(isRemote = true, allowRemoteDelete = BrowseGates.DEFAULT.allowRemoteDelete),
        )
    }

    // ── VM 语义面(daemon 脚本)─────────────────────────────────────────────

    @Test
    fun `加载浏览根与可见行`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.state.value.entries.size == 4 }
            assertEquals("", vm.state.value.path)
            assertFalse(vm.state.value.truncated)
            assertFalse(vm.state.value.loading)
            // 标题 = rootTitle 单段
            assertEquals("契约对端", vm.title)
            // 隐藏过滤:dotfiles 默认滤出可见行;排序 = 名称,目录恒前
            assertEquals(listOf("/docs", "/a.bin", "/z.log"), vm.visibleRows.map { it.entry.path })
            assertEquals(3, vm.itemCount)
            // 请求面:path 空串 = 根卷
            val line = JVal.parse(String(e.fake.connections[0].received[0], Charsets.UTF_8))
            assertEquals("", line.opt("params")!!.opt("path")!!.asString())
        } finally {
            e.close()
        }
    }

    @Test
    fun `门控真源接线 双开关即时换 零重拉`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            // 设置页门流种子:双门全开(T11 持久档投影;消费面 VM 接同一流)
            val gates = MutableStateFlow(BrowseGates(showHidden = true, allowRemoteDelete = true))
            val vm = BrowseViewModel(
                deviceId = device.deviceId,
                rootPath = "",
                rootTitle = "契约对端",
                deviceName = "契约对端",
                repo = e.repo,
                scope = e.scope,
                gates = gates,
            )
            awaitUntil { vm.state.value.showHidden && vm.state.value.allowRemoteDelete }
            vm.loadRoot()
            awaitUntil { vm.state.value.entries.size == 4 }

            // 门开:dotfile 行渲染;远端删除动作在集(行左滑/选择底栏同一门)
            assertTrue(vm.visibleEntries.any { it.path == "/.env" })
            assertEquals(
                listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD, BrowseSwipeAction.DELETE),
                BrowseSwipeAction.actions(isRemote = true, allowRemoteDelete = vm.state.value.allowRemoteDelete),
            )
            assertEquals(
                listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD, BrowseSelectionAction.DELETE),
                BrowseSelectionAction.actions(isRemote = true, allowRemoteDelete = vm.state.value.allowRemoteDelete),
            )

            // 翻门即收(重组驱动,免重拉):dotfile 行不渲染,数据仍在列表;
            // 远端删除动作出集;本地删除不设门恒在
            val requestsBefore = e.fake.connections[0].received.size
            gates.value = BrowseGates.DEFAULT
            awaitUntil { !vm.state.value.showHidden && !vm.state.value.allowRemoteDelete }
            assertEquals(4, vm.state.value.entries.size)
            assertFalse(vm.visibleEntries.any { it.path == "/.env" })
            assertEquals(
                listOf(BrowseSwipeAction.DOWNLOAD, BrowseSwipeAction.FORWARD),
                BrowseSwipeAction.actions(isRemote = true, allowRemoteDelete = vm.state.value.allowRemoteDelete),
            )
            assertEquals(
                listOf(BrowseSelectionAction.DOWNLOAD, BrowseSelectionAction.FORWARD),
                BrowseSelectionAction.actions(isRemote = true, allowRemoteDelete = vm.state.value.allowRemoteDelete),
            )
            assertEquals(
                listOf(BrowseSwipeAction.SEND, BrowseSwipeAction.DELETE),
                BrowseSwipeAction.actions(isRemote = false, allowRemoteDelete = vm.state.value.allowRemoteDelete),
            )
            assertEquals(requestsBefore, e.fake.connections[0].received.size)
        } finally {
            e.close()
        }
    }

    @Test
    fun `树展开序 父子插入位 收起释放`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }

            // 展开根级目录:子树插在父行之后(depth = 父 +1),其余根级行随其后
            // (子级同管线:文件夹恒前 → 目录 sub 先于文件 deep.txt)
            vm.toggleExpand("/docs")
            awaitUntil { vm.state.value.expandedPaths.contains("/docs") }
            assertEquals(
                listOf("/docs", "/docs/sub", "/docs/deep.txt", "/a.bin", "/z.log"),
                vm.visibleRows.map { it.entry.path },
            )
            assertEquals(listOf(0, 1, 1, 0, 0), vm.visibleRows.map { it.depth })

            // 二级展开:子树继续插在父行之后
            vm.toggleExpand("/docs/sub")
            awaitUntil { vm.state.value.expandedPaths.contains("/docs/sub") }
            assertEquals(
                listOf("/docs", "/docs/sub", "/docs/sub/leaf.bin", "/docs/deep.txt", "/a.bin", "/z.log"),
                vm.visibleRows.map { it.entry.path },
            )
            assertEquals(listOf(0, 1, 2, 1, 0, 0), vm.visibleRows.map { it.depth })

            // 收起一级:整棵子树随父消失,兄弟行回位。单路径收口(iOS 同构):
            // expandedPaths 只摘被收路径本身,后代成员残留(投影不可达,行面
            // 不见;再展开时后代携缓存随行——iOS toggleExpand collapse 同语义)
            vm.toggleExpand("/docs")
            awaitUntil { !vm.state.value.expandedPaths.contains("/docs") }
            assertEquals(listOf("/docs", "/a.bin", "/z.log"), vm.visibleRows.map { it.entry.path })
            assertEquals(setOf("/docs/sub"), vm.state.value.expandedPaths)
        } finally {
            e.close()
        }
    }

    @Test
    fun `展开失败不展开且错误不打断浏览主面`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "list" -> {
                        val path = req.opt("params")!!.opt("path")!!.asString()
                        if (path == "/locked") {
                            conn.sendLine("""{"id":$id,"error":{"code":3,"message":"设备离线: dev-peer-fixed"}}""")
                        } else {
                            val (entries, truncated) = listingFor(path)
                            conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                        }
                    }

                    else -> error("意外的请求")
                }
            }
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.toggleExpand("/locked")
            awaitUntil { e.fake.connections[0].received.size == 2 }
            kotlinx.coroutines.delay(100)
            // 拉取失败 = 不展开(行仍在,可再点);浏览主面错误态不受牵连
            assertEquals(emptySet<String>(), vm.state.value.expandedPaths)
            assertNull(vm.state.value.errorText)
        } finally {
            e.close()
        }
    }

    @Test
    fun `搜索起手收起全部 清空不恢复`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.toggleExpand("/docs")
            awaitUntil { vm.state.value.expandedPaths.contains("/docs") }

            // 搜索起手 = 无条件收起全部(含在飞作废);搜索过滤只在根级
            vm.setSearchText("deep")
            awaitUntil { vm.state.value.expandedPaths.isEmpty() }
            // 根级无 "deep" 匹配 → 可见行空
            assertEquals(emptyList<BrowseRow>(), vm.visibleRows)

            // 搜索只属当前层:清空不过滤,但展开不恢复(iOS 同构)
            vm.setSearchText("")
            assertEquals(listOf("/docs", "/a.bin", "/z.log"), vm.visibleRows.map { it.entry.path })
            assertEquals(emptySet<String>(), vm.state.value.expandedPaths)
        } finally {
            e.close()
        }
    }

    @Test
    fun `确认语义 confirm 只退出无副作用`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }

            vm.enterSelection()
            assertTrue(vm.state.value.isSelecting)
            assertEquals(emptySet<String>(), vm.state.value.selection)
            vm.toggle("/a.bin")
            vm.toggle("/docs")
            assertEquals(setOf("/a.bin", "/docs"), vm.state.value.selection)

            vm.confirmSelection()
            assertFalse(vm.state.value.isSelecting)
            assertEquals(emptySet<String>(), vm.state.value.selection)

            // 无副作用:除 list 外无任何请求(delete/transfer 零发出)
            val methods = e.fake.connections[0].received
                .map { JVal.parse(String(it, Charsets.UTF_8)).opt("method")!!.asString() }
            assertTrue(methods.all { it == "list" })
            // exitSelection 同收口
            vm.enterSelection()
            vm.toggle("/a.bin")
            vm.exitSelection()
            assertFalse(vm.state.value.isSelecting)
            assertEquals(emptySet<String>(), vm.state.value.selection)
        } finally {
            e.close()
        }
    }

    @Test
    fun `全选跨层级 已全选再点清空`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.toggleExpand("/docs")
            awaitUntil { vm.state.value.expandedPaths.contains("/docs") }

            vm.selectAll()
            // 可见行集 = 含展开子级;隐藏项不出
            assertEquals(
                setOf("/docs", "/docs/sub", "/docs/deep.txt", "/a.bin", "/z.log"),
                vm.state.value.selection,
            )
            // 已全选再点 = 清空(iOS 文件 App 同惯用)
            vm.selectAll()
            assertEquals(emptySet<String>(), vm.state.value.selection)
            // toggle 再点取消
            vm.toggle("/a.bin")
            vm.toggle("/a.bin")
            assertEquals(emptySet<String>(), vm.state.value.selection)
        } finally {
            e.close()
        }
    }

    @Test
    fun `selectedEntries 只收可见行序`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.enterSelection()
            vm.toggle("/a.bin")
            vm.toggle("/z.log")
            assertEquals(
                listOf("/a.bin", "/z.log"),
                vm.selectedEntries.map { it.path },
            )
        } finally {
            e.close()
        }
    }

    @Test
    fun `历史返回前进与 open 截断前进分支`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.state.value.entries.size == 4 }
            assertFalse(vm.state.value.canGoBack)
            assertFalse(vm.state.value.canGoForward)

            vm.open("/docs")
            awaitUntil { vm.state.value.path == "/docs" && vm.state.value.entries.size == 2 }
            assertTrue(vm.state.value.canGoBack)
            assertFalse(vm.state.value.canGoForward)
            // 深于根标题 = 「设备名 | 目录名」
            assertEquals("契约对端 | docs", vm.title)

            vm.open("/slow")
            awaitUntil { vm.state.value.path == "/slow" }
            vm.goBack()
            awaitUntil { vm.state.value.path == "/docs" }
            assertTrue(vm.state.value.canGoForward)
            vm.goForward()
            awaitUntil { vm.state.value.path == "/slow" }
            vm.goBack()
            awaitUntil { vm.state.value.path == "/docs" }
            // 在 /docs 处 open 新目录 = 截断前进分支(/slow 出栈)
            vm.open("/docs/many")
            awaitUntil { vm.state.value.path == "/docs/many" && vm.state.value.truncated }
            assertTrue(vm.state.value.canGoBack)
            assertFalse(vm.state.value.canGoForward)
            vm.goForward() // 无前进分支:空操作
            kotlinx.coroutines.delay(80)
            assertEquals("/docs/many", vm.state.value.path)
        } finally {
            e.close()
        }
    }

    @Test
    fun `旋转恢复 历史栈种子与快照回写`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            // VM 重建种子(评审 fix round 1;iOS 旋转不丢态同构):历史栈 +
            // 游标恢复位置,listing 由 reload 现拉(iOS 推入新 StateObject 同构)
            val vm = BrowseViewModel(
                deviceId = device.deviceId,
                rootPath = "",
                rootTitle = "契约对端",
                deviceName = "契约对端",
                repo = e.repo,
                scope = e.scope,
                initialHistory = listOf("", "/docs"),
                initialHistoryIndex = 1,
            )
            assertEquals("/docs", vm.state.value.path)
            assertTrue(vm.state.value.canGoBack)
            assertFalse(vm.state.value.canGoForward)
            assertEquals("契约对端 | docs", vm.title)
            assertEquals(listOf("", "/docs") to 1, vm.historySnapshot())
            // 恢复位现拉(reload 不动历史/游标)
            vm.reload()
            awaitUntil { !vm.state.value.loading && vm.state.value.entries.size == 2 }
            assertEquals("/docs", vm.state.value.path)
            assertEquals(listOf("", "/docs") to 1, vm.historySnapshot())
            // 导航后回写快照如实(UI saveable 形)
            vm.open("/docs/sub")
            awaitUntil { vm.state.value.path == "/docs/sub" }
            assertEquals(listOf("", "/docs", "/docs/sub") to 2, vm.historySnapshot())

            // 非法种子(越界游标/空栈)容缺省:回浏览根,不炸面
            val bad = BrowseViewModel(
                deviceId = device.deviceId,
                rootPath = "",
                rootTitle = "契约对端",
                deviceName = "契约对端",
                repo = e.repo,
                scope = e.scope,
                initialHistory = listOf("", "/docs"),
                initialHistoryIndex = 9,
            )
            assertEquals("", bad.state.value.path)
            assertFalse(bad.state.value.canGoBack)
        } finally {
            e.close()
        }
    }

    @Test
    fun `导航即时清列表进 loading 原地刷新保内容`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e, slowPaths = setOf("/slow"))
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.state.value.entries.size == 4 }

            // 换页:标题先行 + 列表即刻清空进 loading(R12 ②)
            vm.open("/slow")
            awaitUntil { vm.state.value.path == "/slow" && vm.state.value.loading }
            assertTrue(vm.state.value.entries.isEmpty())
            assertFalse(vm.state.value.truncated)
            awaitUntil { !vm.state.value.loading && vm.state.value.entries.size == 1 }

            // 原地 reload 不清列表(下拉刷新语义)
            vm.reload()
            awaitUntil { !vm.state.value.loading }
            assertEquals("/slow", vm.state.value.path)
            assertEquals(1, vm.state.value.entries.size)
            assertNull(vm.state.value.errorText)
        } finally {
            e.close()
        }
    }

    @Test
    fun `过期响应丢弃 seq 守卫`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e, slowPaths = setOf("/slow"))
            val vm = newVm(e)
            vm.open("/slow")
            vm.open("/docs") // 快速换向:慢响应到达时已过期
            awaitUntil { !vm.state.value.loading && vm.state.value.entries.size == 2 }
            assertEquals("/docs", vm.state.value.path)
            // 慢路径的响应不得覆盖新位置
            awaitUntil { e.fake.connections[0].received.size == 2 }
            kotlinx.coroutines.delay(300)
            assertEquals("/docs", vm.state.value.path)
            assertEquals(2, vm.state.value.entries.size)
        } finally {
            e.close()
        }
    }

    @Test
    fun `list 错误转错误态遮列表`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                conn.sendLine("""{"id":$id,"error":{"code":3,"message":"设备离线: dev-peer-fixed"}}""")
            }
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.state.value.errorText != null }
            assertTrue(vm.state.value.errorText!!.contains("设备离线"))
            assertFalse(vm.state.value.loading)
            assertTrue(vm.state.value.entries.isEmpty())
            assertFalse(vm.state.value.truncated)
        } finally {
            e.close()
        }
    }

    @Test
    fun `截断标志如实呈现`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.open("/docs/many")
            awaitUntil { vm.state.value.entries.size == 1 }
            assertTrue(vm.state.value.truncated)
        } finally {
            e.close()
        }
    }

    @Test
    fun `选择删除 force 请求面 退出选择并原地重载`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val deleteRequests = ArrayList<JVal>()
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "list" -> {
                        val path = req.opt("params")!!.opt("path")!!.asString()
                        val (entries, truncated) = listingFor(path)
                        conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                    }

                    "delete" -> {
                        deleteRequests.add(req)
                        conn.sendLine("""{"id":$id,"result":{"deleted":2,"recycled":false,"failed":[]}}""")
                    }

                    else -> error("意外的请求")
                }
            }
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.enterSelection()
            vm.toggle("/a.bin")
            vm.toggle("/docs")
            vm.deleteSelected()
            awaitUntil { deleteRequests.isNotEmpty() }
            kotlinx.coroutines.delay(80)

            // 请求面:delete {device=浏览目标, paths, force=true}
            val p = deleteRequests[0].opt("params")!!
            assertEquals("dev-peer-fixed", p.opt("device")!!.asString())
            assertEquals(
                listOf("/a.bin", "/docs").sorted(),
                p.opt("paths")!!.asArray().map { it.asString() }.sorted(),
            )
            assertTrue(p.opt("force")!!.asBoolean())

            // 成功收口:退出选择 + 原地重载;全成静默(无 deleteNotice)
            assertFalse(vm.state.value.isSelecting)
            assertEquals(emptySet<String>(), vm.state.value.selection)
            assertNull(vm.state.value.deleteNotice)
            assertNull(vm.state.value.lastActionError)
            awaitUntil { e.fake.connections[0].received.size == 3 } // root list + delete + reload list
        } finally {
            e.close()
        }
    }

    @Test
    fun `删除部分失败给明细 失败保持选择`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "list" -> {
                        val path = req.opt("params")!!.opt("path")!!.asString()
                        val (entries, truncated) = listingFor(path)
                        conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                    }

                    "delete" -> conn.sendLine(
                        """{"id":$id,"result":{"deleted":1,"recycled":false,"failed":[""" +
                            """{"path":"/docs","code":"INTERNAL","message":"目录未空"}]}}""",
                    )

                    else -> error("意外的请求")
                }
            }
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.enterSelection()
            vm.toggle("/a.bin")
            vm.toggle("/docs")
            vm.deleteSelected()
            awaitUntil { vm.state.value.deleteNotice != null }
            assertEquals("已删除 1 项,1 项失败:目录未空", vm.state.value.deleteNotice)
        } finally {
            e.close()
        }

        val e2 = TestEnv()
        try {
            e2.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "list" -> {
                        val path = req.opt("params")!!.opt("path")!!.asString()
                        val (entries, truncated) = listingFor(path)
                        conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                    }

                    "delete" -> conn.sendLine("""{"id":$id,"error":{"code":3,"message":"设备离线: dev-peer-fixed"}}""")
                    else -> error("意外的请求")
                }
            }
            val vm = newVm(e2)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.enterSelection()
            vm.toggle("/a.bin")
            vm.deleteSelected()
            awaitUntil { vm.state.value.lastActionError != null }
            assertTrue(vm.state.value.lastActionError!!.contains("设备离线"))
            // 失败保持选择(iOS 同构:用户可重试/改选)
            assertTrue(vm.state.value.isSelecting)
            assertEquals(setOf("/a.bin"), vm.state.value.selection)
        } finally {
            e2.close()
        }
    }

    @Test
    fun `转发请求面 remote_copy dir 收件箱哨兵`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val startRequests = ArrayList<JVal>()
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "list" -> {
                        val path = req.opt("params")!!.opt("path")!!.asString()
                        val (entries, truncated) = listingFor(path)
                        conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                    }

                    "transfer.start" -> {
                        startRequests.add(req)
                        conn.sendLine("""{"id":$id,"result":{"job_id":"42"}}""")
                    }

                    else -> error("意外的请求")
                }
            }
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.forward(listOf("/a.bin", "/docs"), DirectoryDevice("dev-b", "B 机", "windows", online = true))
            awaitUntil { vm.state.value.startedCount == 2 }
            assertNull(vm.state.value.lastActionError)

            // 请求面:kind=remote_copy,from = 浏览设备,to = 目标,dir = 收件箱哨兵
            val p = startRequests[0].opt("params")!!
            assertEquals("remote_copy", p.opt("kind")!!.asString())
            assertEquals("dev-peer-fixed", p.opt("from")!!.asString())
            assertEquals("dev-b", p.opt("to")!!.asString())
            assertEquals("inbox", p.opt("dir")!!.asString())
            assertEquals(
                listOf("/a.bin", "/docs").sorted(),
                p.opt("paths")!!.asArray().map { it.asString() }.sorted(),
            )
            assertTrue(p.opt("skip")!!.asArray().isEmpty())
            assertTrue(p.opt("overwrite")!!.asArray().isEmpty())
            assertFalse(p.opt("cut")!!.asBoolean())
        } finally {
            e.close()
        }
    }

    @Test
    fun `下载请求面 收件箱哨兵 默认策略`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val startRequests = ArrayList<JVal>()
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                when (req.opt("method")!!.asString()) {
                    "list" -> {
                        val path = req.opt("params")!!.opt("path")!!.asString()
                        val (entries, truncated) = listingFor(path)
                        conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                    }

                    "transfer.start" -> {
                        startRequests.add(req)
                        conn.sendLine("""{"id":$id,"result":{"job_id":"42"}}""")
                    }

                    else -> error("意外的请求")
                }
            }
            val vm = newVm(e)
            vm.loadRoot()
            awaitUntil { vm.visibleRows.isNotEmpty() }
            vm.download(listOf("/a.bin"))
            awaitUntil { vm.state.value.startedCount == 1 }

            val p = startRequests[0].opt("params")!!
            assertEquals("download", p.opt("kind")!!.asString())
            assertEquals("dev-peer-fixed", p.opt("device")!!.asString())
            assertEquals("inbox", p.opt("dir")!!.asString())
            assertEquals(listOf("/a.bin"), p.opt("paths")!!.asArray().map { it.asString() })
            assertTrue(p.opt("skip")!!.asArray().isEmpty())
            assertTrue(p.opt("overwrite")!!.asArray().isEmpty())
            assertFalse(p.opt("cut")!!.asBoolean())

            // 失败转 lastActionError(iOS 同构;发起面无独立 error 槽)
            vm.clearFeedback() // 一次性计数已被首个成功占用,先清再验失败面
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                if (req.opt("method")!!.asString() == "transfer.start") {
                    conn.sendLine("""{"id":$id,"error":{"code":3,"message":"设备离线: dev-peer-fixed"}}""")
                } else {
                    val path = req.opt("params")!!.opt("path")!!.asString()
                    val (entries, truncated) = listingFor(path)
                    conn.sendLine("""{"id":$id,"result":{"entries":$entries,"truncated":$truncated}}""")
                }
            }
            vm.download(listOf("/z.log"))
            awaitUntil { vm.state.value.lastActionError != null }
            assertTrue(vm.state.value.lastActionError!!.contains("设备离线"))
            assertNull(vm.state.value.startedCount)
        } finally {
            e.close()
        }
    }
}
