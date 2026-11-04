package com.mtdss.android.ui.login

import android.content.Context
import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirectoryDevice
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * 登录屏(V3-1 四相位,流程镜像桌面 GuideView):Form(服务器 + 邮箱 →
 * 获取验证码(dev 回显码自动预填)→ 验证码 + 设备名(预填 Build.MODEL)→
 * 登录)→ Approval(等待审批:受信终端确认 / 6 位审批码直输,轮询收口)/
 * Limit(踢选屏:移出所选设备腾位)→ Done 导航翻转(权威态在
 * SessionViewModel)。上次 server/email 经 SharedPreferences 预填;被移除
 * (event.device_evicted)与轮询终态以警示条呈现。扫码相位随 V3-4 重构,
 * 邮箱路径完整可用。
 */
@Composable
fun LoginScreen(graph: Graph, evicted: Boolean, onVerified: () -> Unit) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("login", Context.MODE_PRIVATE) }
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    val vm = remember { LoginViewModel(graph.repo, graph.hub, scope, graph.devCaPath) }
    val state by vm.state.collectAsState()

    // 预填:上次使用的服务器/邮箱 + 本机型号为默认设备名
    LaunchedEffect(Unit) {
        if (state.server.isEmpty()) prefs.getString("server", null)?.let(vm::setServer)
        if (state.email.isEmpty()) prefs.getString("email", null)?.let(vm::setEmail)
        if (state.name.isEmpty()) vm.setName(Build.MODEL)
    }
    // 取码/成功即记住输入(下次免输);成功回调触发会话对账刷新
    LaunchedEffect(state.devCode, state.loggedIn) {
        if (state.devCode != null || state.loggedIn) {
            prefs.edit().putString("server", state.server).putString("email", state.email).apply()
        }
        if (state.loggedIn) onVerified()
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 24.dp, vertical = 32.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        when (state.phase) {
            LoginViewModel.LoginPhase.Form -> FormPanel(state, vm, evicted)
            LoginViewModel.LoginPhase.Approval -> ApprovalPanel(state, vm)
            LoginViewModel.LoginPhase.Limit -> LimitPanel(state, vm)
            LoginViewModel.LoginPhase.Done -> Unit // 收口:导航随即翻转,不渲染
        }
    }
}

/** Form 相位:邮箱登录表单 + 被移除/终态警示条。 */
@Composable
private fun ColumnScope.FormPanel(state: LoginViewModel.UiState, vm: LoginViewModel, evicted: Boolean) {
    Text("MTDSS", style = MaterialTheme.typography.headlineMedium)
    Text(
        "登录同一账号后,设备间即可互传文件。\n接收统一落在收件箱(Downloads/MTDSS)。",
        style = MaterialTheme.typography.bodyMedium,
    )

    if (evicted) NoticeBanner("本设备已被移除,请重新登录")
    state.terminal?.let { NoticeBanner(it) }

    OutlinedTextField(
        value = state.server,
        onValueChange = vm::setServer,
        label = { Text("服务器") },
        placeholder = { Text("https://mtdss.example.com") },
        singleLine = true,
        enabled = !state.busy,
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        value = state.email,
        onValueChange = vm::setEmail,
        label = { Text("邮箱") },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
        singleLine = true,
        enabled = !state.busy,
        modifier = Modifier.fillMaxWidth(),
    )
    Button(
        onClick = vm::requestCode,
        enabled = !state.busy,
        modifier = Modifier.align(Alignment.End),
    ) { Text("获取验证码") }

    OutlinedTextField(
        value = state.code,
        onValueChange = vm::setCode,
        label = { Text("验证码") },
        supportingText = state.devCode?.let { code -> { Text("开发服务器已回填验证码:$code") } },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        singleLine = true,
        enabled = !state.busy,
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        value = state.name,
        onValueChange = vm::setName,
        label = { Text("设备名") },
        singleLine = true,
        enabled = !state.busy,
        modifier = Modifier.fillMaxWidth(),
    )

    state.error?.let {
        Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
    }

    Button(onClick = vm::verify, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) {
        if (state.busy) {
            CircularProgressIndicator(
                modifier = Modifier.padding(end = 8.dp).size(16.dp),
                strokeWidth = 2.dp,
            )
        }
        Text(if (state.busy) "请稍候…" else "登录")
    }
}

/** Approval 相位:等待受信终端确认(弹窗或 6 位审批码直输),轮询后台跑。 */
@Composable
private fun ColumnScope.ApprovalPanel(state: LoginViewModel.UiState, vm: LoginViewModel) {
    Text("等待审批", style = MaterialTheme.typography.headlineMedium)
    Text("请在任一已登录的设备上确认本机登录", style = MaterialTheme.typography.bodyMedium)

    OutlinedTextField(
        value = state.approvalCode,
        onValueChange = vm::setApprovalCode,
        label = { Text("审批码") },
        placeholder = { Text("6 位数字") },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        singleLine = true,
        enabled = !state.busy,
        modifier = Modifier.fillMaxWidth(),
    )
    Button(onClick = vm::submitApprovalCode, enabled = !state.busy, modifier = Modifier.fillMaxWidth()) {
        Text("提交审批码")
    }

    Row(verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(modifier = Modifier.size(16.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(8.dp))
        Text("等待确认中…", style = MaterialTheme.typography.bodyMedium)
    }

    state.error?.let {
        Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
    }

    TextButton(onClick = vm::cancel, enabled = !state.busy, modifier = Modifier.align(Alignment.End)) {
        Text("取消登录")
    }
}

/** Limit 相位:踢选屏(设备数量已达上限——移出所选设备腾位,本机即登录)。 */
@Composable
private fun ColumnScope.LimitPanel(state: LoginViewModel.UiState, vm: LoginViewModel) {
    Text("设备数量已达上限", style = MaterialTheme.typography.headlineMedium)
    Text("请选择一台设备移出账号,腾出名额后本机即登录", style = MaterialTheme.typography.bodyMedium)

    state.evictDevices.forEach { dev ->
        Card(Modifier.fillMaxWidth()) {
            Row(
                modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 12.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text(dev.name ?: dev.deviceId, style = MaterialTheme.typography.titleMedium)
                    Text(
                        listOfNotNull(dev.platform, if (dev.online) "在线" else "离线").joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                TextButton(onClick = { vm.evict(dev.deviceId) }, enabled = !state.busy) {
                    Text("移出")
                }
            }
        }
    }

    state.error?.let {
        Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
    }

    TextButton(onClick = vm::cancel, enabled = !state.busy, modifier = Modifier.align(Alignment.End)) {
        Text("退出登录")
    }
}

/** 警示条(被移除 / 轮询终态;errorContainer 底)。 */
@Composable
private fun NoticeBanner(text: String) {
    Surface(
        color = MaterialTheme.colorScheme.errorContainer,
        shape = MaterialTheme.shapes.small,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Text(
            text,
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
        )
    }
}
