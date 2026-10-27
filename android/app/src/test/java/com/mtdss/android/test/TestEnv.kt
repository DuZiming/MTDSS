package com.mtdss.android.test

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.FakeDaemon
import com.mtdss.android.ipc.IpcClient
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout

/**
 * VM 层测试台(T8 FakeDaemon 缝式的包装):伪 daemon + IpcClient + 事件
 * hub + 仓库一次配齐;scope 用 Unconfined(init 的 launch 内联推进到首个
 * 挂起点,时序可推理)。
 */
class TestEnv {
    val fake = FakeDaemon()
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    val ipc = IpcClient(fake, scope, backoff = { 25L })
    val hub = MtdssEventHub(ipc, scope)
    val repo = MtdssRepository(ipc)

    fun close() {
        ipc.close()
        scope.cancel()
    }
}

object TestKit {

    /** 限时测试体:30s 上界防桩/回归把测试 JVM 挂死。 */
    fun <T> runTest(body: suspend CoroutineScope.() -> T): T =
        runBlocking { withTimeout(30_000) { body() } }

    /** 条件轮询直至成立(默认 5s 上界,届满抛 IllegalStateException)。 */
    suspend fun awaitUntil(timeoutMs: Long = 5_000, cond: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (!cond()) {
            check(System.currentTimeMillis() < deadline) { "等待条件超时(${timeoutMs}ms)" }
            delay(25)
        }
    }
}
