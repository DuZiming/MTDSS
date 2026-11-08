package com.mtdss.android.ui.favorites

import com.mtdss.android.favorites.Favorite
import com.mtdss.android.favorites.FavMember
import com.mtdss.android.favorites.FavoritesStore
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.FakeDaemon
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import kotlinx.coroutines.launch
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 快捷目录 tab VM JVM 单测(V3-4 T10,spec §3-2;iOS FavoritesView/
 * FavoritesRootList 消费面语义镜像;brief Step 1 全列 + T9 义务钉面):
 * 根行投影(收件箱置顶恒在)/成员行投影(devices join + 离线旗)/成员展开
 * 状态机(乐观展开/在飞吞点/失败收回/收起释放/迟到应答不复活)/新建空名拒
 * (trim 后空 = 零 kv.put)/删除快捷确认后 store 变换(只解除链接,零 ctrl
 * delete)/树行隐藏过滤(子级含文件、目录恒前)/双击禁用(busy 闸吞并发,
 * T9 义务:UI 层不得依赖 put 自拒)/重命名与成员增删变换。
 */
class FavoritesViewModelTest {

    // ── 夹具 ────────────────────────────────────────────────────────────────

    private val t0 = 1_700_000_000_000L

    private fun fav(id: String, name: String, vararg members: FavMember) =
        Favorite(id = id, name = name, createdAt = t0, members = members.toList())

    private fun entry(
        name: String,
        path: String,
        size: Long = 0L,
        isDir: Boolean,
    ): DirEntry = DirEntry(name, path, size, 0L, isDir, null)

    /** 伪 daemon 脚本:kv/devices/list/delete/transfer.start 全记痕,可注失败与闸。 */
    private class Script {
        val kvStore = HashMap<String, String>()
        val kvGets = CopyOnWriteArrayList<String>()
        val kvPuts = CopyOnWriteArrayList<String>()
        val lists = CopyOnWriteArrayList<String>()
        val ctrlDeletes = CopyOnWriteArrayList<String>()
        val starts = CopyOnWriteArrayList<String>()

        @Volatile var putFailure = false
        @Volatile var listFailurePath: String? = null
        @Volatile var putGate: CountDownLatch? = null
        @Volatile var listGate: CountDownLatch? = null

        val kvPutCount: Int get() = kvPuts.size
        val listCount: Int get() = lists.size

        fun seedDoc(json: String) {
            kvStore["favorites"] = json
        }

        fun handler(): (FakeDaemon.ServerSide, ByteArray) -> Unit = handler@{ conn, line ->
            val frame = try {
                JVal.parse(String(line, Charsets.UTF_8))
            } catch (_: IllegalArgumentException) {
                return@handler
            }
            val id = (frame.opt("id") as? JVal.Num)?.raw ?: return@handler
            val method = (frame.opt("method") as? JVal.Str)?.v ?: return@handler
            val params = frame.opt("params") ?: JVal.Obj()
            when (method) {
                "kv.get" -> {
                    val key = (params.opt("key") as? JVal.Str)?.v ?: ""
                    kvGets.add(key)
                    val value = synchronized(kvStore) { kvStore[key] }
                    if (value != null) {
                        conn.sendLine("""{"id":$id,"result":{"value":${quoted(value)}}}""")
                    } else {
                        conn.sendLine(
                            """{"id":$id,"error":{"code":1,"data":{"kind":"not_found"},"message":"无此键: $key"}}""",
                        )
                    }
                }

                "kv.put" -> {
                    val key = (params.opt("key") as? JVal.Str)?.v ?: ""
                    val value = (params.opt("value") as? JVal.Str)?.v ?: ""
                    kvPuts.add(value)
                    putGate?.await()
                    if (putFailure) {
                        conn.sendLine(
                            """{"id":$id,"error":{"code":1,"data":{"kind":"internal"},"message":"kv 落盘失败(测试注入)"}}""",
                        )
                        return@handler
                    }
                    synchronized(kvStore) { kvStore[key] = value }
                    conn.sendLine("""{"id":$id,"result":{"ok":true}}""")
                }

                "devices" -> conn.sendLine(
                    """{"id":$id,"result":{"devices":$devicesJson,"stale":false}}""",
                )

                "list" -> {
                    val path = (params.opt("path") as? JVal.Str)?.v ?: ""
                    lists.add(path)
                    listGate?.await()
                    if (path == listFailurePath) {
                        conn.sendLine("""{"id":$id,"error":{"code":4,"message":"目录不存在(测试注入)"}}""")
                        return@handler
                    }
                    conn.sendLine("""{"id":$id,"result":{"entries":${listingFor(path)},"truncated":false}}""")
                }

                "delete" -> {
                    val device = (params.opt("device") as? JVal.Str)?.v ?: ""
                    ctrlDeletes.add(device)
                    conn.sendLine("""{"id":$id,"result":{"deleted":1,"recycled":false,"failed":[]}}""")
                }

                "transfer.start" -> {
                    starts.add(String(line, Charsets.UTF_8))
                    conn.sendLine("""{"id":$id,"error":{"code":1,"message":"测试不发起传输"}}""")
                }

                else -> conn.sendLine("""{"id":$id,"result":{}}""")
            }
        }

