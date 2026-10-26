package com.mtdss.android.update

import com.mtdss.android.test.TestKit.awaitUntil
import com.mtdss.android.test.TestKit.runTest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList

/**
 * 检查更新编排 JVM 单测(伪 HTTP 面):状态机全链(未配置 fail-closed /
 * 检查三分支 / 下载复核四失败面 / 重试口径)与失败词面逐例锁定——词面镜像
 * crates/daemon/src/update.rs(同一信任链的两端,失败模式一致)。
 */
class UpdateControllerTest {

    private val apk = UpdateFixtures.apkBytes
    private val apkSha = UpdateFixtures.sha256Hex(apk)

    /** 已签安卓清单 JSON(version + 资产可定制;seed=1 私钥签)。 */
    private fun manifestJson(version: String, asset: PlatformAsset): String =
        UpdateFixtures.manifestJson(UpdateFixtures.signed(UpdateFixtures.androidManifest(version, asset), 1))

    private fun apkAsset(url: String = "https://dl.example.invalid/dl/android/9.9.9/MTDSS_9.9.9_arm64.apk"): PlatformAsset =
        UpdateFixtures.apkAsset(url)

    private class Harness(
        val controller: UpdateController,
        val http: FakeHttpStack,
        val scope: CoroutineScope,
        val dir: File,
        val installed: CopyOnWriteArrayList<File>,
        val seen: CopyOnWriteArrayList<UpdateUiState>,
    ) {
        fun close() {
            scope.cancel()
        }
    }

    private fun harness(
        base: String = "https://dl.example.invalid/dl",
        pub: String = UpdateFixtures.pubHex(1),
        current: String = "0.1.0",
        progressGapMs: Long = 0,
        http: FakeHttpStack = FakeHttpStack(),
        installer: (File) -> Unit = {},
    ): Harness {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val dir = Files.createTempDirectory("mtdss-update-test").toFile()
        val installed = CopyOnWriteArrayList<File>()
        val seen = CopyOnWriteArrayList<UpdateUiState>()
        // 下载缺省应答 = 假 APK 本体(失败路径用例随后自行覆写)
        http.downloadEntry = FakeHttpStack.Entry(bytes = UpdateFixtures.apkBytes)
        val controller = UpdateController(
            dlBase = base,
            pubkeyHex = pub,
            currentVersion = current,
            dir = dir,
            http = http,
            installer = { f -> installer(f); installed.add(f) },
            scope = scope,
            progressMinGapMs = progressGapMs,
        )
        scope.launch { controller.state.collect { seen.add(it) } }
        return Harness(controller, http, scope, dir, installed, seen)
    }

    // ── 未配置 fail-closed ─────────────────────────────────────────────────────

    @Test
    fun `未配置基址或公钥即 Unconfigured 且动作无效`() = runTest {
        for (base in listOf("", "   ")) {
            val h = harness(base = base)
            try {
                assertEquals(UpdateUiState.Unconfigured, h.controller.state.value)
                h.controller.check()
                h.controller.download()
                h.controller.retry()
                assertTrue("未配置下不得触网", h.http.gets.isEmpty() && h.http.downloads.isEmpty())
            } finally {
                h.close()
            }
        }
        // 公钥空 / 非法 hex / 长度错:同 Unconfigured(镜像 M4 fail-closed)
        for (pub in listOf("", "zz".repeat(32), "a".repeat(63))) {
            val h = harness(pub = pub)
            try {
                assertEquals(UpdateUiState.Unconfigured, h.controller.state.value)
            } finally {
                h.close()
            }
        }
    }

    // ── 检查 ──────────────────────────────────────────────────────────────────

