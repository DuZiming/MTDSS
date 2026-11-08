package com.mtdss.android.ui.tasks

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.ui.common.ActionGlyph
import com.mtdss.android.ui.common.ActionGlyphKind
import com.mtdss.android.ui.common.CHECK_BOLD_PATH
import com.mtdss.android.ui.common.CenterTopBar
import com.mtdss.android.ui.common.DevicePickerSheet
import com.mtdss.android.ui.common.LocalTopBarMenuClose
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.common.SwipeActionSpec
import com.mtdss.android.ui.common.SwipeRevealRow
import com.mtdss.android.ui.theme.SynshuSemantic
import com.mtdss.android.ui.tasks.TasksReducer.TransferFilter

/**
 * 传输列表 tab(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-5;iOS TransfersView.swift 逐项镜像,tab 根页无返回面):
 * - 顶栏:标题绝对居中,「···」= 清除已完成(无记录面终态行时灰显禁点);
 * - 分段控件 全部/接收/发送/调度(M3 SegmentedButton,发丝边框、选中蓝底
 *   10% + 蓝粗字);
 * - 行 = 徽章(接收蓝/发送绿/调度紫灰,**V3-4 换向**,词面着色式)+ 任务名
 *   + 方向设备「A → B」+ 状态——进行中细进度条+速度+百分比(随徽章色),
 *   中断/失败红字原因(error 空兜底「已中断」),完成绿勾+时间(现场观测
 *   done 翻转,契约无时间戳不编造)+ 跳过段;
 * - **最新在最上**(reducer 重置反转/事件新行插顶);左滑:进行中→取消 /
 *   中断·失败→重新发起+清除 / 完成·已取消→清除(动作出集按状态,空集行
 *   不示能;防连点语义在 VM control 面);
 * - 「分享转交失败」行位(T15 填充,[ShareFailureFace] 空源不渲染);
 * - 「N 项」尾注 = 普通末行;动作失败红胶囊横幅(一次性,点按清除)。
 *
 * VM 为 app 级(AppGraph 装配),本屏只呈现 + 出键;打开即对账(reseed)
 * + 设备目录名现拉(方向设备文案解析)。
 */

// ── 分享转交失败行面(T15 接真源;数据形 = iOS ShareCenter.FailureRow 同构)──

/** 分享转交失败行数据(T8 钉名;渲染字段 name/filesTotal/reason,id 定位动作)。 */
data class ShareFailureRow(
    val id: String,
    val name: String,
    val filesTotal: Long,
    val reason: String?,
)

/**
 * 分享转交失败行源(T15 接真源 = [com.mtdss.android.share.ShareFailureController];
 * 缺省 null = 无分享面,列表零行):失败条目上浮传输列表顶区(spec §4.4,
 * 拷贝中断/校验不过不静默丢),随分段 全部/发送出行(出站语义)。
 * 流形面(行随消费/重试/清除重组刷新);重试 = 重选目标设备(重选即重发),
 * 请求经 [retryRequest] 流出键、屏面挂既有 DevicePickerSheet 收口。
 */
interface ShareFailureFace {

    /** 失败行流(空流 = 无行)。 */
    fun rows(): kotlinx.coroutines.flow.StateFlow<List<ShareFailureRow>>

    /** 重试出键:置重选设备请求([retryRequest] 非空即挂层)。 */
    fun onRetry(id: String)

    /** 重选设备回调(重选即重发;目标覆盖)。 */
    fun retryWith(id: String, targetDeviceId: String)

    /** 重选层取消(请求清位,失败行原样保留)。 */
    fun cancelRetry()

    /** 重选设备请求流(非空 = 待选目标;选毕/取消自清)。 */
    fun retryRequest(): kotlinx.coroutines.flow.StateFlow<String?>

    /** 清除(队列项移除;暂存件一并回收,用户文件不触碰)。 */
    fun onClear(id: String)

    /** 打开/回前台触发:失败行刷新 + 待消费队列一轮(幂等;缺省空实现)。 */
    fun refresh() {}
}

/** 行最小高 58dp(spec §2.2;进行中行随进度条自然增高,iOS v-padding 10 同构)。 */
private val TransferRowMinHeight = 58.dp

