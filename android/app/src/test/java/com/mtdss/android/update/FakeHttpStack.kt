package com.mtdss.android.update

import java.io.File
import java.io.FileOutputStream

/**
 * 伪 HTTP 面(JVM 单测):按编程 Entry 应答,记录全部请求 URL。
 * contentLength >= 0 时模拟 Content-Length 前置复核(与生产
 * UrlConnectionHttpStack 同判定)。onProgress 逐字节回调(时间面节流由
 * UpdateController 的 ProgressThrottle 负责,这里不做)。
 */
class FakeHttpStack : HttpStack {

    /** 一次应答的编程面:bytes = 2xx 载荷;status 非 2xx 即 HttpError;error 优先。 */
    data class Entry(
        val bytes: ByteArray? = null,
        val status: Int = 200,
        val contentLength: Long = -1L,
        val error: Exception? = null,
    )

    val gets = mutableListOf<String>()
    val downloads = mutableListOf<String>()

    var getEntry: Entry = Entry()
    var downloadEntry: Entry = Entry()

    override fun fetch(url: String): Result<ByteArray> {
        gets += url
        val e = getEntry
        e.error?.let { return Result.failure(it) }
        if (e.status !in 200..299) return Result.failure(HttpStack.HttpError(e.status, "HTTP ${e.status}"))
        return Result.success(e.bytes ?: ByteArray(0))
    }

    override fun download(url: String, dest: File, expectedSize: Long, onProgress: (Long) -> Unit): Result<Pair<Long, String>> {
        downloads += url
        val e = downloadEntry
        e.error?.let { return Result.failure(it) }
        if (e.status !in 200..299) return Result.failure(HttpStack.HttpError(e.status, "HTTP ${e.status}"))
        val bytes = e.bytes ?: ByteArray(0)
        if (e.contentLength >= 0 && e.contentLength != expectedSize) {
            return Result.failure(HttpStack.ContentLengthMismatch(e.contentLength, expectedSize))
        }
        dest.parentFile?.mkdirs()
        FileOutputStream(dest).use { it.write(bytes) }
        var received = 0L
        for (b in bytes) {
            received++
            onProgress(received)
        }
        return Result.success(bytes.size.toLong() to UpdateFixtures.sha256Hex(bytes))
    }
}
