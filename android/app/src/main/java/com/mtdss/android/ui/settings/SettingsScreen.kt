package com.mtdss.android.ui.settings

import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.update.UpdateUiState
import com.mtdss.android.ui.common.DiagnosticsSheet
import com.mtdss.android.ui.common.Format
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.common.SynshuGlyphs
import com.mtdss.android.ui.profile.ProfileNavBar
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuColorsDark
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 设置页(V3-4 spec §3-8 分组卡;iOS SettingsView.swift 词面逐字 + 视觉稿
 * 06 底注规格:白卡圆角 10,#F2F2F7 底组间距,行 min 50dp,组内发丝线内缩
 * 16):
 * - **我的设备名**(点入改;alert「修改后将在下次登录时对其他设备生效」;
 *   spec §4.6 设备名迁设置,登录页只读注脚)+ 组脚注「设备名修改将在下次
 *   登录时对其他设备生效」;
 * - **显示隐藏项**(默认关)/ **允许删除远端文件**(默认关)= 双门开关
 *   (spec §4.5/§4.2;持久在 [SettingsPrefs],消费面经 [SettingsViewModel.gates]
 *   单源流转 T13 各屏即时生效);
 * - **信任目录**(其他设备可浏览):授权根列表(移除 = 居中确认)+ 添加
 *   (系统目录选择器即授权 = 既有 RootsStore 账本;**roots.set 引擎同步归
 *   T14**,本页只动 App 侧账本);
 * - **检查更新**(T12 状态机全形态)/ **关于**(版本);
 * - **收件箱**(P4a-r2 安卓留存面:iOS 无 SAF,安卓收件须授权树——spec
 *   §3-8 词表外保留,记录为安卓有意差异)。
 */
