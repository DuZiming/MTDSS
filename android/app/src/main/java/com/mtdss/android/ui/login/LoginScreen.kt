package com.mtdss.android.ui.login

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.profile.ProfileNavBar
import com.mtdss.android.ui.settings.SettingsKeys
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel

/**
 * 登录屏(V3-4 spec §3-7;iOS LoginView 逐词镜像 + 视觉稿 06 底注规格):
 * - **Form**:徽记 + SYNSHU 字标;邮箱、验证码+「获取验证码」、蓝色登录大
 *   CTA(50dp r25);设备名默认(主机名/设置改名)作只读注脚不占表单位
 *   (spec §4.6 设备名迁设置页);「高级选项」链接展开 服务器/CA(缺省
 *   烤入 www.mtdss.top);被移除/轮询终态以警示条呈现;
 * - **Approval**:等待受信终端确认(对端弹窗)或 6 位审批码直输,轮询
 *   后台收口;取消登录回退;
 * - **Limit**:踢选屏——设备数量已达上限,移出所选设备腾位,本机即登录;
 *   退出登录回退(移出二次确认 AlertDialog,行为零变更);
 * - Done:导航翻转(权威态在 SessionViewModel,succeeded 一次性信号)。
 * 状态面全在 [LoginViewModel](VM 测试钉契约帧;本屏纯呈现)。
 *
 * 用法双态:登出根屏(AppNav,无返回钮)/ 我的页推入(带返回钮弹回)。
 */
@Composable
fun LoginScreen(graph: Graph, evicted: Boolean, onVerified: () -> Unit, onBack: (() -> Unit)? = null) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("login", android.content.Context.MODE_PRIVATE) }
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }
    // 生效设备名 = 设置改名 > 主机名(spec §4.6;登录页注脚只读,verify 原样提交)
    val settingsState by graph.settings.state.collectAsState()
    val effectiveName = remember(settingsState.deviceNameOverride) {
        settingsState.deviceNameOverride ?: Build.MODEL
    }
    val vm = remember { LoginViewModel(graph.repo, graph.hub, scope, graph.devCaPath, deviceName = effectiveName) }
    val state by vm.state.collectAsState()

    // 预填:上次使用的服务器/邮箱(iOS AccountStore 高级选项记忆同象)
    LaunchedEffect(Unit) {
        if (state.server.isEmpty()) prefs.getString("server", null)?.let(vm::setServer)
        if (state.email.isEmpty()) prefs.getString(SettingsKeys.LOGIN_EMAIL, null)?.let(vm::setEmail)
    }
    // 取码/成功即记住输入(下次免输);成功回调触发会话对账刷新
    LaunchedEffect(state.devCode, state.loggedIn) {
        if (state.devCode != null || state.loggedIn) {
            prefs.edit()
                .putString("server", state.server)
                .putString(SettingsKeys.LOGIN_EMAIL, state.email)
                .apply()
        }
        if (state.loggedIn) onVerified()
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.surface),
    ) {
        // 顶栏 = ui/profile.ProfileNavBar(T16 台账 T11 合一:原私件
        // LoginNavBar 与其微重复,字形/规格同形,仅顶净空 8→10dp 归一)
        ProfileNavBar(title = "登录", showsBack = onBack != null, onBack = onBack)
        Column(
            modifier = Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp),
        ) {
            Spacer(Modifier.height(34.dp))
            when (state.phase) {
                LoginViewModel.LoginPhase.Form -> FormPanel(state, vm, evicted, effectiveName)
                LoginViewModel.LoginPhase.Approval -> ApprovalPanel(state, vm)
                LoginViewModel.LoginPhase.Limit -> LimitPanel(state, vm)
                LoginViewModel.LoginPhase.Done -> Unit // 收口:导航随即翻转,不渲染
            }
        }
        // 底部字标(iOS 同词面沉底)
        Text(
            "Synshu · 跨公网设备互传",
            Modifier
                .fillMaxWidth()
                .padding(bottom = 26.dp),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 13.sp,
            textAlign = TextAlign.Center,
        )
    }
}

