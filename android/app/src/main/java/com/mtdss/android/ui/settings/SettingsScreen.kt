package com.mtdss.android.ui.settings

import android.provider.DocumentsContract
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.update.UpdateUiState
import com.mtdss.android.ui.common.DiagnosticsSheet
import com.mtdss.android.ui.common.Format
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 设置页(spec §4.3):账号信息(会话状态投影)/ 收件箱(SAF 目录授权
 * 状态与授/重授入口,P4a-r2 spec §7.1)/ 检查更新(T12 状态机:未配置 →
 * 检查 → 已是最新|有新版 → 下载进度 → 安装器已拉起;失败 + 重试)/
 * 关于(版本)。诊断(T7 收编面)与退出登录亦在此(原调试屏入口收拢)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(graph: Graph, onBack: () -> Unit) {
    val vm = graph.settings
    val state by vm.state.collectAsState()
    val updateState by vm.update.state.collectAsState()
    val scope = rememberCoroutineScope()
    var showDiagnostics by remember { mutableStateOf(false) }
    var loggingOut by remember { mutableStateOf(false) }
    var grantError by remember { mutableStateOf<String?>(null) }

    // 收件目录授权(OPEN_DOCUMENT_TREE;初始定位系统 Download:授
    // Download/MTDSS 最小权限,或 Download 由 App 在树内自管 MTDSS 子目录)
    val grantLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri ->
        if (uri != null) {
            scope.launch {
                val ok = withContext(Dispatchers.IO) { vm.grantInbox(uri.toString()) }
                grantError = if (ok) null else "该目录不可授权,请重选"
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("设置") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "返回设备列表")
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // ── 账号 ──────────────────────────────────────────────────────────
            SectionCard("账号") {
                when (state.loggedIn) {
                    false -> Text("未登录(在设备列表页登录)")
                    else -> {
                        InfoRow("设备名", state.deviceName ?: "-")
                        InfoRow("设备 ID", state.deviceId ?: "-")
                        InfoRow(
                            "在线",
                            when (state.selfOnline) {
                                true -> "在线"
                                false -> "离线(信令未连上)"
                                null -> "-"
                            },
                        )
                    }
                }
            }

            // ── 收件箱 ────────────────────────────────────────────────────────
            SectionCard("收件箱") {
                InfoRow("位置", "Download/MTDSS")
                InfoRow("目录授权", if (state.inboxAuthorized) "已授权" else "未授权")
                Text(
                    if (state.inboxAuthorized) {
                        "接收文件经已授权目录落位(中间过程收纳于隐藏目录,完成即正式落名);重新授权可换目录。"
                    } else {
                        "首次收件前请先授权下载目录:选择 Download/MTDSS(推荐)或 Download。未授权时接收任务将记「保存失败」,授权后重发即可,无需重新传输。"
                    },
                    style = MaterialTheme.typography.bodySmall,
                )
                Button(onClick = { grantLauncher.launch(initialDownloadTreeUri()) }) {
                    Text(if (state.inboxAuthorized) "重新授权" else "授权收件目录")
                }
                grantError?.let {
                    Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                }
            }

            // ── 检查更新 ──────────────────────────────────────────────────────
            SectionCard("检查更新") {
                InfoRow("当前版本", state.appVersion)
                UpdateSection(
                    updateState = updateState,
                    onCheck = vm.update::check,
                    onDownload = vm.update::download,
                    onRetry = vm.update::retry,
                )
            }

            // ── 关于 ──────────────────────────────────────────────────────────
            SectionCard("关于") {
                InfoRow("版本", state.appVersion)
                InfoRow("引擎", "MTDSS 引擎(arm64-v8a,随 App 打包)")
                Text(
                    "应急、方便、实用的数据传输与远端浏览。",
                    style = MaterialTheme.typography.bodySmall,
                )
            }

            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 4.dp, bottom = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                TextButton(onClick = { showDiagnostics = true }) { Text("诊断") }
                Button(
                    onClick = {
                        loggingOut = true
                        scope.launch {
                            graph.repo.logout()
                            graph.session.refresh() // 权威对账(event.login 兜底)
                            loggingOut = false
                            onBack()
                        }
                    },
                    enabled = !loggingOut,
                ) { Text(if (loggingOut) "退出中…" else "退出登录") }
            }
        }
    }

    if (showDiagnostics) {
        DiagnosticsSheet(graph, onDismiss = { showDiagnostics = false })
    }
}

/** 检查更新区块:状态机全形态的呈现与动作面(词面与 UpdateUiState 一一对应)。 */
@Composable
private fun UpdateSection(
    updateState: UpdateUiState,
    onCheck: () -> Unit,
    onDownload: () -> Unit,
    onRetry: () -> Unit,
) {
    when (updateState) {
        is UpdateUiState.Unconfigured -> {
            Text("未配置更新源(构建期未注入更新基址/公钥)", color = MaterialTheme.colorScheme.onSurfaceVariant)
        }

        is UpdateUiState.Idle -> {
            Button(onClick = onCheck) { Text("检查更新") }
        }

        is UpdateUiState.Checking -> Row(verticalAlignment = Alignment.CenterVertically) {
            CircularProgressIndicator(Modifier.padding(end = 12.dp))
            Text("检查中…")
        }

        is UpdateUiState.UpToDate -> {
            Text("已是最新版本(${updateState.current})")
            TextButton(onClick = onCheck) { Text("再查一次") }
        }

        is UpdateUiState.Available -> {
            Text("发现新版本 ${updateState.latest}", style = MaterialTheme.typography.titleMedium)
            if (updateState.notes.isNotBlank()) {
                Text(updateState.notes, style = MaterialTheme.typography.bodySmall)
            }
            Button(onClick = onDownload) { Text("下载并安装") }
        }

        is UpdateUiState.Downloading -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("下载中 ${updateState.latest}: ${Format.size(updateState.received, false)} / ${Format.size(updateState.total, false)}")
            val progress = if (updateState.total > 0) {
                (updateState.received.toFloat() / updateState.total).coerceIn(0f, 1f)
            } else {
                0f
            }
            LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
        }

        is UpdateUiState.Installing -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("已下载 ${updateState.latest},系统安装器已拉起(装不装您定)")
            TextButton(onClick = onRetry) { Text("重新拉起安装器") }
        }

        is UpdateUiState.Failed -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(updateState.error, color = MaterialTheme.colorScheme.error)
            Button(onClick = onRetry) { Text("重试") }
        }
    }
}

@Composable
private fun SectionCard(title: String, content: @Composable () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(
            Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleSmall)
            content()
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

/**
 * 选择器件初始定位:主存 Download 目录的 ExternalStorageProvider 文档
 * URI(EXTRA_INITIAL_URI 尽力而为——设备不认时选择器件落缺省位,不阻断)。
 */
private fun initialDownloadTreeUri(): android.net.Uri =
    DocumentsContract.buildDocumentUri(
        "com.android.externalstorage.documents",
        "primary:Download",
    )
