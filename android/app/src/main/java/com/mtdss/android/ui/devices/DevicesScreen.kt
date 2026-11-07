package com.mtdss.android.ui.devices

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.common.CenterTopBar
import com.mtdss.android.ui.common.DeviceRow
import com.mtdss.android.ui.common.LocalTopBarMenuClose
import com.mtdss.android.ui.common.deviceTappable
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * 设备页(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-3;iOS DevicesView.swift 同构):行 = 设备图标(显示器/手机)+ 名称 +
 * 「平台 · 在线/离线/本机」+ 右状态点(本机 primary/绿/灰,[DeviceRow]);
 * **本机在列不可进**(副文「平台 · 本机」即徽章);离线整行灰显禁点;点在线
 * 设备推远端浏览(onDevice,T7 接线真浏览页);「···」= 刷新(仅一项);
 * 设备行无左滑(iOS R9 #2 同裁定);尾注「N 项」沉底(陈旧注记「(缓存)」,
 * iOS ItemCountFooter 同构)。行序 = daemon created_at 定序既有,无二次重排。
 *
 * 行语言数据面(deviceSubtitle/deviceTappable)与弹层同源单份
 * (ui/common/DevicePickerSheet.kt,DevicesViewModelTest 钉)。
 */
@Composable
fun DevicesScreen(
    graph: Graph,
    onDevice: (DirectoryDevice) -> Unit,
) {
    val sessionState by graph.session.state.collectAsState()
    val selfDeviceId = sessionState.deviceId
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember { DevicesViewModel(graph.repo, graph.hub, scope) }
    val state by vm.state.collectAsState()

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        CenterTopBar(
            title = "设备",
            menuContent = {
                // 「···」= 刷新(spec §3-3:设备列表根页为「刷新」;仅一项)
                val closeMenu = LocalTopBarMenuClose.current
                DropdownMenuItem(
                    text = { Text("刷新") },
                    onClick = {
                        closeMenu()
                        vm.refresh()
                    },
                )
            },
        )
        when {
            state.loading && state.devices.isEmpty() -> Box(
                Modifier.weight(1f).fillMaxWidth(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }

            state.error != null && state.devices.isEmpty() -> ErrorFace(
                error = state.error ?: "",
                onRetry = { vm.refresh() },
                modifier = Modifier.weight(1f),
            )

            state.devices.isEmpty() -> EmptyFace(modifier = Modifier.weight(1f))

            else -> Column(Modifier.weight(1f)) {
                LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                    items(state.devices, key = { it.deviceId }) { device ->
                        DeviceRow(
                            device,
                            isSelf = device.deviceId == selfDeviceId,
                            enabled = deviceTappable(device, selfDeviceId),
                        ) {
                            if (deviceTappable(device, selfDeviceId)) onDevice(device)
                        }
                    }
                }
                // 尾注「N 项」13sp 灰沉底;daemon 回退缓存时注记「(缓存)」
                // (iOS ItemCountFooter 同构。差分:iOS 超屏随滚,此处设备
                // 量级不超屏,恒沉底视觉等价)
                Text(
                    if (state.stale) "${state.devices.size} 项(缓存)" else "${state.devices.size} 项",
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 10.dp, bottom = 14.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 13.sp,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

/** 错误面(iOS「无法加载设备」+ 透传 error + 重试,现拉即恢复)。 */
@Composable
private fun ErrorFace(error: String, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                "无法加载设备",
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                error,
                Modifier.padding(top = 6.dp, start = 24.dp, end = 24.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
            )
            TextButton(onClick = onRetry) { Text("重试") }
        }
    }
}

/** 空态(iOS「暂无其他设备」/「其他设备登录同一账号后出现在这里」逐字面)。 */
@Composable
private fun EmptyFace(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Spacer(Modifier.height(8.dp))
            Text(
                "暂无其他设备",
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                "其他设备登录同一账号后出现在这里",
                Modifier.padding(top = 6.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}
