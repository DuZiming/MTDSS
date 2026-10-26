package com.mtdss.android.ui.devices

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.common.DiagnosticsSheet
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * 设备列表屏(spec §4.3:主界面 = 设备列表 → 设备页):在线设备点击进入
 * 浏览;离线置灰(引擎门控:离线设备无 list 通道)。顶栏设置入口内含
 * 诊断(T7 调试面收编)与 T12 占位。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DevicesScreen(
    graph: Graph,
    onDevice: (DirectoryDevice) -> Unit,
) {
    val sessionState by graph.session.state.collectAsState()
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember { DevicesViewModel(graph.repo, graph.hub, scope) }
    val state by vm.state.collectAsState()

    var showDiagnostics by remember { mutableStateOf(false) }
    var showSettings by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("MTDSS") },
                actions = {
                    IconButton(onClick = vm::refresh) {
                        Icon(Icons.Filled.Refresh, contentDescription = "刷新")
                    }
                    IconButton(onClick = { showSettings = true }) {
                        Icon(Icons.Filled.Settings, contentDescription = "设置")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            if (sessionState.selfOnline == false) {
                Banner("本机离线(信令未连上)", MaterialTheme.colorScheme.errorContainer)
            } else if (state.stale) {
                Banner("设备列表可能过期", MaterialTheme.colorScheme.surfaceVariant)
            }

            when {
                state.loading && state.devices.isEmpty() -> Box(
                    Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                state.error != null && state.devices.isEmpty() -> Box(
                    Modifier
                        .fillMaxSize()
                        .padding(24.dp),
                    contentAlignment = Alignment.Center,
                ) { Text(state.error ?: "", color = MaterialTheme.colorScheme.error) }

                else -> LazyColumn(
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    items(state.devices, key = { it.deviceId }) { dev ->
                        DeviceRow(dev) { if (dev.online) onDevice(dev) }
                    }
                    if (state.devices.isEmpty()) {
                        item {
                            Text(
                                "没有其他设备。在另一台设备登录同一账号后会出现在这里。",
                                style = MaterialTheme.typography.bodyMedium,
                                modifier = Modifier.padding(24.dp),
                            )
                        }
                    }
                }
            }
        }
    }

    if (showDiagnostics) {
        DiagnosticsSheet(graph, onDismiss = { showDiagnostics = false })
    }
    if (showSettings) {
        AlertDialog(
            onDismissRequest = { showSettings = false },
            title = { Text("设置") },
            text = { Text("设置页将在后续版本提供(T12)。诊断与退出登录在「诊断」里。") },
            confirmButton = {
                TextButton(onClick = { showSettings = false; showDiagnostics = true }) { Text("诊断") }
            },
            dismissButton = {
                TextButton(onClick = { showSettings = false }) { Text("关闭") }
            },
        )
    }
}

@Composable
private fun DeviceRow(dev: DirectoryDevice, onClick: () -> Unit) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(enabled = dev.online, onClick = onClick),
    ) {
        Row(
            modifier = Modifier.padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // 在线指示点
            Box(
                Modifier
                    .size(10.dp)
                    .background(
                        if (dev.online) Color(0xFF2E7D32) else Color(0xFF9E9E9E),
                        CircleShape,
                    ),
            )
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(dev.name ?: dev.deviceId, style = MaterialTheme.typography.titleMedium)
                Text(
                    listOfNotNull(dev.platform, if (dev.online) "在线" else "离线").joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun Banner(text: String, bg: Color) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        modifier = Modifier
            .fillMaxWidth()
            .background(bg)
            .padding(horizontal = 16.dp, vertical = 6.dp),
    )
}
