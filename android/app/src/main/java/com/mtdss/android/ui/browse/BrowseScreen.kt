package com.mtdss.android.ui.browse

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.ListResult
import com.mtdss.android.ui.common.ActionGlyph
import com.mtdss.android.ui.common.ActionGlyphKind
import com.mtdss.android.ui.common.CHECK_BOLD_PATH
import com.mtdss.android.ui.common.CenterTopBar
import com.mtdss.android.ui.common.DevicePickerSheet
import com.mtdss.android.ui.common.LocalTopBarMenuClose
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.common.SearchField
import com.mtdss.android.ui.common.SelectionActionBar
import com.mtdss.android.ui.common.SelectionTopBar
import com.mtdss.android.ui.common.SwipeActionSpec
import com.mtdss.android.ui.common.SwipeRevealRow
import com.mtdss.android.ui.common.SynshuRow
import com.mtdss.android.ui.common.SynshuSelectionAction
import com.mtdss.android.ui.common.entrySubtitle
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay

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
    /** 浏览根路径种子(缺省 "" = 卷列表;成员目录直入 = 目标路径,T10)。 */
    rootPath: String = "",
    /** 根目录标题种子(缺省 = 设备名/设备 id;成员目录直入 = 目录显示名)。 */
    rootTitle: String? = null,
    /** 对端设备名(标题「设备名 | 目录名」;缺省 = 设备名/id 兜底与 T7 旧形
     * 逐字一致;成员直入传 null = 单段口径)。 */
    deviceName: String? = device.name ?: device.deviceId,
    /** 浏览档(动作集分档;成员直入 = 按成员设备与本机比对,T10)。 */
    isRemote: Boolean = true,
    /**
     * 本机列表接管缝(V3-4 T14 收件箱 SAF 真面):非 null 直通 VM——全部
     * 列表拉取走此(ServeResponder.listInbox 的 SAF 面),repo.list 不触;
     * 动作面仍走 repo 既有流。缺省 null = 引擎列表(远端/既有形态零变化)。
     */
    lister: (suspend (String) -> Result<ListResult>)? = null,
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

    val vm = remember(device.deviceId, rootPath) {
        BrowseViewModel(
            deviceId = device.deviceId,
            rootPath = rootPath,
            rootTitle = rootTitle ?: device.name ?: device.deviceId,
            deviceName = deviceName,
            repo = graph.repo,
            scope = scope,
            gates = graph.settings.gates,
            initialHistory = savedHistory,
            initialHistoryIndex = savedIndex,
            lister = lister,
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
            // 搜索框(灰底圆角,按名称过滤当前列表;上下距 = iOS searchBar
            // top 4 / bottom 10 同值,G2 度量——此前贴顶栏零间距显「粗糙」)
            SearchField(
                text = state.searchText,
                onTextChange = vm::setSearchText,
                modifier = Modifier.padding(top = 4.dp, bottom = 10.dp),
            )

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
                                isRemote = isRemote,
                                allowRemoteDelete = state.allowRemoteDelete,
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
                        isRemote = isRemote,
                        allDirs = allDirs,
                        allowRemoteDelete = state.allowRemoteDelete,
                    )
                } else {
                    BrowseSelectionAction.actions(isRemote = isRemote, allowRemoteDelete = state.allowRemoteDelete)
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
    // 共用左滑皮肤(T8 提取至 common;下标映射回动作枚举)
    SwipeRevealRow(
        actions = swipes.map { it.toSwipeSpec() },
        onAction = { i ->
            when (swipes[i]) {
                // 单条目捕获进选设备弹层(转发;与选择模式同一收口)
                BrowseSwipeAction.SEND, BrowseSwipeAction.FORWARD -> onForward(listOf(entry))

                BrowseSwipeAction.DELETE -> onDelete(
                    DeleteRequest(paths = listOf(entry.path), folders = if (entry.isDir) 1 else 0),
                )

                BrowseSwipeAction.DOWNLOAD -> vm.download(listOf(entry.path))
                BrowseSwipeAction.FAVORITE -> onAddFavorite?.invoke(listOf(entry))
            }
        },
    ) {
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

// ── 行左滑动作规格(spec §2.2「左滑动作」;皮肤件 = common SwipeRevealRow,
//        T8 自本页私有件提取共用)────────────────────────────────────────────

/**
 * 动作 → 左滑钮规格(iOS NativeSwipeActions swipeSpec 同构):删除红底/
 * 其余蓝底(spec §2.2),白图标经字形单源表 [ActionGlyphKind] 直取
 * (G3:canonical Material 24dp filled 统一 22dp;iOS 左滑/底栏同源同构)。
 */
@Composable
private fun BrowseSwipeAction.toSwipeSpec(): SwipeActionSpec = SwipeActionSpec(
    label = label,
    tint = if (this == BrowseSwipeAction.DELETE) SynshuSemantic.Fail else MaterialTheme.colorScheme.primary,
    glyph = {
        ActionGlyph(
            when (this@toSwipeSpec) {
                BrowseSwipeAction.SEND -> ActionGlyphKind.SEND
                BrowseSwipeAction.DELETE -> ActionGlyphKind.DELETE
                BrowseSwipeAction.DOWNLOAD -> ActionGlyphKind.DOWNLOAD
                BrowseSwipeAction.FORWARD -> ActionGlyphKind.FORWARD
                BrowseSwipeAction.FAVORITE -> ActionGlyphKind.FAVORITE
            },
            Color.White,
        )
    },
)

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
