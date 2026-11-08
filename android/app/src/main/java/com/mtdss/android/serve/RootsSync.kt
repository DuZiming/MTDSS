package com.mtdss.android.serve

import android.util.Log
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.RootSpec
import com.mtdss.android.send.RootLookup
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 可通告变更的授权根表([com.mtdss.android.send.RootsStore] 实现同形):
 * roots() 实时查询 + 变更通告(增/删落账后触发,RootsSync 消费即推送)。
 */
interface ObservableRoots : RootLookup {
    /** 变更通告槽(增/删成功落账后触发;null = 无订阅者)。 */
    var onChange: (() -> Unit)?
}

/**
 * roots.set 引擎同步(V3-4 §4.1 实施面 3;T11 双 TODO 的接线本体):
 * 「App 启动(引擎就绪后)与授权根变更 → repo.rootsSet(specs);登出清(空
 * 数组)」三触发面——
 *
 * - **连接就绪**(connState → Connected):全量同步。引擎重启 = 侧根集随
 *   进程消亡(新进程缺省直通),重连补挂(IpcClient 自愈覆盖冷启动间隙);
 * - **登录态翻转**:登录(true)= 全量;登出/被踢(false)= **空数组**
 *   (`roots.set []` = 清回默认直通——登出设备不向对端暴露授权面);
 *   null(冷启动窗)零动作,连接面接管;
 * - **授权根变更**(roots.onChange):增/删落账即推(SettingsViewModel 的
 *   add/removeTrustedRoot 落账路径,热生效免「重启生效」窗)。
 *
 * push 失败(Result.failure,引擎不在/传输断)吞掉记日志——循环存活,
 * 下一次翻转/重连自然重试;同步幂等(整单替换)。
 */
class RootsSync(
    private val roots: ObservableRoots,
    /** 推送缝(repo.rootsSet 同形;JVM 注入记录断言)。 */
    private val push: suspend (List<RootSpec>) -> Result<Unit>,
    /** 登录态流(SessionViewModel.State 投影;null = 冷启动窗)。 */
    loggedIn: Flow<Boolean?>,
    /** 连接态流(IpcClient.connectionState;Connected = 引擎就绪)。 */
    connState: Flow<ConnState>,
    scope: CoroutineScope,
    private val lookupDispatcher: CoroutineDispatcher = Dispatchers.IO,
) {

    companion object {
        private const val TAG = "RootsSync"
    }

    init {
        scope.launch {
            connState.collect { st ->
                if (st == ConnState.Connected) sync()
            }
        }
        scope.launch {
            loggedIn.collect { v ->
                when (v) {
                    true -> sync()
                    false -> clear()
                    null -> Unit // 冷启动窗:连接面接管
                }
            }
        }
        roots.onChange = { scope.launch { sync() } }
    }

    /** 全量同步(收件箱恒在 + 授权根;SAF 无关,IO 线程读账本)。 */
    private suspend fun sync() {
        val specs = withContext(lookupDispatcher) {
            rootsSpecs(authorizedRoots = roots.roots())
        }
        push(specs).fold(
            onSuccess = { Log.i(TAG, "roots.set 已同步(${specs.size} 根)") },
            onFailure = { e -> Log.w(TAG, "roots.set 推送失败(下次翻转重试): ${e.message}") },
        )
    }

    /** 登出清:空数组 = 引擎清回默认直通(全局约束语义)。 */
    private suspend fun clear() {
        push(emptyList()).fold(
            onSuccess = { Log.i(TAG, "roots.set 已清空(登出,恢复直通)") },
            onFailure = { e -> Log.w(TAG, "roots.set 清空推送失败(下次翻转重试): ${e.message}") },
        )
    }
}
