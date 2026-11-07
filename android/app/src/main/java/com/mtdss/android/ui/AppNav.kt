package com.mtdss.android.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ui.login.ApprovalSheetHost
import com.mtdss.android.ui.login.LoginScreen
import com.mtdss.android.ui.tabs.AppTabView

/**
 * 应用导航(四标签壳版,spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-1):会话状态(SessionViewModel 的权威 status)是顶层路由依据:
 * - loggedIn == null:引擎冷启动「拉起 → socket 可连」间隙的 splash
 *   (IpcClient 自愈重连,秒级收敛);
 * - loggedIn == false:登录屏(登出/被移除 event.device_evicted 同样回到
 *   这里,「已被移除」警示条由 LoginScreen 呈现);
 * - loggedIn == true:四标签壳 [AppTabView](快捷目录/设备/传输列表/我的,
 *   胶囊底栏),已登录期间受信终端审批弹窗(event.approval)随时可弹
 *   ([ApprovalSheetHost] 在壳根挂载)。
 *
 * V3-4 过渡说明:旧版设备/浏览/本机/任务/设置并列路由随四标签壳退役
 * (各 tab 先以旧屏过渡,后续任务逐屏替换、T16 扫尾)。浏览位与「重连
 * 即弹回设备列表」语义一并退役——浏览位不存在后弹回对象消失,断连窗
 * 由壳内「重连中…」蓝胶囊呈现;T6/T7 浏览经设备 tab 重入时再评估回退。
 */
@Composable
fun MtdssApp(graph: Graph) {
    val sessionState by graph.session.state.collectAsState()

    Surface(
        modifier = Modifier.fillMaxSize(),
        color = MaterialTheme.colorScheme.background,
    ) {
        when (sessionState.loggedIn) {
            null -> ConnectingSplash(sessionState.conn)

            false -> LoginScreen(
                graph,
                evicted = sessionState.evicted,
                onVerified = { graph.session.refresh() },
            )

            else -> {
                // 受信终端审批弹窗(任意已登录屏面期间 event.approval 到达即弹)
                ApprovalSheetHost(graph)
                // 四标签壳(选择模式藏栏 T7 接线,过渡期恒 false)
                AppTabView(graph, selectionHidden = false)
            }
        }
    }
}

@Composable
private fun ConnectingSplash(conn: ConnState) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            if (conn == ConnState.Connected) {
                // 已连接但 status 未归:毫秒级瞬态,同 splash 呈现
                CircularProgressIndicator()
                Text("读取登录态…", Modifier.padding(top = 12.dp))
            } else {
                CircularProgressIndicator()
                Text("连接引擎中…", Modifier.padding(top = 12.dp))
            }
        }
    }
}
