package com.mtdss.android.favorites

import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.FakeDaemon
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 快捷目录 store JVM 单测(V3-4 T9,spec §4.10;iOS FavoritesStoreTests
 * 语义镜像 + TS favstore.test.ts 六场景同源):
 * ①refresh 无键(not_found)→ 空单可编辑;②解析失败/非对象/无 v → 只读空单;
 * ③v:2 前向守卫 → 只读 + 尽力解析 + 编辑拒 + kv.put 零调用;④put 前置取最新
 * (注入他端并发新文档,put=双方条目+新条目,put payload 逐字节锁 golden 键序);
 * ⑤kvPut 失败 → 保旧值、不自动重试;⑥transform null 无可写变更不回写;
 * ⑦清缓存 clearFavCache(快照/只读旗复位);⑧watcher 接线:data_changed
 * {favorites} 增量重拉、他 key 忽略、登录拉全量、登出+被踢清缓存。
 */
class FavoritesStoreTest {

    /** kv 伪 daemon 脚本(镜像 iOS KvScript 手法):内存键值仓 + 逐次调用留痕。 */
    private class KvScript {
        private val lock = Any()
        private val store = HashMap<String, String>()
        val gets = java.util.concurrent.CopyOnWriteArrayList<String>()
        val puts = java.util.concurrent.CopyOnWriteArrayList<String>()

        @Volatile
        var putFailure = false

        fun set(key: String, value: String) {
            synchronized(lock) { store[key] = value }
        }