@Composable
fun SettingsScreen(graph: Graph, onBack: () -> Unit) {
    val vm = graph.settings
    val state by vm.state.collectAsState()
    val updateState by vm.update.state.collectAsState()
    val trustedRoots by vm.trustedRoots.collectAsState()
    val scope = rememberCoroutineScope()
    var showRename by rememberSaveable { mutableStateOf(false) }
    var renameText by rememberSaveable { mutableStateOf("") }
    var pendingRemove by remember { mutableStateOf<StoredRoot?>(null) }
    var grantError by remember { mutableStateOf<String?>(null) }
    var showDiagnostics by remember { mutableStateOf(false) }

    // 信任目录添加(OPEN_DOCUMENT_TREE;选定即授权 = RootsStore 账本入账;
    // roots.set 引擎同步 = RootsStore.onChange → RootsSync 自动推送,T14)
    val trustLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri ->
        if (uri != null) {
            val docId = graph.saf.treeDocumentId(uri.toString())
            val name = docId?.substringAfterLast('/')?.ifBlank { null }
                ?: docId?.substringAfter(':').orEmpty()
            vm.addTrustedRoot(uri.toString(), name)
        }
    }

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

    Column(
        Modifier
            .fillMaxSize()
            .background(settingsPageBackground()),
    ) {
        ProfileNavBar(title = "设置", showsBack = true, onBack = onBack)
        Column(
            Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp)
                .padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            // ── 身份组:设备名 + 双开关 ────────────────────────────────────
            GroupCard {
                RowMin(
                    label = "我的设备名",
                    onClick = {
                        renameText = state.deviceNameOverride ?: state.deviceName ?: Build.MODEL
                        showRename = true
                    },
                    trailing = {
                        Text(
                            state.deviceNameOverride ?: state.deviceName ?: Build.MODEL,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = 12.5.sp,
                            maxLines = 1,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        Chevron()
                    },
                )
                Hairline(leadingInset = 16.dp)
                ToggleRow(
                    label = "显示隐藏项",
                    sub = "浏览时显示隐藏目录与文件",
                    checked = state.showHidden,
                    onToggle = vm::setShowHidden,
                )
                Hairline(leadingInset = 16.dp)
                ToggleRow(
                    label = "允许删除远端文件",
                    sub = "开启后,浏览其他设备时可对文件执行删除",
                    checked = state.allowRemoteDelete,
                    onToggle = vm::setAllowRemoteDelete,
                    toggleLabel = "allow-remote-delete-toggle",
                )
                // 改名生效面注脚(iOS 脚注逐字)
                Text(
                    "设备名修改将在下次登录时对其他设备生效",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.5.sp,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }

            // ── 信任目录组(iOS trustSection:组头 + 根行 + 添加 + 脚注)────
            Text(
                "信任目录(其他设备可浏览)",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                modifier = Modifier
                    .padding(start = 16.dp, top = 12.dp, bottom = 6.dp),
            )
            GroupCard {
                trustedRoots.forEachIndexed { i, root ->
                    if (i > 0) Hairline(leadingInset = 16.dp)
                    RowMin(
                        label = root.displayName,
                        onClick = null,
                        leading = {
                            MockGlyph(SynshuGlyphs.FOLDER, 23.dp, MaterialTheme.colorScheme.primary, fill = true)
                            Spacer(Modifier.width(13.dp))
                        },
                        trailing = {
                            Column(horizontalAlignment = Alignment.End) {
                                Text(
                                    "移除",
                                    color = SynshuSemantic.Fail,
                                    fontSize = 13.sp,
                                    modifier = Modifier
                                        .clickable(onClickLabel = "移除") { pendingRemove = root }
                                        .padding(horizontal = 8.dp, vertical = 6.dp),
                                )
                            }
                        },
                        sub = {
                            Text(
                                "已授权",
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                fontSize = 12.5.sp,
                            )
                        },
                    )
                }
                if (trustedRoots.isNotEmpty()) Hairline(leadingInset = 16.dp)
                RowMin(
                    label = "添加信任目录…",
                    labelColor = MaterialTheme.colorScheme.primary,
                    onClick = { trustLauncher.launch(initialDownloadTreeUri()) },
                    leading = {
                        MockGlyph(PLUS_PATH, 17.dp, MaterialTheme.colorScheme.primary, strokeWidth = 2f)
                        Spacer(Modifier.width(13.dp))
                    },
                )
                Text(
                    "点「添加」打开系统目录选择器,选定即授权\n收件箱默认对其他设备可见,无需添加",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.5.sp,
                    textAlign = TextAlign.Center,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 18.dp, bottom = 10.dp),
                )
            }

            // ── 收件箱组(P4a-r2 安卓留存面;iOS 无此组——无 SAF)────────────
            Text(
                "收件箱",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                modifier = Modifier
                    .padding(start = 16.dp, top = 12.dp, bottom = 6.dp),
            )
            GroupCard {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    InfoLine("位置", "Download/MTDSS")
                    InfoLine("目录授权", if (state.inboxAuthorized) "已授权" else "未授权")
                    Text(
                        if (state.inboxAuthorized) {
                            "接收文件经已授权目录落位(中间过程收纳于隐藏目录,完成即正式落名);重新授权可换目录。"
                        } else {
                            "首次收件前请先授权下载目录:选择 Download/MTDSS(推荐)或 Download。未授权时接收任务将记「保存失败」,授权后重发即可,无需重新传输。"
                        },
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 12.5.sp,
                    )
                    Button(onClick = { grantLauncher.launch(initialDownloadTreeUri()) }) {
                        Text(if (state.inboxAuthorized) "重新授权" else "授权收件目录")
                    }
                    grantError?.let {
                        Text(it, color = MaterialTheme.colorScheme.error, fontSize = 12.5.sp)
                    }
                }
            }

            // ── 检查更新组 ────────────────────────────────────────────────
            GroupCard {
                RowMin(
                    label = "当前版本",
                    onClick = null,
                    trailing = {
                        Text(
                            state.appVersion,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = 12.5.sp,
                            maxLines = 1,
                        )
                    },
                )
                Hairline(leadingInset = 16.dp)
                UpdateSection(
                    updateState = updateState,
                    onCheck = vm.update::check,
                    onDownload = vm.update::download,
                    onRetry = vm.update::retry,
                )
            }

            // ── 关于组 ────────────────────────────────────────────────────
            GroupCard {
                RowMin(
                    label = "版本",
                    onClick = null,
                    trailing = {
                        Text(
                            state.appVersion,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = 12.5.sp,
                            maxLines = 1,
                        )
                    },
                )
                Hairline(leadingInset = 16.dp)
                RowMin(
                    label = "引擎",
                    onClick = null,
                    trailing = {
                        Text(
                            "MTDSS 引擎(arm64-v8a,随 App 打包)",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = 12.5.sp,
                            maxLines = 1,
                        )
                    },
                )
                Hairline(leadingInset = 16.dp)
                // 诊断(安卓留存调试面;spec 词表外,iOS 无此行)
                RowMin(label = "诊断", onClick = { showDiagnostics = true }, trailing = { Chevron() })
                Text(
                    "应急、方便、实用的数据传输与远端浏览。",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.5.sp,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
    }

    // 改名 alert(iOS 同词面:标题「我的设备名」+ 正文「修改后将在下次登录
    // 时对其他设备生效」+ 保存/取消;空名禁提交)
    if (showRename) {
        AlertDialog(
            onDismissRequest = { showRename = false },
            title = { Text("我的设备名") },
            text = {
                Column {
                    Text(
                        "修改后将在下次登录时对其他设备生效",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                    )
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = renameText,
                        onValueChange = { renameText = it },
                        placeholder = { Text("设备名") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(
                    enabled = renameText.isNotBlank(),
                    onClick = {
                        vm.setDeviceName(renameText)
                        showRename = false
                    },
                ) { Text("保存") }
            },
            dismissButton = {
                TextButton(onClick = { showRename = false }) { Text("取消") }
            },
        )
    }

    // 信任目录移除确认(iOS 同词面:「移除「N」?」+「其他设备将无法再浏览
    // 该目录」+ 移除红/取消;居中 alert)
    pendingRemove?.let { root ->
        AlertDialog(
            onDismissRequest = { pendingRemove = null },
            title = { Text("移除「${root.displayName}」?") },
            text = { Text("其他设备将无法再浏览该目录") },
            confirmButton = {
                TextButton(onClick = {
                    pendingRemove = null
                    vm.removeTrustedRoot(root.rootId)
                }) { Text("移除", color = SynshuSemantic.Fail) }
            },
            dismissButton = {
                TextButton(onClick = { pendingRemove = null }) { Text("取消") }
            },
        )
    }

    // 添加失败弹层(iOS 同词面:「添加信任目录失败」+ 好)
    state.trustError?.let { error ->
        AlertDialog(
            onDismissRequest = vm::clearTrustError,
            title = { Text("添加信任目录失败") },
            text = { Text(error) },
            confirmButton = {
                TextButton(onClick = vm::clearTrustError) { Text("好") }
            },
        )
    }

    if (showDiagnostics) {
        DiagnosticsSheet(graph, onDismiss = { showDiagnostics = false })
    }
}

// ── 分组卡件(task 规格:白卡 r10,行 min 50dp,发丝线内缩 16)─────────────

/** 分组卡容器(白底 r10;浅色页底 #F2F2F7、深色页底渐变末端的映衬面)。 */
@Composable
private fun GroupCard(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(if (dark) SynshuColorsDark.SelectedRow else Color.White),
    ) {
        content()
    }
}

/** 设置页底(浅色 = #F2F2F7;深色 = 渐变末端同源)。 */
@Composable
private fun settingsPageBackground(): Color =
    if (isSystemInDarkTheme()) MaterialTheme.colorScheme.background else SynshuColors.GroupCard

/**
 * 分组行(min 50dp;label 17sp + 可选副文/前置图标/尾件;onClick = null 即
 * 静态行;行内容按行变暗沿主题按压面缺省——M3 无全局按压样式,静态灰显
 * 不适用,点击行交由 clickable 缺省波纹)。
 */
@Composable
private fun RowMin(
    label: String,
    onClick: (() -> Unit)?,
    modifier: Modifier = Modifier,
    labelColor: Color = MaterialTheme.colorScheme.onSurface,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    sub: (@Composable () -> Unit)? = null,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 50.dp)
            .then(if (onClick != null) Modifier.clickable(onClickLabel = label, onClick = onClick) else Modifier)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        leading?.invoke()
        Column(Modifier.weight(1f)) {
            Text(label, color = labelColor, fontSize = 17.sp, maxLines = 1)
            sub?.invoke()
        }
        Spacer(Modifier.width(10.dp))
        trailing?.invoke()
    }
}

