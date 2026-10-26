package com.mtdss.android.ipc

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.concurrent.thread

/**
 * JVM 单测伪 daemon:实现 [SocketFactory],每次 [connect] 配一对内存管道,
 * 客户端侧交还 IpcClient;服务端侧由独立线程逐行读入并交脚本处理。
 *
 * - [onLine]:每条请求行(原始字节,含尾换行)到达时回调;返回前可向客户端
 *   写应答/事件帧;回调抛异常 = 服务端线程死亡(客户端侧随后见断连),
 *   异常栈会打印到测试输出。
 * - [connectFailure]:非 null 时所有连接尝试立即抛该 IOException
 *   (模拟 daemon 未就绪)。
 * - [ServerSide.received]:逐连接累积已收请求行,字节级断言用。
 */
class FakeDaemon : SocketFactory {

    /** 每行到达回调;null = 静默吞掉。 */
    @Volatile
    var onLine: ((conn: ServerSide, line: ByteArray) -> Unit)? = null

    /** connect() 注入失败(非 null 即抛,模拟 daemon 未就绪/不可达)。 */
    @Volatile
    var connectFailure: IOException? = null

    private val registry = CopyOnWriteArrayList<ServerSide>()

    /** 已建立的连接(服务端侧句柄,按建立顺序)。 */
    val connections: List<ServerSide> get() = registry

    override fun connect(): SocketConnection {
        connectFailure?.let { throw it }
        val clientIn = PipedInputStream(PIPE_BUF)
        val serverOut = PipedOutputStream(clientIn)
        val serverIn = PipedInputStream(PIPE_BUF)
        val clientOut = PipedOutputStream(serverIn)
        val side = ServerSide(serverIn, serverOut, registry.size)
        registry.add(side)
        thread(name = "fake-daemon-${side.index}", isDaemon = true) { side.readLoop { onLine } }
        return object : SocketConnection {
            override val input: InputStream = clientIn
            override val output: OutputStream = clientOut
            override fun close() {
                runCatching { clientIn.close() }
                runCatching { clientOut.close() }
            }
        }
    }

    /** 服务端侧连接句柄。 */
    class ServerSide internal constructor(
        private val input: InputStream,
        private val output: OutputStream,
        /** 连接序号(第几条连接,自 0 起)。 */
        val index: Int,
    ) {
        /** 已收请求行(原始字节,含尾换行)。 */
        val received = CopyOnWriteArrayList<ByteArray>()

        /** 向客户端写一行(字符串形式,自动补尾换行)。 */
        fun sendLine(line: String) = sendLine(line.toByteArray(Charsets.UTF_8))

        /** 向客户端写一行(字节形式;无尾换行则补)。 */
        fun sendLine(bytes: ByteArray) {
            val line = if (bytes.isNotEmpty() && bytes.last() == '\n'.code.toByte()) {
                bytes
            } else {
                bytes + '\n'.code.toByte()
            }
            output.write(line)
            output.flush()
        }

        /** 服务端主动断连:客户端读侧随后见 EOF。 */
        fun close() {
            runCatching { output.close() }
            runCatching { input.close() }
        }

        internal fun readLoop(onLineProvider: () -> ((ServerSide, ByteArray) -> Unit)?) {
            try {
                while (true) {
                    val line = readLine(input) ?: break
                    received.add(line)
                    onLineProvider()?.invoke(this, line)
                }
            } catch (_: IOException) {
                // 客户端关闭/管道破裂:服务端线程自然收尾
            } finally {
                close()
            }
        }

        private companion object {
            /** 逐字节读到 '\n'(**含**尾换行——线上帧原文,字节级断言用);
             *  EOF 返回 null(残余无换行也按 null 丢弃)。 */
            fun readLine(input: InputStream): ByteArray? {
                val out = ByteArrayOutputStream(128)
                while (true) {
                    val b = input.read()
                    if (b < 0) return null
                    out.write(b)
                    if (b == '\n'.code) return out.toByteArray()
                }
            }
        }
    }

    private companion object {
        /** 管道缓冲:transfer.list 应答等帧 > 1KB,默认 1024 会无谓阻塞。 */
        const val PIPE_BUF = 64 * 1024
    }
}
