package com.mtdss.android.ui.browse

import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.opt
import com.mtdss.android.test.TestEnv
import com.mtdss.android.test.TestKit
import com.mtdss.android.test.TestKit.awaitUntil
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 浏览 VM JVM 单测:根卷/目录导航、seq 守卫(过期响应丢弃)、截断标志、
 * 选择集、下载收件箱(dir="inbox" 哨兵 + skip/overwrite 策略面)。
 */
class BrowseViewModelTest {

    private val device = DirectoryDevice(
        deviceId = "dev-peer-fixed",
        name = "契约对端",
        platform = "linux",
        online = true,
    )

    /** 路径 → 列表内容脚本(根卷/文档目录/慢目录)。JSON 恒单行:NDJSON
     *  一行一帧,嵌入换行会被客户端按垃圾帧丢弃。 */
    private fun listingFor(path: String): Pair<String, Boolean> = when (path) {
        "" -> ("[" +
            "{\"name\":\"docs\",\"path\":\"/docs\",\"size\":0,\"mtime_ms\":0,\"is_dir\":true}," +
            "{\"name\":\"a.bin\",\"path\":\"/a.bin\",\"size\":1024,\"mtime_ms\":1760000000000,\"is_dir\":false}," +
            "{\"name\":\"b.bin\",\"path\":\"/b.bin\",\"size\":2048,\"mtime_ms\":1760000000001,\"is_dir\":false}" +
            "]") to false
        "/docs" -> ("[" +
            "{\"name\":\"深 件.bin\",\"path\":\"/docs/深 件.bin\",\"size\":4096,\"mtime_ms\":1760000000002,\"is_dir\":false}" +
            "]") to false
        "/docs/many" -> ("[" +
            "{\"name\":\"x\",\"path\":\"/docs/many/x\",\"size\":0,\"mtime_ms\":0,\"is_dir\":false}" +
            "]") to true
        "/slow" -> ("[" +
            "{\"name\":\"慢.bin\",\"path\":\"/slow/慢.bin\",\"size\":1,\"mtime_ms\":0,\"is_dir\":false}" +
            "]") to false
        else -> "[]" to false
    }

    /** 伪 daemon:按请求 params.path 应答列表(可选延迟);transfer.start 应答 job_id。 */
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

