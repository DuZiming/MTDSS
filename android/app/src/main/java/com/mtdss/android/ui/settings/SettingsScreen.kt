package com.mtdss.android.ui.settings

import android.Manifest
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import android.provider.Settings
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.serve.photoPermissionGranted
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.ui.common.Hairline
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.common.SynshuGlyphs
import com.mtdss.android.ui.profile.ProfileNavBar
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuColorsDark
import com.mtdss.android.ui.theme.SynshuSemantic

/**
 * 设置页(V3-4 spec §3-8 分组卡;iOS SettingsView.swift 词面逐字 + 视觉稿
 * 06 底注规格:白卡圆角 10,#F2F2F7 底组间距,行 min 58dp,组内发丝线内缩
 * 16;**R3 H4 用户钦定瘦身——只保留五项**:设备名 / 显示隐藏项 /
 * 允许删除远端文件 / 照片图库 / 信任目录(含收件箱默认置顶行)):
 * - **我的设备名**(点入改;alert「修改后将在下次登录时对其他设备生效」;
 *   spec §4.6 设备名迁设置,登录页只读注脚)+ 组脚注「设备名修改将在下次
 *   登录时对其他设备生效」;
 * - **显示隐藏项**(默认关)/ **允许删除远端文件**(默认关)= 双门开关
 *   (spec §4.5/§4.2;持久在 [SettingsPrefs],消费面经 [SettingsViewModel.gates]
 *   单源流转 T13 各屏即时生效);
 * - **照片图库**(默认关,V3-4 §4.3 T14):其他设备可浏览/下载相册照片;
 *   开 = 运行时权限请求(tier 档),授予 = 开;拒绝 = 回关 + 指路系统设置
 *   (ScanSheet 相机流同款 spirit)——开关只持请求入口,应答面以实时权限
 *   为准(权限即数据面;授予后对端 ≤30s 内见根,引擎桥级负缓存 TTL);
 * - **信任目录**(其他设备可浏览):置顶「收件箱(默认)」恒在条目(不可
 *   移除,R1 F6b 呈现面——roots.set 恒含收件箱,授权集语义零改)+ 授权根
 *   列表(移除 = 居中确认)+ 添加(系统目录选择器即授权 = 既有 RootsStore
 *   账本;roots.set 引擎同步经 RootsStore.onChange → RootsSync 自动推送)。
 *
 * R3 H4 移除面(整组撤,**功能面代码保留不删**——入口消失):
 * - 检查更新组 + 关于组(当前版本/版本/引擎/诊断行):App 内更新入口按
 *   用户裁定移除,更新走 /dl 装包;[UpdateController] 状态机与测试面原样
 *   保留(未挂消费面);版本号沉底「我的」页(ProfileScreen 版本沉底在位);
 *   诊断面 [DiagnosticsSheet] 件保留(common),本页入口撤;
 * - 收件箱组(位置/目录授权/重新授权):SAF 授权失效时的恢复路径 = 留票
 *   (授权数据面 [InboxGrantStore]/ServeResponder 原样保留;重授权入口
 *   随组撤,首次收件失败引导词面指向恢复票)。
 */
