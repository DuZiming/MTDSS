package com.mtdss.android.ui.browse

import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.common.ActionGlyph
import com.mtdss.android.ui.common.CHECK_BOLD_PATH
import com.mtdss.android.ui.common.CenterTopBar
import com.mtdss.android.ui.common.DevicePickerSheet
import com.mtdss.android.ui.common.LocalTopBarMenuClose
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.common.SearchField
import com.mtdss.android.ui.common.SelectionActionBar
import com.mtdss.android.ui.common.SelectionTopBar
import com.mtdss.android.ui.common.SynshuRow
import com.mtdss.android.ui.common.SynshuSelectionAction
import com.mtdss.android.ui.common.entrySubtitle
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/**
 * 浏览屏(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-4 远端浏览页 / §2.2 浏览模板全套;iOS BrowseView.swift 逐项镜像):
 * 顶栏(双箭头 + 绝对居中标题中截断「设备名 | 目录名」+ 「···」= 选择 +
 * 排序单选勾)→ 搜索框 → 通栏行(SynshuRow:选圈列/展开钮/图标/副文)
 * → 「N 项(已截断)」尾注;长按 0.4s 进选择模式;下拉刷新;错误态
 * 「无法加载目录」+ 重试遮列表;加载态。
 *
 * **选择模式**(Ruling O):顶栏换 全选/N 项/蓝圈确认,壳藏胶囊底栏
 * (selectionHidden 经 [onSelectionChange] 上提到壳);底部动作条 远端 =
 * [下载, 转发](+ 删除受 BrowseGates 门)/ 纯目录多选追加「快捷」;
 * **确认 = 只退出无副作用**——操作仅自底栏/左滑发起(先捕获 selectedEntries
 * 再退出)。
 *
 * 行左滑(iOS NativeSwipeActions 同构,自绘皮肤无全滑逐钮确认):远端行 =
 * [下载, 转发] + 删除(受门),目录行追加「快捷」;删除红底/其余蓝底,
 * 白图标;动作 = 单条目走与选择模式同一收口。
 *
 * 动作收口:转发/下载经 VM(remote_copy 转发滤来源经弹层 excluding;下载
 * 直达收件箱哨兵);删除入居中确认(文件夹形文案三分支);发起成功
 * toast「已开始 N 项 · 进度见传输列表」(进度归传输列表)。
 *
 * [onAddFavorite] = V3-3 T18「快捷」回调(swipe 目录行单条 / 底栏纯目录
 * 多选 → 快捷目录 picker);null 缺省 = 不出「快捷」且出集走既有 actions
 * (T10 快捷目录 tab 落 FavoritePickerSheet 后接线)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowseScreen(
    graph: Graph,
    device: DirectoryDevice,
    onBack: () -> Unit,
    onSelectionChange: (Boolean) -> Unit = {},
    onAddFavorite: ((List<DirEntry>) -> Unit)? = null,
) {
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose {
            scope.cancel()
            onSelectionChange(false) // 卸载(登出/弹回)时壳藏胶囊复位
        }
    }

    // 浏览位存续(评审 fix round 1;manifest 无 configChanges,旋转重建 VM,
    // repo 既有形态 = 旧浏览屏 rememberSaveable 位置 + 重挂重拉,位置不丢
    // 是底线):历史栈 + 游标持久化,VM 构造种子恢复,listing 现拉
    // (iOS 旋转不丢态同杆对齐;iOS 推入新 StateObject = 重拉同构)。
    var savedHistory by rememberSaveable { mutableStateOf(listOf<String>()) }
    var savedIndex by rememberSaveable { mutableIntStateOf(0) }

    val vm = remember(device.deviceId) {
        BrowseViewModel(
            deviceId = device.deviceId,
            rootPath = "", // 契约 list path 空串 = 根卷列表(daemon ctrl 面)
            rootTitle = device.name ?: device.deviceId,
            deviceName = device.name ?: device.deviceId, // R4 #5:标题 = 设备名 | 目录名
            repo = graph.repo,
            scope = scope,
            initialHistory = savedHistory,
            initialHistoryIndex = savedIndex,
        )
    }
    val state by vm.state.collectAsState()

    // 初次进页拉浏览根;旋转恢复 = 按恢复位现拉(错误态不自动重拉,重试承担)
    LaunchedEffect(vm) {
        if (vm.state.value.entries.isEmpty() && vm.state.value.errorText == null && !vm.state.value.loading) {
            if (savedHistory.isEmpty()) vm.loadRoot() else vm.reload()
        }
    }
    // 导航写回:VM 重建(旋转/进程恢复)时按栈恢复位置
    LaunchedEffect(state.path) {
        val (h, i) = vm.historySnapshot()
        savedHistory = h
        savedIndex = i
    }
    // 选择模式 → 壳藏胶囊(T5 selectionHidden 消费面)
    LaunchedEffect(state.isSelecting) { onSelectionChange(state.isSelecting) }

    // 系统返回 = 页面回退(spec §2.2):选择中退选择;历史内 goBack;浏览根弹回设备标签根
    BackHandler {
        when {
            state.isSelecting -> vm.exitSelection()
            state.canGoBack -> vm.goBack()
            else -> onBack()
        }
    }

    // 确认后待转发的选择集(non-null → 选设备弹层;iOS PickedSelection 同构)
    var pickedForForward by remember { mutableStateOf<List<DirEntry>?>(null) }
    // 待确认删除请求(选择删除/行滑删同一收口;iOS DeleteRequest 同构)
    var deleteRequest by remember { mutableStateOf<DeleteRequest?>(null) }
    // 动作发起反馈胶囊(R3 #3:发起即 toast,进度归传输列表)
    var toast by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(state.startedCount) {
        state.startedCount?.let {
            toast = "已开始 $it 项 · 进度见传输列表"
            vm.clearFeedback()
        }
    }
    LaunchedEffect(toast) {
        toast?.let {
            delay(2200)
            toast = null
        }
    }

    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxSize()) {
            if (state.isSelecting) {
                SelectionTopBar(
                    count = state.selection.size,
                    onSelectAll = vm::selectAll,
                    onConfirm = vm::confirmSelection,
                )
            } else {
                CenterTopBar(
                    title = vm.title,
                    // 返回:历史内 = 目录级后退;浏览根 = 弹回设备标签根(iOS onPopToRoot 同构)
                    onBack = { if (state.canGoBack) vm.goBack() else onBack() },
                    onForward = { vm.goForward() },
                    canBack = state.canGoBack,
                    canForward = state.canGoForward,
                    menuContent = {
                        BrowseOverflowMenu(
                            sort = state.sort,
                            onSelect = vm::enterSelection,
                            onSort = vm::setSort,
                        )
                    },
                )
            }
            // 搜索框(灰底圆角,按名称过滤当前列表)
            SearchField(text = state.searchText, onTextChange = vm::setSearchText)

            when {
                // 错误态遮列表(iOS ContentUnavailableView 同构;T6 deferred minor ① 口径)
                state.errorText != null && state.entries.isEmpty() -> ErrorFace(
                    error = state.errorText ?: "",
                    onRetry = vm::reload,
                    modifier = Modifier.weight(1f),
                )

                state.loading && state.entries.isEmpty() -> Box(
                    Modifier.weight(1f).fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                else -> PullToRefreshBox(
                    isRefreshing = state.loading && state.entries.isNotEmpty(),
                    onRefresh = vm::reload,
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                ) {
                    LazyColumn(Modifier.fillMaxSize()) {
                        items(vm.visibleRows, key = { it.id }) { row ->
                            BrowseRowItem(
                                row = row,
                                state = state,
                                vm = vm,
                                isRemote = true,
                                allowRemoteDelete = BrowseGates.ALLOW_REMOTE_DELETE,
                                onAddFavorite = onAddFavorite,
                                onDelete = { deleteRequest = it },
                                onForward = { pickedForForward = it },
                            )
                        }
                        // 尾注 = 普通末行(iOS BrowseList 同取舍:原生左滑 > 沉底尾注)
                        item(key = "item-count-footer") {
                            Text(
                                if (state.truncated) "${vm.itemCount} 项(已截断)" else "${vm.itemCount} 项",
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
            }

            // 选择模式底部动作条(远端 = 下载/转发 + 删除受门;纯目录多选追加「快捷」)
            if (state.isSelecting) {
                val selected = vm.selectedEntries
                val allDirs = selected.isNotEmpty() && selected.all { it.isDir }
                val actions = if (onAddFavorite != null) {
                    BrowseSelectionAction.dirSelectionActions(
                        isRemote = true,
                        allDirs = allDirs,
                        allowRemoteDelete = BrowseGates.ALLOW_REMOTE_DELETE,
                    )
                } else {
                    BrowseSelectionAction.actions(isRemote = true, allowRemoteDelete = BrowseGates.ALLOW_REMOTE_DELETE)
                }
                SelectionActionBar(
                    actions = actions.map { it.toBarAction() },
                    onAction = { action ->
                        when (action) {
                            // 转发与发送同形:捕获 → 退出选择 → 选设备弹层(Ruling O 次序)
                            SynshuSelectionAction.Share, SynshuSelectionAction.Forward -> {
                                val items = vm.selectedEntries
                                if (items.isNotEmpty()) {
                                    vm.exitSelection()
                                    pickedForForward = items
                                }
                            }

                            SynshuSelectionAction.Delete -> {
                                val entries = vm.selectedEntries
                                if (entries.isNotEmpty()) {
                                    deleteRequest = DeleteRequest(
                                        paths = entries.map { it.path },
                                        folders = entries.count { it.isDir },
                                    )
                                }
                            }

                            SynshuSelectionAction.Download -> {
                                val entries = vm.selectedEntries
                                if (entries.isNotEmpty()) {
                                    vm.exitSelection()
                                    vm.download(entries.map { it.path })
                                }
                            }

                            // 底栏「快捷」:捕获选择集 → 退出选择模式 → onAddFavorite
                            SynshuSelectionAction.Favorite -> {
                                val entries = vm.selectedEntries
                                if (entries.isNotEmpty()) {
                                    vm.exitSelection()
                                    onAddFavorite?.invoke(entries)
                                }
                            }
                        }
                    },
                )
            }
        }

        // 动作发起反馈胶囊(iOS toast 同构:底部悬浮胶囊,自动消隐)
        toast?.let { text ->
            ToastPill(text, Modifier.align(Alignment.BottomCenter).padding(bottom = 64.dp))
        }
    }

    // 删除确认(R5b #9:居中 alert 破坏性确认;选择删除与行滑删同一收口)
    deleteRequest?.let { request ->
        AlertDialog(
            onDismissRequest = { deleteRequest = null },
            title = { Text(request.alertTitle) },
            text = { Text(request.alertMessage) },
            confirmButton = {
                TextButton(onClick = {
                    deleteRequest = null
                    if (vm.state.value.isSelecting) {
                        vm.deleteSelected()
                    } else {
                        vm.deletePaths(request.paths, reloadAfter = true)
                    }
                }) {
                    Text("删除", color = SynshuSemantic.Fail)
                }
            },
            dismissButton = {
                TextButton(onClick = { deleteRequest = null }) { Text("取消") }
            },
        )
    }

    // 操作失败(删除/下载/转发失败文案;iOS lastActionError alert 同构)
    state.lastActionError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { vm.clearFeedback() },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { vm.clearFeedback() }) { Text("好") }
            },
        )
    }

    // 删除完成(部分失败明细;全成静默)
    state.deleteNotice?.let { notice ->
        AlertDialog(
            onDismissRequest = { vm.clearFeedback() },
            title = { Text("删除完成") },
            text = { Text(notice) },
            confirmButton = {
                TextButton(onClick = { vm.clearFeedback() }) { Text("好") }
            },
        )
    }

    // 选设备弹层(转发;标题逐字面,转发滤来源设备 = daemon 拒 from==to 前置滤除)
    pickedForForward?.let { items ->
        DevicePickerSheet(
            graph = graph,
            title = "转发 ${items.size} 项到…",
            excluding = device.deviceId,
            onPick = { target ->
                vm.forward(items.map { it.path }, to = target)
                pickedForForward = null
            },
        )
    }
}

/** VM 动作集 → 底栏钮(iOS SelectionBar.ActionKind(Action) 映射同构)。 */
private fun BrowseSelectionAction.toBarAction(): SynshuSelectionAction = when (this) {
    BrowseSelectionAction.SHARE -> SynshuSelectionAction.Share
    BrowseSelectionAction.DELETE -> SynshuSelectionAction.Delete
    BrowseSelectionAction.DOWNLOAD -> SynshuSelectionAction.Download
    BrowseSelectionAction.FORWARD -> SynshuSelectionAction.Forward
    BrowseSelectionAction.FAVORITE -> SynshuSelectionAction.Favorite
}

