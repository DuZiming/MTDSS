package com.mtdss.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.rememberCoroutineScope
import com.mtdss.android.engine.MtdssEngine
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.HelloResult
import com.mtdss.android.ipc.IpcClient
import kotlinx.coroutines.launch

/**
 * 临时调试屏(T7 垂直骨架,T8 换正式 IpcClient):证明 App 能拉起引擎并经
 * daemon 套接字走通 hello。T9 起以真实 UI(设备列表/任务/设置)替换,勿在此堆功能。
 *
 * 屏上三块信息:引擎状态(MtdssEngine.status)、IPC 连接态(IpcClient 自愈
 * 重连的可见面)与 hello 响应(version/logged_in/device_id)。冷启动时引擎从
 * 拉起到 socket 可连之间有间隙——IpcClient 首连失败后转入后台自愈重连,
 * 无需屏面轮询;连接态每次翻到 Connected 即自动重发 hello(引擎重启自愈后
 * 同样适用)。
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    HelloDebugScreen()
                }
            }
        }
    }
}

/** hello 结果或错误文本(成功/失败共用一个展示位)。 */
private sealed interface HelloUi {
    data class Ok(val hello: HelloResult) : HelloUi
    data class Failed(val message: String) : HelloUi
}

@Composable
private fun HelloDebugScreen() {
    val context = LocalContext.current
    val engineStatus by MtdssEngine.status.collectAsState()
    var helloUi by remember { mutableStateOf<HelloUi?>(null) }
    var connState by remember { mutableStateOf<ConnState>(ConnState.Disconnected) }
    val scope = rememberCoroutineScope()
    val client = remember { IpcClient(MtdssEngine.daemonSocketFile(context), scope) }

    // 屏面销毁即关停客户端(划走/退出);旋转重建则换新实例,旧实例随之关闭
    DisposableEffect(client) {
        onDispose { client.close() }
    }

    LaunchedEffect(client) {
        client.connect() // 首连快速失败亦无妨:客户端后台自愈,连接态流转驱动后续
        client.connectionState.collect { st ->
            connState = st
            if (st == ConnState.Connected) helloUi = tryHello(client)
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("MTDSS 调试屏(T7 临时)", style = MaterialTheme.typography.titleLarge)

        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("引擎状态", style = MaterialTheme.typography.titleSmall)
                Text(
                    when (engineStatus.phase) {
                        MtdssEngine.Phase.RUNNING -> "运行中"
                        MtdssEngine.Phase.BACKOFF -> "退避等待(意外退出后重启)"
                        MtdssEngine.Phase.STOPPED -> "已停止"
                    },
                )
                Text("累计重启:${engineStatus.restartCount}")
                engineStatus.lastExit?.let { Text("上次退出:$it") }
            }
        }

        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("IPC(hello / IpcClient)", style = MaterialTheme.typography.titleSmall)
                Text(
                    when (connState) {
                        ConnState.Connected -> "连接:已连接"
                        ConnState.Connecting -> "连接:连接中"
                        ConnState.Disconnected -> "连接:已断开(自愈重连中)"
                    },
                )
                when (val h = helloUi) {
                    is HelloUi.Ok -> {
                        Text("version:${h.hello.version}")
                        Text("logged_in:${h.hello.loggedIn}")
                        Text("device_id:${h.hello.device?.deviceId ?: "-"}")
                    }
                    is HelloUi.Failed -> Text("失败:${h.message}", color = MaterialTheme.colorScheme.error)
                    null -> Text("探测中…")
                }
            }
        }

        Card(
            modifier = Modifier.fillMaxWidth(),
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
        ) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("验证要点", style = MaterialTheme.typography.titleSmall)
                Text("App 退出(划走)→ 引擎随亡(PDEATHSIG);logcat 过滤 mtdssd / MtdssEngine")
                Text("引擎重启 → IPC 自愈重连,连接态与 hello 自动恢复")
            }
        }

        Button(onClick = { scope.launch { helloUi = tryHello(client) } }) {
            Text("重发 hello")
        }
    }
}

/** hello 调用(客户端自动按需连接),结果折叠为 UI 态。 */
private suspend fun tryHello(client: IpcClient): HelloUi =
    client.hello().fold(
        onSuccess = { HelloUi.Ok(it) },
        onFailure = { HelloUi.Failed(it.message ?: it.javaClass.simpleName) },
    )
