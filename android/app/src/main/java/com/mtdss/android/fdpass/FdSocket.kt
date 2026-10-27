package com.mtdss.android.fdpass

import android.net.LocalSocket
import android.net.LocalSocketAddress
import java.io.Closeable
import java.io.File
import java.io.FileDescriptor
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * fdpass 传输层抽象(JVM 单测注入内存管道,镜像 ipc/SocketFactory 的测试缝):
 * 字节面走 [input]/[output],fd 面走 [setOutboundFds]/[harvestInboundFds]。
 * [FdPassChannel] 的全部连接语义(ack 关联、0x03 交错、断连清理)只依赖
 * 本抽象,不感知传输介质——SCM_RIGHTS 的真实行为由实机 LocalFdSocketFactory
 * 承担(设备验证面)。
 *
 * ## SCM_RIGHTS 交付语义(实机约束,伪 server 须镜像)
 * - **发送**:[setOutboundFds] 武装后,fd 随**下一次 write** 的首数据字节
 *   交付(内核在 sendmsg 时复制 fd);write 后武装即消耗。因此发送侧须
 *   把整帧(帧头+payload)在**单次 write** 内写出,且失败路径须解除武装
 *   (防 fd 错挂到后续帧)。
 * - **接收**:fd 随携它的字节所在的 read 交付。[FdPassChannel] 按帧边界
 *   对齐读取(每帧恰读 帧头 5 字节 + payload len 字节),每读一次即
 *   [harvestInboundFds] 收割累积到当前帧——fd 归属无歧义。
 */
interface FdSocketFactory {

    /** 建立一条到 daemon fds.sock 的连接;失败抛 [IOException](由调用方切 IO 调度)。 */
    fun connect(): FdSocketConnection
}

/** 一条 fdpass 双工连接:读侧由 FdPassChannel 读协程独占;写侧由写互斥串行整帧写出。 */
interface FdSocketConnection : Closeable {
    val input: InputStream
    val output: OutputStream

    /** 随下一次 write 附带的 SCM_RIGHTS fd(0/1 个);null = 不带。须紧邻 write 前设置。 */
    fun setOutboundFds(fds: Array<FileDescriptor>?)

    /** 收割随最近一次 read 到达的 fd(每读即收,调用即清;无则空表)。 */
    fun harvestInboundFds(): List<FileDescriptor>

    /** 关闭一个收到的 fd(协议异常断连时的防御收口;不关 = 泄漏)。 */
    fun closeInboundFd(fd: FileDescriptor)
}

/**
 * 实机工厂:LocalSocket(FILESYSTEM 命名空间)连 `{filesDir}/mtdss/fds.sock`
 * (serve_fdpass 的监听位;与 MtdssEngine.daemonHome 同口径派生)。
 */
class LocalFdSocketFactory(private val socketFile: File) : FdSocketFactory {

    override fun connect(): FdSocketConnection {
        val socket = LocalSocket() // 无参构造即未绑定套接字;命名空间只由 LocalSocketAddress 决定
        try {
            socket.connect(
                LocalSocketAddress(socketFile.absolutePath, LocalSocketAddress.Namespace.FILESYSTEM),
            )
            return LocalFdSocketConnection(socket)
        } catch (e: IOException) {
            runCatching { socket.close() }
            throw e
        }
    }

    private class LocalFdSocketConnection(private val socket: LocalSocket) : FdSocketConnection {
        override val input: InputStream get() = socket.inputStream
        override val output: OutputStream get() = socket.outputStream

        override fun setOutboundFds(fds: Array<FileDescriptor>?) {
            socket.setFileDescriptorsForSend(fds)
        }

        override fun harvestInboundFds(): List<FileDescriptor> =
            socket.ancillaryFileDescriptors?.toList() ?: emptyList()

        override fun closeInboundFd(fd: FileDescriptor) {
            // 收到的 fd 自带 CLOEXEC(recvmsg MSG_CMSG_CLOEXEC),但不会自动关闭:
            // Os.close 是公开 API 面上关闭裸 FileDescriptor 的唯一正道
            runCatching { android.system.Os.close(fd) }
        }

        override fun close() {
            socket.close()
        }
    }
}