/** 「···」菜单 = 选择 + 排序单选勾(iOS overflowMenu 同构;点项后菜单自合)。 */
@Composable
private fun ColumnScope.BrowseOverflowMenu(
    sort: BrowseSort,
    onSelect: () -> Unit,
    onSort: (BrowseSort) -> Unit,
) {
    val closeMenu = LocalTopBarMenuClose.current
    DropdownMenuItem(
        text = { Text("选择") },
        onClick = {
            closeMenu()
            onSelect()
        },
    )
    HorizontalDivider()
    BrowseSort.entries.forEach { field ->
        DropdownMenuItem(
            text = { Text(field.label) },
            trailingIcon = if (field == sort) {
                { MockGlyph(CHECK_BOLD_PATH, 14.dp, MaterialTheme.colorScheme.primary, strokeWidth = 3f) }
            } else {
                null
            },
            onClick = {
                closeMenu()
                onSort(field)
            },
        )
    }
}

/**
 * 浏览行(SynshuRow + 行左滑自绘皮肤 + 长按 0.4s 进选择;iOS BrowseEntryRow
 * + NativeSwipeActions + simultaneousGesture(LongPress 0.4) 三件同位)。
 */
@Composable
private fun BrowseRowItem(
    row: BrowseRow,
    state: BrowseViewModel.UiState,
    vm: BrowseViewModel,
    isRemote: Boolean,
    allowRemoteDelete: Boolean,
    onAddFavorite: ((List<DirEntry>) -> Unit)?,
    onDelete: (DeleteRequest) -> Unit,
    onForward: (List<DirEntry>) -> Unit,
) {
    val entry = row.entry
    // 左滑动作集:选择态翻空集(滑面收,iOS 同构);远端档挂「快捷」时目录行追加
    val swipes = if (state.isSelecting) {
        emptyList()
    } else if (onAddFavorite != null) {
        BrowseSwipeAction.dirActions(entry, isRemote = isRemote, allowRemoteDelete = allowRemoteDelete)
    } else {
        BrowseSwipeAction.actions(isRemote = isRemote, allowRemoteDelete = allowRemoteDelete)
    }
    SwipeRevealRow(actions = swipes, onAction = { action ->
        when (action) {
            // 单条目捕获进选设备弹层(转发;与选择模式同一收口)
            BrowseSwipeAction.SEND, BrowseSwipeAction.FORWARD -> onForward(listOf(entry))

            BrowseSwipeAction.DELETE -> onDelete(
                DeleteRequest(paths = listOf(entry.path), folders = if (entry.isDir) 1 else 0),
            )

            BrowseSwipeAction.DOWNLOAD -> vm.download(listOf(entry.path))
            BrowseSwipeAction.FAVORITE -> onAddFavorite?.invoke(listOf(entry))
        }
    }) {
        // 长按 0.4s 进选择(iOS simultaneousGesture(LongPress 0.4) 同构;
        // detectTapGestures 只收长按,短点由行内 clickable 照常处理)
        Box(
            Modifier.pointerInput(entry.path, state.isSelecting) {
                detectTapGestures(
                    onLongPress = {
                        if (!vm.state.value.isSelecting) vm.enterSelection()
                    },
                )
            },
        ) {
            SynshuRow(
                name = entry.name,
                isDir = entry.isDir,
                depth = row.depth,
                expanded = entry.path in state.expandedPaths,
                selecting = state.isSelecting,
                selected = entry.path in state.selection,
                onOpen = {
                    if (vm.state.value.isSelecting) {
                        vm.toggle(entry.path)
                    } else if (entry.isDir) {
                        vm.open(entry.path)
                    } // 文件行点按无导航面(iOS 同构:文件行为叶)
                },
                onToggleExpand = if (entry.isDir) {
                    { vm.toggleExpand(entry.path) }
                } else {
                    null
                },
                sub = entrySubtitle(entry.mtimeMs, entry.children?.toInt(), entry.size, entry.isDir),
            )
        }
    }
}

