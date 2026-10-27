package com.mtdss.android.ui.devices

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.material.icons.filled.Send
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.send.PickerBridge
import com.mtdss.android.ui.send.SendConfirmDialog
import com.mtdss.android.ui.send.SendController
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 设备列表屏(spec §4.3:主界面 = 设备列表 → 设备页):在线设备点击进入
 * 浏览;离线置灰(引擎门控:离线设备无 list 通道)。行内快发(选择器
 * 多选 → 直发该设备,T10);顶栏「任务」入任务页(T11 一等屏,断点续传/
 * 重发操作面)、「本机」入本机页(收件箱 + 授权根),设置图标入设置页
 * (T12:账号/收件箱位置/检查更新/关于;诊断经设置页内入口)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DevicesScreen(
    graph: Graph,
    onDevice: (DirectoryDevice) -> Unit,
    onLocalFiles: () -> Unit,
    onTasks: () -> Unit,
    onSettings: () -> Unit,
) {
    val context = LocalContext.current
    val sessionState by graph.session.state.collectAsState()
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember { DevicesViewModel(graph.repo, graph.hub, scope) }
    val state by vm.state.collectAsState()

    val snackbar = remember { SnackbarHostState() }

    // 行内快发:选目标设备 → 系统选择器多选 → 确认弹层
    val sendController = remember { SendController.create(graph.repo, scope, context) }
    val pickerBridge = remember { PickerBridge(graph.saf) }
    var quickShareTarget by remember { mutableStateOf<DirectoryDevice?>(null) }
    var quickShareItems by remember { mutableStateOf<List<BridgeItem>?>(null) }
    val quickSharePicker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenMultipleDocuments(),
    ) { uris ->
        val target = quickShareTarget
        scope.launch {
            val picked = withContext(Dispatchers.IO) { pickerBridge.itemsFromUris(uris.map { it.toString() }) }
            if (target == null || picked.items.isEmpty()) {
                quickShareTarget = null
            } else {
                if (picked.skipped > 0) snackbar.showSnackbar("${picked.skipped} 个文件不可读,已跳过")
                quickShareItems = picked.items
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("MTDSS") },
                actions = {
                    IconButton(onClick = vm::refresh) {
                        Icon(Icons.Filled.Refresh, contentDescription = "刷新")
                    }
                    TextButton(onClick = onTasks) { Text("任务") }
                    TextButton(onClick = onLocalFiles) { Text("本机") }
                    IconButton(onClick = onSettings) {
                        Icon(Icons.Filled.Settings, contentDescription = "设置")
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
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
                        DeviceRow(
                            dev,
                            onClick = { if (dev.online) onDevice(dev) },
                            onQuickShare = {
                                quickShareTarget = dev
                                quickSharePicker.launch(arrayOf("*/*"))
                            },
                        )
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

    val target = quickShareTarget
    val items = quickShareItems
    if (target != null && items != null) {
        SendConfirmDialog(
            controller = sendController,
            items = items,
            target = target,
            onDone = {
                quickShareTarget = null
                quickShareItems = null
            },
        )
    }
}

@Composable
private fun DeviceRow(dev: DirectoryDevice, onClick: () -> Unit, onQuickShare: () -> Unit) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(enabled = dev.online, onClick = onClick),
    ) {
        Row(
            modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 16.dp, end = 4.dp),
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
            // 行内快发(在线设备;选择器多选 → 该设备)
            IconButton(onClick = onQuickShare, enabled = dev.online) {
                Icon(Icons.Filled.Send, contentDescription = "发送文件到此设备")
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