        @Volatile var devicesJson: String =
            """[{"device_id":"dev-a","name":"A 机","platform":"windows","online":true}]"""

        private fun listingFor(path: String): String = when (path) {
            "/docs" ->
                ("[" +
                    "{\"name\":\"sub\",\"path\":\"/docs/sub\",\"size\":0,\"mtime_ms\":0,\"is_dir\":true,\"children\":1}," +
                    "{\"name\":\"b.txt\",\"path\":\"/docs/b.txt\",\"size\":1024,\"mtime_ms\":0,\"is_dir\":false}," +
                    "{\"name\":\".env\",\"path\":\"/docs/.env\",\"size\":16,\"mtime_ms\":0,\"is_dir\":false}" +
                    "]")
            "/docs/sub" ->
                ("[" +
                    "{\"name\":\"leaf.bin\",\"path\":\"/docs/sub/leaf.bin\",\"size\":1,\"mtime_ms\":0,\"is_dir\":false}" +
                    "]")
            "/远端/相册" ->
                ("[" +
                    "{\"name\":\"冬日.jpg\",\"path\":\"/远端/相册/冬日.jpg\",\"size\":2048,\"mtime_ms\":0,\"is_dir\":false}" +
                    "]")
            else -> "[]"
        }

        /** JSON 字符串字面量(转义面够测试域:引号与反斜杠)。 */
        private fun quoted(s: String): String {
            val sb = StringBuilder("\"")
            for (c in s) when (c) {
                '"' -> sb.append("\\\"")
                '\\' -> sb.append("\\\\")
                else -> sb.append(c)
            }
            return sb.append('"').toString()
        }
    }

    /** kv 存值串(wire snake_case,与 encodeDoc 键序逐字节同构)。 */
    private fun docJson(vararg favs: Favorite): String {
        val sb = StringBuilder("""{"v":1,"updated_at":$t0,"favorites":[""")
        favs.forEachIndexed { i, f ->
            if (i > 0) sb.append(',')
            sb.append("""{"id":${quoted(f.id)},"name":${quoted(f.name)},"created_at":${f.createdAt},"members":[""")
            f.members.forEachIndexed { j, m ->
                if (j > 0) sb.append(',')
                sb.append("""{"device":${quoted(m.device)},"path":${quoted(m.path)}}""")
            }
            sb.append("]}")
        }
        return sb.append("]}").toString()
    }

    private fun quoted(s: String): String {
        val sb = StringBuilder("\"")
        for (c in s) when (c) {
            '"' -> sb.append("\\\"")
            '\\' -> sb.append("\\\\")
            else -> sb.append(c)
        }
        return sb.append('"').toString()
    }

    /** 线到达等待(慢机预算放宽:5s 缺省在负载窗可被伪 daemon 冷启拖延,
     * 本文件统一 10s 上界——票池「慢机预算」家族同口径)。 */
    private suspend fun awaitArrived(cond: () -> Boolean) =
        TestKit.awaitUntil(timeoutMs = 10_000, cond = cond)

    // ── ① 根行投影:收件箱置顶恒在 ───────────────────────────────────────────

