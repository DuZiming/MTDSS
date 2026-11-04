package com.mtdss.android.ui.login

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.MtdssEvent
import kotlinx.coroutines.launch

/**
 * 受信终端审批弹窗(V3-1,镜像桌面 ApprovalModal;AppNav 在已登录分支挂载):
 * event.approval 到达即弹——候选设备名/平台 + 6 位审批码大字 + 同意/拒绝。
 * 下滑关闭 = **不应答**(不替用户表态,悬置会话由服务端 TTL 收口,桌面同款
 * 语义);应答经 approval.respond,在途两键置灰防连点,失败弹层内呈错误。
 * 审批只在前台生效(无推送):事件随渲染到达,弹层随即出现。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ApprovalSheetHost(graph: Graph) {
    val sessionState by graph.session.state.collectAsState()
    val scope = rememberCoroutineScope()
    var req by remember { mutableStateOf<MtdssEvent.Approval?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    // 事件收集(仅已登录期间:approval.respond 须已登录会话;登出清挂起弹层)
    LaunchedEffect(sessionState.loggedIn) {
        if (sessionState.loggedIn != true) {
            req = null
            return@LaunchedEffect
        }
        graph.hub.approval.collect {
            req = it
            error = null
        }
    }

    fun respond(allow: Boolean) {
        val r = req ?: return
        busy = true
        error = null
        scope.launch {
            graph.repo.respondApproval(r.sessionId, allow).fold(
                onSuccess = { req = null },
                onFailure = { e -> error = "应答失败:${e.message}" },
            )
            busy = false
        }
    }

    val r = req ?: return
    ModalBottomSheet(onDismissRequest = { req = null }) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp, vertical = 8.dp)
                .padding(bottom = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("新终端登录请求", style = MaterialTheme.typography.titleMedium)
            Text(
                "「${r.candidateName}」请求登录你的账号",
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
            )
            r.candidatePlatform?.let {
                Text("平台:$it", style = MaterialTheme.typography.bodySmall)
            }
            Text(
                "审批码",
                style = MaterialTheme.typography.labelMedium,
                modifier = Modifier.padding(top = 8.dp),
            )
            Text(r.code, style = MaterialTheme.typography.displaySmall)
            error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
            Row(modifier = Modifier.padding(top = 8.dp)) {
                TextButton(onClick = { respond(false) }, enabled = !busy) {
                    Text(if (busy) "请稍候…" else "拒绝")
                }
                Button(onClick = { respond(true) }, enabled = !busy, modifier = Modifier.padding(start = 16.dp)) {
                    Text("同意")
                }
            }
        }
    }
}
