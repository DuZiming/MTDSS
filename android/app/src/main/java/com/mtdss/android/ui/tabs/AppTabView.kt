package com.mtdss.android.ui.tabs

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.engine.MtdssEngine
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ui.common.CapsuleNavBar
import com.mtdss.android.ui.common.CapsuleTabGlyph
import com.mtdss.android.ui.common.CapsuleTabSpec
import com.mtdss.android.ui.devices.DevicesScreen
import com.mtdss.android.ui.settings.SettingsScreen
import com.mtdss.android.ui.tasks.TasksScreen
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuSemantic

/**
 * 四标签枚举(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-1):序 = 胶囊底栏自左向右,与 iOS AppTab.swift 同序同词面(快捷目录/
 * 设备/传输列表/我的;spec「中文文案与 iOS 端逐字一致」约束)。glyph 为
 * 胶囊底栏图标形(CapsuleTabGlyph,T4 组件层)。
 */
enum class AppTab(val title: String, val glyph: CapsuleTabGlyph) {
    /** 快捷目录(iOS favorites)。 */
    FAVORITES("快捷目录", CapsuleTabGlyph.FAVORITES),

    /** 设备(iOS devices)。 */
    DEVICES("设备", CapsuleTabGlyph.DEVICES),

    /** 传输列表(iOS transfers)。 */
    TRANSFERS("传输列表", CapsuleTabGlyph.TRANSFERS),

    /** 我的(iOS profile)。 */
    PROFILE("我的", CapsuleTabGlyph.PROFILE),
}

/**
 * 四标签壳(spec §3-1;iOS AppTab.swift 镜像):登录态期间的唯一根面——
 * 快捷目录/设备/传输列表/我的四标签 + 毛玻璃胶囊底栏(T4 CapsuleNavBar,
 * 自带 16dp 悬浮边距,选择模式期间由壳藏栏)。
 *
 * 顶部横幅(iOS AppTabView overlay(.top) 同位;bootError 优先):
 * - 红胶囊 = 引擎 spawn 连败(MtdssEngine BACKOFF + spawn-failed,词面
 *   「引擎启动失败:…」= iOS bootError「引擎启动失败:\(error)」同构;
 *   二进制缺失等永久性失败时横幅常驻);
 * - 蓝胶囊 = 「重连中…」(iOS engineRestarting 的安卓对应面):引擎死亡
 *   退避重生窗(BACKOFF + exit)或 IPC 自愈重连窗(conn != Connected;
 *   引擎重启后 socket 必经此窗,二者合并覆盖,秒级收敛自消)。
 *
 * 选择模式/扫码 sheet 期间藏栏的消费面钉死为 [selectionHidden](计划 T5
 * 接口面;T7 选择模式接线时由壳级状态驱动)。
 *
 * 过渡(T5 旧屏共存期,各 tab 由 T6-T11 逐个替换、T16 扫尾):
 * - 快捷目录 = 居中占位(T10 落「收件箱置顶 + 快捷目录清单」真面);
 * - 设备 = 旧 DevicesScreen(点设备的浏览深链随旧路由退役,过渡期空操作;
 *   T6/T7 后浏览经设备 tab 重入;「任务」「设置」顶栏入口改切对应 tab);
 * - 传输列表 = 旧 TasksScreen(tab 化无返回面,onBack 置 null:返回键
 *   不拦截、顶栏返回钮不渲染);
 * - 我的 = 旧 SettingsScreen(T11 重皮;顶栏返回钮过渡空操作)。
 */
