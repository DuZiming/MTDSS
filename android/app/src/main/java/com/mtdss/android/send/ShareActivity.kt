package com.mtdss.android.send

import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Share
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.AppGraph
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.share.ShareQueueConsumer
import com.mtdss.android.share.ShareQueueFile
import com.mtdss.android.share.ShareQueueStatus
import com.mtdss.android.share.ShareQueueStore
import com.mtdss.android.ui.common.DeviceRow
import com.mtdss.android.ui.common.Format
import com.mtdss.android.ui.common.deviceTappable
import com.mtdss.android.ui.send.DevicePickerViewModel
import com.mtdss.android.ui.theme.SynshuTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.IOException

/**
 * 系统分享入口(V3-4 §4.4 T15 重做;**暂存转交模型,对齐 iOS 方案 A**——
 * 一次性 content URI 语义退役):分享到达 → **立即拷贝进 App 私有暂存区**
 * ([ShareQueueStore],{filesDir}/share,引擎 home 外、不进授权集、对端不可
 * 见)→ 入队落盘(meta.json 两步原子)→ 转交主 App 消费面桥式发送(0x01
 * request_source,暂存件作源)。中断/进程死亡不再「源已不存在」:半件由
 * 消费端宽限窗折算「文件拷贝中断」失败行,发送失败折「分享转交失败」行
 * (传输列表顶区,重试 = 重选设备即重发,暂存件不重拷)。
 *
 * 面板形态(A 皮肤,spec §3 语言):条目化([PickerBridge/itemsFromShareUris]
 * 取名链:OpenableColumns 主通道 → MediaStore 直查 → DocumentsContract,
 * 相册类 provider 纯数字垃圾名下沉取真名——R5/J1;不可读逐条带因——F9
 * 词面直达)→ **设备列表**(
 * [DeviceRow] 既有共用,在线可选、离线灰显、滤本机)→ 点设备 = 两步拷贝
 * + 入队 → 收口三分:已受理「已开始发送」/ 未登录等待态「已暂存,打开
 * Synshu 后自动发送」/ 受理失败(失败行已入传输列表,面板给词面+关闭)。
 *
 * 引擎与 AppGraph 由 MtdssApplication.onCreate 随进程冷启动装配(与
 * MainActivity 同径),此处只取 [AppGraph.get]。
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uris = PickerBridge.shareUrisFromIntent(intent)
        setContent {
            SynshuTheme {
                ShareScreen(uris = uris, onDone = { finish() })
            }
        }
    }
}

/** 面板阶段(条目化 → 设备列表 → 暂存拷贝 → 收口)。 */
private sealed interface SharePhase {
    /** 设备列表(选目标)。 */
    data object Pick : SharePhase

    /** 两步拷贝+入队进行中(拷贝不可取消语义:半件由失败行折算,不打断)。 */
    data class Staging(val done: Int, val total: Int) : SharePhase

    /** 收口(三分,见类注释)。 */
    data class Result(val outcome: ShareOutcome) : SharePhase
}

/** 收口三分(已受理/等待登录/受理失败)。 */
private sealed interface ShareOutcome {
    data object Sent : ShareOutcome
    data object Queued : ShareOutcome
    data class Failed(val reason: String) : ShareOutcome
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
    // 设备列表(devices 现拉 + event.devices 跟线;DevicePickerSheet 同 VM)
    val deviceVm = remember { DevicePickerViewModel(graph.repo, graph.hub, scope) }
    val deviceState by deviceVm.state.collectAsState()
    val sessionState by graph.session.state.collectAsState()
    val selfDeviceId = sessionState.deviceId

