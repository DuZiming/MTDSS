package com.mtdss.android.ui.profile

import android.content.Context
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.mtdss.android.BuildConfig
import com.mtdss.android.Graph
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.login.LoginScreen
import com.mtdss.android.ui.settings.SettingsKeys
import com.mtdss.android.ui.settings.SettingsScreen
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.launch
import androidx.compose.foundation.layout.Arrangement

/**
 * 我的 tab(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-6;iOS ProfileView.swift 逐项镜像):
 * - 左上扫码(V3-4 T12 点亮 = 全屏深色取景 sheet,扫得本家码 → qr.claim →
 *   候选确认弹层 → 裁决结果弹层,iOS QrApprovalModel 闭环同构;未登录/
 *   相机权限拒 = 灰显)+ 右上齿轮(进设置);
 * - 账号行 = 字母头像(邮箱首字符大写,蓝底白字圆)+ 邮箱(或生效设备名;
 *   引擎无 email 面,登录屏记账为准)+ 「设备名 · 在线/离线」;未登录 =
 *   「登录」按钮(推入登录页);
 * - 联系我们 / 为我打分 / 我的权益 三个占位行(› 进占位页「敬请期待」);
 * - 退出登录红字行(居中确认 alert「退出当前账号?」后 logout;会话清理链
 *   在 SessionViewModel,快捷目录缓存清场由 FavoritesWatcher 登出钩子承担);
 * - 版本号沉底「Synshu x.y.z」。
 *
 * 栈内推入(设置/登录/占位页)= tab 内路由(saveable,旋转/进程恢复不丢;
 * 系统返回弹回,iOS NavigationStack 同构)。
 */
@Composable
fun ProfileScreen(graph: Graph) {
    // 推入路由(枚举名落档;旋转/进程恢复不丢推入位,FavoritesScreen 同形态)
    var routeName by rememberSaveable { mutableStateOf<String?>(null) }
    val route = routeName?.let { n -> ProfileRoute.entries.firstOrNull { it.name == n } }

    BackHandler(enabled = route != null) { routeName = null }

    when (route) {
        null -> ProfileRoot(graph, onPush = { routeName = it.name })
        ProfileRoute.SETTINGS -> SettingsScreen(graph, onBack = { routeName = null })
        ProfileRoute.LOGIN -> {
            val sessionState by graph.session.state.collectAsState()
            LoginScreen(
                graph,
                evicted = sessionState.evicted,
                onVerified = {
                    graph.session.refresh()
                    routeName = null
                },
                onBack = { routeName = null },
            )
        }
        ProfileRoute.CONTACT -> PlaceholderPage("联系我们", ENVELOPE_PATH)
        ProfileRoute.RATING -> PlaceholderPage("为我打分", STAR_PATH)
        ProfileRoute.BENEFITS -> PlaceholderPage("我的权益", GIFT_PATH)
    }
}

/** 我的栈内路由(iOS ProfileRoute 同集)。 */
private enum class ProfileRoute { SETTINGS, LOGIN, CONTACT, RATING, BENEFITS }

