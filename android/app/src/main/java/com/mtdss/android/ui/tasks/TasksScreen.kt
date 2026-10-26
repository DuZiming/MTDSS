package com.mtdss.android.ui.tasks

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.ui.common.Format

/**
 * 任务屏(spec §4.2:任务列表是一等屏——中断是 Android 传输的常态,任务
 * 行是断点续传/重发的操作面)。VM 为 app 级(AppGraph 装配,任意屏面期间
 * 活任务均被追踪并驱动 KEEP_SCREEN_ON),本屏只呈现 + 出键:
 * - 行面:三色徽章(发送/接收/调度;被动 incoming 归接收,未知种类灰档
 *   防御渲染)+ 任务名(多文件「· N 项」)+ 状态 + 方向(src → dst)+
 *   进度(bytes/files + 速率;失败行呈错误词面;跳过计数随行显)+
 *   操作行(可用性矩阵 = [TasksReducer.actionsFor],busy 置灰防连点);
 * - 筛选四档(全部/发送/接收/调度,镜像桌面抽屉)+ 手动刷新;
 * - 打开即对账(reseed):事件新建行(帧不带 name/src/dst)由此补齐。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TasksScreen(graph: Graph, onBack: () -> Unit) {
    val vm = graph.tasks
    val state by vm.state.collectAsState()
    val snackbar = remember { SnackbarHostState() }
    var filter by rememberSaveable { mutableIntStateOf(0) }

    // 打开即对账(全量重拉;name/src/dst 补齐 + 摘除幽灵行)
    LaunchedEffect(Unit) { vm.reseed() }

    // 操作失败一次性反馈 → snackbar
    val opError = state.opError
    LaunchedEffect(opError) {
        if (opError != null) {
            snackbar.showSnackbar(opError)
            vm.clearOpError()
        }
    }

    BackHandler(onBack = onBack)
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("任务") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回设备列表")
                    }
                },
                actions = {
                    IconButton(onClick = vm::reseed) {
                        Icon(Icons.Filled.Refresh, contentDescription = "刷新")
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
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                FILTERS.forEachIndexed { i, label ->
                    FilterChip(
                        selected = filter == i,
                        onClick = { filter = i },
                        label = { Text(label) },
                    )
                }
            }

            when {
                state.loading && state.rows.isEmpty() -> Box(
                    Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                state.error != null && state.rows.isEmpty() -> Box(
                    Modifier
                        .fillMaxSize()
                        .padding(24.dp),
                    contentAlignment = Alignment.Center,
                ) { Text(state.error ?: "", color = MaterialTheme.colorScheme.error) }

                else -> {
                    val shown = filterRows(state.rows, filter)
                    LazyColumn(
                        modifier = Modifier.fillMaxSize(),
                        contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        items(shown, key = { it.id }) { job ->
                            TaskRow(
                                job = job,
                                rows = state.rows,
                                busy = job.id in state.busy,
                                onPause = vm::pause,
                                onResume = vm::resume,
                                onCancel = vm::cancel,
                                onReinitiate = vm::reinitiate,
                                onClear = vm::clear,
                            )
                        }
                        if (shown.isEmpty()) {
                            item {
                                Text(
                                    if (state.rows.isEmpty()) {
                                        "暂无任务。发送/接收的任务会出现在这里,可暂停、续传、重新发起。"
                                    } else {
                                        "该筛选下暂无任务。"
                                    },
                                    style = MaterialTheme.typography.bodyMedium,
                                    modifier = Modifier.padding(24.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

/** 筛选四档(镜像桌面抽屉 chips;「接收」含被动来件 incoming)。 */
private val FILTERS = listOf("全部", "发送", "接收", "调度")

/** 按筛选档过滤(保首见序;index 与 [FILTERS] 对应,越界兜底「全部」)。 */
private fun filterRows(rows: List<TransferJob>, index: Int): List<TransferJob> = when (index) {
    1 -> rows.filter { it.kind == "upload" }
    2 -> rows.filter { it.kind == "download" || it.kind == "incoming" }
    3 -> rows.filter { it.kind == "remote_copy" }
    else -> rows
}

