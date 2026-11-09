package com.mtdss.android.serve

import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.RootSpec
import com.mtdss.android.send.StoredRoot
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancelChildren
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

/**
 * roots.set 同步 JVM 单测(T14 启动/变更/登出三面):连接就绪即全量同步、
 * 引擎重启重连补挂、授权根变更(增/删)即同步、登出清空数组(直通语义)、
 * 登录回全量、未投影登录态(null)零动作(冷启动窗由连接面接管)、push 失败
 * 不炸同步循环(下次翻转自然重试)。
 */
class RootsSyncTest {

    /** 可通告变更的伪授权根表(RootsStore 同形缝)。 */
    private class FakeRoots : ObservableRoots {
        override var onChange: (() -> Unit)? = null
        var items: List<StoredRoot> = emptyList()
        override fun roots(): List<StoredRoot> = items
    }

    private class Env {
        val roots = FakeRoots()
        val loggedIn = MutableStateFlow<Boolean?>(null)
        val conn = MutableStateFlow<ConnState>(ConnState.Disconnected)
        val pushes = CopyOnWriteArrayList<List<RootSpec>>()

        /** push 失败注入(单次;下次成功)。 */
        var failOnce = false

        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

        val sync = RootsSync(
            roots = roots,
            push = { specs ->
                pushes.add(specs)
                if (failOnce) {
                    failOnce = false
                    Result.failure(IllegalStateException("引擎不在"))
                } else {
                    Result.success(Unit)
                }
            },
            loggedIn = loggedIn,
            connState = conn,
            scope = scope,
            lookupDispatcher = Dispatchers.Unconfined,
        )

        fun close() = scope.coroutineContext.cancelChildren()
    }

    private val aRoot = StoredRoot(rootId = "r1", treeUri = "u1", treeDocId = "d1", displayName = "文档")

    private fun runTest(body: suspend CoroutineScope.() -> Unit) =
        runBlocking { withTimeout(30_000) { body() } }

    @Test
    fun `连接就绪即同步——收件箱恒在加授权根`() = runTest {
        val env = Env()
        try {
            env.roots.items = listOf(aRoot)
            env.conn.value = ConnState.Connected
            assertEquals(1, env.pushes.size)
            assertEquals(
                listOf(
                    RootSpec("收件箱", "/收件箱", "bridge", true),
                    RootSpec("文档", "/文档", "bridge", true),
                ),
                env.pushes[0],
            )
            // 断连→再连(引擎重启):重同步(引擎侧集随进程消亡,重连补挂)
            env.conn.value = ConnState.Disconnected
            env.conn.value = ConnState.Connected
            assertEquals(2, env.pushes.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `授权根变更即同步——增删热生效`() = runTest {
        val env = Env()
        try {
            env.conn.value = ConnState.Connected
            env.pushes.clear()
            env.roots.items = listOf(aRoot)
            env.roots.onChange?.invoke()
            assertEquals(
                listOf(RootSpec("收件箱", "/收件箱", "bridge", true), RootSpec("文档", "/文档", "bridge", true)),
                env.pushes.single(),
            )
            env.roots.items = emptyList()
            env.roots.onChange?.invoke()
            assertEquals("移除后仅收件箱", listOf(RootSpec("收件箱", "/收件箱", "bridge", true)), env.pushes.last())
        } finally {
            env.close()
        }
    }

    @Test
    fun `登出清空数组——直通语义且登录回全量`() = runTest {
        val env = Env()
        try {
            env.roots.items = listOf(aRoot)
            env.loggedIn.value = true // 登录 → 全量
            assertEquals(2, env.pushes.single().size)
            env.loggedIn.value = false // 登出 → 空数组(roots.set [] = 清回直通)
            assertEquals(emptyList<RootSpec>(), env.pushes.last())
            env.loggedIn.value = true // 重登 → 再同步
            assertEquals(2, env.pushes.last().size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `未投影登录态零动作——冷启动窗不误清`() = runTest {
        val env = Env()
        try {
            assertNull(env.loggedIn.value)
            env.roots.items = listOf(aRoot)
            // null 态无 push(登录态面不动作;连接面接管冷启动窗)
            assertEquals(0, env.pushes.size)
        } finally {
            env.close()
        }
    }

    @Test
    fun `push 失败不炸同步循环——下次翻转自然重试`() = runTest {
        val env = Env()
        try {
            env.roots.items = listOf(aRoot)
            env.failOnce = true
            env.conn.value = ConnState.Connected // 首推失败(引擎不在)
            assertEquals(1, env.pushes.size)
            env.conn.value = ConnState.Disconnected
            env.conn.value = ConnState.Connected // 下次翻转重试成功
            assertEquals(2, env.pushes.size)
        } finally {
            env.close()
        }
    }
}
