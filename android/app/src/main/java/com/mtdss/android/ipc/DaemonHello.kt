package com.mtdss.android.ipc

import android.net.LocalSocket
import android.net.LocalSocketAddress
import org.json.JSONObject
import java.io.File
import java.io.IOException

/**
 * 【临时】一次性 NDJSON hello 客户端 —— T7 垂直骨架验证专用,T8 将以正式
 * IpcClient(LocalSocket 连接 + id 路由 + 事件流)整体替换本文件。
 *
 * 线协议与 crates/ipc 契约逐字节对齐(hello 的 golden 锚:
 * crates/ipc/contract/hello.req.json):
 *   请求 {"id":1,"jsonrpc":"2.0","method":"hello","params":{}} + '\n'
 *   响应 {"id":1,"result":{"device":{...},"logged_in":..,"version":".."}}
 * 连接上同时会广播事件帧(带 "method": "event.*"、无 id),须跳过只认
 * id 匹配的响应帧。socket 为 Unix 文件系统套接字
 * (LocalSocket Namespace.FILESYSTEM),路径由 MtdssEngine.daemonSocketFile
 * 派生({filesDir}/mtdss/daemon.sock)。
 */
object DaemonHello {

    /** hello 响应的可展示摘要(version/logged_in 为验收要点)。 */
    data class Hello(
        val version: String,
        val loggedIn: Boolean,
        val deviceId: String,
        val rawLine: String,
    )

    /**
     * 阻塞调用(调用方自行切 IO 线程)。连接失败/超时/业务错误抛 [IOException]
     * 或 [IllegalStateException],由调试屏按「引擎尚未就绪」处理。
     *
     * 注意:写完请求不得先关输出流——daemon 侧把客户端 EOF 视为「读半工
     * 终局」随即断开(ipc write_loop 的 read_gone 直达断开),会丢弃尚未
     * 写出的响应帧。读完响应后 finally 里统一 close 整个 socket。
     */
    fun call(socketFile: File, timeoutMs: Int = 3_000): Hello {
        val socket = LocalSocket() // 命名空间只由 LocalSocketAddress 决定,无参构造即未绑定套接字
        try {
            socket.connect(
                LocalSocketAddress(socketFile.absolutePath, LocalSocketAddress.Namespace.FILESYSTEM),
            )
            socket.soTimeout = timeoutMs
            socket.outputStream.apply {
                write(REQUEST_BYTES)
                flush()
            }
            val reader = socket.inputStream.bufferedReader(Charsets.UTF_8)
            while (true) {
                val line = reader.readLine()
                    ?: throw IOException("daemon 在响应前关闭了连接(引擎可能仍在启动)")
                val frame = JSONObject(line)
                if (frame.has("method")) continue // 事件广播帧(event.*),非本请求响应
                if (frame.optInt("id") != 1) continue
                val error = frame.optJSONObject("error")
                if (error != null) {
                    throw IllegalStateException("hello 业务错误: $error")
                }
                val result = frame.getJSONObject("result")
                return Hello(
                    version = result.getString("version"),
                    loggedIn = result.getBoolean("logged_in"),
                    deviceId = result.getJSONObject("device").optString("device_id"),
                    rawLine = line,
                )
            }
        } finally {
            runCatching { socket.close() }
        }
    }

    /** 与 golden hello.req.json 逐字节一致的请求帧(含行终止符)。 */
    private val REQUEST_BYTES: ByteArray =
        """{"id":1,"jsonrpc":"2.0","method":"hello","params":{}}""".toByteArray(Charsets.UTF_8) +
            "\n".toByteArray(Charsets.UTF_8)
}