/** 单行:徽章/名/状态 + 方向 + 进度 + 操作(镜像桌面抽屉六列的移动端折叠)。 */
@Composable
private fun TaskRow(
    job: TransferJob,
    rows: List<TransferJob>,
    busy: Boolean,
    onPause: (String) -> Unit,
    onResume: (String) -> Unit,
    onCancel: (String) -> Unit,
    onReinitiate: (String) -> Unit,
    onClear: (String) -> Unit,
) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 6.dp)) {
            // 头行:徽章 + 任务名(+中继兜底角标) + 状态
            Row(verticalAlignment = Alignment.CenterVertically) {
                KindBadge(TasksReducer.badgeFor(job.kind))
                Spacer(Modifier.width(8.dp))
                Text(
                    TasksReducer.displayName(job),
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (job.relay) {
                    Spacer(Modifier.width(6.dp))
                    Text(
                        "中继",
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Spacer(Modifier.width(8.dp))
                Text(
                    TasksReducer.stateText(job.state),
                    style = MaterialTheme.typography.labelLarge,
                    color = stateColor(job.state),
                )
            }
            // 方向行:src → dst(事件新建行无路径面,占位「…」)
            Text(
                "${job.src ?: "…"}  →  ${job.dst ?: "…"}",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            // 进度/错误面
            if (job.state == "failed" || (job.error != null && TasksReducer.isTerminalState(job.state))) {
                Text(
                    job.error ?: "失败",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            } else {
                Spacer(Modifier.height(6.dp))
                val ratio = TasksReducer.progressRatio(job.bytesDone, job.bytesTotal)
                if (ratio == null) {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                } else {
                    LinearProgressIndicator(progress = { ratio }, modifier = Modifier.fillMaxWidth())
                }
                Spacer(Modifier.height(4.dp))
                Text(
                    buildString {
                        append(
                            "${Format.size(job.bytesDone, isDir = false)} / ${Format.size(job.bytesTotal, isDir = false)}",
                        )
                        append(" · ${job.filesDone}/${job.filesTotal} 项")
                        if (job.state == "running") append(" · ${TasksReducer.speedText(job.speedBps)}")
                        if (job.skipped > 0) append(" · 跳过 ${job.skipped}")
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            // 操作行(可用性矩阵;busy 置灰防连点)
            val actions = TasksReducer.actionsFor(job, rows)
            if (actions.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    for (a in actions) {
                        val (label, onClick) = when (a) {
                            TasksReducer.TaskAction.PAUSE -> "暂停" to onPause
                            TasksReducer.TaskAction.RESUME -> "继续" to onResume
                            TasksReducer.TaskAction.CANCEL -> "取消" to onCancel
                            TasksReducer.TaskAction.REINITIATE -> "重新发起" to onReinitiate
                            TasksReducer.TaskAction.CLEAR -> "清除" to onClear
                        }
                        TextButton(onClick = { onClick(job.id) }, enabled = !busy) { Text(label) }
                    }
                }
            }
        }
    }
}

/** 三色徽章(桌面同款配色:发送蓝/接收绿/调度橙;其他灰档防御渲染)。 */
@Composable
private fun KindBadge(badge: TasksReducer.TaskBadge) {
    val bg = when (badge) {
        TasksReducer.TaskBadge.UPLOAD -> Color(0xFF1E88E5)
        TasksReducer.TaskBadge.DOWNLOAD -> Color(0xFF2E7D32)
        TasksReducer.TaskBadge.SCHEDULE -> Color(0xFFEF6C00)
        TasksReducer.TaskBadge.OTHER -> Color(0xFF757575)
    }
    Text(
        "${badge.glyph} ${badge.label}",
        style = MaterialTheme.typography.labelSmall,
        color = Color.White,
        modifier = Modifier
            .background(bg, RoundedCornerShape(4.dp))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

@Composable
private fun stateColor(state: String): Color = when (state) {
    "queued", "running" -> MaterialTheme.colorScheme.primary
    "paused", "cancelled", "interrupted" -> MaterialTheme.colorScheme.onSurfaceVariant
    "failed" -> MaterialTheme.colorScheme.error
    "done" -> Color(0xFF2E7D32)
    else -> MaterialTheme.colorScheme.onSurfaceVariant
}