@Composable
fun SettingsScreen(graph: Graph, onBack: () -> Unit) {
    val vm = graph.settings
    val state by vm.state.collectAsState()
    val trustedRoots by vm.trustedRoots.collectAsState()
    var showRename by rememberSaveable { mutableStateOf(false) }
    var renameText by rememberSaveable { mutableStateOf("") }
    var pendingRemove by remember { mutableStateOf<StoredRoot?>(null) }

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

    // 收件目录授权(OPEN_DOCUMENT_TREE)入口随 R3 H4 收件箱组撤——恢复路径
    // 留票(SAF 授权失效时无 App 内重授权入口;授权数据面 InboxGrantStore/
    // ServeResponder 原样保留,见类注)。

    // 照片图库共享(V3-4 §4.3 T14;spec「运行时权限请求时机 = 设置页开启
    // 照片共享」):开关只持请求入口——开 = 拉运行时权限(tier 档权限集),
    // 授予 = 开关开;拒绝 = 开关收口回关 + 指路系统设置(ScanSheet 相机流
    // 同款词面 spirit)。应答面(PhotoResponder)以**实时权限**为准:此处
    // 授予后对端 ≤30s 内见 /照片图库(引擎桥级负缓存 TTL,冒烟接受项);
    // 系统设置撤销后开关态不动、根自动隐藏(权限即数据面,fail-closed)。
    val context = LocalContext.current
    var showPhotoDenied by rememberSaveable { mutableStateOf(false) }
    val photoPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions(),
    ) { grants ->
        if (grants.values.all { it }) {
            vm.setPhotoShare(true)
        } else {
            vm.setPhotoShare(false)
            showPhotoDenied = true
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
                Hairline(leadingInset = 16.dp)
                ToggleRow(
                    label = "照片图库",
                    sub = "开启后,其他设备可浏览并下载相册照片",
                    checked = state.photoShare,
                    onToggle = { on ->
                        if (on) {
                            // 开 = 运行时权限请求(已授权直开;拒绝收口回关 + 指路)
                            if (photoPermissionGranted(context)) {
                                vm.setPhotoShare(true)
                            } else {
                                photoPermissionLauncher.launch(photoSharePermissions())
                            }
                        } else {
                            // 关 = 收请求入口(不撤销系统授权;应答面以实时权限为准)
                            vm.setPhotoShare(false)
                        }
                    },
                    toggleLabel = "photo-share-toggle",
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
                // 收件箱置顶恒在条目(R1 F6b;iOS 对齐:授权集恒含收件箱且
                // 用户可见)。**呈现面 only**——RootsStore/roots.set 语义零改
                // (ServeResponder rootsSpecs 恒含 INBOX_VPATH);不可移除,
                // 无「移除」钮;授权副文随收件目录授权态(与「收件箱」组同源)。
                RowMin(
                    label = "收件箱(默认)",
                    onClick = null,
                    leading = {
                        MockGlyph(SynshuGlyphs.FOLDER, 23.dp, MaterialTheme.colorScheme.primary, fill = true)
                        Spacer(Modifier.width(13.dp))
                    },
                    sub = {
                        Text(
                            if (state.inboxAuthorized) "已授权" else "未授权",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = 12.5.sp,
                        )
                    },
                )
                if (trustedRoots.isNotEmpty()) {
                    Hairline(leadingInset = 16.dp)
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
                }
                Hairline(leadingInset = 16.dp)
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

            // R3 H4 瘦身:收件箱组(位置/目录授权/重新授权)/ 检查更新组 /
            // 关于组(版本/引擎/诊断行/标语)整组撤——只留五项(用户钦定);
            // 功能面代码保留不删,更新走 /dl 装包,版本沉底「我的」页(类注)。
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

    // 照片权限拒指引(ScanSheet 相机流同款 spirit:词面 + 指路系统设置;
    // 确认 = 前往设置·本应用详情页,取消 = 留在设置页开关已收口回关)
    if (showPhotoDenied) {
        AlertDialog(
            onDismissRequest = { showPhotoDenied = false },
            title = { Text("未获得照片权限") },
            text = { Text("照片图库需要访问照片。\n请在系统设置中允许 Synshu 访问照片后重试。") },
            confirmButton = {
                TextButton(onClick = {
                    showPhotoDenied = false
                    openAppSettings(context)
                }) { Text("前往设置") }
            },
            dismissButton = {
                TextButton(onClick = { showPhotoDenied = false }) { Text("取消") }
            },
        )
    }
}

/**
 * 照片共享运行时权限集(§4.3 权限矩阵;minSdk 29 无 ≤28 面):
 * API 33+ = READ_MEDIA_IMAGES + READ_MEDIA_VIDEO 双权;API 29-32 =
 * READ_EXTERNAL_STORAGE。33+ 上请求 READ_EXTERNAL_STORAGE 为无操作,
 * 不混发。
 */
private fun photoSharePermissions(): Array<String> =
    if (Build.VERSION.SDK_INT >= 33) {
        arrayOf(
            Manifest.permission.READ_MEDIA_IMAGES,
            Manifest.permission.READ_MEDIA_VIDEO,
        )
    } else {
        arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)
    }

/** 系统设置·本应用详情页(§4.3 指路设置;ScanSheet 相机流同构)。 */
private fun openAppSettings(context: Context) {
    context.startActivity(
        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
            .setData(Uri.fromParts("package", context.packageName, null)),
    )
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
 * 分组行(min 58dp = iOS 设置行 FullBleedRow 默认行高同值,G2 度量——原
 * 50dp 偏矮显「不协调」;label 17sp + 可选副文/前置图标/尾件;onClick =
 * null 即静态行;行内容按行变暗沿主题按压面缺省——M3 无全局按压样式,
 * 静态灰显不适用,点击行交由 clickable 缺省波纹)。
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
            .heightIn(min = 58.dp)
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

// 发丝分割线 = ui/common.Hairline(T16 台账 T11 合一,原本文件私件已撤)

/** 行尾 › 字形(xchev;本文件私件复用)。 */
private const val CHEVRON_PATH = "M9.5 5l7 7-7 7"
/** 加号(添加信任目录)。 */
private const val PLUS_PATH = "M12 5v14M5 12h14"

// 检查更新区块呈现(UpdateSection/ActionText)随 R3 H4 检查更新组撤——
// UpdateController 状态机与测试面保留(update/ 包,未挂消费面;更新走
// /dl 装包,见类注)。

/**
 * 选择器件初始定位:主存 Download 目录的 ExternalStorageProvider 文档
 * URI(EXTRA_INITIAL_URI 尽力而为——设备不认时选择器件落缺省位,不阻断)。
 */
private fun initialDownloadTreeUri(): Uri =
    DocumentsContract.buildDocumentUri(
        "com.android.externalstorage.documents",
        "primary:Download",
    )
