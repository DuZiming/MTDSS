package com.mtdss.android.ui.common

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
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
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.send.DevicePickerViewModel
import com.mtdss.android.ui.theme.SynshuColors
import kotlinx.coroutines.launch

/**
 * 设备行语言 + 选设备弹层(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-3 设备页 + §3-11 选设备弹层;iOS DevicesView.swift / DevicePickerSheet.swift
 * 同构:设备行组件与弹层同文件,设备页列表与弹层复用同一行)。
 *
 * 数据面纯函数(宿主可测,DevicesViewModelTest 钉):`deviceDisplayName`/
 * `deviceSubtitle`/`deviceTappable`;组件面:`DeviceRow`(设备行,58dp)、
 * `DevicePickerSheet`(半屏弹层,T7/T10 消费)。
 *
 * 视觉值(spec §2.2 + 视觉稿 06-detail-screens.html ④ .srow 逐值):行高
 * 58dp、图标槽 30dp 内蓝平台形 23dp(显示器/手机,#007AFF)、名 17sp、
 * 副文 12.5sp #8E8E93、右缘状态点 10dp(本机 = primary 蓝 = 视觉稿 ②;
 * 在线绿 #34A853;离线灰 #C7C7CC)、离线整行灰显(名/副文 #B4B4B9,图标
 * 45%)、发丝分割线行首内缩 20dp、按下行内容变暗 60%。
 */

// ── 数据面(iOS MTDSSKit DeviceIcon/DevicesRowFormat 逐字镜像,JVM 可测)────

/** 平台显示名(视觉稿「Windows · 在线」;apple 系保原生写法,未知 = 「未知」)。 */
fun deviceDisplayName(platform: String?): String = when (platform?.lowercase()) {
    "windows" -> "Windows"
    "linux" -> "Linux"
    "macos" -> "macOS"
    "ios" -> "iOS"
    "android" -> "Android"
    null -> "未知"
    else -> platform.replaceFirstChar { it.uppercase(java.util.Locale.ROOT) }
}

/**
 * 设备行副文(iOS DevicesRowFormat.subtitle 逐字镜像):本机 = 「平台 ·
 * 本机」(不可进标记即徽章,isSelf 优先于 online);对端 = 「平台 · 在线/离线」。
 */
fun deviceSubtitle(platform: String?, isSelf: Boolean, online: Boolean): String =
    if (isSelf) {
        "${deviceDisplayName(platform)} · 本机"
    } else {
        "${deviceDisplayName(platform)} · ${if (online) "在线" else "离线"}"
    }

/**
 * 设备行可进门(iOS DevicesRootView `guard device.online, !isSelf` 数据面):
 * 在线且非本机;selfDeviceId 未就绪(AppModel 投影晚到短窗)时行暂按对端样。
 */
fun deviceTappable(device: DirectoryDevice, selfDeviceId: String?): Boolean =
    device.online && device.deviceId != selfDeviceId

// ── 设备行(spec §3-3/§3-11「设备行同设备页语言」;视觉稿 .srow 逐值)────────

/** 行高 58dp / 图标槽 30dp / 状态点 10dp / 行内缩 20dp / 行内间距 12dp。 */
private val DeviceRowHeight = 58.dp
private val DeviceIconSlot = 30.dp
private val DeviceDotSize = 10.dp
private val DeviceRowInset = 20.dp
private val DeviceRowGap = 12.dp
/** 拖把手 36×5dp #D1D1D6(视觉稿 .grab;spec §2.2)。 */
private val GrabWidth = 36.dp
private val GrabHeight = 5.dp
private val GrabRadius = 2.5.dp
private val GrabColor = Color(0xFFD1D1D6)

private const val PHONE_PATH =
    "M17 1H7C5.9 1 5 1.9 5 3v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-2-2-2zm0 18H7V5h10v14z"

