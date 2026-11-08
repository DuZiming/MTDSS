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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.mtdss.android.Graph
import com.mtdss.android.engine.MtdssEngine
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.browse.BrowseScreen
import com.mtdss.android.ui.common.CapsuleNavBar
import com.mtdss.android.ui.common.CapsuleTabGlyph
import com.mtdss.android.ui.common.CapsuleTabSpec
import com.mtdss.android.ui.devices.DevicesScreen
import com.mtdss.android.ui.favorites.FavoritePickerSheet
import com.mtdss.android.ui.favorites.FavoritesScreen
import com.mtdss.android.ui.profile.ProfileScreen
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
 * 接口面;选择状态经 [onSelectionActive] 上提到 AppNav 调用点,T7 布线)。
 *
 * 四标签面(T16 收口:T5-T11 逐屏替换完毕,旧屏零残留):
 * - 快捷目录 =「收件箱置顶 + 快捷目录清单」真面(T10);
 * - 设备 = 设备页(T6);点在线设备推远端浏览页(T7,tab 内推入,系统
 *   返回/顶栏返回弹回设备标签根;浏览选择模式经 [onSelectionActive] 藏胶囊);
 * - 传输列表 = 传输列表页(T8,tab 根页无返回面);
 * - 我的 = 我的页(T11:扫码/齿轮进设置/账号行/退出红字居中确认/版本沉底;
 *   设置/登录/占位页 tab 内推入)。
 */
