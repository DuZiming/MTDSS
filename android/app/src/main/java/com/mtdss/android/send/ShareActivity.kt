package com.mtdss.android.send

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.mtdss.android.AppGraph
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.send.DevicePickerDialog
import com.mtdss.android.ui.send.SendConfirmDialog
import com.mtdss.android.ui.send.SendController
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.withContext

/**
 * 系统分享入口(ACTION_SEND / ACTION_SEND_MULTIPLE,type 任意):分享件
 * → 选目标设备 → [SendConfirmDialog] 发送。引擎与 AppGraph 由
 * MtdssApplication.onCreate 随进程冷启动装配(与 MainActivity 同径),
 * 此处只取 [AppGraph.get]。
 *
 * ## 分享件的授权退化(spec 评审裁定③,产品级绑定)
 * ACTION_SEND 给的 URI 是**临时授权**(随本进程存活,不可持久)——
 * [PickerBridge.itemsFromUris] 里的 takePersistableUriPermission 仍会
 * **尝试**(个别来源的 ACTION_SEND 附带持久授权标志,允许则白赚)+ 忽略
 * 失败(常态 SecurityException)。后果:中断后「重新发起」时 ref 还指向
 * 同一 URI 但授权已失 → request_source 打开失败 → 任务 failed
 * 「源不可用」= **分享件为一次性**。UI 文案明示:
 * 「分享件为一次性;可续传重发请用选择器/授权根」。
 *
 * ## 分享源兼容(设备场 F9)
 * URI 提取 = EXTRA_STREAM(单/多件)× ClipData 的并集(MIUI 部分分享源
 * 只填 ClipData);MIUI 相册等给的是 MediaStore URI(content://media/…),
 * 无 last_modified 列——元数据查询在 [SafFace] 的最小投影兜底上通过。
 * 跳过/不可读的原因逐条带 provider 异常词面(见 [PickerBridge.Picked.skipReasons])。
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uris = PickerBridge.shareUrisFromIntent(intent)
        setContent {
            MaterialTheme {
                ShareScreen(uris = uris, onDone = { finish() })
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ShareScreen(uris: List<String>, onDone: () -> Unit) {
    val graph = AppGraph.get()
    val context = LocalContext.current
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val picker = remember { PickerBridge(graph.saf) }
    val sendController = remember { SendController.create(graph.repo, scope, context) }

    // 条目化:IO 线程逐 URI 取持久授权(尽力)+ 元数据;Compose 侧只持结果
    var picked by remember { mutableStateOf<PickerBridge.Picked?>(null) }
    var target by remember { mutableStateOf<DirectoryDevice?>(null) }
    var showPicker by remember { mutableStateOf(true) }
    LaunchedEffect(uris) {
        picked = withContext(Dispatchers.IO) { picker.itemsFromUris(uris) }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("分享到 MTDSS") },
                navigationIcon = {
                    IconButton(onClick = onDone) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "取消")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            val p = picked
            if (p == null) {
                // 条目化进行中(元数据查询)
                LinearProgressIndicator(Modifier.fillMaxWidth())
            } else if (p.items.isEmpty()) {
                Icon(
                    Icons.Filled.Share,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.align(Alignment.CenterHorizontally).padding(top = 32.dp),
                )
                Text(
                    if (uris.isEmpty()) "未收到可发送的文件(纯文本分享暂不支持)" else "分享的文件不可读,无法发送",
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.align(Alignment.CenterHorizontally),
                )
                // F9:「不可读」带因——provider 异常词面直达 UI,复现可从词面定位
                p.skipReasons.take(3).forEach { reason ->
                    Text(
                        reason,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.align(Alignment.CenterHorizontally),
                    )
                }
                if (p.skipReasons.size > 3) {
                    Text(
                        "……另有 ${p.skipReasons.size - 3} 条",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.align(Alignment.CenterHorizontally),
                    )
                }
                TextButton(onClick = onDone, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("关闭") }
            } else {
                // 分享件一次性语义明示(词面为任务列表/验收共用口径)
                Text(
                    "分享件为一次性;可续传重发请用选择器/授权根。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (p.skipped > 0) {
                    Text(
                        "${p.skipped} 个文件不可读,已跳过",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    // F9:跳过原因(词面带 provider 异常,复现可定位)
                    p.skipReasons.take(3).forEach { reason ->
                        Text(
                            reason,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                Text("共 ${p.items.size} 项:", style = MaterialTheme.typography.titleSmall)
                LazyColumn(Modifier.weight(1f)) {
                    items(p.items, key = { it.ref }) { item ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text(item.rel, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
                if (target == null) {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Spacer(Modifier.height(4.dp))
                    Text("正在选择目标设备…", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }

    val p = picked
    if (p != null && p.items.isNotEmpty() && showPicker && target == null) {
        DevicePickerDialog(
            graph,
            onPick = {
                target = it
                showPicker = false
            },
            onDismiss = {
                showPicker = false
                onDone()
            },
        )
    }
    val dev = target
    if (p != null && p.items.isNotEmpty() && dev != null) {
        SendConfirmDialog(
            controller = sendController,
            items = p.items,
            target = dev,
            note = "分享件为一次性;可续传重发请用选择器/授权根。",
            onDone = onDone,
        )
    }
}