/** 开关行(label + 副文 + M3 Switch;色随主题)。 */
@Composable
private fun ToggleRow(
    label: String,
    sub: String,
    checked: Boolean,
    onToggle: (Boolean) -> Unit,
    toggleLabel: String? = null,
) {
    RowMin(
        label = label,
        onClick = null,
        sub = {
            Text(
                sub,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.5.sp,
            )
        },
        trailing = {
            val m = if (toggleLabel != null) {
                Modifier.semantics { contentDescription = toggleLabel }
            } else {
                Modifier
            }
            Switch(checked = checked, onCheckedChange = onToggle, modifier = m)
        },
    )
}

/** 行尾 ›(蓝 17sp;iOS placeholderRow 同形)。 */
@Composable
private fun Chevron() {
    MockGlyph(
        pathData = CHEVRON_PATH,
        size = 17.dp,
        strokeWidth = 2.2f,
        color = MaterialTheme.colorScheme.primary,
    )
}

/** 发丝分割线(0.5dp;leading 内缩 16,通栏到右缘)。 */
@Composable
private fun Hairline(leadingInset: Dp) {
    HorizontalDivider(
        modifier = Modifier.padding(start = leadingInset),
        thickness = 0.5.dp,
        color = MaterialTheme.colorScheme.outlineVariant,
    )
}