@Composable
fun AppTabView(
    graph: Graph,
    selectionHidden: Boolean,
    onSelectionActive: (Boolean) -> Unit = {},
) {
    val sessionState by graph.session.state.collectAsState()
    val engine by MtdssEngine.status.collectAsState()

    // 选中标签(saveable:旋转/进程恢复;登出卸壳后重挂默认回快捷目录)
    var selection by rememberSaveable { mutableStateOf(AppTab.FAVORITES) }

    // 设备 tab 推入的远端浏览目标(壳级持有:标签切换不冲页,iOS 每标签
    // 导航栈同构;弹回 = 清目标回设备列表)。saveable = 旋转/进程恢复不丢
    // 推入位(评审 fix round 1:manifest 无 configChanges,plain remember
    // 旋转即弹回设备列表;iOS 旋转不丢态,同杆对齐)。
    var browseTarget by rememberSaveable(stateSaver = BrowseTargetSaver) {
        mutableStateOf<DirectoryDevice?>(null)
    }

    // 「加入快捷」待选载荷(T10:远端浏览「快捷」动作 → FavoritePickerSheet;
    // saveable = 旋转/进程恢复不丢弹层面,沿 browseTarget 形态)。
    var favPickRequest by rememberSaveable(stateSaver = FavPickRequestSaver) {
        mutableStateOf<FavPickRequest?>(null)
    }

    val bootError = bootErrorText(engine)
    val reconnecting = bootError == null &&
        (engine.phase == MtdssEngine.Phase.BACKOFF || sessionState.conn != ConnState.Connected)

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f)) {
                when (selection) {
                    // 快捷目录 tab(T10:收件箱置顶 DirNode + 快捷行成员树;
                    // 选择模式经 onSelectionActive 藏胶囊,成员/收件箱点入 =
                    // tab 内推浏览模板)
                    AppTab.FAVORITES -> FavoritesScreen(graph, onSelectionActive)

                    AppTab.DEVICES -> {
                        val target = browseTarget
                        if (target != null) {
                            // 陈旧目标守卫(评审 fix round 1):恢复/持有期间设备
                            // 可能已被移除——devices 对账正向确认不在列才清回设备
                            // 列表(RPC 失败不清,浏览页自有「无法加载目录」错误面)
                            LaunchedEffect(target.deviceId) {
                                graph.repo.devices().onSuccess { r ->
                                    if (r.devices.none { it.deviceId == target.deviceId }) {
                                        browseTarget = null
                                    }
                                }
                            }
                            BrowseScreen(
                                graph = graph,
                                device = target,
                                onBack = { browseTarget = null },
                                onSelectionChange = onSelectionActive,
                                // 「快捷」回调随 T10 FavoritePickerSheet 接线(V3-3
                                // T18:目录行单条/纯目录多选底栏统一入口)
                                onAddFavorite = { entries ->
                                    favPickRequest = FavPickRequest(
                                        entries = entries,
                                        sourceDevice = target.deviceId,
                                    )
                                },
                            )
                        } else {
                            DevicesScreen(
                                graph,
                                onDevice = { browseTarget = it },
                            )
                        }
                    }

                    // 传输列表 tab(T8 重写:分段含调度/徽章换向/最新在上/左滑
                    // 出集;tab 根页无返回面。T15:分享转交失败行源 = app 级
                    // ShareFailureController,重试层屏内挂既有 DevicePickerSheet)
                    AppTab.TRANSFERS -> TasksScreen(graph, shareFailures = graph.shareFailures)

                    // 我的 tab(T11 重做:扫码/齿轮进设置/账号行/占位行/
                    // 退出红字居中确认/版本沉底;设置登录占位页 tab 内推入)
                    AppTab.PROFILE -> ProfileScreen(graph)
                }

                // 「加入快捷」弹层(T10;V3-3 T18 接线面——远端浏览目录行
                // 「快捷」/纯目录多选底栏统一入口,壳级挂载)
                favPickRequest?.let { request ->
                    FavoritePickerSheet(
                        graph = graph,
                        entries = request.entries,
                        sourceDevice = request.sourceDevice,
                        onDismiss = { favPickRequest = null },
                    )
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

/**
 * 「加入快捷」待选载荷(V3-3 T18;iOS FavoritePickRequest 同构):[DirEntry]
 * 集合 + 成员 device(FavMember.device = 浏览目标设备 id)。saveable 载荷:
 * 条目六元(name/path/size/mtime/isDir/children)␁ 连接、条目间 ␂ 连接、
 * 尾段 = sourceDevice(路径不含控制字节,memberKey 同象防歧义)。
 */
internal data class FavPickRequest(val entries: List<DirEntry>, val sourceDevice: String)

private const val PICK_FIELD_SEP = "\u0001"
private const val PICK_ENTRY_SEP = "\u0002"

/** 待选载荷 saveable 形(旋转/进程恢复不丢弹层面)。 */
private val FavPickRequestSaver = Saver<FavPickRequest?, String>(
    save = { r ->
        r?.entries?.joinToString(PICK_ENTRY_SEP) { e ->
            listOf(
                e.name,
                e.path,
                e.size.toString(),
                e.mtimeMs.toString(),
                if (e.isDir) "1" else "0",
                e.children?.toString() ?: "",
            ).joinToString(PICK_FIELD_SEP)
        }?.plus(PICK_ENTRY_SEP + r.sourceDevice)
    },
    restore = { s ->
        val parts = s.split(PICK_ENTRY_SEP)
        if (parts.size < 2) {
            null
        } else {
            val entries = parts.dropLast(1).mapNotNull { p ->
                val f = p.split(PICK_FIELD_SEP)
                if (f.size < 6) {
                    null
                } else {
                    DirEntry(
                        name = f[0],
                        path = f[1],
                        size = f[2].toLongOrNull() ?: 0L,
                        mtimeMs = f[3].toLongOrNull() ?: 0L,
                        isDir = f[4] == "1",
                        children = f[5].takeIf { it.isNotEmpty() }?.toLongOrNull(),
                    )
                }
            }
            FavPickRequest(entries = entries, sourceDevice = parts.last())
        }
    },
)

/**
 * 浏览目标 saveable 形(旋转/进程恢复):持久化设备身份三元(deviceId/名/
 * 平台),在线旗不进档(恢复时快照未到不可靠,浏览门不在旗上——BrowseScreen
 * 以 deviceId 浏览,离线由列表错误面/陈旧目标守卫收口);空表 = 无推入。
 */
private val BrowseTargetSaver = Saver<DirectoryDevice?, List<String>>(
    save = { d ->
        if (d == null) emptyList() else listOf(d.deviceId, d.name ?: "", d.platform ?: "")
    },
    restore = { l ->
        if (l.size < 3) {
            null
        } else {
            DirectoryDevice(
                deviceId = l[0],
                name = l[1].takeIf { it.isNotEmpty() },
                platform = l[2].takeIf { it.isNotEmpty() },
                online = true,
            )
        }
    },
)