/** 我的根页:顶栏(扫码/标题/齿轮)→ 账号区 + 占位行 + 退出行 → 版本沉底。 */
@Composable
private fun ProfileRoot(graph: Graph, onPush: (ProfileRoute) -> Unit) {
    val context = LocalContext.current
    val sessionState by graph.session.state.collectAsState()
    val scope = rememberCoroutineScope()
    var showLogoutConfirm by remember { mutableStateOf(false) }
    // 扫码授权闭环(T12;iOS qrModel/qrVote 同形):扫得 token 即建模型走
    // claim→确认→approve;qrVote = 在途裁决(同/拒即置值,确认层随即摘下,
    // 迟到双击不再触达);nil(此处 null)= 无在途授权
    var showScan by remember { mutableStateOf(false) }
    var qrModel by remember { mutableStateOf<QrApprovalModel?>(null) }
    var qrVote by remember { mutableStateOf<Boolean?>(null) }

    fun startClaim(token: String) {
        val m = QrApprovalModel(graph.repo)
        qrModel = m
        qrVote = null
        scope.launch { m.claim(token) }
    }

    // 账号行 email = 登录屏记账(登录成功落 SharedPreferences;引擎无 email
    // 面,iOS AccountStore.email 同口径);缺席退化生效设备名
    val email = remember { readLoginEmail(context) }
    // 生效设备名 = 设置改名 > 登录实名 > 主机名(spec §4.6 链;iOS
    // effectiveDeviceName 同构)
    val effectiveName = graph.settings.state.value.deviceNameOverride
        ?: sessionState.deviceName
        ?: Build.MODEL
    val loggedIn = sessionState.loggedIn == true
    val title = email ?: effectiveName

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        // 顶栏(iOS navRow:左扫码 + 标题绝对居中 + 右齿轮)
        Box(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 18.dp)
                .padding(top = 10.dp, bottom = 8.dp)
                .height(34.dp),
        ) {
            Text(
                "我的",
                Modifier.align(Alignment.Center),
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
            )
            // 扫码入口(V3-4 T12 点亮:全屏取景 sheet → qr.claim 闭环;未登录
            // /相机权限拒 = 灰显,spec §4.9;ON_RESUME 刷新权限态)
            val cameraGranted = rememberCameraGranted()
            val scanEnabled = loggedIn && cameraGranted
            Box(
                Modifier
                    .align(Alignment.CenterStart)
                    .size(34.dp)
                    .clickable(onClickLabel = "扫码登录", enabled = loggedIn) { showScan = true },
                contentAlignment = Alignment.Center,
            ) {
                MockGlyph(QR_PATH, 20.dp, if (scanEnabled) MaterialTheme.colorScheme.primary else SynshuColors.Disabled, strokeWidth = 1.8f)
                MockGlyph(QR_DOTS_PATH, 20.dp, if (scanEnabled) MaterialTheme.colorScheme.primary else SynshuColors.Disabled, fill = true)
            }
            // 齿轮(进设置)
            Box(
                Modifier
                    .align(Alignment.CenterEnd)
                    .size(34.dp)
                    .clickable(onClickLabel = "设置") { onPush(ProfileRoute.SETTINGS) },
                contentAlignment = Alignment.Center,
            ) {
                MockGlyph(GEAR_PATH, 20.dp, MaterialTheme.colorScheme.primary, fill = true)
            }
        }

        Column(
            Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState()),
        ) {
            if (loggedIn) {
                // 账号行(iOS FullBleedRow height 74:头像 52 圆 + 标题/副文)
                Row(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 74.dp)
                        .padding(horizontal = 20.dp, vertical = 11.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier
                            .size(52.dp)
                            .background(MaterialTheme.colorScheme.primary, CircleShape),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            title.firstOrNull()?.uppercase() ?: "?",
                            color = Color.White,
                            fontSize = 23.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                    Spacer(Modifier.width(13.dp))
                    Column {
                        Text(
                            title,
                            color = MaterialTheme.colorScheme.onSurface,
                            fontSize = 18.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            "$effectiveName · ${if (sessionState.selfOnline == true) "在线" else "离线"}",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = 12.5.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                Hairline(leadingInset = 20.dp)
            } else {
                // 未登录 = 「登录」按钮(iOS 同词面;推入登录页)
                Box(
                    Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 20.dp, vertical = 14.dp),
                ) {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .height(50.dp)
                            .clip(RoundedCornerShape(14.dp))
                            .background(MaterialTheme.colorScheme.primary)
                            .clickable(onClickLabel = "登录") { onPush(ProfileRoute.LOGIN) },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            "登录",
                            color = Color.White,
                            fontSize = 17.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                }
            }

            // 占位行(› 进占位页;iOS placeholderRows 同集同词面)
            PlaceholderRow(ENVELOPE_PATH, "联系我们") { onPush(ProfileRoute.CONTACT) }
            Hairline(leadingInset = 20.dp)
            PlaceholderRow(STAR_PATH, "为我打分") { onPush(ProfileRoute.RATING) }
            Hairline(leadingInset = 20.dp)
            PlaceholderRow(GIFT_PATH, "我的权益") { onPush(ProfileRoute.BENEFITS) }

            if (loggedIn) {
                // 退出登录红字行(iOS padding top 22;居中确认 alert)
                Spacer(Modifier.height(22.dp))
                Box(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 44.dp)
                        .clickable(onClickLabel = "退出登录") { showLogoutConfirm = true },
                    contentAlignment = Alignment.Center,
                ) {
                    Text("退出登录", color = SynshuSemantic.Fail, fontSize = 17.sp)
                }
            }
        }

        // 版本号沉底(iOS SinkingFooterList footer 同词面)
        Text(
            "Synshu ${BuildConfig.VERSION_NAME}",
            Modifier
                .fillMaxWidth()
                .padding(top = 10.dp, bottom = 14.dp),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 13.sp,
            textAlign = TextAlign.Center,
        )
    }

    // 退出确认(居中 alert;iOS「退出当前账号?」+ 退出登录红/取消)
    if (showLogoutConfirm) {
        AlertDialog(
            onDismissRequest = { showLogoutConfirm = false },
            title = { Text("退出当前账号?") },
            confirmButton = {
                TextButton(onClick = {
                    showLogoutConfirm = false
                    scope.launch {
                        graph.repo.logout()
                        // 权威对账(event.login 兜底);loggedIn 翻 false 即
                        // AppNav 换登录屏,快捷目录缓存清场由 FavoritesWatcher
                        // 登出钩子承担
                        graph.session.refresh()
                    }
                }) { Text("退出登录", color = SynshuSemantic.Fail) }
            },
            dismissButton = {
                TextButton(onClick = { showLogoutConfirm = false }) { Text("取消") }
            },
        )
    }

    // 扫码取景 sheet(T12;全屏 Dialog = iOS sheet 覆全屏同象,胶囊底栏随之
    // 遮蔽,spec §3-1「扫码 sheet 期间藏栏」)
    if (showScan) {
        Dialog(
            onDismissRequest = { showScan = false },
            properties = DialogProperties(
                usePlatformDefaultWidth = false,
                decorFitsSystemWindows = false,
            ),
        ) {
            ScanSheet(
                onClose = { showScan = false },
                onCode = { token ->
                    showScan = false
                    startClaim(token)
                },
            )
        }
    }

    // 扫码授权闭环弹层(T12,iOS ProfileView alert 同构;词面逐字)
    qrModel?.let { m ->
        val stage by m.stage.collectAsState()
        // 候选确认层:candidate_name/platform + 同意/拒绝——同意屏是钓鱼防线
        // (用户目视核对设备名与平台);qrVote 摘层防迟到裁决与重弹
        val confirming = stage as? QrApprovalModel.Stage.Confirming
        if (confirming != null && qrVote == null) {
            val c = confirming.candidate
            AlertDialog(
                onDismissRequest = { /* iOS set:{} 同象:关闭必经按钮动作 */ },
                title = { Text("扫码登录") },
                text = {
                    Text(
                        "${c.candidateName}(${c.candidatePlatform}) 请求扫码登录本账号。" +
                            "\n\n请核对设备名与平台后再决定,同意后该设备将登录你的 Synshu 账号。",
                    )
                },
                confirmButton = {
                    TextButton(onClick = {
                        qrVote = true
                        scope.launch { m.approve(allow = true) }
                    }) { Text("同意登录") }
                },
                dismissButton = {
                    TextButton(onClick = { m.dismiss(); qrModel = null; qrVote = null }) { Text("取消") }
                    TextButton(onClick = {
                        qrVote = false
                        scope.launch { m.approve(allow = false) }
                    }) { Text("拒绝", color = SynshuSemantic.Fail) }
                },
            )
        }
        // 结果行层(approve settle/claim 失败共用;好即清场)
        val resultText = when (val s = stage) {
            is QrApprovalModel.Stage.Result -> s.line
            is QrApprovalModel.Stage.Failed -> s.text
            else -> null
        }
        if (resultText != null) {
            AlertDialog(
                onDismissRequest = { qrModel = null; qrVote = null },
                title = { Text("扫码登录") },
                text = { Text(resultText) },
                confirmButton = {
                    TextButton(onClick = { qrModel = null; qrVote = null }) { Text("好") }
                },
            )
        }
    }
}