    // 条目化:IO 线程逐 URI 取持久授权(尽力)+ 取名链(OpenableColumns 主
    // 通道 → MediaStore 直查 → DocumentsContract,R5/J1 相册垃圾名下沉);Compose 侧只持结果
    var picked by remember { mutableStateOf<PickerBridge.Picked?>(null) }
    var phase by remember { mutableStateOf<SharePhase>(SharePhase.Pick) }
    // 入队被守卫/拷贝守卫拒的词面(设备列表上的红字,可另选设备重来)
    var enqueueError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(uris) {
        picked = withContext(Dispatchers.IO) { picker.itemsFromShareUris(uris) }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("分享到 Synshu") },
                navigationIcon = {
                    IconButton(onClick = onDone) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "取消")
                    }
                },
            )
        },
    ) { padding ->
        val p = picked
        when {
            // 条目化进行中(元数据查询)
            p == null -> Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                CircularProgressIndicator()
            }

            // 分享件不可用(纯文本分享/全部不可读;iOS ContentUnavailableView 同构留白)
            p.items.isEmpty() -> UnreadableFace(picked = p, uris = uris, onDone = onDone, modifier = Modifier.padding(padding))

            // 条目数护栏(>512 桥接上限;词面与 daemon 逐字一致,不进拷贝)
            BridgeLimits.itemCountError(p.items.size) != null -> CenterFace(
                modifier = Modifier.padding(padding),
                icon = { Icon(Icons.Filled.Share, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                head = BridgeLimits.itemCountError(p.items.size) ?: "",
            ) {
                TextButton(onClick = onDone, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("关闭") }
            }

            phase is SharePhase.Result -> ResultFace(
                outcome = (phase as SharePhase.Result).outcome,
                modifier = Modifier.padding(padding),
                onDone = onDone,
            )

            phase is SharePhase.Staging -> Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                val s = phase as SharePhase.Staging
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator()
                    Text(
                        "正在暂存 ${s.done}/${s.total} 项…",
                        Modifier.padding(top = 12.dp),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            else -> PickFace(
                picked = p,
                devices = deviceState.devices.filter { it.deviceId != selfDeviceId },
                loading = deviceState.loading && deviceState.devices.isEmpty(),
                error = deviceState.error,
                enqueueError = enqueueError,
                modifier = Modifier.padding(padding),
                onPick = { target ->
                    enqueueError = null
                    scope.launch {
                        val r = runEnqueue(graph, context, p, target) { phase = it }
                        if (r.failure != null) {
                            enqueueError = r.failure
                            phase = SharePhase.Pick
                        } else {
                            phase = SharePhase.Result(r.outcome ?: ShareOutcome.Queued)
                        }
                    }
                },
            )
        }
    }
}

/** 入队产物(outcome 非空 = 已入队并经消费面;failure 非空 = 入队被拒词面)。 */
private class EnqueueOutcome(val outcome: ShareOutcome?, val failure: String?)

/**
 * 入队(拷贝+落盘+转交;IO 线程):begin(copying)→ 逐文件两步拷贝(
 * 拷后计数守卫,坏件整项移除不留半套)→ finish(ready)→ 等待版消费(
 * 登录门在消费面;未登录 = 等待态)→ 读 meta 三分收口。[onPhase] 回报
 * 暂存进度(Staging 面)。
 */
private suspend fun runEnqueue(
    graph: com.mtdss.android.Graph,
    context: android.content.Context,
    picked: PickerBridge.Picked,
    target: DirectoryDevice,
    onPhase: (SharePhase) -> Unit,
): EnqueueOutcome {
    val store = graph.shareQueue
    val id = ShareQueueStore.newItemId(System.currentTimeMillis())
    val files = picked.items.map { ShareQueueFile(it.rel, it.size) }
    var done = 0
    onPhase(SharePhase.Staging(done = 0, total = files.size))
    val failure: String? = withContext(Dispatchers.IO) {
        try {
            store.beginItem(id, target.deviceId, files, System.currentTimeMillis())
            for (item in picked.items) {
                val stream = context.contentResolver.openInputStream(Uri.parse(item.ref))
                    ?: throw IOException("源不可读")
                stream.use { ins ->
                    store.copyStream(id, ShareQueueFile(item.rel, item.size), ins)
                }
                done++
                onPhase(SharePhase.Staging(done = done, total = files.size))
            }
            store.finishItem(id, System.currentTimeMillis())
            null
        } catch (e: Exception) {
            // 不留坏项(半件/守卫拒收整项移除);可另选设备重来
            store.removeItem(id)
            ShareQueueConsumer.errorText(e)
        }
    }
    if (failure != null) {
        return EnqueueOutcome(outcome = null, failure = failure)
    }
    // 转交主 App 消费(等待版;登录前不动队列 = 等待态收口)
    graph.shareFailures.consumeAwaiting()
    val meta = withContext(Dispatchers.IO) { store.readMeta(id) }
    val outcome = when (meta?.status) {
        ShareQueueStatus.SENT -> ShareOutcome.Sent
        ShareQueueStatus.FAILED -> ShareOutcome.Failed(meta.error ?: "转交失败")
        else -> ShareOutcome.Queued
    }
    return EnqueueOutcome(outcome = outcome, failure = null)
}

// ── 面板各态(A 皮肤;spec §3 设备行/留白语言)──────────────────────────────

/** 设备列表态:条目摘要 + 入队错误词面(可另选设备重试)+ 设备行列表。 */
@Composable
private fun PickFace(
    picked: PickerBridge.Picked,
    devices: List<DirectoryDevice>,
    loading: Boolean,
    error: String?,
    enqueueError: String?,
    modifier: Modifier = Modifier,
    onPick: (DirectoryDevice) -> Unit,
) {
    Column(
        modifier
            .fillMaxSize()
            .padding(horizontal = 20.dp),
    ) {
        Text(
            "发送 ${picked.items.size} 项到…",
            Modifier.padding(top = 14.dp, bottom = 4.dp),
            color = MaterialTheme.colorScheme.onSurface,
            fontSize = 17.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            "共 ${Format.size(picked.items.sumOf { it.size }, isDir = false)} · 暂存后自动发送到对端收件箱",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        // 分享件清单(iOS panel 项目清单同构;最多 3 行溢出尾注)
        val preview = picked.items.take(3)
        preview.forEach { item ->
            Text(
                item.rel,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (picked.items.size > preview.size) {
            Text(
                "……另有 ${picked.items.size - preview.size} 项",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (picked.skipped > 0) {
            // F9:「不可读」带因——provider 异常词面直达 UI,复现可从词面定位
            Text(
                "${picked.skipped} 个文件不可读,已跳过",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
            picked.skipReasons.take(2).forEach { reason ->
                Text(
                    reason,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        enqueueError?.let {
            Text(
                "暂存失败:$it(可另选设备重试)",
                Modifier.padding(top = 6.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        Text(
            "设备",
            Modifier.padding(top = 14.dp, bottom = 2.dp),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        when {
            loading -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator()
            }

            error != null && devices.isEmpty() -> CenterFace(
                modifier = Modifier.weight(1f),
                head = "设备列表不可用",
                body = error,
            )

            devices.isEmpty() -> CenterFace(
                modifier = Modifier.weight(1f),
                head = "暂无其他设备",
                body = "其他设备登录同一账号后出现在这里",
            )

            else -> LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                items(devices, key = { it.deviceId }) { device ->
                    DeviceRow(
                        device = device,
                        isSelf = false,
                        enabled = deviceTappable(device, selfDeviceId = null),
                        onClick = { if (device.online) onPick(device) },
                    )
                }
            }
        }
    }
}

/** 收口三分(iOS panel 完成/排队/失败同构):词面 + 关闭。 */
@Composable
private fun ResultFace(outcome: ShareOutcome, modifier: Modifier = Modifier, onDone: () -> Unit) {
    CenterFace(
        modifier = modifier,
        icon = { Icon(Icons.Filled.Share, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
        head = when (outcome) {
            is ShareOutcome.Sent -> "已开始发送"
            is ShareOutcome.Queued -> "已暂存"
            is ShareOutcome.Failed -> "分享转交失败"
        },
        body = when (outcome) {
            is ShareOutcome.Sent -> "进度见传输列表;完成前暂存件自动保留"
            is ShareOutcome.Queued -> "打开 Synshu 登录后自动发送"
            is ShareOutcome.Failed -> outcome.reason
        },
        bodyError = outcome is ShareOutcome.Failed,
    ) {
        TextButton(onClick = onDone, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("关闭") }
    }
}

/** 分享件不可用态(纯文本/全部条目化失败;iOS ContentUnavailableView 同构)。 */
@Composable
private fun UnreadableFace(
    picked: PickerBridge.Picked,
    uris: List<String>,
    modifier: Modifier = Modifier,
    onDone: () -> Unit,
) {
    CenterFace(
        modifier = modifier,
        icon = { Icon(Icons.Filled.Share, contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant) },
        head = if (uris.isEmpty()) "未收到可发送的文件(纯文本分享暂不支持)" else "分享的文件不可读,无法发送",
    ) {
        // F9:「不可读」带因——provider 异常词面直达 UI,复现可从词面定位
        picked.skipReasons.take(3).forEach { reason ->
            Text(
                reason,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.align(Alignment.CenterHorizontally),
            )
        }
        if (picked.skipReasons.size > 3) {
            Text(
                "……另有 ${picked.skipReasons.size - 3} 条",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.align(Alignment.CenterHorizontally),
            )
        }
        TextButton(onClick = onDone, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("关闭") }
    }
}

/** 居中留白面(iOS ContentUnavailableView 同构:图标/标题/正文/尾部槽)。 */
@Composable
private fun CenterFace(
    modifier: Modifier = Modifier,
    icon: (@Composable () -> Unit)? = null,
    head: String,
    body: String? = null,
    bodyError: Boolean = false,
    tail: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit = {},
) {
    Box(modifier.fillMaxSize().padding(horizontal = 24.dp), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            icon?.invoke()
            Text(
                head,
                Modifier.padding(top = 12.dp),
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
            )
            body?.let {
                Text(
                    it,
                    Modifier.padding(top = 6.dp),
                    color = if (bodyError) {
                        MaterialTheme.colorScheme.error
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    fontSize = 13.sp,
                    textAlign = TextAlign.Center,
                )
            }
            tail()
        }
    }
}
