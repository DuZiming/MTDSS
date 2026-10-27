package com.mtdss.android.ui.common

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.engine.MtdssEngine
import com.mtdss.android.ipc.ConnState
import kotlinx.coroutines.launch

/**
 * 诊断面板(T7 临时调试屏的收编残留:引擎状态 + IPC 连接态 + 本机身份 +
 * 退出登录;设备联调时的最小观测面)。保持小——真实观测工具是
 * `adb logcat -s mtdssd MtdssEngine`。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DiagnosticsSheet(graph: Graph, onDismiss: () -> Unit) {
    val engineStatus by MtdssEngine.status.collectAsState()
    val sessionState by graph.session.state.collectAsState()
    val scope = rememberCoroutineScope()
    var loggingOut by remember { mutableStateOf(false) }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp)
                .padding(bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("诊断", style = MaterialTheme.typography.titleLarge)

            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("引擎", style = MaterialTheme.typography.titleSmall)
                    Text(
                        when (engineStatus.phase) {
                            MtdssEngine.Phase.RUNNING -> "运行中"
                            MtdssEngine.Phase.BACKOFF -> "退避等待(意外退出后重启)"
                            MtdssEngine.Phase.STOPPED -> "已停止"
                        },
                    )
                    Text("累计重启:${engineStatus.restartCount}")
                    engineStatus.lastExit?.let { Text("上次退出:$it") }
                }
            }

            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("IPC", style = MaterialTheme.typography.titleSmall)
                    Text(
                        when (sessionState.conn) {
                            ConnState.Connected -> "已连接"
                            ConnState.Connecting -> "连接中(自愈重连)"
                            ConnState.Disconnected -> "已断开(自愈重连中)"
                        },
                    )
                    Text("设备:${sessionState.deviceName ?: "-"}(${sessionState.deviceId ?: "-"})")
                }
            }

            Text(
                "收件箱位置:Downloads/MTDSS(系统文件管理器可见)",
                style = MaterialTheme.typography.bodySmall,
            )

            Button(
                onClick = {
                    loggingOut = true
                    scope.launch {
                        graph.repo.logout()
                        graph.session.refresh() // 权威对账(event.login 兜底)
                        loggingOut = false
                        onDismiss()
                    }
                },
                enabled = !loggingOut,
            ) { Text(if (loggingOut) "退出中…" else "退出登录") }
        }
    }
}
