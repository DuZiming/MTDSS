package com.mtdss.android.ui.local

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
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
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
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.common.Format
import com.mtdss.android.ui.send.DevicePickerDialog
import com.mtdss.android.ui.send.SendConfirmDialog
import com.mtdss.android.ui.send.SendController
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 本机页(设计 §4.3「本机侧:收件箱 + 已授权根列表」;Android 无全盘
 * 浏览,商店合规边界内只呈现这两面):
 * - **收件箱**:Downloads/MTDSS 子树(MediaStore 枚举;接收中的条目标
 *   「接收中」);
 * - **授权根**:列表 + 添加(OPEN_DOCUMENT_TREE)/移除(连带释放授权);
 *   点入经 DocumentsContract 逐级浏览,多选(文件+目录)发送;顶层
 *   「发送本目录」整树发送(大目录场景)。
 *
 * 发送流:展开条目 → 选目标设备 → [SendConfirmDialog](对端目录 + 策略)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LocalFilesScreen(graph: Graph, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember { LocalFilesViewModel(graph.roots, graph.saf, InboxListing(context), scope) }
    val state by vm.state.collectAsState()
    val sendController = remember { SendController.create(graph.repo, scope, context) }
    val snackbar = remember { SnackbarHostState() }

    var tab by remember { mutableIntStateOf(0) }
    // 发送流状态:展开出的条目 → 待选设备 → 确认
    var pendingItems by remember { mutableStateOf<List<BridgeItem>?>(null) }
    var showDevicePicker by remember { mutableStateOf(false) }
    var sendTarget by remember { mutableStateOf<DirectoryDevice?>(null) }

    // 授权根添加(OPEN_DOCUMENT_TREE:持久授权 + 账本登记;元数据/授权调用在 IO)
    val addRootLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri ->
        if (uri != null) {
            scope.launch {
                val treeUri = uri.toString()
                val name = withContext(Dispatchers.IO) {
                    graph.saf.documentMeta(treeUri)?.name ?: uri.lastPathSegment ?: "授权根"
                }
                if (vm.addRoot(treeUri, name) == null) {
                    snackbar.showSnackbar("该目录不可授权,请重选")
                }
            }
        }
    }

    // 一次性反馈:展开失败 / 发送已开始
    LaunchedEffect(state.expandError) {
        state.expandError?.let {
            snackbar.showSnackbar(it)
            vm.clearExpandError()
        }
    }

    // 树内返回键:子目录逐级上退;根层回根列表;根列表回设备页
    BackHandler(enabled = state.currentRoot != null) {
        vm.up()
    }

    val inTree = state.currentRoot != null
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text(if (inTree) state.currentRoot!!.displayName else "本机", style = MaterialTheme.typography.titleMedium)
                        if (inTree) {
                            Text(
                                state.path.joinToString("/") { it.name }.ifEmpty { "(根)" },
                                style = MaterialTheme.typography.bodySmall,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                },
                navigationIcon = {
                    IconButton(onClick = if (inTree) vm::up else onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = if (inTree) "上一级" else "返回设备列表")
                    }
                },
                actions = {
                    if (inTree) {
                        IconButton(onClick = vm::up, enabled = state.path.isNotEmpty()) {
                            Icon(Icons.Filled.KeyboardArrowUp, contentDescription = "上一级")
                        }
                        // 大目录整树发送(条目数护栏在展开层拦截)
                        IconButton(onClick = {
                            vm.expandDirectory { items ->
                                if (items.isEmpty()) {
                                    scope.launch { snackbar.showSnackbar("目录为空,无可发送内容") }
                                } else {
                                    pendingItems = items
                                    showDevicePicker = true
                                }
                            }
                        }) {
                            Icon(Icons.Filled.Send, contentDescription = "发送本目录")
                        }
                    } else {
                        IconButton(onClick = vm::refreshInbox) {
                            Icon(Icons.Filled.Refresh, contentDescription = "刷新")
                        }
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            if (inTree && state.selected.isNotEmpty()) {
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
                    TextButton(onClick = {
                        vm.expandSelection { items ->
                            if (items.isEmpty()) {
                                scope.launch { snackbar.showSnackbar("所选内容为空,无可发送文件") }
                            } else {
                                pendingItems = items
                                showDevicePicker = true
                            }
                        }
                    }) {
                        Text("发送到设备…")
                    }
                }
            }
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            if (state.currentRoot == null) {
                // 根列表层:收件箱 / 授权根 两页签
                TabRow(selectedTabIndex = tab) {
                    Tab(selected = tab == 0, onClick = { tab = 0; vm.refreshInbox() }, text = { Text("收件箱") })
                    Tab(selected = tab == 1, onClick = { tab = 1 }, text = { Text("授权根") })
                }
                when (tab) {
                    0 -> InboxPane(state)
                    else -> RootsPane(state, onAdd = { addRootLauncher.launch(null) }, onOpen = vm::openRoot, onRemove = vm::removeRoot)
                }
            } else {
                // 树浏览层
                if (state.treeLoading) LinearProgressIndicator(Modifier.fillMaxWidth())
                state.treeError?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(16.dp))
                }
                if (!state.treeLoading && state.entries.isEmpty() && state.treeError == null) {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                        Text("空目录", style = MaterialTheme.typography.bodyMedium)
                    }
                } else {
                    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 4.dp)) {
                        items(state.entries, key = { it.documentId }) { entry ->
                            TreeRow(
                                entry = entry,
                                checked = entry.documentId in state.selected,
                                onToggle = { vm.toggle(entry.documentId) },
                                onOpen = { if (entry.isDir) vm.openDir(LocalFilesViewModel.PathSegment(entry.name, entry.documentId)) },
                            )
                        }
                    }
                }
            }
        }
    }

    if (showDevicePicker) {
        DevicePickerDialog(
            graph,
            onPick = {
                showDevicePicker = false
                sendTarget = it
            },
            onDismiss = { showDevicePicker = false },
        )
    }
    val items = pendingItems
    val target = sendTarget
    if (items != null && target != null) {
        SendConfirmDialog(
            controller = sendController,
            items = items,
            target = target,
            onDone = {
                pendingItems = null
                sendTarget = null
            },
        )
    }
}