        val kvGetCount: Int get() = gets.size
        val kvPutCount: Int get() = puts.size
        val putValues: List<String> get() = puts.toList()

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
                    gets.add(key) // 请求到达即留痕(应答前)
                    val value = synchronized(lock) { store[key] }
                    if (value != null) {
                        conn.sendLine("""{"id":$id,"result":{"value":${jsonQuoted(value)}}}""")
                    } else {
                        conn.sendLine(
                            """{"id":$id,"error":{"code":1,"data":{"kind":"not_found"},"message":"无此键: $key"}}""",
                        )
                    }
                }
                "kv.put" -> {
                    val key = (params.opt("key") as? JVal.Str)?.v ?: ""
                    val value = (params.opt("value") as? JVal.Str)?.v ?: ""
                    puts.add(value) // 调用留痕(失败也计,恰一次断言用)
                    if (putFailure) {
                        conn.sendLine(
                            """{"id":$id,"error":{"code":1,"data":{"kind":"internal"},"message":"kv 落盘失败(测试注入)"}}""",
                        )
                        return@handler
                    }
                    synchronized(lock) { store[key] = value } // UPSERT 落内存仓
                    conn.sendLine("""{"id":$id,"result":{"ok":true}}""")
                }
                else -> conn.sendLine("""{"id":$id,"result":{}}""")
            }
        }

        /** JSON 字符串字面量(转义面够测试域:引号与反斜杠;键值无控制字符)。 */
        private fun jsonQuoted(s: String): String {
            val sb = StringBuilder("\"")
            for (c in s) when (c) {
                '"' -> sb.append("\\\"")
                '\\' -> sb.append("\\\\")
                else -> sb.append(c)
            }
            return sb.append('"').toString()
        }
    }

    // ── 夹具 ────────────────────────────────────────────────────────────────

    /** 种子历史戳(seed 文档固定此值,断言 store 盖新戳用)。 */
    private val t0: Long = 1_700_000_000_000L

    private fun fav(id: String, name: String, vararg members: FavMember) =
        Favorite(id = id, name = name, createdAt = t0, members = members.toList())

    /** 构造 kv 存值串(wire snake_case;键序与 iOS Codable 逐字节同构)。 */
    private fun seedDoc(favorites: List<Favorite>, updatedAt: Long = t0) =
        encodeDoc(FavoritesDoc(v = 1, updatedAt = updatedAt, favorites = favorites))

    // ── ①refresh:not_found → 空单可编辑 ────────────────────────────────────

    @Test
    fun `refresh 无键得空单可编辑`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)

            store.refresh()
            assertEquals(1, script.kvGetCount)
            assertTrue("无键(404)= 合法空单", store.snapshot().isEmpty())
            assertFalse("空单可编辑", store.readOnly.value)
        } finally {
            e.close()
        }
    }

    // ── ②解析容差:空串/解码失败/非对象/无 v → 只读空单;缺字段补默认 ────────

    @Test
    fun `refresh 空串值得空单可编辑`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(FavoritesStore.KV_KEY, "")
            val store = FavoritesStore(e.repo)

            store.refresh()
            assertTrue(store.snapshot().isEmpty())
            assertFalse(store.readOnly.value)
        } finally {
            e.close()
        }
    }

    @Test
    fun `refresh 解码失败非对象无v只读空单`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            val store = FavoritesStore(e.repo)

            for (bad in listOf("不是 json", "[1,2]", """{"updated_at":5,"favorites":[]}""")) {
                script.set(FavoritesStore.KV_KEY, bad)
                store.refresh()
                assertTrue("解码失败/非对象/无 v → 只读:$bad", store.readOnly.value)
                assertTrue("失败态折进空单:$bad", store.snapshot().isEmpty())
            }
        } finally {
            e.close()
        }
    }

    @Test
    fun `refresh 缺字段补默认与残项过滤`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            // ①裸 v:1(其余缺省)→ 可编辑空单;②id/name 非字符串条目整条丢弃、
            // members 残项逐项过滤;③v 大整数 → 只读 + 已知字段尽力解析。
            script.set(
                FavoritesStore.KV_KEY,
                """{"v":1,"favorites":[{"id":"a","name":"完整","created_at":5,"members":[{"device":"d1","path":"C:\\x"},{"device":1,"path":"bad"},{"device":"d2"}]},{"id":9,"name":"坏id"},{"name":"缺id"},{"id":"c","name":"零成员"}]}""",
            )
            val store = FavoritesStore(e.repo)
            store.refresh()
            assertFalse("v=1 可编辑", store.readOnly.value)
            assertEquals(listOf("完整", "零成员"), store.snapshot().map { it.name })
            assertEquals(listOf(FavMember("d1", "C:\\x")), store.snapshot()[0].members)
            assertEquals(5L, store.snapshot()[0].createdAt)

            script.set(
                FavoritesStore.KV_KEY,
                """{"v":2,"updated_at":42,"favorites":[{"id":"new-v2","name":"新v2条目","created_at":42,"members":[]}]}""",
            )
            store.refresh()
            assertTrue("v>1 只读", store.readOnly.value)
            assertEquals("已知字段尽力解析", listOf("新v2条目"), store.snapshot().map { it.name })
        } finally {
            e.close()
        }
    }

    // ── ③v:2 前向守卫:readOnly + 编辑拒 + kv.put 零调用 ────────────────────

    @Test
    fun `v2 前向守卫只读且编辑零回写`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(
                FavoritesStore.KV_KEY,
                """{"v":2,"updated_at":42,"favorites":[{"id":"new-v2","name":"新v2条目","created_at":42,"members":[]}]}""",
            )
            val store = FavoritesStore(e.repo)
            store.refresh()
            assertTrue(store.readOnly.value)

            val ok = store.put { cur -> cur + Favorite("local-1", "本地新建", t0, emptyList()) }
            assertFalse("只读拒编辑", ok)
            assertTrue("守卫旗保持", store.readOnly.value)
            assertEquals("编辑前置仍取一次(守卫在取后裁定)", 2, script.kvGetCount)
            assertEquals("前向守卫绝不回写", 0, script.kvPutCount)
        } finally {
            e.close()
        }
    }

    // ── ④put 前置取最新:两次不同 kv.get,put=并发条目+新条目(逐字节 golden)──

    @Test
    fun `put 前置取最新合并他端并发条目且payload逐字节同构`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(FavoritesStore.KV_KEY, seedDoc(listOf(fav("seed-1", "已有条目"))))
            val store = FavoritesStore(e.repo)
            store.refresh()
            assertEquals(1, store.snapshot().size)

            // 注入他端并发新文档:编辑前置 kv.get 必读到
            script.set(
                FavoritesStore.KV_KEY,
                seedDoc(listOf(fav("seed-1", "已有条目"), fav("seed-2", "他端并发新增"))),
            )
            val ok = store.put { cur ->
                cur + Favorite("local-1", "本地新建", t0, listOf(FavMember("d1", "C:\\go")))
            }
            assertTrue(ok)
            assertEquals("编辑前置必再取一次", 2, script.kvGetCount)
            assertEquals(1, script.kvPutCount)

            // put payload 逐字节锁定(iOS Codable/TS JSON.stringify 键序金标准)
            val put = script.putValues[0]
            val stamp = JVal.parse(put).opt("updated_at")!!.asLong()
            assertTrue("写回统一盖新戳", stamp >= t0)
            assertEquals(
                "{\"v\":1,\"updated_at\":$stamp,\"favorites\":[" +
                    "{\"id\":\"seed-1\",\"name\":\"已有条目\",\"created_at\":$t0,\"members\":[]}," +
                    "{\"id\":\"seed-2\",\"name\":\"他端并发新增\",\"created_at\":$t0,\"members\":[]}," +
                    "{\"id\":\"local-1\",\"name\":\"本地新建\",\"created_at\":$t0,\"members\":[{\"device\":\"d1\",\"path\":\"C:\\\\go\"}]}]}",
                put,
            )
            // put 成功才落状态:本地整单与 put 一致
            assertEquals(listOf("已有条目", "他端并发新增", "本地新建"), store.snapshot().map { it.name })
            assertFalse(store.readOnly.value)
        } finally {
            e.close()
        }
    }

    // ── ⑤kvPut 失败:保旧值、不自动重试 ─────────────────────────────────────

    @Test
    fun `put 失败保旧值不自动重试`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(FavoritesStore.KV_KEY, seedDoc(listOf(fav("keep-1", "旧值"))))
            val store = FavoritesStore(e.repo)
            store.refresh()

            script.putFailure = true
            val ok = store.put { cur -> cur + Favorite("local-1", "不会落盘", t0, emptyList()) }
            assertFalse(ok)
            assertEquals("无自动重试", 1, script.kvPutCount)
            assertEquals("put 失败保旧值", listOf("旧值"), store.snapshot().map { it.name })
        } finally {
            e.close()
        }
    }

    // ── ⑥transform null(无可写变更)不回写 ─────────────────────────────────

    @Test
    fun `transform null 不回写`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(FavoritesStore.KV_KEY, seedDoc(listOf(fav("f1", "项目A"))))
            val store = FavoritesStore(e.repo)
            store.refresh()
            val gets = script.kvGetCount

            val ok = store.put { null }
            assertFalse(ok)
            assertEquals("无可写变更不回写", 0, script.kvPutCount)
            assertEquals("前置取仍发生", gets + 1, script.kvGetCount)
        } finally {
            e.close()
        }
    }

    // ── ⑦清缓存:快照与只读旗复位 ────────────────────────────────────────────

    @Test
    fun `clearFavCache 清快照与只读旗`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(
                FavoritesStore.KV_KEY,
                """{"v":2,"updated_at":42,"favorites":[{"id":"new-v2","name":"新v2条目","created_at":42,"members":[]}]}""",
            )
            val store = FavoritesStore(e.repo)
            store.refresh()
            assertTrue(store.readOnly.value)
            assertEquals(listOf("新v2条目"), store.snapshot().map { it.name })

            store.clearFavCache()
            assertTrue("清后快照空(旧账号绝不复用)", store.snapshot().isEmpty())
            assertFalse("只读旗复位", store.readOnly.value)
        } finally {
            e.close()
        }
    }

    // ── ⑧watcher 接线:登录拉全量 / data_changed 增量重拉他 key 忽略 / 登出+被踢清缓存

    @Test
    fun `watcher 登录拉全量 data_changed 增量重拉他key忽略`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(FavoritesStore.KV_KEY, seedDoc(listOf(fav("f1", "项目A"))))
            val store = FavoritesStore(e.repo)
            val loggedIn = MutableStateFlow<Boolean?>(null)
            FavoritesWatcher(store, loggedIn, e.hub, e.scope)

            // 登录态翻转 → 拉全量
            loggedIn.value = true
            awaitUntil { store.snapshot().map { it.name } == listOf("项目A") }
            val pullsAfterLogin = script.kvGetCount

            // 他端编辑落盘 → data_changed{favorites} → 增量重拉
            script.set(
                FavoritesStore.KV_KEY,
                seedDoc(listOf(fav("f1", "项目A"), fav("f2", "他端新增"))),
            )
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.data_changed","params":{"key":"favorites"}}""",
            )
            awaitUntil { store.snapshot().map { it.name } == listOf("项目A", "他端新增") }
            val pullsAfterEvent = script.kvGetCount
            assertTrue("data_changed 触发重拉", pullsAfterEvent > pullsAfterLogin)

            // 他 key 忽略:零重拉
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.data_changed","params":{"key":"bookmarks"}}""",
            )
            delay(200)
            assertEquals("他 key 不得触发重拉", pullsAfterEvent, script.kvGetCount)
        } finally {
            e.close()
        }
    }

    @Test
    fun `watcher 登出与被踢清缓存`() = TestKit.runTest {
        val e = TestEnv()
        try {
            val script = KvScript()
            e.fake.onLine = script.handler()
            script.set(FavoritesStore.KV_KEY, seedDoc(listOf(fav("f1", "项目A"))))
            val store = FavoritesStore(e.repo)
            val loggedIn = MutableStateFlow<Boolean?>(null)
            FavoritesWatcher(store, loggedIn, e.hub, e.scope)

            loggedIn.value = true
            awaitUntil { store.snapshot().isNotEmpty() }

            // 登出:清缓存(旧账号清单绝不可瞬时复用)
            loggedIn.value = false
            awaitUntil { store.snapshot().isEmpty() }
            assertFalse(store.readOnly.value)

            // 重登:再拉全量
            loggedIn.value = true
            awaitUntil { store.snapshot().map { it.name } == listOf("项目A") }

            // 被踢:清缓存
            e.fake.connections[0].sendLine(
                """{"jsonrpc":"2.0","method":"event.device_evicted","params":{}}""",
            )
            awaitUntil { store.snapshot().isEmpty() }
        } finally {
            e.close()
        }
    }

    // ── 键名(契约恒定,不改)────────────────────────────────────────────────

    @Test
    fun `kv键恒favorites`() {
        assertEquals("favorites", FavoritesStore.KV_KEY)
    }
}