                "transfer.start" -> conn.sendLine("""{"id":$id,"result":{"job_id":"42"}}""")
                else -> error("意外的请求: ${req.opt("method")}")
            }
        }
    }

    private fun newVm(e: TestEnv): BrowseViewModel = BrowseViewModel(device, e.repo, e.scope)

    @Test
    fun `打开根卷列表`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.entries.size == 3 }
            assertEquals("", vm.state.value.path)
            assertFalse(vm.state.value.truncated)
            assertFalse(vm.state.value.loading)
            // 服务端收到的请求面:path 空串 = 根卷
            val line = JVal.parse(String(e.fake.connections[0].received[0], Charsets.UTF_8))
            assertEquals("", line.opt("params")!!.opt("path")!!.asString())
        } finally {
            e.close()
        }
    }

    @Test
    fun `目录导航与上级`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.entries.size == 3 }
            vm.open("/docs") // 目录条目再入
            awaitUntil { vm.state.value.path == "/docs" && vm.state.value.entries.size == 1 }
            assertEquals("深 件.bin", vm.state.value.entries[0].name)
            vm.up()
            // POSIX 逐级上溯:/docs → /(根目录,fixture 为空)→ ""(卷列表)
            awaitUntil { vm.state.value.path == "/" }
            vm.up()
            awaitUntil { vm.state.value.path == "" && vm.state.value.entries.size == 3 }
            // 卷列表的上级:空操作(设备层由路由栈承担)——不发出新请求
            val requestsBefore = e.fake.connections[0].received.size
            vm.up()
            kotlinx.coroutines.delay(150)
            assertEquals(requestsBefore, e.fake.connections[0].received.size)
            assertEquals("", vm.state.value.path)
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
    fun `过期响应丢弃 seq 守卫`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e, slowPaths = setOf("/slow"))
            val vm = newVm(e)
            vm.open("/slow")
            vm.open("/docs") // 快速换向:慢响应到达时已过期
            awaitUntil { !vm.state.value.loading && vm.state.value.entries.size == 1 }
            assertEquals("/docs", vm.state.value.path)
            // 慢路径的响应不得覆盖新位置
            awaitUntil { e.fake.connections[0].received.size == 2 }
            kotlinx.coroutines.delay(300)
            assertEquals("/docs", vm.state.value.path)
            assertEquals(1, vm.state.value.entries.size)
            assertEquals("深 件.bin", vm.state.value.entries[0].name)
        } finally {
            e.close()
        }
    }

    @Test
    fun `选择与下载跳过策略`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.entries.size == 3 }
            vm.toggle("/a.bin")
            vm.toggle("/b.bin")
            assertEquals(setOf("/a.bin", "/b.bin"), vm.state.value.selected)
            vm.download(overwrite = false)
            awaitUntil { vm.state.value.startedJob != null }
            assertEquals("42", vm.state.value.startedJob)
            assertEquals(emptySet<String>(), vm.state.value.selected)
            assertNull(vm.state.value.downloadError)
            // 请求面:download 形态 + dir="inbox" 哨兵 + 默认空策略
            val req = e.fake.connections[0].received.map { JVal.parse(String(it, Charsets.UTF_8)) }
                .first { it.opt("method")!!.asString() == "transfer.start" }
            val p = req.opt("params")!!
            assertEquals("download", p.opt("kind")!!.asString())
            assertEquals("dev-peer-fixed", p.opt("device")!!.asString())
            assertEquals(listOf("/a.bin", "/b.bin"), p.opt("paths")!!.asArray().map { it.asString() })
            assertEquals("inbox", p.opt("dir")!!.asString())
            assertTrue(p.opt("skip")!!.asArray().isEmpty())
            assertTrue(p.opt("overwrite")!!.asArray().isEmpty())
            assertFalse(p.opt("cut")!!.asBoolean())
        } finally {
            e.close()
        }
    }

    @Test
    fun `下载覆盖策略带顶层名`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.entries.size == 3 }
            vm.toggle("/docs") // 目录也可选:整树下载(协议原生支持)
            vm.toggle("/a.bin")
            vm.download(overwrite = true)
            awaitUntil { vm.state.value.startedJob != null }
            val req = e.fake.connections[0].received.map { JVal.parse(String(it, Charsets.UTF_8)) }
                .first { it.opt("method")!!.asString() == "transfer.start" }
            val p = req.opt("params")!!
            // overwrite 名单 = 顶层名(daemon collect_items 的 rel 语义:输入名)
            assertEquals(
                listOf("docs", "a.bin").sorted(),
                p.opt("overwrite")!!.asArray().map { it.asString() }.sorted(),
            )
            assertEquals(listOf("/docs", "/a.bin").sorted(),
                p.opt("paths")!!.asArray().map { it.asString() }.sorted())
        } finally {
            e.close()
        }
    }

    @Test
    fun `下载失败转错误反馈`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
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
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.entries.size == 3 }
            vm.toggle("/a.bin")
            vm.download(overwrite = false)
            awaitUntil { vm.state.value.downloadError != null }
            assertTrue(vm.state.value.downloadError!!.contains("设备离线"))
            // 失败不清选择(用户可重试或改选)
            assertEquals(setOf("/a.bin"), vm.state.value.selected)
        } finally {
            e.close()
        }
    }

    @Test
    fun `toggle 只收当前列表条目且导航清选择`() = TestKit.runTest {
        val e = TestEnv()
        try {
            scriptList(e)
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.entries.size == 3 }
            vm.toggle("/ghost") // 不在当前列表:忽略(防陈旧选择混入)
            assertEquals(emptySet<String>(), vm.state.value.selected)
            vm.toggle("/a.bin")
            assertEquals(setOf("/a.bin"), vm.state.value.selected)
            vm.toggle("/a.bin") // 再点取消
            assertEquals(emptySet<String>(), vm.state.value.selected)
            vm.toggle("/a.bin")
            vm.open("/docs") // 导航换目录:选择清空
            awaitUntil { vm.state.value.path == "/docs" }
            assertEquals(emptySet<String>(), vm.state.value.selected)
        } finally {
            e.close()
        }
    }

    @Test
    fun `list 错误转错误态不清已选路径位`() = TestKit.runTest {
        val e = TestEnv()
        try {
            e.fake.onLine = { conn, line ->
                val req = JVal.parse(String(line, Charsets.UTF_8))
                val id = req.opt("id")!!.asLong()
                conn.sendLine("""{"id":$id,"error":{"code":3,"message":"设备离线: dev-peer-fixed"}}""")
            }
            val vm = newVm(e)
            vm.open("")
            awaitUntil { vm.state.value.error != null }
            assertTrue(vm.state.value.error!!.contains("设备离线"))
            assertFalse(vm.state.value.loading)
            assertTrue(vm.state.value.entries.isEmpty())
        } finally {
            e.close()
        }
    }
}