    @Test
    fun `根行投影 收件箱置顶恒在 快捷行随创建序`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val favA = fav("fav-a", "工作")
            val favB = fav("fav-b", "相册", FavMember("dev-a", "/远端/相册"))
            script.seedDoc(docJson(favA, favB))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.size == 2 }

            // 收件箱恒在且置顶;快捷行按创建序跟随
            assertEquals(
                listOf("inbox", "fav-a", "fav-b"),
                vm.rootRows.map { if (it is FavoritesViewModel.RootRow.Inbox) "inbox" else (it as FavoritesViewModel.RootRow.Fav).favorite.id },
            )

            // 清单空(无键)同样收件箱置顶(恒在)
            val emptyScript = Script()
            e.fake.onLine = emptyScript.handler()
            val emptyStore = FavoritesStore(e.repo)
            emptyStore.refresh()
            val vm2 = FavoritesViewModel(emptyStore, e.repo, e.hub, e.scope)
            awaitArrived { vm2.state.value.favorites.isEmpty() }
            assertEquals(1, vm2.rootRows.size)
            assertTrue(vm2.rootRows[0] is FavoritesViewModel.RootRow.Inbox)
        } finally {
            e.close()
        }
    }

    // ── ② 成员行投影:devices join + 离线旗 ─────────────────────────────────

    @Test
    fun `成员行投影 设备名join 离线旗与退化id 事件翻面重算`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            script.devicesJson =
                """[{"device_id":"dev-a","name":"A 机","platform":"windows","online":true},""" +
                    """{"device_id":"dev-b","name":"B 机","platform":"linux","online":false}]"""
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val fav = fav(
                "fav-m",
                "混合",
                FavMember("dev-a", "/远端/相册"),
                FavMember("dev-b", "/docs"),
                FavMember("dev-zzz", "/ghost"),
            )
            script.seedDoc(docJson(fav))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.isNotEmpty() }
            awaitArrived { vm.state.value.devices.size == 2 }

            val rows = vm.memberRows(fav)
            assertEquals("A 机 - 相册", rows[0].displayName)
            assertFalse(rows[0].offline)
            assertEquals("B 机 - docs", rows[1].displayName)
            assertTrue("快照在场但 online=false = 离线", rows[1].offline)
            assertEquals("dev-zzz - ghost", rows[2].displayName)
            assertTrue("快照缺该设备 = 离线且名退化 id", rows[2].offline)

            // event.devices 快照翻面 → 树重算(B 机上线)
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.devices","params":{"devices":[""" +
                    """{"device_id":"dev-a","name":"A 机","platform":"windows","online":true},""" +
                    """{"device_id":"dev-b","name":"B 机","platform":"linux","online":true}]}}""",
            )
            awaitArrived { !vm.memberRows(fav)[1].offline }
        } finally {
            e.close()
        }
    }

    // ── ③ 成员展开状态机 ─────────────────────────────────────────────────────

    @Test
    fun `展开状态机 乐观展开 懒加载缓存 收起释放`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val fav = fav("fav-x", "工作", FavMember("dev-a", "/docs"))
            script.seedDoc(docJson(fav))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.isNotEmpty() }

            // 快捷行展开 = 成员行现拼(零拉取)
            vm.toggleFavoriteExpand("fav-x")
            assertTrue("fav-x" in vm.state.value.expandedFavIds)
            assertEquals(0, script.listCount)

            // 成员节点展开 = 懒加载写缓存(device␀path 键)
            val nodeId = "fav-x\u00000\u0000dev-a\u0000/docs"
            vm.toggleDirNode("fav-x", nodeId, null, "dev-a", "/docs")
            awaitArrived { vm.state.value.memberChildren.containsKey("dev-a\u0000/docs") }
            assertTrue(nodeId in vm.state.value.expandedNodes)
            assertTrue("应答落地后在飞旗清", vm.state.value.loadingKeys.isEmpty())

            // 缓存命中再展开零重复拉取(收起→展开,lists 计数不变)
            vm.toggleDirNode("fav-x", nodeId, null, "dev-a", "/docs") // 收起
            assertFalse(nodeId in vm.state.value.expandedNodes)
            vm.toggleDirNode("fav-x", nodeId, null, "dev-a", "/docs") // 再展开
            assertEquals(1, script.listCount)
            assertTrue(nodeId in vm.state.value.expandedNodes)

            // 树行隐藏过滤投影:dotfile 滤、文件行在场、目录恒前
            assertEquals(
                listOf("/docs/sub", "/docs/b.txt"),
                FavoritesViewModel.favTreeChildren(
                    vm.state.value.memberChildren["dev-a\u0000/docs"]!!,
                    showHidden = false,
                ).map { it.path },
            )
            assertEquals(
                listOf("/docs/sub", "/docs/.env", "/docs/b.txt"),
                FavoritesViewModel.favTreeChildren(
                    vm.state.value.memberChildren["dev-a\u0000/docs"]!!,
                    showHidden = true,
                ).map { it.path },
            )

            // 快捷行收起 = 成员子树整片收拢 + 缓存按引用释放
            vm.toggleFavoriteExpand("fav-x")
            assertTrue(vm.state.value.expandedFavIds.isEmpty())
            assertTrue(vm.state.value.expandedNodes.isEmpty())
            assertTrue("无引用缓存释放", vm.state.value.memberChildren.isEmpty())
        } finally {
            e.close()
        }
    }

    @Test
    fun `展开状态机 拉取失败收回乐观展开 可再点重试`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            script.listFailurePath = "/docs"
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val fav = fav("fav-f", "工作", FavMember("dev-a", "/docs"))
            script.seedDoc(docJson(fav))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.isNotEmpty() }
            vm.toggleFavoriteExpand("fav-f")

            val nodeId = "fav-f\u00000\u0000dev-a\u0000/docs"
            vm.toggleDirNode("fav-f", nodeId, null, "dev-a", "/docs")
            awaitArrived { vm.state.value.loadingKeys.isEmpty() }
            assertFalse("失败收回乐观展开", nodeId in vm.state.value.expandedNodes)
            assertTrue(vm.state.value.memberChildren.isEmpty())

            // 行仍在,可再点重试(解除注入失败后成功)
            script.listFailurePath = null
            vm.toggleDirNode("fav-f", nodeId, null, "dev-a", "/docs")
            awaitArrived { vm.state.value.memberChildren.containsKey("dev-a\u0000/docs") }
            assertTrue(nodeId in vm.state.value.expandedNodes)
        } finally {
            e.close()
        }
    }

    @Test
    fun `展开状态机 在飞吞点 迟到应答不复活`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            val gate = CountDownLatch(1)
            script.listGate = gate
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val fav = fav("fav-l", "工作", FavMember("dev-a", "/docs"))
            script.seedDoc(docJson(fav))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.isNotEmpty() }
            vm.toggleFavoriteExpand("fav-l")

            val nodeId = "fav-l\u00000\u0000dev-a\u0000/docs"
            vm.toggleDirNode("fav-l", nodeId, null, "dev-a", "/docs")
            awaitArrived { vm.state.value.loadingKeys.contains("dev-a\u0000/docs") }
            awaitArrived { script.listCount >= 1 } // 请求到达 daemon(在飞窗)
            // 在飞点按吞掉(不重复拉取)
            vm.toggleDirNode("fav-l", nodeId, null, "dev-a", "/docs")
            assertEquals(1, script.listCount)

            // 收起快捷后在飞应答落地:展开态不复活、缓存不写
            vm.toggleFavoriteExpand("fav-l")
            assertTrue(vm.state.value.expandedFavIds.isEmpty())
            gate.countDown()
            awaitArrived { vm.state.value.loadingKeys.isEmpty() }
            assertTrue("fav 已收起:展开态不复活", nodeId !in vm.state.value.expandedNodes)
            assertTrue("fav 已收起:缓存不写不复活", vm.state.value.memberChildren.isEmpty())
        } finally {
            e.close()
        }
    }

    // ── ④ 新建快捷:空名禁提交 ───────────────────────────────────────────────

    @Test
    fun `新建空名禁提交 正名尾部追加`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)

            assertFalse("空名拒", vm.createFavorite(""))
            assertFalse("纯空白拒(trim 后空)", vm.createFavorite("   "))
            assertEquals("空名零 kv.put", 0, script.kvPutCount)

            assertTrue(vm.createFavorite(" 工作 "))
            awaitArrived { vm.state.value.favorites.size == 1 }
            assertEquals(1, script.kvPutCount)
            val created = vm.state.value.favorites.single()
            assertEquals("trim 后落名", "工作", created.name)
            assertTrue("空成员合法", created.members.isEmpty())
            assertEquals(16, created.id.length) // 客户端生成 16 hex
            assertTrue(created.id.all { it in "0123456789abcdef" })
        } finally {
            e.close()
        }
    }

    // ── ⑤ 删除快捷:确认后 store 变换,只解除链接 ────────────────────────────

    @Test
    fun `删除快捷 store变换只删该条 绝不触碰远端文件`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val favA = fav("fav-a", "工作")
            val favB = fav("fav-b", "相册")
            script.seedDoc(docJson(favA, favB))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.size == 2 }

            assertTrue(vm.deleteFavorite("fav-a"))
            awaitArrived { vm.state.value.favorites.size == 1 }
            assertEquals(1, script.kvPutCount)
            // 回写整单 = 只剩 fav-b(解除链接 = put transform 删该快捷)
            val payload = script.kvPuts.single()
            assertFalse("被删条目不回写", payload.contains("fav-a"))
            assertTrue("他条目保留", payload.contains("fav-b"))
            // 绝不触碰远端文件:零 ctrl delete、零 transfer.start
            assertTrue(script.ctrlDeletes.isEmpty())
            assertTrue(script.starts.isEmpty())
            assertEquals("成功 toast", "已删除快捷目录", vm.state.value.toast)
        } finally {
            e.close()
        }
    }

    // ── ⑤b 新建即加(picker 空清单入口;评审 fix round 1 钉面)───────────────

    @Test
    fun `新建即加 空单认领尾部直加成员 同步读无投影窗`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            store.refresh() // 无键 = 空单起点(picker 空清单唯一可达路径)
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.isEmpty() }

            val added = vm.createAndAddMembers(
                " 工作 ",
                listOf(FavMember("dev-a", "/远端/相册"), FavMember("dev-a", "/远端/相册")),
            )
            assertEquals("内部重复折叠 = 实际新增 1", 1, added)
            // 认领 = store snapshot 同步读(评审 fix round 1):store 单口即终
            assertEquals(1, store.snapshot().size)
            val created = store.snapshot().single()
            assertEquals("工作", created.name)
            assertEquals(listOf(FavMember("dev-a", "/远端/相册")), created.members)
            // 两写:新建落库 + 成员回写(iOS create → addMembers 同形)
            assertEquals(2, script.kvPutCount)
            assertTrue("成员已落第二写", script.kvPuts[1].contains("/远端/相册"))

            // 空名拒(零回写)
            assertEquals(2, script.kvPutCount)
            assertEquals(0, vm.createAndAddMembers("   ", listOf(FavMember("dev-a", "/x"))))
            assertEquals("空名零 kv.put", 2, script.kvPutCount)
        } finally {
            e.close()
        }
    }

    // ── ⑥ 双击禁用:在途编辑吞并发(T9 义务)────────────────────────────────

    @Test
    fun `双击禁用 busy闸吞并发编辑恰一次回写`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            val gate = CountDownLatch(1)
            script.putGate = gate
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)

            val first = e.scope.launch { vm.createFavorite("一") }
            awaitArrived { vm.state.value.busy }
            awaitArrived { script.kvPutCount >= 1 } // 首击 kv.put 已到 daemon(闸前留痕)
            var secondResult: Boolean? = null
            e.scope.launch { secondResult = vm.createFavorite("二") }
            awaitArrived { secondResult != null }
            assertFalse("在途窗内第二击吞掉", secondResult!!)
            assertEquals(1, script.kvPutCount)

            gate.countDown()
            awaitArrived { vm.state.value.favorites.size == 1 }
            assertEquals("一", vm.state.value.favorites.single().name)
            assertFalse("收尾复位", vm.state.value.busy)
            assertTrue(first.isCompleted)
        } finally {
            e.close()
        }
    }

    // ── ⑦ 管理面变换:重命名/成员增删 ────────────────────────────────────────

    @Test
    fun `重命名与成员增删 store变换`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            val fav = fav("fav-r", "旧名", FavMember("dev-a", "/docs"))
            script.seedDoc(docJson(fav))
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.favorites.isNotEmpty() }

            assertTrue(vm.renameFavorite("fav-r", "新名"))
            awaitArrived { vm.state.value.favorites.single().name == "新名" }
            assertEquals("已重命名", vm.state.value.toast)

            // 成员去重:同 memberKey 内部重复折叠;零新增不回写
            assertEquals(
                1,
                vm.addMembers("fav-r", listOf(FavMember("dev-a", "/远端/相册"), FavMember("dev-a", "/远端/相册"))),
            )
            awaitArrived { vm.state.value.favorites.single().members.size == 2 }
            val putsAfterAdd = script.kvPutCount
            assertEquals(
                "全重复零新增不回写",
                0,
                vm.addMembers("fav-r", listOf(FavMember("dev-a", "/远端/相册"))),
            )
            assertEquals(putsAfterAdd, script.kvPutCount)

            assertTrue(vm.removeMember("fav-r", FavMember("dev-a", "/远端/相册")))
            awaitArrived { vm.state.value.favorites.single().members.size == 1 }
        } finally {
            e.close()
        }
    }

    // ── ⑧ readOnly 守卫:编辑拒(VM 面)────────────────────────────────────

    @Test
    fun `readOnly编辑拒零回写`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = Script()
            // v:2 文档 = 前向守卫只读(T9 解析面;VM 编辑入口同步拒)
            script.seedDoc(
                """{"v":2,"updated_at":$t0,"favorites":[{"id":"fav-v2","name":"只读","created_at":$t0,"members":[]}]}""",
            )
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)
            store.refresh()
            val vm = FavoritesViewModel(store, e.repo, e.hub, e.scope)
            awaitArrived { vm.state.value.readOnly }

            assertFalse("只读新建拒", vm.createFavorite("新"))
            assertFalse("只读删除拒", vm.deleteFavorite("fav-v2"))
            assertEquals("零回写(store 本体绝不 kvPut)", 0, script.kvPutCount)
            assertTrue(vm.state.value.readOnly)
        } finally {
            e.close()
        }
    }
}