@Composable
fun AppTabView(graph: Graph, selectionHidden: Boolean) {
    val sessionState by graph.session.state.collectAsState()
    val engine by MtdssEngine.status.collectAsState()

    // 选中标签(saveable:旋转/进程恢复;登出卸壳后重挂默认回快捷目录)
    var selection by rememberSaveable { mutableStateOf(AppTab.FAVORITES) }

    val bootError = bootErrorText(engine)
    val reconnecting = bootError == null &&
        (engine.phase == MtdssEngine.Phase.BACKOFF || sessionState.conn != ConnState.Connected)

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f)) {
                when (selection) {
                    AppTab.FAVORITES -> FavoritesPlaceholder()

                    AppTab.DEVICES -> DevicesScreen(
                        graph,
                        // 过渡:点设备浏览深链随旧路由退役(T6/T7 接管)
                        onDevice = { },
                        // 过渡:本机页 T10 落快捷目录 tab,入口先空操作
                        onLocalFiles = { },
                        onTasks = { selection = AppTab.TRANSFERS },
                        onSettings = { selection = AppTab.PROFILE },
                    )

                    // 过渡:旧任务屏 tab 化,无返回面(onBack null,T8 重写)
                    AppTab.TRANSFERS -> TasksScreen(graph, onBack = null)

                    // 过渡:旧设置屏占我的 tab(T11 重皮;返回钮空操作)
                    AppTab.PROFILE -> SettingsScreen(graph, onBack = { })
                }
            }
            if (!selectionHidden) {
                CapsuleNavBar(
                    tabs = AppTab.entries.map { CapsuleTabSpec(key = it.name, label = it.title, glyph = it.glyph) },
                    active = selection.ordinal,
                    onTab = { selection = AppTab.entries[it] },
                )
            }
        }
        // 顶部横幅(bootError 优先,其次「重连中…」;iOS if/else 同序)
        if (bootError != null) {
            BootErrorBanner(bootError, Modifier.align(Alignment.TopCenter))
        } else if (reconnecting) {
            ReconnectingBanner(Modifier.align(Alignment.TopCenter))
        }
    }
}

/**
 * 引擎 bootError 词面(iOS bootError 的安卓对应源):spawn 连败 = 引擎
 * 无法启动,取「引擎启动失败:」+ spawn 失败细节(EngineStatus.lastExit
 * 的 "spawn-failed: " 前缀剥除);其余 BACKOFF(死亡退避重生)归「重连中」
 * 蓝胶囊面,RUNNING/STOPPED 无横幅。
 */
internal fun bootErrorText(status: MtdssEngine.EngineStatus): String? {
    val exit = status.lastExit ?: return null
    if (status.phase != MtdssEngine.Phase.BACKOFF || !exit.startsWith("spawn-failed:")) return null
    return "引擎启动失败:" + exit.removePrefix("spawn-failed: ")
}

/** 横幅胶囊形(任意宽全圆 = iOS Capsule)。 */
private val BannerCapsule = RoundedCornerShape(percent = 50)
/** 横幅顶部净空(iOS .padding(.top, 4))。 */
private val BannerTopPad = 4.dp

/** bootError 红胶囊横幅(iOS 同构:白字 footnote≈13sp + red 85% 胶囊,h12/v8)。 */
@Composable
private fun BootErrorBanner(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        modifier
            .padding(top = BannerTopPad)
            .background(SynshuSemantic.Fail.copy(alpha = 0.85f), BannerCapsule)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        color = Color.White,
        style = MaterialTheme.typography.bodyMedium,
    )
}

/** 「重连中…」蓝胶囊横幅(iOS 同词面:白字 + 蓝 85% 胶囊 h14/v8 + 小白圈)。 */
@Composable
private fun ReconnectingBanner(modifier: Modifier = Modifier) {
    Row(
        modifier
            .padding(top = BannerTopPad)
            .background(SynshuColors.Accent.copy(alpha = 0.85f), BannerCapsule)
            .padding(horizontal = 14.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CircularProgressIndicator(modifier = Modifier.size(14.dp), strokeWidth = 2.dp, color = Color.White)
        Text("重连中…", color = Color.White, style = MaterialTheme.typography.bodyMedium)
    }
}

/** 过渡:快捷目录占位(T10 落「收件箱置顶 + 快捷目录清单」真面)。 */
@Composable
private fun FavoritesPlaceholder() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text(
            "快捷目录",
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