    @Test
    fun `检查发现新版本`() = runTest {
        val h = harness(base = "https://dl.example.invalid/dl/") // 尾斜杠:URL 拼接收敛
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            assertEquals(
                UpdateUiState.Available("9.9.9", "测试更新说明"),
                h.controller.state.value,
            )
            assertEquals(listOf("https://dl.example.invalid/dl/android/manifest.json"), h.http.gets)
        } finally {
            h.close()
        }
    }

    @Test
    fun `同版本与旧版本已是最新`() = runTest {
        for (ver in listOf("0.1.0", "0.0.9")) {
            val h = harness()
            try {
                h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson(ver, apkAsset()).toByteArray())
                h.controller.check()
                awaitUntil { h.controller.state.value is UpdateUiState.UpToDate }
                assertEquals(UpdateUiState.UpToDate("0.1.0"), h.controller.state.value)
            } finally {
                h.close()
            }
        }
    }

    @Test
    fun `源 404 即无更新而非失败`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(status = 404)
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.UpToDate }
        } finally {
            h.close()
        }
    }

    @Test
    fun `拉取失败落 Failed 且词面镜像`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(status = 500)
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("拉取更新清单失败: HTTP 500"), h.controller.state.value)
        } finally {
            h.close()
        }
    }

    @Test
    fun `坏签名落 Failed`() = runTest {
        val h = harness(pub = UpdateFixtures.pubHex(2)) // 清单 seed=1 签,公钥 2 验
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("清单验签失败"), h.controller.state.value)
        } finally {
            h.close()
        }
    }

    @Test
    fun `非法版本落 Failed`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("1.2", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("清单版本非法(须为严格 X.Y.Z)"), h.controller.state.value)
        } finally {
            h.close()
        }
    }

    @Test
    fun `解析失败落 Failed`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = "{\"version\":}".toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            val err = (h.controller.state.value as UpdateUiState.Failed).error
            assertTrue(err.startsWith("解析更新清单失败"))
        } finally {
            h.close()
        }
    }

    @Test
    fun `下载在途时检查被防抖`() = runTest {
        // busy() 守卫(在途态点检查无效)。检查自身对同步伪桩无在途窗(单线程
        // inline 跑完),下载臂经 withContext(IO) 有真实挂起点——Downloading
        // 同步翻转后、IO 派发完成前的检查必然被拒。
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            h.controller.check() // Downloading/Installing 在途:无效
            awaitUntil { h.controller.state.value is UpdateUiState.Installing }
            assertEquals("在途检查不得触网", 1, h.http.gets.size)
        } finally {
            h.close()
        }
    }

    // ── 下载 ──────────────────────────────────────────────────────────────────

    @Test
    fun `下载全链到安装器`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Installing }
            assertEquals(UpdateUiState.Installing("9.9.9"), h.controller.state.value)
            // 就位文件 + 无 .part 残留 + 内容逐字节
            val apkFile = File(h.dir, "mtdss-9.9.9.apk")
            assertTrue(apkFile.isFile)
            assertEquals(1, h.dir.listFiles()!!.size)
            assertTrue(apkFile.readBytes().contentEquals(apk))
            assertEquals(1, h.installed.size)
            assertEquals(apkFile.absolutePath, h.installed[0].absolutePath)
            // 下载 URL = 清单资产 URL(验签后才受信,不另设白名单)
            assertEquals(listOf("https://dl.example.invalid/dl/android/9.9.9/MTDSS_9.9.9_arm64.apk"), h.http.downloads)
            // 进度面:至少见过一个 Downloading(节流 0 = 全量回调)
            assertTrue(h.seen.any { it is UpdateUiState.Downloading && it.total == apk.size.toLong() })
        } finally {
            h.close()
        }
    }

    @Test
    fun `无缓存清单时 download 无效`() = runTest {
        val h = harness()
        try {
            h.controller.download() // Idle 且无清单缓存:静默无效
            assertTrue(h.http.downloads.isEmpty())
            assertEquals(UpdateUiState.Idle, h.controller.state.value)
        } finally {
            h.close()
        }
    }

    @Test
    fun `清单缺安卓资产在下载时拒绝`() = runTest {
        val h = harness()
        try {
            // 平台键错(非 ANDROID_PLATFORM_KEY):检查不拒(镜像 M4 资产查找在 apply),
            // 下载时按「缺少资产」拒
            val m = UpdateFixtures.signed(
                UpdateManifest(
                    version = "9.9.9",
                    notes = "测试更新说明",
                    pubdate = "2026-10-26T00:00:00Z",
                    platforms = linkedMapOf("windows-x86_64" to apkAsset()),
                    signature = "",
                ),
                1,
            )
            h.http.getEntry = FakeHttpStack.Entry(bytes = UpdateFixtures.manifestJson(m).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("清单缺少 $ANDROID_PLATFORM_KEY 资产"), h.controller.state.value)
            assertTrue("拒绝路径零落盘", h.http.downloads.isEmpty())
        } finally {
            h.close()
        }
    }

    @Test
    fun `sha 不符即删临时件`() = runTest {
        val h = harness()
        try {
            val badShaAsset = PlatformAsset(
                url = "https://dl.example.invalid/dl/android/9.9.9/x.apk",
                sha256 = "c".repeat(64),
                size = apk.size.toLong(),
            )
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", badShaAsset).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("下载复核失败: SHA-256 不符(得 $apkSha)"), h.controller.state.value)
            assertEquals("临时件即删(系统零改动)", 0, h.dir.listFiles()!!.size)
            assertEquals(0, h.installed.size)
        } finally {
            h.close()
        }
    }

    @Test
    fun `字节数不符即删临时件`() = runTest {
        val h = harness()
        try {
            val badSizeAsset = PlatformAsset(
                url = "https://dl.example.invalid/dl/android/9.9.9/x.apk",
                sha256 = apkSha,
                size = apk.size.toLong() + 1,
            )
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", badSizeAsset).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(
                UpdateUiState.Failed("下载复核失败: 字节数不符(${apk.size} != ${apk.size + 1})"),
                h.controller.state.value,
            )
            assertEquals(0, h.dir.listFiles()!!.size)
        } finally {
            h.close()
        }
    }

    @Test
    fun `响应长度前置复核`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.http.downloadEntry = FakeHttpStack.Entry(bytes = apk, contentLength = apk.size.toLong() - 1)
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(
                UpdateUiState.Failed("下载复核失败: 响应长度 ${apk.size - 1} 与清单不符(${apk.size})"),
                h.controller.state.value,
            )
        } finally {
            h.close()
        }
    }

    @Test
    fun `下载网络失败即删临时件`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.http.downloadEntry = FakeHttpStack.Entry(error = java.io.IOException("connection reset"))
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            val err = (h.controller.state.value as UpdateUiState.Failed).error
            assertTrue(err, err.startsWith("下载失败"))
            assertEquals(0, h.dir.listFiles()!!.size)
        } finally {
            h.close()
        }
    }

    @Test
    fun `下载 4xx 即删临时件且词面带状态码`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.http.downloadEntry = FakeHttpStack.Entry(status = 404)
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("下载失败: HTTP 404"), h.controller.state.value)
            assertEquals(0, h.dir.listFiles()!!.size)
        } finally {
            h.close()
        }
    }

    @Test
    fun `rename 失败落 Failed 且临时件清`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            // 预占最终名(非空目录):renameTo 必败(空目录在本测试 JVM 上会被
            // 原子替换,非空目录不可——RenameDiag 探测结论,跨平台 ENOTEMPTY 同形)
            val occupy = File(h.dir, "mtdss-9.9.9.apk")
            occupy.mkdirs()
            File(occupy, "inner").writeBytes(byteArrayOf(1))
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            val err = (h.controller.state.value as UpdateUiState.Failed).error
            assertTrue(err, err.startsWith("下载就位失败(rename)"))
            // 只剩预占目录:.part 已删
            assertEquals(listOf("mtdss-9.9.9.apk"), h.dir.listFiles()!!.map { it.name })
        } finally {
            h.close()
        }
    }

    @Test
    fun `安装器拉起失败转 Failed`() = runTest {
        val h = harness(installer = { throw IllegalStateException("no installer") })
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            assertEquals(UpdateUiState.Failed("无法拉起安装器: no installer"), h.controller.state.value)
            // APK 已就位(rename 先于拉起),不清(重试=重拉)
            assertTrue(File(h.dir, "mtdss-9.9.9.apk").isFile)
        } finally {
            h.close()
        }
    }

    // ── 重试(镜像 M4:清单缓存保留,失败后免重查)───────────────────────────────

    @Test
    fun `下载失败后重试免重查直下`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.http.downloadEntry = FakeHttpStack.Entry(status = 500)
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            val getsAfterFail = h.http.gets.size

            h.http.downloadEntry = FakeHttpStack.Entry(bytes = apk)
            h.controller.retry()
            awaitUntil { h.controller.state.value is UpdateUiState.Installing }
            assertEquals("重试不重查清单", getsAfterFail, h.http.gets.size)
            assertEquals(2, h.http.downloads.size)
            assertTrue(File(h.dir, "mtdss-9.9.9.apk").isFile)
        } finally {
            h.close()
        }
    }

    @Test
    fun `检查失败后重试走重查`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(status = 500)
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }

            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.retry()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            assertEquals(2, h.http.gets.size)
        } finally {
            h.close()
        }
    }

    @Test
    fun `安装器失败后重试重拉而非重下`() = runTest {
        var fail = true
        val h = harness(installer = { if (fail) throw IllegalStateException("denied") })
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Failed }
            val downloadsAfterFail = h.http.downloads.size

            fail = false
            h.controller.retry()
            awaitUntil { h.controller.state.value is UpdateUiState.Installing }
            assertEquals("重拉安装器不重下 APK", downloadsAfterFail, h.http.downloads.size)
        } finally {
            h.close()
        }
    }

    @Test
    fun `Installing 态重试即重拉安装器`() = runTest {
        val h = harness()
        try {
            h.http.getEntry = FakeHttpStack.Entry(bytes = manifestJson("9.9.9", apkAsset()).toByteArray())
            h.controller.check()
            awaitUntil { h.controller.state.value is UpdateUiState.Available }
            h.controller.download()
            awaitUntil { h.controller.state.value is UpdateUiState.Installing }
            h.controller.retry()
            awaitUntil { h.installed.size == 2 }
            assertEquals(UpdateUiState.Installing("9.9.9"), h.controller.state.value)
            assertEquals(1, h.http.downloads.size)
        } finally {
            h.close()
        }
    }

    // ── 进度节流(纯函数面,确定性验证)────────────────────────────────────────

    @Test
    fun `进度节流首条即发其后限距终条恒发`() {
        var now = 1_000L
        val t = ProgressThrottle(minGapMs = 400, clock = { now })
        assertTrue(t.shouldEmit(force = false)) // 首条
        now += 100
        assertTrue(!t.shouldEmit(force = false)) // 距上次 100ms < 400ms
        now += 301
        assertTrue(t.shouldEmit(force = false)) // 401ms >= 400ms
        now += 1
        assertTrue(!t.shouldEmit(force = false))
        assertTrue(t.shouldEmit(force = true)) // 终条恒发(不受距限制)
        now += 1
        assertTrue(!t.shouldEmit(force = false)) // 终条后普通条仍按距
    }

    @Test
    fun `进度节流零距全放行`() {
        var now = 5L
        val t = ProgressThrottle(minGapMs = 0, clock = { now })
        repeat(5) {
            assertTrue(t.shouldEmit(force = false))
            now += 1
        }
    }
}