@Composable
private fun InboxPane(state: LocalFilesViewModel.UiState) {
    if (state.inboxLoading && state.inboxEntries.isEmpty()) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        return
    }
    if (state.inboxEntries.isEmpty()) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text(
                "收件箱为空。对端发来的文件落在 Downloads/MTDSS。",
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(24.dp),
            )
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 4.dp)) {
        items(state.inboxEntries, key = { it.relPath + "/" + it.displayName }) { e ->
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 6.dp),
            ) {
                Column(Modifier.weight(1f)) {
                    Text(e.displayName, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOfNotNull(
                            e.relPath.ifEmpty { null },
                            Format.size(e.size, isDir = false),
                            Format.mtime(e.mtimeMs).ifEmpty { null },
                        ).joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (e.pending) {
                    Text("接收中", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                }
            }
        }
    }
}

@Composable
private fun RootsPane(
    state: LocalFilesViewModel.UiState,
    onAdd: () -> Unit,
    onOpen: (com.mtdss.android.send.StoredRoot) -> Unit,
    onRemove: (String) -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "授权根可进目录树浏览与整目录发送;移除即回收授权。",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = onAdd) {
                Icon(Icons.Filled.Add, contentDescription = null, Modifier.width(18.dp))
                Spacer(Modifier.width(4.dp))
                Text("添加")
            }
        }
        if (state.roots.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(
                    "尚无授权根。添加后可浏览目录树并多选发送。",
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(24.dp),
                )
            }
        } else {
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 4.dp)) {
                items(state.roots, key = { it.rootId }) { root ->
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { onOpen(root) }
                            .padding(horizontal = 16.dp, vertical = 8.dp),
                    ) {
                        Icon(
                            Icons.Filled.KeyboardArrowRight,
                            contentDescription = "目录",
                            tint = MaterialTheme.colorScheme.primary,
                        )
                        Spacer(Modifier.width(4.dp))
                        Text(
                            root.displayName,
                            style = MaterialTheme.typography.bodyLarge,
                            modifier = Modifier.weight(1f),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        IconButton(onClick = { onRemove(root.rootId) }) {
                            Icon(Icons.Filled.Delete, contentDescription = "移除授权根")
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TreeRow(
    entry: com.mtdss.android.send.TreeChild,
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
            Text(entry.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (!entry.isDir) {
                Text(Format.size(entry.size, isDir = false), style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
