package com.mtdss.android.ui.login

import android.content.Context
import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * 登录屏(spec §4.3;流程镜像桌面 GuideView):服务器 + 邮箱 → 获取验证码
 * (dev 服务器回显码自动预填)→ 验证码 + 设备名(预填 Build.MODEL)→ 登录。
 * 上次 server/email 经 SharedPreferences 预填(手机上重输 URL 痛苦)。
 * 验证成功的导航翻转以会话 status 为权威(本屏回调触发对账刷新)。
 */
@Composable
fun LoginScreen(graph: Graph, onVerified: () -> Unit) {
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
        Text("MTDSS", style = MaterialTheme.typography.headlineMedium)
        Text(
            "登录同一账号后,设备间即可互传文件。\n接收统一落在收件箱(Downloads/MTDSS)。",
            style = MaterialTheme.typography.bodyMedium,
        )

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
}