/** 平台形图标(spec §3-3 显示器/手机;桌面系 = 显示器,其余 = 手机)。 */
@Composable
private fun DevicePlatformIcon(platform: String?, dimmed: Boolean, modifier: Modifier = Modifier) {
    val desktop = when (platform?.lowercase()) {
        "windows", "linux", "macos" -> true
        else -> false
    }
    Box(modifier.width(DeviceIconSlot), contentAlignment = Alignment.Center) {
        MockGlyph(
            pathData = if (desktop) MONITOR_PATH else PHONE_PATH,
            size = 23.dp,
            fill = true,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.graphicsLayer { alpha = if (dimmed) 0.45f else 1f },
        )
    }
}

/**
 * 设备行(设备页列表与选设备弹层复用;iOS DevicePickerRow 参数面同构):
 * 平台图标 23dp 蓝 + 名 17sp + 副文 12.5sp + 右缘状态点 10dp(本机 primary/
 * 在线绿/离线灰);离线整行灰显(名/副文 #B4B4B9、图标 45%);发丝分割线
 * 行首内缩 20dp;按下行内容变暗 60%(enabled 行)。
 *
 * @param isSelf  本机行(设备页传;弹层恒滤己不传)——副文本机徽章 + 点 primary
 * @param enabled 点按门([deviceTappable] 产物;离线/本机行禁点)
 */
@Composable
fun DeviceRow(
    device: DirectoryDevice,
    isSelf: Boolean,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val offline = !device.online
    val name = device.name ?: device.deviceId
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = DeviceRowHeight)
            .clickable(
                interactionSource = interaction,
                indication = null,
                enabled = enabled,
                onClickLabel = name,
                onClick = onClick,
            )
            .semantics {
                contentDescription = "$name," + if (isSelf) "本机" else if (device.online) "在线" else "离线"
            },
    ) {
        Row(
            Modifier
                .fillMaxSize()
                .graphicsLayer { alpha = if (pressed) 0.6f else 1f }
                .padding(horizontal = DeviceRowInset),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(DeviceRowGap),
        ) {
            DevicePlatformIcon(device.platform, dimmed = offline)
            Column(Modifier.weight(1f)) {
                Text(
                    name,
                    color = if (offline) SynshuColors.Disabled else MaterialTheme.colorScheme.onSurface,
                    fontSize = 17.sp,
                    lineHeight = 22.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    deviceSubtitle(device.platform, isSelf, device.online),
                    color = if (offline) SynshuColors.Disabled else MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.5.sp,
                    lineHeight = 16.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            // 右缘状态点 10dp:本机 = primary(视觉稿 ②)/在线绿 #34A853/离线灰 #C7C7CC
            Box(
                Modifier
                    .size(DeviceDotSize)
                    .background(
                        when {
                            isSelf -> MaterialTheme.colorScheme.primary
                            device.online -> MaterialTheme.colorScheme.secondary
                            else -> MaterialTheme.colorScheme.outline
                        },
                        CircleShape,
                    ),
            )
        }
        // 发丝分割线:0.5dp 行首内缩 20dp(iOS FullBleedRow 同构)
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = DeviceRowInset)
                .fillMaxWidth()
                .height(0.5.dp)
                .background(MaterialTheme.colorScheme.outlineVariant),
        )
    }
}

// ── 选设备弹层(spec §3-11;iOS DevicePickerSheet 同构)──────────────────────

