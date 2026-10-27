package com.mtdss.android.fdpass

import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asObject
import java.io.FileDescriptor
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicLong

/**
 * JVM 单测伪 fdpass server(镜像 test 侧 FakeDaemon 的缝式):每次
 * [connect] 配一对内存管道,客户端侧交还 FdPassChannel;客户端→服务端
 * 帧由通道的 write 经 [FdWireDecoder] 解码直接捕获(无读线程),
 * 服务端→客户端帧经管道送达(通道读循环阻塞读)。
 *
 * ## fd 传递的模拟口径(镜像内核 SCM_RIGHTS 语义,见 FdSocket 类注释)
 * - **出向**(通道→伪 server):setOutboundFds 武装的 fd 挂到武装后
 *   **本次 write 完成的帧**(通道约定单帧单 write;帧不完整则保持武装);
 * - **入向**(伪 server→通道):[ServerSide.send] 把 fd 挂到帧**首字节
 *   的字节偏移**上,通道读循环消费到该偏移后的首次 [FdSocketConnection.harvestInboundFds]
 *   才收割——跨帧交错的注入不会把 fd 错挂到前帧尾部(精确镜像「fd 随携
 *   它的 sendmsg 首数据字节交付」的内核语义);
 * - [ServerSide.closedInbound]:协议异常路径经 closeInboundFd 关闭的 fd
 *   记录面(防泄漏断言用)。
 *
 * 伪 fd 用裸 [FileDescriptor] 实例(身份相等断言;JVM 上无真实 fd 语义)。
 */
class FakeFdServer : FdSocketFactory {

    /** 通道每帧到达回调(在通道 write 所在线程执行);null = 静默吞掉。 */
    @Volatile
    var onFrame: ((conn: ServerSide, frame: ReceivedFrame) -> Unit)? = null

    /** connect() 注入失败(非 null 即抛,模拟 fds.sock 不可达)。 */
    @Volatile
    var connectFailure: IOException? = null

    private val registry = CopyOnWriteArrayList<ServerSide>()

    /** 已建立的连接(服务端侧句柄,按建立顺序)。 */
    val connections: List<ServerSide> get() = registry

    override fun connect(): FdSocketConnection {
        connectFailure?.let { throw it }
        val conn = FakeConnection()
        val side = ServerSide(conn, registry.size)
        conn.side = side
        registry.add(side)
        return conn
    }

    /** 服务端侧连接句柄。 */
    class ServerSide internal constructor(
        private val conn: FakeFdServer.FakeConnection,
        /** 连接序号(自 0 起,按建立顺序)。 */
        val index: Int,
    ) {
        /** 通道发来的全部帧(kind + payload + 随帧 fd)。 */
        val frames = CopyOnWriteArrayList<ReceivedFrame>()

        /** 协议异常路径被 closeInboundFd 关闭的 fd(防泄漏断言面)。 */
        val closedInbound = CopyOnWriteArrayList<FileDescriptor>()

        /** 向通道注入一帧;fds 非空时随帧附带(挂到帧首字节偏移)。 */
        fun send(kind: Int, payload: JVal, fds: List<FileDescriptor> = emptyList()) {
            conn.inject(kind, payload, fds)
        }

        /** 服务端主动断连:通道读侧随后见 EOF。 */
        fun close() = conn.shutdownOutput()
    }

    /** 伪连接:字节面为管道;fd 面为偏移挂载/武装捕获。 */
    inner class FakeConnection : FdSocketConnection {
        internal lateinit var side: ServerSide

        // 客户端→服务端:解码式输出流(帧完成即入账 + 回调)
        private val decoder = FdWireDecoder()

        @Volatile
        private var armedFds: List<FileDescriptor>? = null

        // 服务端→客户端:管道字节面 + fd 偏移追踪
        private val clientIn = PipedInputStream(PIPE_BUF)
        private val serverOut = PipedOutputStream(clientIn)

        /** 入向 fd 挂载点队列:(帧首字节在字节流中的偏移, fds)。 */
        private val fdAnchors = ConcurrentLinkedQueue<Pair<Long, List<FileDescriptor>>>()

        /** 已注入字节流的累计偏移(send 侧维护)。 */
        private val written = AtomicLong()

        /** 通道已读走的累计偏移(input 包装层维护)。 */
        private val readTotal = AtomicLong()

        @Volatile
        private var closed = false

        override val input: InputStream = object : InputStream() {
            override fun read(): Int = clientIn.read().also { if (it >= 0) readTotal.incrementAndGet() }

            override fun read(b: ByteArray, off: Int, len: Int): Int =
                clientIn.read(b, off, len).also { if (it > 0) readTotal.addAndGet(it.toLong()) }
        }

        override val output: OutputStream = object : OutputStream() {
            override fun write(b: Int) = write(byteArrayOf(b.toByte()))

            override fun write(b: ByteArray, off: Int, len: Int) {
                val frames = decoder.feed(b, off, len)
                if (frames.isEmpty()) return
                // 武装的 fd 挂到本次 write 完成的首个帧(通道单帧单 write 约定)
                val fds = armedFds ?: emptyList()
                armedFds = null
                frames.forEachIndexed { i, f ->
                    val payload = JVal.parse(String(f.payload, Charsets.UTF_8)).asObject()
                    val frame = ReceivedFrame(f.kind, payload, if (i == 0) fds else emptyList())
                    side.frames.add(frame)
                    onFrame?.invoke(side, frame)
                }
            }
        }

        override fun setOutboundFds(fds: Array<FileDescriptor>?) {
            armedFds = fds?.toList()
        }

        override fun harvestInboundFds(): List<FileDescriptor> {
            val total = readTotal.get()
            val out = mutableListOf<FileDescriptor>()
            // 首字节已被读走的挂载点即交付(每读必收,调用即清;未读到的留队)
            while (true) {
                val head = fdAnchors.peek() ?: break
                if (head.first >= total) break
                fdAnchors.poll()
                out += head.second
            }
            return out
        }

        override fun closeInboundFd(fd: FileDescriptor) {
            side.closedInbound.add(fd)
        }

        override fun close() {
            closed = true
            runCatching { clientIn.close() }
            runCatching { serverOut.close() }
        }

        /** 服务端注入:fd 挂帧首偏移 → 写字节;连接已关 → 静默丢。 */
        internal fun inject(kind: Int, payload: JVal, fds: List<FileDescriptor>) {
            synchronized(serverOut) {
                if (closed) return
                if (fds.isNotEmpty()) fdAnchors.add(written.get() to fds)
                val bytes = FdWire.encodeFrame(kind, payload)
                serverOut.write(bytes)
                serverOut.flush()
                written.addAndGet(bytes.size.toLong())
            }
        }

        internal fun shutdownOutput() {
            synchronized(serverOut) { runCatching { serverOut.close() } }
        }
    }

    private companion object {
        const val PIPE_BUF = 64 * 1024
    }
}

/** 通道 → 伪 server 的一帧(kind + payload JSON + 随帧 fd)。 */
data class ReceivedFrame(val kind: Int, val payload: JVal.Obj, val fds: List<FileDescriptor>)