/** Form 相位:徽记字标 + 邮箱登录表单 + 被移除/终态警示条 + 设备名注脚。 */
@Composable
private fun ColumnScope.FormPanel(
    state: LoginViewModel.UiState,
    vm: LoginViewModel,
    evicted: Boolean,
    effectiveName: String,
) {
    var showAdvanced by remember { mutableStateOf(false) }

    // 徽记 + SYNSHU 字标(spec §3-7)
    // TODO(T16 品牌字形):双弧S 徽记占位(简单 S 形;T4 无该字形,品牌
    // 资产字形随 T16 收尾统一移植)
    Box(
        Modifier
            .size(56.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(MaterialTheme.colorScheme.primary),
        contentAlignment = Alignment.Center,
    ) {
        Text("S", color = Color.White, fontSize = 30.sp, fontWeight = FontWeight.Bold)
    }
    Spacer(Modifier.height(10.dp))
    Text(
        "SYNSHU",
        color = MaterialTheme.colorScheme.onSurface,
        fontSize = 17.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 4.sp,
    )
    Spacer(Modifier.height(22.dp))

    if (evicted) NoticeBanner("本设备已被移除,请重新登录")
    state.terminal?.let { NoticeBanner(it) }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        LoginField {
            FieldText(
                value = state.email,
                onValueChange = vm::setEmail,
                placeholder = "邮箱",
                enabled = !state.busy,
                keyboardType = KeyboardType.Email,
            )
        }
        LoginField {
            FieldText(
                value = state.code,
                onValueChange = vm::setCode,
                placeholder = "验证码",
                enabled = !state.busy,
                keyboardType = KeyboardType.Number,
            )
            // 发送钮(iOS 分隔线 + 获取验证码;空邮箱禁点)
            Box(
                Modifier
                    .width(1.dp)
                    .height(20.dp)
                    .background(MaterialTheme.colorScheme.outlineVariant),
            )
            Text(
                "获取验证码",
                Modifier.clickable(
                    onClickLabel = "获取验证码",
                    enabled = !state.busy && state.email.isNotBlank(),
                ) { vm.requestCode() },
                color = if (!state.busy && state.email.isNotBlank()) {
                    MaterialTheme.colorScheme.primary
                } else {
                    SynshuColors.Disabled
                },
                fontSize = 14.5.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
            )
        }
        // dev 服务器回显验证码注记(产线 echo 关不呈现;桌面同款便利)
        state.devCode?.let { code ->
            Text(
                "开发服务器已回填验证码:$code",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.5.sp,
            )
        }
        if (showAdvanced) {
            LoginField {
                FieldText(
                    value = state.server,
                    onValueChange = vm::setServer,
                    placeholder = "服务器(默认 https://www.mtdss.top)",
                    enabled = !state.busy,
                    keyboardType = KeyboardType.Uri,
                )
            }
            LoginField {
                // CA 演练钩子由屏面 devCaPath 承担(iOS 高级选项 caPath 同位;
                // 安卓调试经 adb 放文件,无表单字段——词面保留只读占位)
                Text(
                    "CA 证书路径(可选)",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 16.sp,
                )
            }
        }
    }
    Spacer(Modifier.height(14.dp))

    PrimaryButton(title = if (state.busy) "请稍候…" else "登录", busy = state.busy, enabled = !state.busy) {
        vm.verify()
    }

    Spacer(Modifier.height(10.dp))
    // 注脚(设备名只读 + 高级选项折叠;iOS footnote 同词面)
    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
        state.error?.let {
            Text(
                it,
                color = MaterialTheme.colorScheme.error,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
            )
        }
        Text(
            "设备名将默认使用「$effectiveName」,可在 设置 中修改",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 12.5.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            if (showAdvanced) "收起高级选项" else "高级选项(服务器 / CA 证书)",
            Modifier.clickable(onClickLabel = "高级选项") { showAdvanced = !showAdvanced },
            color = MaterialTheme.colorScheme.primary,
            fontSize = 13.sp,
        )
    }
}

/** Approval 相位:等待受信终端确认(弹窗或 6 位审批码直输),轮询后台跑。 */
@Composable
private fun ColumnScope.ApprovalPanel(state: LoginViewModel.UiState, vm: LoginViewModel) {
    PanelHeadline("等待审批", "请在任一已登录的设备上确认本机登录")
    Spacer(Modifier.height(14.dp))
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        LoginField {
            FieldText(
                value = state.approvalCode,
                onValueChange = vm::setApprovalCode,
                placeholder = "审批码",
                enabled = !state.busy,
                keyboardType = KeyboardType.Number,
            )
        }
    }
    Spacer(Modifier.height(14.dp))
    PrimaryButton(title = "提交审批码", busy = state.busy, enabled = !state.busy) {
        vm.submitApprovalCode()
    }
    Spacer(Modifier.height(14.dp))
    Row(verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(modifier = Modifier.size(16.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(8.dp))
        Text("等待确认中…", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 14.sp)
    }
    state.error?.let {
        Text(it, color = MaterialTheme.colorScheme.error, fontSize = 13.sp)
    }
    Spacer(Modifier.height(8.dp))
    Row(Modifier.fillMaxWidth()) {
        Spacer(Modifier.weight(1f))
        Text(
            "取消登录",
            Modifier.clickable(onClickLabel = "取消登录", enabled = !state.busy) { vm.cancel() },
            color = MaterialTheme.colorScheme.primary,
            fontSize = 14.sp,
        )
    }
}

/** Limit 相位:踢选屏(设备数量已达上限——移出所选设备腾位,本机即登录)。
 * 移出是不可逆动作(final review I3):行按钮先入二次确认 AlertDialog,
 * 确认后才发 login.evict(V3-1 行为零变更,重皮 = A 配色行卡)。 */
