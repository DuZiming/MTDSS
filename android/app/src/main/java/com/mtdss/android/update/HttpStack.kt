package com.mtdss.android.update

import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * 更新 HTTP 面(可注入缝:JVM 单测伪桩 / 本地 HttpServer 真跑,生产
 * [UrlConnectionHttpStack])。超时口径镜像 crates/daemon/src/update.rs:
 * 连接 10s / 清单 15s / 下载 600s。
 */
interface HttpStack {

    /** 非 2xx(404 由 UpdateController 按「源无更新」口径特判,其余转失败)。 */
    class HttpError(val code: Int, override val message: String) : Exception(message)

    /**
     * Content-Length 与清单 size 前置不符(M4「响应长度」复核:下载零字节前
     * 即拒绝)。词面已终形,UpdateController 原样上屏。
     */
    class ContentLengthMismatch(len: Long, expected: Long) :
        IllegalStateException("下载复核失败: 响应长度 $len 与清单不符($expected)")

    /** 拉取小件(manifest)。 */
    fun fetch(url: String): Result<ByteArray>

    /**
     * 流式下载到 dest(临时名 .part,调用方复核通过后 rename 就位):逐块
     * 回调已收字节,返回 (字节数, sha256 hex 小写)。失败时调用方负责删临时件。
     */
    fun download(url: String, dest: File, expectedSize: Long, onProgress: (Long) -> Unit): Result<Pair<Long, String>>
}

/**
 * 生产实现:HttpURLConnection(spec §6「Kotlin 自行下载」,零第三方 HTTP
 * 依赖)。SHA-256 逐块增量(与 daemon download_asset 的流式记账同构)。
 */
class UrlConnectionHttpStack(
    private val connectTimeoutMs: Int = CONNECT_TIMEOUT_MS,
    private val fetchTimeoutMs: Int = CHECK_TIMEOUT_MS,
    private val downloadTimeoutMs: Int = DOWNLOAD_TIMEOUT_MS,
) : HttpStack {

    override fun fetch(url: String): Result<ByteArray> = runCatching {
        val conn = open(url, fetchTimeoutMs)
        try {
            require2xx(conn)
            conn.inputStream.use { it.readBytes() }
        } finally {
            conn.disconnect()
        }
    }

    override fun download(url: String, dest: File, expectedSize: Long, onProgress: (Long) -> Unit): Result<Pair<Long, String>> =
        runCatching {
            val conn = open(url, downloadTimeoutMs)
            try {
                require2xx(conn)
                val declared = conn.contentLengthLong
                if (declared >= 0 && declared != expectedSize) {
                    throw HttpStack.ContentLengthMismatch(declared, expectedSize)
                }
                dest.parentFile?.mkdirs()
                val digest = MessageDigest.getInstance("SHA-256")
                var received = 0L
                FileOutputStream(dest).use { out ->
                    conn.inputStream.use { input ->
                        val buf = ByteArray(BUFFER_BYTES)
                        while (true) {
                            val n = input.read(buf)
                            if (n < 0) break
                            out.write(buf, 0, n)
                            digest.update(buf, 0, n)
                            received += n
                            onProgress(received)
                        }
                    }
                }
                received to hexLower(digest.digest())
            } finally {
                conn.disconnect()
            }
        }

    private fun open(url: String, readTimeoutMs: Int): HttpURLConnection {
        val conn = URL(url).openConnection() as HttpURLConnection
        conn.connectTimeout = connectTimeoutMs
        conn.readTimeout = readTimeoutMs
        conn.instanceFollowRedirects = true
        return conn
    }

    private fun require2xx(conn: HttpURLConnection) {
        val code = conn.responseCode
        if (code !in 200..299) throw HttpStack.HttpError(code, "HTTP $code")
    }

    private fun hexLower(bytes: ByteArray): String =
        bytes.joinToString("") { String.format(java.util.Locale.ROOT, "%02x", it) }

    private companion object {
        private const val CONNECT_TIMEOUT_MS = 10_000
        private const val CHECK_TIMEOUT_MS = 15_000
        private const val DOWNLOAD_TIMEOUT_MS = 600_000
        private const val BUFFER_BYTES = 64 * 1024
    }
}
