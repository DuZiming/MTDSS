package com.mtdss.android

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Handler
import android.os.Looper
import com.mtdss.android.ipc.IpcClient
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * 网络变化桥(r58;iOS r50/r54 NetworkChangeBridge 同象):宿主 OS 网络显著
 * 变化 → 引擎 `network.changed`(kick 信令重连 + ctrl 池全量驱逐 + 5s
 * best-effort 等在线)——切 WiFi↔蜂窝后预览「秒失败」、锁屏/退后台回来
 * 半死连接不自愈的宿主信号面(引擎自愈面早已在,此前安卓无桥 = 原样踩坑)。
 *
 * - **显著变化判定**:可达网络集合的成员变化(加入/离开)——同集合的
 *   代价波动(信号格变化)不触发;iOS NWPath「接口类型/满足态翻面」同语义。
 * - **去抖 500ms**:切换瞬间的 onLost→onAvailable 连拍合并为一次。
 * - **回前台无条件触发**([onAppResumed],MainActivity.onResume 挂):锁屏
 *   不翻网络路径(挂起期翻面事件也不投递),显式钩子补口(iOS scenePhase
 *   realignOnActive 同款)。
 * - 尽力而为:IPC 失败静默(引擎三连幂等);启动基线化(注册即采集现集,
 *   不为开机态空打一发)。
 */
class NetworkChangeBridge(
    private val context: Context,
    private val scope: CoroutineScope,
    private val ipc: IpcClient,
) {
    private val handler = Handler(Looper.getMainLooper())
    private var lastKey: String? = null
    private var pending: Runnable? = null

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            schedule()
        }

        override fun onLost(network: Network) {
            schedule()
        }
    }

    /** Application.onCreate 挂(先基线化再注册,启动态不空打)。 */
    fun start() {
        refreshKey() // 基线
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return
        try {
            cm.registerNetworkCallback(NetworkRequest.Builder().build(), callback)
        } catch (_: Exception) {
            // 极老机型/系统裁剪:桥缺席,引擎 25s 陈旧 kick 兜底仍在
        }
    }

    /** 回前台无条件触发(锁屏形态补口;幂等廉价)。 */
    fun onAppResumed() {
        fire(force = true)
    }

    private fun schedule() {
        pending?.let { handler.removeCallbacks(it) }
        val r = Runnable { fire(force = false) }
        pending = r
        handler.postDelayed(r, 500)
    }

    /// 现刻可达集合的稳定键(互联网能力网络 id 排序;空集 = 离线)。
    private fun currentKey(): String {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return ""
        return cm.allNetworks
            .filter { net ->
                cm.getNetworkCapabilities(net)
                    ?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
            }
            .map { it.toString() }
            .sorted()
            .joinToString(",")
    }

    private fun refreshKey() {
        lastKey = currentKey()
    }

    private fun fire(force: Boolean) {
        val key = currentKey()
        if (!force && key == lastKey) return
        lastKey = key
        scope.launch { ipc.networkChanged() }
    }
}