/** 行名 17sp / 副文 12.5sp(spec §2.2 字面值)。 */
private val TransferNameSize = 17.sp
private val TransferSubSize = 12.5.sp

@Composable
fun TasksScreen(graph: Graph, shareFailures: ShareFailureFace? = null) {
    val vm = graph.tasks
    val state by vm.state.collectAsState()
    // 分段(saveable:旋转/进程恢复;越界防御取「全部」)
    var filter by rememberSaveable { mutableIntStateOf(0) }

    // 打开即对账 + 设备目录名现拉(方向设备文案解析)+ 分享队列触发
    // (失败行刷新 + 待消费一轮——前台/登录消费的屏面入口,T15)
    LaunchedEffect(Unit) {
        vm.reseed()
        vm.refreshDevices()
        shareFailures?.refresh()
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        CenterTopBar(
            title = "传输列表",
            menuContent = {
                // 「···」= 清除已完成(spec §3-5;无记录面终态行时灰显禁点)
                val closeMenu = LocalTopBarMenuClose.current
                DropdownMenuItem(
                    text = { Text("清除已完成") },
                    enabled = TasksReducer.hasClearable(state.rows),
                    onClick = {
                        closeMenu()
                        vm.clearCompleted()
                    },
                )
            },
        )
        SegmentBar(filter = filter, onFilter = { filter = it })

        // 动作失败横幅(iOS actionErrorBanner 同构:红胶囊白字一次性,点按清除)
        state.opError?.let { text ->
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(bottom = 6.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text,
                    Modifier
                        .background(SynshuSemantic.Fail.copy(alpha = 0.85f), RoundedCornerShape(percent = 50))
                        .clickable(onClickLabel = "关闭") { vm.clearOpError() }
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                    color = Color.White,
                    fontSize = 13.sp,
                    maxLines = 2,
                )
            }
        }

        val segment = TransferFilter.entries[filter.coerceIn(0, TransferFilter.entries.lastIndex)]
        val shown = TasksReducer.filterRows(state.rows, segment)
        // 分享转交失败行(出站语义归 全部/发送;iOS visibleShareRows 同构;
        // 流形观察:行随消费/重试/清除重组刷新,T15 真源)
        val shareRowsAll = shareFailures?.rows()?.collectAsState()?.value.orEmpty()
        val shareRows =
            if (segment == TransferFilter.ALL || segment == TransferFilter.SEND) {
                shareRowsAll
            } else {
                emptyList()
            }

        when {
            // 空态三分判据含分享行面(iOS 同构:rows 空 + 分享行空才遮列表)
            state.loading && state.rows.isEmpty() && shareRows.isEmpty() -> Box(
                Modifier
                    .weight(1f)
                    .fillMaxWidth(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }

            state.error != null && state.rows.isEmpty() && shareRows.isEmpty() -> ErrorFace(
                error = state.error ?: "",
                onRetry = vm::reseed,
                modifier = Modifier.weight(1f),
            )

            shown.isEmpty() && shareRows.isEmpty() -> EmptyFace(
                hasRows = state.rows.isNotEmpty(),
                modifier = Modifier.weight(1f),
            )

            else -> LazyColumn(
                Modifier
                    .weight(1f)
                    .fillMaxWidth(),
            ) {
                items(shareRows, key = { "share-${it.id}" }) { row ->
                    ShareFailureRowItem(
                        row = row,
                        onRetry = { shareFailures?.onRetry(row.id) },
                        onClear = { shareFailures?.onClear(row.id) },
                    )
                }
                items(shown, key = { it.id }) { job ->
                    TransferRowItem(
                        job = job,
                        rows = state.rows,
                        direction = state.directionText(job),
                        doneTime = state.completionTime(job.id),
                        onAction = { action ->
                            when (action) {
                                TasksReducer.TaskAction.CANCEL -> vm.cancel(job.id)
                                TasksReducer.TaskAction.REINITIATE -> vm.reinitiate(job.id)
                                TasksReducer.TaskAction.CLEAR -> vm.clear(job.id)
                                // pause/resume 契约在而 UI 不上此页(iOS 同构)
                                else -> {}
                            }
                        },
                    )
                }
                // 尾注「N 项」= 普通末行(分享行计入;iOS ItemCountFooter 同构)
                item(key = "item-count-footer") {
                    Text(
                        "${shown.size + shareRows.size} 项",
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 10.dp, bottom = 14.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }

        // 分享失败重试 = 重选目标设备弹层(既有 DevicePickerSheet;T15 接线
        // ——onRetry 置请求,选毕 retryWith 换目标重发,取消清请求)
        if (shareFailures != null) {
            val retryId = shareFailures.retryRequest().collectAsState().value
            if (retryId != null) {
                DevicePickerSheet(
                    graph = graph,
                    title = "重新发送到…",
                    excluding = null,
                    onPick = { device -> shareFailures.retryWith(retryId, device.deviceId) },
                    onDismissed = { shareFailures.cancelRetry() },
                )
            }
        }
    }
}

// ── 分段控件(spec §2.2「分段控件」:M3 SegmentedButton,发丝边框、选中蓝底 10% + 蓝粗字)──

/** 分段水平内缩 20dp(iOS segmentBar padding.horizontal 20 同构)。 */
private val SegmentHorizontalInset = 20.dp

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SegmentBar(filter: Int, onFilter: (Int) -> Unit) {
    SingleChoiceSegmentedButtonRow(
        Modifier
            .fillMaxWidth()
            .padding(top = 2.dp, bottom = 10.dp)
            .padding(horizontal = SegmentHorizontalInset),
    ) {
        val segments = TransferFilter.entries
        segments.forEachIndexed { i, f ->
            SegmentedButton(
                selected = filter == i,
                onClick = { onFilter(i) },
                shape = SegmentedButtonDefaults.itemShape(i, segments.size),
                colors = SegmentedButtonDefaults.colors(
                    activeContainerColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.10f),
                    activeContentColor = MaterialTheme.colorScheme.primary,
                    inactiveContainerColor = Color.Transparent,
                    inactiveContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                ),
                // 发丝边框(spec §2.2;outlineVariant = 主题发丝线 token)
                border = BorderStroke(0.5.dp, MaterialTheme.colorScheme.outlineVariant),
                // 选中勾图标抑制(T16 台账 T8:iOS 分段选中无勾,M3 缺省
                // 选中画勾;icon 置空槽对齐 iOS 无勾形态)
                icon = {},
                label = {
                    Text(
                        f.label,
                        fontSize = 13.sp,
                        fontWeight = if (filter == i) FontWeight.Bold else FontWeight.Normal,
                        maxLines = 1,
                    )
                },
            )
        }
    }
}

// ── 传输行(徽章 + 名 + 方向设备 + 状态 + 细进度条/绿勾 + 左滑)──────────────

/**
 * 单行(iOS TransferRowView 解剖镜像):徽章 | 名/副文/细进度条 | 尾缘
 * (进行中 = 徽章色百分比,缺省状态词;完成 = 绿勾 a11y「完成」);左滑
 * 动作出集按状态(空集行不示能;VM busy 防连点收在 control 面)。
 */
@Composable
private fun TransferRowItem(
    job: TransferJob,
    rows: List<TransferJob>,
    direction: String,
    doneTime: Long?,
    onAction: (TasksReducer.TaskAction) -> Unit,
) {
    val tint = TasksReducer.badgeFor(job.kind).color
    val inFlight = !TasksReducer.isTerminalState(job.state)
    val swipes = TasksReducer.swipeActionsFor(job, rows)
    SwipeRevealRow(
        actions = swipes.map { swipeSpec(it) },
        onAction = { i -> onAction(swipes[i]) },
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = TransferRowMinHeight)
                .padding(horizontal = 20.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            KindBadge(TasksReducer.badgeFor(job.kind))
            Spacer(Modifier.width(12.dp))
            // 名/副文/进度条行距 3dp(iOS TransferRowView VStack spacing 3 同值,
            // G2 度量;进度条距由同一 spacedBy 承担,免外加 Spacer)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    TasksReducer.displayName(job),
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = TransferNameSize,
                    lineHeight = 22.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    transferSubtitle(job, direction, doneTime).joinToString(" · "),
                    color = if (job.state == "interrupted" || job.state == "failed") {
                        SynshuSemantic.Fail // 红因整段红档(spec §3-5 中断失败红字原因)
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    fontSize = TransferSubSize,
                    lineHeight = 16.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (inFlight) {
                    TransferBar(TasksReducer.progressRatio(job.bytesDone, job.bytesTotal), tint)
                }
            }
            Spacer(Modifier.width(12.dp))
            TransferTrailing(job, tint)
        }
    }
}

/**
 * 副文段(iOS TransferRowView.subtitleText 逐项镜像):方向打头;running 追
 * 速度,paused/publishing/cancelled 追状态词,interrupted/failed = 红因整段
 * 承载,done 追完成时间(现场观测)+ 跳过段;中继兜底角标恒在尾。
 */
private fun transferSubtitle(job: TransferJob, direction: String, doneTime: Long?): List<String> {
    val relay = if (job.relay) listOf("中继") else emptyList()
    return when (job.state) {
        "running" -> listOf(direction, TasksReducer.speedText(job.speedBps)) + relay
        "paused", "publishing", "cancelled" ->
            listOf(direction, TasksReducer.stateText(job.state, job.error)) + relay
        "interrupted", "failed" ->
            listOfNotNull(TasksReducer.causeText(job.state, job.error)) + relay
        "done" ->
            buildList {
                add(direction)
                doneTime?.let { add(TasksReducer.timeText(it)) }
                TasksReducer.skippedText(job.skipped)?.let { add(it) }
            } + relay
        else -> listOf(direction) + relay
    }
}

/**
 * 尾缘(iOS TransferRowView.trailing 同构):进行中 = 徽章色百分比(总量
 * 未知 → 状态词灰档);完成 = 绿勾(a11y「完成」);其余空(状态在副文)。
 */
@Composable
private fun TransferTrailing(job: TransferJob, tint: Color) {
    val inFlight = !TasksReducer.isTerminalState(job.state)
    when {
        inFlight -> {
            val ratio = TasksReducer.progressRatio(job.bytesDone, job.bytesTotal)
            if (ratio != null) {
                // 半上取整(iOS .rounded() 正数域同值)
                Text(
                    "${Math.round(ratio * 100)}%",
                    color = tint,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            } else {
                Text(
                    TasksReducer.stateText(job.state, job.error),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = TransferSubSize,
                )
            }
        }

        job.state == "done" -> Box(Modifier.semantics { contentDescription = "完成" }) {
            MockGlyph(CHECK_BOLD_PATH, 16.dp, SynshuSemantic.Done, strokeWidth = 3f)
        }
    }
}

/** 类型徽章(词面着色式:11sp 粗体 + 同色 12% 底圆角 6;iOS badge 同形)。 */
@Composable
private fun KindBadge(badge: TasksReducer.TaskBadge) {
    Text(
        badge.label,
        fontSize = 11.sp,
        fontWeight = FontWeight.Bold,
        color = badge.color,
        modifier = Modifier
            .background(badge.color.copy(alpha = 0.12f), RoundedCornerShape(6.dp))
            .padding(horizontal = 8.dp, vertical = 3.dp),
    )
}

/**
 * 细进度条(iOS progressBar 同构,4dp 胶囊):总量已知 = 徽章色填充
 * (0.2s 线性随比);未知(≤0,起步前/远端计数面)= 不确定态动画条。
 */
@Composable
private fun TransferBar(ratio: Float?, tint: Color) {
    // 轨 ≈ iOS systemFill:副文字 20%(浅深两族均近 systemFill 观感)
    val track = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.2f)
    if (ratio == null) {
        LinearProgressIndicator(
            Modifier.fillMaxWidth().height(4.dp),
            color = tint,
            trackColor = track,
        )
    } else {
        val animated by animateFloatAsState(
            targetValue = ratio,
            animationSpec = tween(200),
            label = "transferBar",
        )
        Box(
            Modifier
                .fillMaxWidth()
                .height(4.dp)
                .clip(CircleShape)
                .background(track),
        ) {
            Box(
                Modifier
                    .fillMaxWidth(animated)
                    .fillMaxHeight()
                    .background(tint),
            )
        }
    }
}

/**
 * 左滑动作钮规格(iOS TransferRowAction.swipeSpec 同构:取消灰/重新发起蓝/
 * 清除红,白图标统一 22dp;破坏钮贴缘由簇序保证——[TasksReducer.swipeActionsFor]
 * 出集末位恒破坏面)。字形 = 单源表直取(G3:取消 = Close(X)/重新发起 =
 * Refresh/清除 = delete_sweep,canonical Material filled)。
 */
@Composable
private fun swipeSpec(action: TasksReducer.TaskAction): SwipeActionSpec = when (action) {
    TasksReducer.TaskAction.CANCEL -> SwipeActionSpec("取消", MaterialTheme.colorScheme.onSurfaceVariant) {
        ActionGlyph(ActionGlyphKind.CANCEL, Color.White)
    }
    TasksReducer.TaskAction.REINITIATE -> SwipeActionSpec("重新发起", MaterialTheme.colorScheme.primary) {
        ActionGlyph(ActionGlyphKind.REINITIATE, Color.White)
    }
    TasksReducer.TaskAction.CLEAR -> SwipeActionSpec("清除", SynshuSemantic.Fail) {
        ActionGlyph(ActionGlyphKind.CLEAR, Color.White)
    }
    // 不可达:swipeActionsFor 出集只含上述三枚(矩阵 JVM 钉死);防御性炸面
    else -> throw IllegalStateException("传输列表左滑只出 取消/重新发起/清除")
}

// ── 分享转交失败行(T15 填充;渲染复用失败态语言:iOS ShareFailureRowView 同构)──

/**
 * 分享转交失败行:灰档「分享」徽章(与引擎任务收发/调度徽章区分)+ 名
 * (多项「· N 项」)+ 红因(「分享转交失败 · 明细」,明细缺省兜底
 * 「未能转交」);左滑 = [重新发起, 清除](清除贴缘)。**空源不渲染**
 * (T15 前列表零分享行)。
 */
@Composable
private fun ShareFailureRowItem(row: ShareFailureRow, onRetry: () -> Unit, onClear: () -> Unit) {
    SwipeRevealRow(
        actions = listOf(
            swipeSpec(TasksReducer.TaskAction.REINITIATE),
            swipeSpec(TasksReducer.TaskAction.CLEAR),
        ),
        onAction = { i -> if (i == 0) onRetry() else onClear() },
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = TransferRowMinHeight)
                .padding(horizontal = 20.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val gray = MaterialTheme.colorScheme.onSurfaceVariant
            Text(
                "分享",
                fontSize = 11.sp,
                fontWeight = FontWeight.Bold,
                color = gray,
                modifier = Modifier
                    .background(gray.copy(alpha = 0.12f), RoundedCornerShape(6.dp))
                    .padding(horizontal = 8.dp, vertical = 3.dp),
            )
            Spacer(Modifier.width(12.dp))
            // 名/因行距 3dp(iOS ShareFailureRowView VStack spacing 3 同值,G2 度量)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    if (row.filesTotal > 1) "${row.name} · ${row.filesTotal} 项" else row.name,
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = TransferNameSize,
                    lineHeight = 22.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    "分享转交失败 · ${row.reason?.takeIf { it.isNotEmpty() } ?: "未能转交"}",
                    color = SynshuSemantic.Fail,
                    fontSize = TransferSubSize,
                    lineHeight = 16.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

// ── 错误/空态(iOS ContentUnavailableView 同构留白)─────────────────────────

/** 错误态(iOS「无法加载传输列表」+ 透传 error + 重试,现拉即恢复)。 */
@Composable
private fun ErrorFace(error: String, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                "无法加载传输列表",
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                error,
                Modifier.padding(top = 6.dp, start = 24.dp, end = 24.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
            )
            TextButton(onClick = onRetry) { Text("重试") }
        }
    }
}

/** 空态(iOS「暂无传输」/「该分类暂无任务」逐字面;有历史行但该分段无 = 分类空)。 */
@Composable
private fun EmptyFace(hasRows: Boolean, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                if (hasRows) "该分类暂无任务" else "暂无传输",
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                if (hasRows) "换个分段看看" else "发送/接收的任务会出现在这里",
                Modifier.padding(top = 6.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
            )
        }
    }
}
