package com.mtdss.android.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.browse.BrowseScreen
import com.mtdss.android.ui.devices.DevicesScreen
import com.mtdss.android.ui.local.LocalFilesScreen
import com.mtdss.android.ui.login.LoginScreen

/**
 * 应用导航(spec §4.3):登录 → 设备列表 → 按设备浏览;本机页(收件箱 +
 * 授权根,T10)为设备列表层的并列页。会话状态(SessionViewModel 的权威
 * status)是顶层路由依据:
 * - loggedIn == null:引擎冷启动「拉起 → socket 可连」间隙的 splash
 *   (IpcClient 自愈重连,秒级收敛);
 * - loggedIn == false:登录屏(登出同样回到这里);
 * - loggedIn == true:设备列表/浏览/本机(浏览位 rememberSaveable 存续)。
 *
 * 断连重连(引擎重启)后历史浏览位不再可信(daemon 可能已换实例,桌面
 * 同款口径):重连即弹回设备列表。系统返回键在浏览页 = 回设备列表。
 */
@Composable
fun MtdssApp(graph: Graph) {
    val sessionState by graph.session.state.collectAsState()

    // 浏览位(saveable:旋转/进程恢复);null = 设备列表
    var browseDeviceId by rememberSaveable { mutableStateOf<String?>(null) }
    var browseDeviceName by rememberSaveable { mutableStateOf<String?>(null) }

    // 本机页(收件箱 + 授权根);false = 设备列表层
    var localOpen by rememberSaveable { mutableStateOf(false) }

    // 重连检测:曾连上又断开 → 弹回设备列表(历史浏览位不可信)
    var wasConnected by remember { mutableStateOf(false) }
    LaunchedEffect(sessionState.conn) {
        if (sessionState.conn == ConnState.Connected) {
            if (wasConnected) browseDeviceId = null
            wasConnected = true
        }
    }
    // 登出:清浏览位(重登后回设备列表,不复活旧浏览位)
    LaunchedEffect(sessionState.loggedIn) {
        if (sessionState.loggedIn == false) {
            browseDeviceId = null
            localOpen = false
            wasConnected = false
        }
    }

    Surface(
        modifier = Modifier.fillMaxSize(),
        color = MaterialTheme.colorScheme.background,
    ) {
        when (sessionState.loggedIn) {
            null -> ConnectingSplash(sessionState.conn)

            false -> LoginScreen(graph, onVerified = { graph.session.refresh() })

            else -> when {
                localOpen -> LocalFilesScreen(graph, onBack = { localOpen = false })

                else -> {
                    val deviceId = browseDeviceId
                    if (deviceId == null) {
                        DevicesScreen(
                            graph,
                            onDevice = { dev: DirectoryDevice ->
                                browseDeviceId = dev.deviceId
                                browseDeviceName = dev.name
                            },
                            onLocalFiles = { localOpen = true },
                        )
                    } else {
                        val device = remember(deviceId) {
                            DirectoryDevice(
                                deviceId = deviceId,
                                name = browseDeviceName,
                                platform = null,
                                online = true,
                            )
                        }
                        BrowseScreen(graph, device, onBack = { browseDeviceId = null })
                    }
                }
            }
        }
    }
}

@Composable
private fun ConnectingSplash(conn: ConnState) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            if (conn == ConnState.Connected) {
                // 已连接但 status 未归:毫秒级瞬态,同 splash 呈现
                CircularProgressIndicator()
                Text("读取登录态…", Modifier.padding(top = 12.dp))
            } else {
                CircularProgressIndicator()
                Text("连接引擎中…", Modifier.padding(top = 12.dp))
            }
        }
    }
}
