package com.mtdss.android.ui.browse

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Send
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.send.PickerBridge
import com.mtdss.android.ui.common.Format
import com.mtdss.android.ui.send.SendConfirmDialog
import com.mtdss.android.ui.send.SendController
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 浏览屏(spec §4.3 设备页:对端目录树 + 下载收件箱 + 发送入口):目录
 * 导航(根卷 → 逐级)、文件/目录多选、下载确认(策略二选一:跳过已收完
 * 的 = 默认断点续传;覆盖 = 全部重收)→ transfer.start download
 * (dir="inbox")。发送入口(顶栏 ACTION_OPEN_DOCUMENT 多选 → 选择器件
 * → [SendConfirmDialog],ref = 持久化 URI)。浏览位经 [rememberSaveable]
 * 存续(旋转重建后恢复);反馈走 snackbar(任务列表是 T11 的一等界面,
 * 此处只确认「已开始」)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowseScreen(
    graph: Graph,
    device: DirectoryDevice,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val scope = remember {
        CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember(device.deviceId) { BrowseViewModel(device, graph.repo, scope) }
    val state by vm.state.collectAsState()
    val sendController = remember { SendController.create(graph.repo, scope, context) }
    val pickerBridge = remember { PickerBridge(graph.saf) }

    // 浏览位存续:导航写回,VM 重建(进/出设备页、旋转)时恢复
    var savedPath by rememberSaveable(device.deviceId) { mutableStateOf("") }
    LaunchedEffect(vm) { vm.open(savedPath) }
    LaunchedEffect(state.path) { savedPath = state.path }

    var showDownloadDialog by remember { mutableStateOf(false) }
    var overwritePolicy by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }

    // 发送流:选择器结果 → 条目化 → 确认弹层(元数据查询在 IO 线程)
    var sendItems by remember { mutableStateOf<List<BridgeItem>?>(null) }
    val pickForSend = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenMultipleDocuments(),
    ) { uris ->
        scope.launch {
            val picked = withContext(Dispatchers.IO) { pickerBridge.itemsFromUris(uris.map { it.toString() }) }
            if (picked.items.isEmpty()) {
                snackbar.showSnackbar("未选定可发送的文件")
            } else {
                if (picked.skipped > 0) {
                    snackbar.showSnackbar("${picked.skipped} 个文件不可读,已跳过")
                }
                sendItems = picked.items
            }
        }
    }

    // 一次性反馈:下载/发送已开始 / 失败(T11 任务页接管后续进度)
    LaunchedEffect(state.startedJob, state.downloadError) {
        state.downloadError?.let {
            snackbar.showSnackbar("下载失败:$it")
            vm.clearFeedback()
        } ?: state.startedJob?.let {
            snackbar.showSnackbar("已开始下载到收件箱(Downloads/MTDSS)")
            vm.clearFeedback()
        }
    }

    // 详情弹层(stat 的权威元数据;关闭即清)
    val details by vm.details.collectAsState()
    details?.let { d ->
        AlertDialog(
            onDismissRequest = vm::dismissDetails,
            title = { Text(d.name) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(d.path, style = MaterialTheme.typography.bodySmall)
                    Text("大小:" + Format.size(d.size, isDir = d.isDir))
                    if (d.mtimeMs != 0L) Text("修改时间:" + Format.mtime(d.mtimeMs))
                }
            },
            confirmButton = {
                TextButton(onClick = vm::dismissDetails) { Text("关闭") }
            },
        )
    }

    BackHandler(onBack = onBack)

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(device.name ?: device.deviceId, style = MaterialTheme.typography.titleMedium)
                        Text(
                            if (state.path.isEmpty()) "根" else state.path,
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回设备列表")
                    }
                },
                actions = {
                    IconButton(onClick = { vm.open(state.path) }, enabled = !state.loading) {
                        Icon(Icons.Filled.Refresh, contentDescription = "刷新")
                    }
                    IconButton(onClick = vm::up, enabled = state.path.isNotEmpty()) {
                        Icon(Icons.Filled.KeyboardArrowUp, contentDescription = "上一级")
                    }
                    // 发送入口:系统选择器多选本机文件 → 发给本设备
                    IconButton(onClick = { pickForSend.launch(arrayOf("*/*")) }) {
                        Icon(Icons.Filled.Send, contentDescription = "发送本机文件到此设备")
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            if (state.selected.isNotEmpty()) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "已选 ${state.selected.size} 项",
                        modifier = Modifier.weight(1f),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    TextButton(onClick = { showDownloadDialog = true }) { Text("下载到收件箱") }
                }
            }
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            if (state.truncated) {
                Text(
                    "目录过大,列表已截断(仅显示前 10000 项)",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 4.dp),
                )
            }
            if (state.loading) {
                LinearProgressIndicator(Modifier.fillMaxWidth())
            }
            state.error?.let {
                Text(
                    it,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(16.dp),
                )
            }
            if (!state.loading && state.entries.isEmpty() && state.error == null) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("空目录", style = MaterialTheme.typography.bodyMedium)
                }
            } else {
                LazyColumn(
                    Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(vertical = 4.dp),
                ) {
                    items(state.entries, key = { it.path }) { entry ->
                        EntryRow(
                            entry = entry,
                            checked = entry.path in state.selected,
                            onToggle = { vm.toggle(entry.path) },
                            onOpen = {
                                if (entry.isDir) vm.open(entry.path) else vm.showDetails(entry.path)
                            },
                        )
                    }
                }
            }
        }
    }

    if (showDownloadDialog) {
        AlertDialog(
            onDismissRequest = { showDownloadDialog = false },
            title = { Text("下载 ${state.selected.size} 项") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("目标:收件箱(Downloads/MTDSS)")
                    Spacer(Modifier.width(4.dp))
                    PolicyRadio(
                        selected = !overwritePolicy,
                        label = "跳过已收完的文件",
                        detail = "收完的自动跳过;中断的自动断点续传(推荐)",
                        onClick = { overwritePolicy = false },
                    )
                    PolicyRadio(
                        selected = overwritePolicy,
                        label = "全部重新接收",
                        detail = "同名文件覆盖重收",
                        onClick = { overwritePolicy = true },
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        showDownloadDialog = false
                        vm.download(overwritePolicy)
                    },
                ) { Text("下载") }
            },
            dismissButton = {
                TextButton(onClick = { showDownloadDialog = false }) { Text("取消") }
            },
        )
    }

    sendItems?.let { items ->
        SendConfirmDialog(
            controller = sendController,
            items = items,
            target = device,
            onDone = { sendItems = null },
        )
    }
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

@Composable
private fun EntryRow(
    entry: DirEntry,
    checked: Boolean,
    onToggle: () -> Unit,
    onOpen: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onOpen)
            .padding(horizontal = 8.dp, vertical = 2.dp),
    ) {
        Checkbox(checked = checked, onCheckedChange = { onToggle() })
        // 类型区分走排版(目录:前导 › + titleSmall;文件:大小副行)——
        // 核心图标集无 Folder,不为此引入 15MB 的 extended 集
        if (entry.isDir) {
            Icon(
                Icons.Filled.KeyboardArrowRight,
                contentDescription = "目录",
                tint = MaterialTheme.colorScheme.primary,
            )
            Spacer(Modifier.width(4.dp))
        } else {
            Spacer(Modifier.width(28.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(
                entry.name,
                style = if (entry.isDir) MaterialTheme.typography.titleSmall else MaterialTheme.typography.bodyLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (!entry.isDir) {
                Text(Format.size(entry.size, isDir = false), style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