/**
 * 选设备弹层(T7 发送/T10 转发消费):半屏(0.65 屏高,iOS .fraction(0.65)
 * 同构)白底圆角 16 顶 + 拖把手 36×5dp + 居中标题 17sp·600(「发送 N 项到…」/
 * 「转发 N 项到…」由调用方拼);设备行同设备页语言,在线可选、离线灰显禁点;
 **滤本机**(不给发自己)+ [excluding](转发滤来源设备,daemon 拒 from==to
 * 前置拦截)。点在线设备即回调(发起由调用方承担),回调后弹层自收。
 *
 * @param graph    app 装配(repo/hub/session;devices 现拉 + event 跟线)
 * @param title    居中标题(逐字面:「发送 N 项到…」「转发 N 项到…」)
 * @param excluding 额外滤除的设备 id(转发 = 来源设备;null = 仅滤本机)
 * @param onPick   点在线设备回调
 * @param onDismissed 下滑/点外收层回调(选层取消收口用;null = 无——T7/T10
 *   既有调用形不感知取消,T15 重试选层用它清请求)
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DevicePickerSheet(
    graph: Graph,
    title: String,
    excluding: String?,
    onPick: (DirectoryDevice) -> Unit,
    onDismissed: (() -> Unit)? = null,
) {
    // 自持可见面(T7/T10 组合面即弹层;pick/下滑任一路径收口后自卸载)
    var visible by rememberSaveable { mutableStateOf(true) }
    if (!visible) return
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scope = rememberCoroutineScope()
    val vm = remember(graph) { DevicePickerViewModel(graph.repo, graph.hub, scope) }
    val state by vm.state.collectAsState()
    val sessionState by graph.session.state.collectAsState()
    val selfDeviceId = sessionState.deviceId

    val pick: (DirectoryDevice) -> Unit = { device ->
        onPick(device)
        // 先回调(调用方可即时接续发起)后收层;hide 完成再卸载(M3 惯例)
        scope.launch { sheetState.hide() }.invokeOnCompletion {
            if (!sheetState.isVisible) visible = false
        }
    }

    ModalBottomSheet(
        onDismissRequest = {
            visible = false
            onDismissed?.invoke()
        },
        sheetState = sheetState,
        dragHandle = null, // 自绘把手(视觉稿 .grab 36×5 #D1D1D6)
        shape = RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp),
        containerColor = MaterialTheme.colorScheme.surface,
        contentWindowInsets = { WindowInsets(0, 0, 0, 0) },
    ) {
        // 半屏 0.65(iOS .presentationDetents([.fraction(0.65)]) 同构;
        // R3 真机轮 #4 同因:.medium 太矮,设备多时挤压且标题贴角)
        val sheetHeight = LocalConfiguration.current.screenHeightDp.dp * 0.65f
        Column(
            Modifier
                .fillMaxWidth()
                .height(sheetHeight)
                .padding(bottom = 26.dp), // 视觉稿 .sheet 底净空
        ) {
            Box(
                Modifier
                    .align(Alignment.CenterHorizontally)
                    .padding(top = 8.dp)
                    .size(GrabWidth, GrabHeight)
                    .background(GrabColor, RoundedCornerShape(GrabRadius)),
            )
            Text(
                title,
                Modifier
                    .fillMaxWidth()
                    .padding(top = 10.dp, bottom = 10.dp),
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
                maxLines = 1,
            )
            val devices = state.devices.filter {
                it.deviceId != selfDeviceId && it.deviceId != excluding
            }
            when {
                state.loading && state.devices.isEmpty() -> {
                    Box(
                        Modifier.fillMaxSize(),
                        contentAlignment = Alignment.Center,
                    ) { CircularProgressIndicator() }
                }

                state.error != null && state.devices.isEmpty() -> {
                    SheetStatusFace(
                        head = "设备列表不可用",
                        body = state.error ?: "",
                    )
                }

                devices.isEmpty() -> {
                    SheetStatusFace(
                        head = "暂无其他设备",
                        body = "其他设备登录同一账号后出现在这里",
                    )
                }

                else -> LazyColumn(Modifier.fillMaxSize()) {
                    items(devices, key = { it.deviceId }) { device ->
                        DeviceRow(
                            device,
                            isSelf = false,
                            enabled = deviceTappable(device, selfDeviceId),
                        ) {
                            if (device.online) pick(device)
                        }
                    }
                }
            }
        }
    }
}

/** 弹层状态面(加载外:错误「设备列表不可用」/空「暂无其他设备」,iOS 同词面)。 */
@Composable
private fun SheetStatusFace(head: String, body: String) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(head, color = MaterialTheme.colorScheme.onSurface, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            Text(
                body,
                Modifier.padding(top = 6.dp, start = 24.dp, end = 24.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
            )
        }
    }
}