/** 信息行(左 label 灰 / 右 value)。 */
@Composable
private fun InfoLine(label: String, value: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 12.5.sp)
        Text(value, color = MaterialTheme.colorScheme.onSurface, fontSize = 12.5.sp)
    }
}

/** 行尾 › 字形(xchev;本文件私件复用)。 */
private const val CHEVRON_PATH = "M9.5 5l7 7-7 7"
/** 加号(添加信任目录)。 */
private const val PLUS_PATH = "M12 5v14M5 12h14"

/**
 * 检查更新区块(T12 状态机全形态的呈现与动作面,词面与 UpdateUiState 一一
 * 对应;卡内纵排)。
 */
@Composable
private fun UpdateSection(
    updateState: UpdateUiState,
    onCheck: () -> Unit,
    onDownload: () -> Unit,
    onRetry: () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        when (updateState) {
            is UpdateUiState.Unconfigured -> {
                Text("未配置更新源(构建期未注入更新基址/公钥)", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 12.5.sp)
            }

            is UpdateUiState.Idle -> {
                ActionText("检查更新", onCheck)
            }

            is UpdateUiState.Checking -> Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.width(16.dp).height(16.dp), strokeWidth = 2.dp)
                Spacer(Modifier.width(12.dp))
                Text("检查中…", fontSize = 14.sp)
            }

            is UpdateUiState.UpToDate -> {
                Text("已是最新版本(${updateState.current})", fontSize = 14.sp)
                ActionText("再查一次", onCheck)
            }

            is UpdateUiState.Available -> {
                Text("发现新版本 ${updateState.latest}", fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                if (updateState.notes.isNotBlank()) {
                    Text(updateState.notes, color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 12.5.sp)
                }
                ActionText("下载并安装", onDownload)
            }

            is UpdateUiState.Downloading -> {
                Text(
                    "下载中 ${updateState.latest}: ${Format.size(updateState.received, false)} / ${Format.size(updateState.total, false)}",
                    fontSize = 14.sp,
                )
                val progress = if (updateState.total > 0) {
                    (updateState.received.toFloat() / updateState.total).coerceIn(0f, 1f)
                } else {
                    0f
                }
                LinearProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
            }

            is UpdateUiState.Installing -> {
                Text("已下载 ${updateState.latest},系统安装器已拉起(装不装您定)", fontSize = 14.sp)
                ActionText("重新拉起安装器", onRetry)
            }

            is UpdateUiState.Failed -> {
                Text(updateState.error, color = MaterialTheme.colorScheme.error, fontSize = 13.sp)
                ActionText("重试", onRetry)
            }
        }
    }
}

/** 卡内动作文字钮(蓝 17sp;iOS 组内蓝字行同形)。 */
@Composable
private fun ActionText(label: String, onClick: () -> Unit) {
    Text(
        label,
        color = MaterialTheme.colorScheme.primary,
        fontSize = 17.sp,
        modifier = Modifier
            .clickable(onClickLabel = label, onClick = onClick)
            .padding(vertical = 6.dp),
    )
}

/**
 * 选择器件初始定位:主存 Download 目录的 ExternalStorageProvider 文档
 * URI(EXTRA_INITIAL_URI 尽力而为——设备不认时选择器件落缺省位,不阻断)。
 */
private fun initialDownloadTreeUri(): Uri =
    DocumentsContract.buildDocumentUri(
        "com.android.externalstorage.documents",
        "primary:Download",
    )
