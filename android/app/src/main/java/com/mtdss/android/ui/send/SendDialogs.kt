package com.mtdss.android.ui.send

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.send.BridgeLimits
import com.mtdss.android.ui.common.Format
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * 发送确认弹层(所有发送入口的统一收口:选择器/授权根/分享):条目数 +
 * 对端目标目录(dir = 对端绝对路径,记忆最近一次成功值)+ 冲突策略二选一
 * (镜像下载面:跳过已收完的 = 断点续传默认;全部重发 = 整批覆盖)→
 * [SendController.send]。>512 条目在此友好拦截(词面与 daemon 一致)。
 */
@Composable
fun SendConfirmDialog(
    controller: SendController,
    items: List<BridgeItem>,
    target: DirectoryDevice,
    onDone: () -> Unit,
    /** 入口附加提示(分享入口的「一次性」退化文案等)。 */
    note: String? = null,
) {
    var dir by remember { mutableStateOf(controller.lastDir) }
    var overwrite by remember { mutableStateOf(false) }
    val state by controller.state.collectAsState()

    // 终态一次性反馈:已开始 → 关弹层(进度归 T11 任务页);错误留弹层内呈现
    LaunchedEffect(state.sentJobId) {
        if (state.sentJobId != null) {
            controller.clearFeedback()
            onDone()
        }
    }
    DisposableEffect(Unit) {
        onDispose { controller.clearFeedback() }
    }

    val totalBytes = items.sumOf { it.size }
    AlertDialog(
        onDismissRequest = { if (!state.sending) onDone() },
        title = { Text("发送 ${items.size} 项到 ${target.name ?: target.deviceId}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    "共 ${Format.size(totalBytes, isDir = false)}(对端按断点续传接收)",
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedTextField(
                    value = dir,
                    onValueChange = { dir = it },
                    label = { Text("对端接收目录") },
                    supportingText = { Text("对端的绝对路径(如 Windows 的 C:\\Users\\me\\Downloads)") },
                    singleLine = true,
                    enabled = !state.sending,
                    modifier = Modifier.fillMaxWidth(),
                )
                PolicyRadio(
                    selected = !overwrite,
                    label = "跳过对端已收完的",
                    detail = "已收完自动跳过,未收完断点续传(推荐)",
                    onClick = { overwrite = false },
                )
                PolicyRadio(
                    selected = overwrite,
                    label = "全部重新发送",
                    detail = "同名文件在对端覆盖重收",
                    onClick = { overwrite = true },
                )
                note?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                state.error?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            if (state.sending) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.height(18.dp).width(18.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.width(8.dp))
                    Text("发送中…")
                }
            } else {
                TextButton(
                    enabled = dir.isNotBlank() && items.isNotEmpty() &&
                        BridgeLimits.itemCountError(items.size) == null,
                    onClick = { controller.send(items, target.deviceId, dir.trim(), overwrite) },
                ) { Text("发送") }
            }
        },
        dismissButton = {
            if (!state.sending) {
                TextButton(onClick = onDone) { Text("取消") }
            }
        },
    )
}

/**
 * 目标设备选择弹层(分享入口与本机树发送用;浏览/设备页的发送入口已知
 * 目标,不经此层):在线设备列表(devices op + event.devices 快照),
 * 离线设备置灰不可选。
 */
@Composable
fun DevicePickerDialog(
    graph: Graph,
    onPick: (DirectoryDevice) -> Unit,
    onDismiss: () -> Unit,
) {
    val scope = remember {
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember { DevicePickerViewModel(graph.repo, graph.hub, scope) }
    val state by vm.state.collectAsState()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("选择目标设备") },
        text = {
            Column {
                if (state.loading && state.devices.isEmpty()) {
                    Row(
                        Modifier.fillMaxWidth().padding(vertical = 16.dp),
                        horizontalArrangement = Arrangement.Center,
                    ) { CircularProgressIndicator() }
                } else if (state.devices.isEmpty()) {
                    Text(
                        state.error ?: "没有其他在线设备。在另一台设备登录同一账号后会出现在这里。",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                } else {
                    LazyColumn {
                        items(state.devices.size) { i ->
                            val dev = state.devices[i]
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable(enabled = dev.online) { onPick(dev) }
                                    .padding(vertical = 10.dp),
                            ) {
                                Column(Modifier.weight(1f)) {
                                    Text(
                                        dev.name ?: dev.deviceId,
                                        style = MaterialTheme.typography.bodyLarge,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                }
                                Text(
                                    if (dev.online) "在线" else "离线",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = if (dev.online) {
                                        MaterialTheme.colorScheme.primary
                                    } else {
                                        MaterialTheme.colorScheme.onSurfaceVariant
                                    },
                                )
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("取消") }
        },
    )
}

@Composable
private fun PolicyRadio(selected: Boolean, label: String, detail: String, onClick: () -> Unit) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
    ) {
        RadioButton(selected = selected, onClick = onClick)
        Column {
            Text(label, style = MaterialTheme.typography.bodyMedium)
            Text(detail, style = MaterialTheme.typography.bodySmall)
        }
    }
}
