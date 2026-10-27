package com.mtdss.android.update

import com.sun.net.httpserver.HttpServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.nio.file.Files

/**
 * 生产 HTTP 栈真跑测(JDK 内建 HttpServer 本地回环):状态码映射 / 流式落盘
 * 与 SHA-256 / Content-Length 前置复核 / 分块(无 Content-Length)形态。
 * HttpURLConnection 走真实网络栈,JVM 单测与实机同实现。
 */
class UrlConnectionHttpStackTest {

    private lateinit var server: HttpServer
    private lateinit var stack: UrlConnectionHttpStack
    private lateinit var tmp: File

    private val manifest = """{"version":"9.9.9","signature":"x"}""".toByteArray()
    private val apk = UpdateFixtures.apkBytes

    private var manifestStatus = 200
    private var apkStatus = 200
    private var apkDeclaredLength: Long? = apk.size.toLong() // null = 分块(不发长度头)

    private val base: String get() = "http://127.0.0.1:${server.address.port}/dl"

    @Before
    fun setUp() {
        stack = UrlConnectionHttpStack()
        tmp = Files.createTempDirectory("mtdss-http-test").toFile()
        server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext("/dl/android/manifest.json") { ex ->
            if (manifestStatus in 200..299) {
                ex.sendResponseHeaders(manifestStatus, manifest.size.toLong())
                ex.responseBody.use { it.write(manifest) }
            } else {
                ex.sendResponseHeaders(manifestStatus, -1)
                ex.close()
            }
        }
        server.createContext("/dl/android/9.9.9/app.apk") { ex ->
            if (apkStatus !in 200..299) {
                ex.sendResponseHeaders(apkStatus, -1)
                ex.close()
                return@createContext
            }
            val declared = apkDeclaredLength ?: run {
                // 分块形态:长度 0 = chunked
                ex.sendResponseHeaders(200, 0)
                ex.responseBody.use { it.write(apk) }
                return@createContext
            }
            // 声明长度可主动造假(测前置复核);实际体恒真身
            ex.sendResponseHeaders(200, declared)
            ex.responseBody.use { it.write(apk) }
        }
        server.start()
    }

    @After
    fun tearDown() {
        server.stop(0)
        tmp.deleteRecursively()
    }

    @Test
    fun `fetch 2xx 载荷逐字节`() {
        val res = stack.fetch("$base/android/manifest.json")
        assertTrue(res.isSuccess)
        assertTrue(res.getOrThrow().contentEquals(manifest))
    }

    @Test
    fun `fetch 404 与 500 映射 HttpError`() {
        manifestStatus = 404
        val e404 = stack.fetch("$base/android/manifest.json").exceptionOrNull()
        assertTrue(e404 is HttpStack.HttpError)
        assertEquals(404, (e404 as HttpStack.HttpError).code)
        assertEquals("HTTP 404", e404.message)

        manifestStatus = 500
        val e500 = stack.fetch("$base/android/manifest.json").exceptionOrNull()
        assertTrue(e500 is HttpStack.HttpError)
        assertEquals(500, (e500 as HttpStack.HttpError).code)
    }

    @Test
    fun `download 流式落盘 sha 与进度单调`() {
        val dest = File(tmp, "a.apk.part")
        val progress = mutableListOf<Long>()
        val res = stack.download("$base/android/9.9.9/app.apk", dest, apk.size.toLong()) { progress.add(it) }
        assertTrue(res.isSuccess)
        val (received, sha) = res.getOrThrow()
        assertEquals(apk.size.toLong(), received)
        assertEquals(UpdateFixtures.sha256Hex(apk), sha)
        assertTrue(dest.readBytes().contentEquals(apk))
        // 进度单调不减且终值 = 总量(单调 + 终值即覆盖最大值)
        assertTrue(progress.isNotEmpty())
        assertTrue(progress.zipWithNext().all { (a, b) -> b >= a })
        assertEquals(apk.size.toLong(), progress.last())
    }

    @Test
    fun `download 分块无长度头不受前置复核`() {
        apkDeclaredLength = null
        val dest = File(tmp, "b.apk.part")
        val res = stack.download("$base/android/9.9.9/app.apk", dest, apk.size.toLong()) {}
        assertTrue(res.isSuccess)
        assertEquals(UpdateFixtures.sha256Hex(apk), res.getOrThrow().second)
    }

    @Test
    fun `download 声明长度与清单不符即前置拒绝`() {
        // 裸 ServerSocket 手写响应头:sun HttpServer 对「声明长度 ≠ 实际体」
        // 的收尾会断流(客户端连头都拿不到, SOCKET 异常),注入不出稳定形态。
        val bogusLen = apk.size.toLong() + 7
        val server = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))
        val thread = Thread {
            server.accept().use { s ->
                // 先读完请求头(简单 GET 无请求体,读到空行止)
                val input = s.getInputStream()
                val req = StringBuilder()
                while (!req.endsWith("\r\n\r\n")) {
                    val b = input.read()
                    check(b >= 0) { "客户端提前断开" }
                    req.append(b.toChar())
                }
                s.getOutputStream().apply {
                    write("HTTP/1.1 200 OK\r\nContent-Length: $bogusLen\r\n\r\n".toByteArray(Charsets.ISO_8859_1))
                    write(apk)
                    flush()
                }
            }
            server.close()
        }
        thread.start()
        try {
            val dest = File(tmp, "c.apk.part")
            val res = stack.download("http://127.0.0.1:${server.localPort}/x.apk", dest, apk.size.toLong()) {}
            val e = res.exceptionOrNull()
            assertTrue("应前置拒绝,得 $e", e is HttpStack.ContentLengthMismatch)
            assertEquals("下载复核失败: 响应长度 $bogusLen 与清单不符(${apk.size})", e!!.message)
        } finally {
            thread.join(2_000)
        }
    }

    @Test
    fun `download 非 2xx 映射 HttpError`() {
        apkStatus = 404
        val dest = File(tmp, "d.apk.part")
        val res = stack.download("$base/android/9.9.9/app.apk", dest, apk.size.toLong()) {}
        val e = res.exceptionOrNull()
        assertTrue(e is HttpStack.HttpError)
        assertEquals(404, (e as HttpStack.HttpError).code)
    }
}
