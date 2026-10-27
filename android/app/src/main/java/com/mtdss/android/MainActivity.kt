package com.mtdss.android

import android.app.Activity
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalContext
import com.mtdss.android.ui.MtdssApp
import com.mtdss.android.ui.tasks.TasksViewModel

/**
 * 唯一 Activity:T7 的临时 hello 调试屏已被真实导航(登录 → 设备 → 浏览)
 * 取代;调试观测面收编进设备页「设置 → 诊断」(DiagnosticsSheet)。
 * 引擎拉起/IPC 装配在 MtdssApplication(AppGraph),与 Activity 生命周期
 * 解耦(旋转/重建不重置连接与订阅)。
 *
 * KEEP_SCREEN_ON(T11):活任务存在(任一非终态任务,含会话级接收行)时
 * 给窗口挂 [WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON],纯态随任务
 * 快照驱动(无权限/无服务;手动电源键熄屏不受此旗影响,熄屏中断→续传的
 * headline 路径照常可达)。VM 是 app 级,任意屏面期间旗面都成立。
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val graph = AppGraph.get()
        setContent {
            MaterialTheme {
                KeepScreenOnGate(graph.tasks)
                MtdssApp(graph)
            }
        }
    }
}

/** KEEP_SCREEN_ON 副作用面:旗位翻转时挂/摘(幂等;旗清 = 空闲态不锁屏)。 */
@Composable
private fun KeepScreenOnGate(tasks: TasksViewModel) {
    val state by tasks.state.collectAsState()
    val keep = state.keepScreenOn
    val window = (LocalContext.current as? Activity)?.window
    LaunchedEffect(keep, window) {
        if (window == null) return@LaunchedEffect
        if (keep) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }
}