// ── 行左滑自绘皮肤(spec §2.2「左滑动作」;M3 SwipeToDismiss 自绘件)──────────

/** 单钮宽 68dp(iOS 原生 swipeActions 钮宽同量级);白图标 18dp(spec §2.2)。 */
private val SwipeButtonWidth = 68.dp

/**
 * 左滑揭示行(自绘皮肤,零新增依赖):水平拖拽揭示尾缘动作簇,**无全滑**
 * (松手落最近锚点 = 揭示/回弹,永不触发动作——逐钮确认);拖拽与列表纵向
 * 滚动天然分轴(横向 drag LazyColumn 不消费,iOS 的 pan 争用问题在 Compose
 * 分轴判定下不成立);揭示态点内容先合左滑(iOS 同:首点只合不复触行)。
 * 动作集空 = 直通(不挂手势,iOS 空集无滑面同构)。
 */
@Composable
private fun SwipeRevealRow(
    actions: List<BrowseSwipeAction>,
    onAction: (BrowseSwipeAction) -> Unit,
    content: @Composable () -> Unit,
) {
    if (actions.isEmpty()) {
        Box { content() }
        return
    }
    val density = LocalDensity.current
    val revealWidthPx = with(density) { (SwipeButtonWidth * actions.size).toPx() }
    val offsetX = remember(actions.size) { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val settled = offsetX.value != 0f

    Box(Modifier.fillMaxWidth()) {
        // 背景:动作簇贴尾缘(破坏面钮 = 簇内末位贴缘,spec §2.2;iOS reversed
        // 声明的有效视觉 = 数组末位最贴右缘,同构)
        Row(Modifier.matchParentSize()) {
            Spacer(Modifier.weight(1f))
            actions.forEach { action ->
                val tint = if (action == BrowseSwipeAction.DELETE) SynshuSemantic.Fail else MaterialTheme.colorScheme.primary
                Box(
                    Modifier
                        .width(SwipeButtonWidth)
                        .fillMaxHeight()
                        .background(tint)
                        .clickable(onClickLabel = action.label) {
                            onAction(action)
                            scope.launch { offsetX.animateTo(0f, tween(200)) }
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    // 白图标:与选择底栏同字形(iOS 左滑/底栏同 SF symbol 同构)
                    ActionGlyph(
                        when (action) {
                            BrowseSwipeAction.SEND -> SynshuSelectionAction.Share
                            BrowseSwipeAction.DELETE -> SynshuSelectionAction.Delete
                            BrowseSwipeAction.DOWNLOAD -> SynshuSelectionAction.Download
                            BrowseSwipeAction.FORWARD -> SynshuSelectionAction.Forward
                            BrowseSwipeAction.FAVORITE -> SynshuSelectionAction.Favorite
                        },
                        Color.White,
                    )
                }
            }
        }
        // 内容行(偏移揭示;横向拖拽自绘,纵向滚动让列表)
        Box(
            Modifier
                .offset { IntOffset(offsetX.value.roundToInt(), 0) }
                .pointerInput(actions) {
                    detectHorizontalDragGestures(
                        onHorizontalDrag = { change, dragAmount ->
                            change.consume()
                            scope.launch {
                                offsetX.snapTo((offsetX.value + dragAmount).coerceIn(-revealWidthPx, 0f))
                            }
                        },
                        onDragEnd = {
                            // 无全滑:松手落最近锚点(揭示/回弹),不触发动作
                            val target = if (offsetX.value < -revealWidthPx / 2f) -revealWidthPx else 0f
                            scope.launch { offsetX.animateTo(target, tween(200)) }
                        },
                        onDragCancel = {
                            scope.launch { offsetX.animateTo(0f, tween(200)) }
                        },
                    )
                },
        ) {
            content()
            // 揭示态透明拦截层:点内容先合左滑(iOS 同构;不复触行点按)
            if (settled) {
                Box(
                    Modifier
                        .matchParentSize()
                        .pointerInput(Unit) {
                            detectTapGestures {
                                scope.launch { offsetX.animateTo(0f, tween(200)) }
                            }
                        },
                )
            }
        }
    }
}

/** 动作发起反馈胶囊(iOS toast 同构:accent 85% 胶囊白字,自动消隐由宿主管)。 */
@Composable
private fun ToastPill(text: String, modifier: Modifier = Modifier) {
    Box(
        modifier
            .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.85f), RoundedCornerShape(percent = 50))
            .padding(horizontal = 14.dp, vertical = 8.dp),
    ) {
        Text(text, color = Color.White, fontSize = 13.sp, maxLines = 1)
    }
}

/** 错误态遮列表(iOS「无法加载目录」+ 原因 + 重试,ContentUnavailableView 同构)。 */
@Composable
private fun ErrorFace(error: String, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text("无法加载目录", color = MaterialTheme.colorScheme.onSurface, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
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
