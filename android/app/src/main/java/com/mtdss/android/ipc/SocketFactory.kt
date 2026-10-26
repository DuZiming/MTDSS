package com.mtdss.android.ipc

import android.net.LocalSocket
import android.net.LocalSocketAddress
import java.io.Closeable
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * 传输层抽象:JVM 单测注入内存管道(见 test 侧 FakeDaemon),实机以
 * LocalSocket(FILESYSTEM 命名空间)连接 daemon 的 unix 域套接字。
 * IpcClient 的全部连接语义(断连自愈、补拉)只依赖本抽象,不感知传输介质。
 */
interface SocketFactory {

    /** 建立一条到 daemon 的连接;失败抛 [IOException]。阻塞调用(由 IpcClient 切 IO 调度)。 */
    fun connect(): SocketConnection
}

/** 一条双工连接:读侧由 IpcClient 读协程独占;写侧由写互斥串行整帧写出。 */
interface SocketConnection : Closeable {
    val input: InputStream
    val output: OutputStream
}

/**
 * 实机工厂:LocalSocket 连 `{filesDir}/mtdss/daemon.sock`
 * (与 MtdssEngine.daemonSocketFile / mtdss_ipc::socket_path 的 unix 分支同口径)。
 */
class LocalSocketFactory(private val socketFile: File) : SocketFactory {

    override fun connect(): SocketConnection {
        val socket = LocalSocket() // 命名空间只由 LocalSocketAddress 决定,无参构造即未绑定套接字
        try {
            socket.connect(
                LocalSocketAddress(socketFile.absolutePath, LocalSocketAddress.Namespace.FILESYSTEM),
            )
            return LocalSocketConnection(socket)
        } catch (e: IOException) {
            runCatching { socket.close() }
            throw e
        }
    }

    private class LocalSocketConnection(private val socket: LocalSocket) : SocketConnection {
        override val input: InputStream get() = socket.inputStream
        override val output: OutputStream get() = socket.outputStream
        override fun close() {
            socket.close()
        }
    }
}
