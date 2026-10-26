package com.mtdss.android

import android.os.Bundle
import android.os.SystemClock
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
import com.mtdss.android.ipc.DaemonHello
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 临时调试屏(T7 垂直骨架):证明 App 能拉起引擎并经 daemon 套接字走通
 * hello。T9 起以真实 UI(设备列表/任务/设置)替换,勿在此堆功能。
 *
 * 屏上两块信息:引擎状态(MtdssEngine.status:运行/退避/停止 + 重启计数)
 * 与 hello 响应(version/logged_in/device_id,来自 DaemonHello 临时客户端)。
 * 冷启动时引擎从拉起到 socket 可连之间有间隙,自动轮询直至成功(上限 15s)。
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
    data class Ok(val hello: DaemonHello.Hello) : HelloUi
    data class Failed(val message: String) : HelloUi
}

@Composable
private fun HelloDebugScreen() {
    val context = LocalContext.current
    val engineStatus by MtdssEngine.status.collectAsState()
    var helloUi by remember { mutableStateOf<HelloUi?>(null) }
    val scope = rememberCoroutineScope()

    // 冷启动自动探测:引擎拉起 → socket 可连有间隙,1s 轮询至成功或 15s 超时
    LaunchedEffect(Unit) {
        val startedAt = SystemClock.elapsedRealtime()
        while (SystemClock.elapsedRealtime() - startedAt < 15_000) {
            val result = tryHello(context)
            if (result is HelloUi.Ok) {
                helloUi = result
                return@LaunchedEffect
            }
            helloUi = result
            delay(1_000)
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
                Text("hello(临时客户端,T8 换正式 IpcClient)", style = MaterialTheme.typography.titleSmall)
                when (val h = helloUi) {
                    is HelloUi.Ok -> {
                        Text("version:${h.hello.version}")
                        Text("logged_in:${h.hello.loggedIn}")
                        Text("device_id:${h.hello.deviceId}")
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
            }
        }

        Button(onClick = { scope.launch { helloUi = tryHello(context) } }) {
            Text("重发 hello")
        }
    }
}

/** 阻塞 hello 调用切 IO 线程,结果折叠为 UI 态。 */
private suspend fun tryHello(context: android.content.Context): HelloUi = withContext(Dispatchers.IO) {
    try {
        HelloUi.Ok(DaemonHello.call(MtdssEngine.daemonSocketFile(context)))
    } catch (e: Exception) {
        HelloUi.Failed(e.message ?: e.javaClass.simpleName)
    }
}