@Composable
private fun ColumnScope.LimitPanel(state: LoginViewModel.UiState, vm: LoginViewModel) {
    // 待确认移出的设备(null = 无待确认行);相位离 Limit(取消/收口)时
    // 本面板整体卸载,待确认态随之消散,无需额外清场
    var pendingEvict by remember { mutableStateOf<DirectoryDevice?>(null) }

    PanelHeadline("设备数量已达上限", "请选择一台设备移出账号,腾出名额后本机即登录")
    Spacer(Modifier.height(14.dp))
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        state.evictDevices.forEach { dev ->
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(11.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .padding(start = 14.dp, top = 10.dp, bottom = 10.dp, end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text(
                        dev.name ?: dev.deviceId,
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 16.sp,
                        maxLines = 1,
                    )
                    Text(
                        listOfNotNull(dev.platform, if (dev.online) "在线" else "离线").joinToString(" · "),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 12.5.sp,
                        maxLines = 1,
                    )
                }
                Text(
                    if (state.busy) "请稍候…" else "移出",
                    Modifier
                        .clip(CircleShape)
                        .clickable(onClickLabel = "移出", enabled = !state.busy) { pendingEvict = dev }
                        .padding(horizontal = 14.dp, vertical = 7.dp),
                    color = SynshuSemantic.Fail,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }

    pendingEvict?.let { dev ->
        AlertDialog(
            onDismissRequest = { pendingEvict = null },
            title = { Text("移出「${dev.name ?: dev.deviceId}」?") },
            text = { Text("移出后该设备将立即下线且需重新登录,确认移出?") },
            confirmButton = {
                TextButton(onClick = {
                    pendingEvict = null
                    vm.evict(dev.deviceId)
                }) { Text("确认移出", color = SynshuSemantic.Fail) }
            },
            dismissButton = {
                TextButton(onClick = { pendingEvict = null }) { Text("取消") }
            },
        )
    }

    state.error?.let {
        Text(it, color = MaterialTheme.colorScheme.error, fontSize = 13.sp)
    }
    Spacer(Modifier.height(8.dp))
    Row(Modifier.fillMaxWidth()) {
        Spacer(Modifier.weight(1f))
        Text(
            "退出登录",
            Modifier.clickable(onClickLabel = "退出登录", enabled = !state.busy) { vm.cancel() },
            color = MaterialTheme.colorScheme.primary,
            fontSize = 14.sp,
        )
    }
}

// ── 共用件(spec §2.2 登录面规格;iOS LoginView 共用件同构)─────────────────

/** 相位标题 + 副题(iOS panelHeadline:标题 24sp·600 左对齐 + 副题 14sp 灰)。 */
@Composable
private fun PanelHeadline(title: String, subtitle: String) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            title,
            color = MaterialTheme.colorScheme.onSurface,
            fontSize = 24.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Text(subtitle, color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 14.sp)
    }
}

/**
 * 灰底圆角输入条(spec/视觉稿 .field:48dp 高,radius 11,#F0F0F3 底 =
 * 主题 surfaceVariant token;iOS 46pt 同象,安卓取任务规格 48dp)。
 */
@Composable
private fun LoginField(content: @Composable RowScope.() -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(48.dp)
            .clip(RoundedCornerShape(11.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        content = content,
    )
}

/** 输入条文本槽(占位词覆盖在文字起点,占满输入条余宽;iOS TextField 同构)。 */
@Composable
private fun RowScope.FieldText(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    enabled: Boolean,
    keyboardType: KeyboardType,
) {
    Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            singleLine = true,
            enabled = enabled,
            keyboardOptions = KeyboardOptions(keyboardType = keyboardType),
            textStyle = TextStyle(
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 16.sp,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        if (value.isEmpty()) {
            Text(
                placeholder,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 16.sp,
                maxLines = 1,
            )
        }
    }
}

/** 蓝色大 CTA(spec/任务规格:50dp r25,白 17sp·600;busy 时白圈+40% 底)。 */
@Composable
private fun PrimaryButton(title: String, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val shape = RoundedCornerShape(25.dp)
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .height(50.dp)
            .clip(shape)
            .background(
                if (busy || !enabled) {
                    MaterialTheme.colorScheme.primary.copy(alpha = 0.4f)
                } else {
                    MaterialTheme.colorScheme.primary
                },
                shape,
            )
            .clickable(onClickLabel = title, enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (busy) {
                CircularProgressIndicator(
                    modifier = Modifier.size(16.dp),
                    strokeWidth = 2.dp,
                    color = Color.White,
                )
            }
            Text(title, color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

/** 警示条(被移除/轮询终态;iOS banner 同 spirit:红字 + 红 10% 底 r11)。 */
@Composable
private fun NoticeBanner(text: String) {
    Text(
        text,
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(11.dp))
            .background(SynshuSemantic.Fail.copy(alpha = 0.1f))
            .padding(horizontal = 12.dp, vertical = 10.dp),
        color = SynshuSemantic.Fail,
        fontSize = 13.5.sp,
    )
}