/** 占位行(iOS placeholderRow:图标 20dp 灰 26 槽 + 标题 17sp + › 蓝)。 */
@Composable
private fun PlaceholderRow(glyphPath: String, title: String, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 50.dp)
            .clickable(onClickLabel = title, onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(26.dp), contentAlignment = Alignment.Center) {
            MockGlyph(glyphPath, 20.dp, MaterialTheme.colorScheme.onSurfaceVariant, strokeWidth = 1.8f)
        }
        Spacer(Modifier.width(13.dp))
        Text(title, color = MaterialTheme.colorScheme.onSurface, fontSize = 17.sp)
        Spacer(Modifier.weight(1f))
        MockGlyph(
            pathData = CHEVRON_PATH,
            size = 17.dp,
            strokeWidth = 2.2f,
            color = MaterialTheme.colorScheme.primary,
        )
    }
}

/**
 * 我的栈顶栏(推入页共用件:返回 + 标题绝对居中;iOS ProfileNavBar 像素
 * 参照同构。设置/占位页复用;internal = 本模块设置页消费)。
 */
@Composable
internal fun ProfileNavBar(title: String, showsBack: Boolean = false, onBack: (() -> Unit)? = null) {
    Box(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 18.dp)
            .padding(top = 10.dp, bottom = 8.dp)
            .height(30.dp),
    ) {
        Text(
            title,
            Modifier.align(Alignment.Center),
            color = MaterialTheme.colorScheme.onSurface,
            fontSize = 17.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.MiddleEllipsis,
        )
        if (showsBack) {
            Box(
                Modifier
                    .align(Alignment.CenterStart)
                    .size(width = 20.dp, height = 30.dp)
                    .clickable(onClickLabel = "返回") { onBack?.invoke() },
                contentAlignment = Alignment.Center,
            ) {
                MockGlyph(
                    pathData = BACK_PATH,
                    size = 16.dp,
                    strokeWidth = 2.8f,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}

/** 占位页(联系我们/为我打分/我的权益;iOS PlaceholderPage:图标+标题+敬请期待)。 */
@Composable
private fun PlaceholderPage(title: String, iconPath: String) {
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        ProfileNavBar(title = title, showsBack = true)
        Column(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 40.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            MockGlyph(iconPath, 44.dp, MaterialTheme.colorScheme.onSurfaceVariant, strokeWidth = 1.6f)
            Spacer(Modifier.height(12.dp))
            Text(
                title,
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 20.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                "敬请期待",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}

/** 发丝分割线(0.5dp;leading 内缩,通栏到右缘;iOS FullBleedRow 同构)。 */
@Composable
private fun Hairline(leadingInset: Dp) {
    Box(
        Modifier
            .padding(start = leadingInset)
            .fillMaxWidth()
            .height(0.5.dp)
            .background(MaterialTheme.colorScheme.outlineVariant),
    )
}

/** 登录屏记账 email(SharedPreferences "login";无记账 = null)。 */
private fun readLoginEmail(context: Context): String? =
    context.getSharedPreferences("login", Context.MODE_PRIVATE)
        .getString(SettingsKeys.LOGIN_EMAIL, null)
        ?.takeIf { it.isNotBlank() }

// ── 字形(同风格手作,24 视口;iOS SF symbol 同象,零第三方依赖)──────────

/** 返回箭头(xback 同款折线)。 */
private const val BACK_PATH = "M14.5 5l-7 7 7 7"
/** 行尾 ›(xchev 同款)。 */
private const val CHEVRON_PATH = "M9.5 5l7 7-7 7"
/** 扫码取景框(qrcode.viewfinder 同象:四角 + 内点阵;点阵层见 QR_DOTS)。 */
private const val QR_PATH =
    "M4 8V5.5C4 4.7 4.7 4 5.5 4H8M16 4h2.5c.8 0 1.5.7 1.5 1.5V8M20 16v2.5c0 .8-.7 1.5-1.5 1.5H16M8 20H5.5C4.7 20 4 19.3 4 18.5V16M8 8h2.5v2.5H8zM13.5 8h2.5v2.5h-2.5zM8 13.5h2.5V16H8z"
/** 扫码内点阵(fill 层)。 */
private const val QR_DOTS_PATH = "M15 13.5h1.5V15H15zM16.5 16.5h2v2h-2zM19.5 13h.5v.5h-.5z"
/** 齿轮(gearshape 同象;Material settings 公版形)。 */
private const val GEAR_PATH =
    "M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58c.18-.14.23-.41.12-.61l-1.92-3.32c-.12-.22-.37-.29-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54c-.04-.24-.24-.41-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58c-.18.14-.23.41-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z"
/** 信封(envelope 同象;联系我们)。 */
private const val ENVELOPE_PATH = "M4 6.5h16v11H4zM4.5 7.5l7.5 5.5 7.5-5.5"
/** 星形(star 同象;为我打分)。 */
private const val STAR_PATH =
    "M12 3.8l2.5 5 5.6.8-4 4 .9 5.6-5-2.7-5 2.7.9-5.6-4-4 5.6-.8z"
/** 礼盒(gift 同象;我的权益)。 */
private const val GIFT_PATH =
    "M4 12h16v8.5H4zM2.5 7.5h19V12h-19zM12 7.5v13M12 7.5c-1.6 0-4.5-.6-4.5-2.5 0-1.4 1.1-2 2.2-2C11.2 3 12 5.5 12 7.5zM12 7.5c1.6 0 4.5-.6 4.5-2.5 0-1.4-1.1-2-2.2-2C12.8 3 12 5.5 12 7.5z"
