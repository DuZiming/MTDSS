package com.mtdss.android.ui.favorites

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.favorites.Favorite
import com.mtdss.android.favorites.FavMember
import com.mtdss.android.favorites.FavoritesTree
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.serve.ServeResponder
import com.mtdss.android.ui.browse.BrowseRow
import com.mtdss.android.ui.browse.BrowseScreen
import com.mtdss.android.ui.browse.BrowseSelectionAction
import com.mtdss.android.ui.browse.BrowseSort
import com.mtdss.android.ui.browse.BrowseViewModel
import com.mtdss.android.ui.browse.DeleteRequest
import com.mtdss.android.ui.common.ActionGlyph
import com.mtdss.android.ui.common.ActionGlyphKind
import com.mtdss.android.ui.common.CHECK_BOLD_PATH
import com.mtdss.android.ui.common.CenterTopBar
import com.mtdss.android.ui.common.DevicePickerSheet
import com.mtdss.android.ui.common.FileTypeIcon
import com.mtdss.android.ui.common.LocalTopBarMenuClose
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.common.SelectionActionBar
import com.mtdss.android.ui.common.SelectionTopBar
import com.mtdss.android.ui.common.SwipeActionSpec
import com.mtdss.android.ui.common.SwipeRevealRow
import com.mtdss.android.ui.common.SynshuGlyphs
import com.mtdss.android.ui.common.SynshuRow
import com.mtdss.android.ui.common.toGlyphKind
import com.mtdss.android.ui.common.SynshuSelectionAction
import com.mtdss.android.ui.common.entrySubtitle
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * 快捷目录 tab(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §3-2;iOS FavoritesView.swift 逐项镜像,安卓树形态——iOS 混合视图不移植):
 * - **收件箱置顶 DirNode 恒在**:行内展开 = 本机浏览([BrowseViewModel] 树
 *   投影,浏览根 = /收件箱 vpath;数据面 = SAF 真面经列表接管缝,T14——
 *   引擎 recv_dir 实机为收件哨兵非真路径);行点按 = 推收件箱整页浏览(同
 *   缝接管);子行左滑 = 树行同集(本机恒 [下载, 转发, 删除]);选择模式
 *   根行不可选、子行可选;
 * - **快捷行**(创建序):箭头 = 行内展开成员行树(「设备名 - 目录名」,
 *   离线整行灰显禁点禁展开);行点按 = 进页(R2 G1 iOS 语义,展开/进页两
 *   条路并存):多成员 = 推快捷分组页([FavGroupPage],iOS 混合页安卓落形),
 *   单成员在线 = 直推该成员目录整页,单成员离线/零成员 = 翻展开(诚实披
 *   露);左滑 = [管理, 删除快捷]
 *   (readOnly 翻空集——前向守卫禁编辑);清单空 = 引导行(非 readOnly);
 * - **树子级含文件行**(叶,无展开无点按导航);点成员行/子目录行 = 推
 *   成员目录整页浏览(.member 路由,[BrowseScreen] 种子直入);树行左滑 =
 *   [下载, 转发, 删除](本机行恒可删、远端行受 BrowseGates 门;X7 口径
 *   动作语义归 [FavoritesViewModel]);
 * - 「···」= 选择 + 新建快捷目录(readOnly 隐藏)+ 排序保留形(禁用,
 *   iOS 同构);新建 alert 空名禁提交;删除快捷确认(只解除链接文案);
 *   readOnly(v>1)= 滑删/新建禁、清单照渲;
 * - 选择模式 = 顶栏换 全选/N 项/确认(确认只退出),底栏 分享/删除,壳藏
 *   胶囊经 [onSelectionActive] 上提(T5 链路)。
 *
 * 推入页 = [BrowseScreen] 同模板:浏览根 = 目标路径,标题 = 目录显示名不携
 * 设备名(单段口径,iOS .member 同构);弹回 = 清路由回根页。
 */
@Composable
fun FavoritesScreen(graph: Graph, onSelectionActive: (Boolean) -> Unit) {
    // 推入路由(壳级持有:旋转/进程恢复不丢推入位,沿 browseTarget 形态;
    // 弹回 = 清路由回上一面)。两态并存:浏览推入(成员整页/收件箱/分组页
    // 钻取)置顶,分组页置顶于根页——浏览页弹回分组页、分组页弹回根页。
    // 设备名不进档(标题 = 目录名,iOS .member 同构)。
    var pushRoute by rememberSaveable(stateSaver = BrowseRouteSaver) {
        mutableStateOf<BrowseRoute?>(null)
    }
    var groupRoute by rememberSaveable(stateSaver = GroupRouteSaver) {
        mutableStateOf<GroupRoute?>(null)
    }
    val sessionState by graph.session.state.collectAsState()

    // 快捷 tab VM(壳级持有:根页/分组页共用一面——分组页被钻取顶掉的窗内
    // 组态与树展开状态存活,弹回即位;离 tab/登出随壳卸载)
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose {
            scope.cancel()
            onSelectionActive(false) // 卸载(登出)时壳藏胶囊复位
        }
    }
    val vm = remember(graph) { FavoritesViewModel(graph.favorites, graph.repo, graph.hub, scope, graph.settings.gates) }
    // 会话投影注入(树行本机判定/本机源下载 dir;瞬时入档,无事件面)
    LaunchedEffect(sessionState.deviceId) { vm.setSelfDeviceId(sessionState.deviceId) }
    LaunchedEffect(sessionState.recvDir) { vm.setRecvDir(sessionState.recvDir) }

    val route = pushRoute
    val group = groupRoute
    when {
        route != null -> {
            val isRemote = route.deviceId != FavoritesViewModel.SELF_ALIAS &&
                route.deviceId != sessionState.deviceId
            BrowseScreen(
                graph = graph,
                device = DirectoryDevice(deviceId = route.deviceId, name = null, platform = null, online = true),
                onBack = { pushRoute = null },
                onSelectionChange = onSelectionActive,
                onAddFavorite = null, // 成员页不挂「快捷」picker(iOS .member 同构)
                rootPath = route.path,
                rootTitle = route.title,
                deviceName = null,
                isRemote = isRemote,
                // 收件箱子树路由(V3-4 T14 接缝):列表面 = ServeResponder 的 SAF
                // 真面(引擎 recv_dir 实机为哨兵非真路径);成员/远端路由照旧引擎
                lister = if (route.inbox) ({ path -> graph.serveResponder.listInbox(path) }) else null,
            )
        }

        group != null -> FavGroupPage(
            graph = graph,
            vm = vm,
            favId = group.favId,
            fallbackTitle = group.title,
            onBack = { groupRoute = null },
            onDrill = { deviceId, path, title ->
                pushRoute = BrowseRoute(deviceId = deviceId, path = path, title = title, inbox = false)
            },
        )

        else -> FavoritesRoot(graph, vm, onSelectionActive, { deviceId, path, title, inbox ->
            pushRoute = BrowseRoute(deviceId = deviceId, path = path, title = title, inbox = inbox)
        }, { favId, title ->
            groupRoute = GroupRoute(favId = favId, title = title)
        })
    }
}

/** tab 内推入路由值(浏览设备 + 浏览根路径 + 根标题 + 收件箱旗;收件箱/
 * 成员同形,inbox = 列表接管缝——SAF 真面直喂,见 FavoritesScreen 注)。 */
private data class BrowseRoute(val deviceId: String, val path: String, val title: String, val inbox: Boolean = false)

/** 分组页推入路由值(快捷 id + 推入时名;名跟随 store 现值,仅兜底)。 */
private data class GroupRoute(val favId: String, val title: String)

/** 分组页路由 saveable 形(旋转/进程恢复;持久化 favId + 标题)。 */
private val GroupRouteSaver = Saver<GroupRoute?, List<String>>(
    save = { r -> if (r == null) emptyList() else listOf(r.favId, r.title) },
    restore = { l -> if (l.size < 2) null else GroupRoute(favId = l[0], title = l[1]) },
)

/** 推入路由 saveable 形(旋转/进程恢复;持久化设备 id + 路径 + 标题 +
 * 收件箱旗四元;旧档三元缺旗 = false 兼容)。 */
private val BrowseRouteSaver = Saver<BrowseRoute?, List<String>>(
    save = { r ->
        if (r == null) emptyList() else listOf(r.deviceId, r.path, r.title, r.inbox.toString())
    },
    restore = { l ->
        if (l.size < 3) {
            null
        } else {
            BrowseRoute(deviceId = l[0], path = l[1], title = l[2], inbox = l.getOrElse(3) { "false" } == "true")
        }
    },
)

/** 本机浏览设备 id(iOS BrowseViewModel.localBrowseDeviceId 同构:本机 id,
 * 未投影窗退化引擎 self 别名——daemon ctrl 对本机短路不要求会话)。 */
private fun localBrowseDeviceId(selfDeviceId: String?): String =
    selfDeviceId ?: FavoritesViewModel.SELF_ALIAS

/**
 * 快捷目录根页:顶栏(「快捷目录」无箭头 + 「···」菜单)→ 列表(收件箱
 * 置顶行 + 行内展开子级 + 快捷行区段 + 「N 项」尾注);下拉刷新(status
 * 重投影/收件箱原地重载/store 整单/设备快照/树已展键现拉)。
 * 推入通道携收件箱旗([onPush] 末参):收件箱行/子目录行 = true(SAF 真面
 * 接管),成员/树行 = false(引擎浏览照旧)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FavoritesRoot(
    graph: Graph,
    vm: FavoritesViewModel,
    onSelectionActive: (Boolean) -> Unit,
    onPush: (String, String, String, Boolean) -> Unit,
    onOpenGroup: (String, String) -> Unit,
) {
    val rootScope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose {
            rootScope.cancel()
            onSelectionActive(false) // 卸载(登出)时壳藏胶囊复位
        }
    }
    val state by vm.state.collectAsState()
    val sessionState by graph.session.state.collectAsState()

    // 成员/子目录推入标题 = 目录显示名(iOS .member 同构);收件箱推入特称
    // 「收件箱」(.root 同构,标题不携原始路径段)。成员路由非收件箱旗
    // (引擎浏览);收件箱路由携旗(SAF 真面接管)由收件箱各推入点直发。
    val onOpenMember = { deviceId: String, path: String ->
        onPush(deviceId, path, FavoritesTree.dirName(path), false)
    }

    // 会话投影注入归 FavoritesScreen(vm 壳级持有,见上)

    // 收件箱本机浏览 VM(V3-4 T14 接缝:数据面 = ServeResponder 的 SAF 真
    // 面——引擎 recv_dir 实机为收件哨兵「inbox」非真路径;列表接管缝喂
    // 全部拉取,动作面仍走既有流)。vpath 命名空间 = /收件箱(roots.set
    // 同锚;条目 path = 完整 vpath,子目录推入页同缝直喂)。收件箱行恒在,
    // 授权与否在展开/点入的错误态呈现(「收件箱未授权」词面)。
    val inboxVm = remember(sessionState.deviceId) {
        BrowseViewModel(
            deviceId = localBrowseDeviceId(sessionState.deviceId),
            rootPath = ServeResponder.INBOX_VPATH,
            rootTitle = "收件箱",
            deviceName = null,
            repo = graph.repo,
            scope = rootScope,
            gates = graph.settings.gates,
            lister = { path -> graph.serveResponder.listInbox(path) },
        )
    }
    val inboxState: BrowseViewModel.UiState = inboxVm.state.collectAsState().value

    // 收件箱行内展开态 + 进页即拉一次(根行展开零等待,子级的子级仍走懒加载;
    // 失败不阻塞首页,展开后行内重试;iOS loadRoot 同构)
    var inboxExpanded by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(inboxVm) {
        inboxExpanded = false
        if (inboxVm.state.value.entries.isEmpty() &&
            inboxVm.state.value.errorText == null &&
            !inboxVm.state.value.loading
        ) {
            inboxVm.loadRoot()
        }
    }
    // 选择模式 → 壳藏胶囊(T5 selectionHidden 消费面;本页选择同链路)
    LaunchedEffect(inboxState.isSelecting) { onSelectionActive(inboxState.isSelecting) }

    val listScope = rememberCoroutineScope()

    // 选择流捕获(iOS shareSelection/confirmDeleteSelection:先捕获后退选)
    var pickedForSend by remember { mutableStateOf<List<DirEntry>?>(null) }
    var deleteRequest by remember { mutableStateOf<DeleteRequest?>(null) }
    // 树行左滑捕获(§17.5;转发 = 单行捕获进选设备弹层,删除 = 待确认档)
    var treeForwardPicked by remember { mutableStateOf<TreeForwardPick?>(null) }
    var treeDeleteTarget by remember { mutableStateOf<TreeDeleteTarget?>(null) }
    var treeDeleteRequest by remember { mutableStateOf<DeleteRequest?>(null) }
    // 快捷行左滑捕获(管理 sheet / 删除快捷确认)
    var manageTargetId by rememberSaveable { mutableStateOf<String?>(null) }
    var favDeleteTarget by remember { mutableStateOf<Favorite?>(null) }
    // 新建快捷目录对话框(readOnly 隐藏入口)
    var showCreate by remember { mutableStateOf(false) }
    var newName by remember { mutableStateOf("") }
    // 动作发起反馈胶囊(iOS toast 同构;VM 计数与本地分享两来源)
    var sendToast by remember { mutableStateOf<String?>(null) }
    // 分享发起失败(路径形态上传;iOS lastActionError 同口,本地态承担)
    var sendError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(state.startedCount) {
        state.startedCount?.let {
            sendToast = "已开始 $it 项 · 进度见传输列表"
            vm.clearActionFeedback()
        }
    }
    LaunchedEffect(sendToast) {
        sendToast?.let {
            delay(2200)
            sendToast = null
        }
    }
    LaunchedEffect(state.toast) {
        state.toast?.let {
            delay(2200)
            vm.clearToast()
        }
    }

    // 下拉刷新(iOS refreshable 同构:status 重投影 + 收件箱原地重载 + store
    // 整单/设备快照重拉 + 树已展键现拉;失败各自保旧值)
    fun refreshAll() {
        graph.session.refresh()
        inboxVm.reload()
        vm.refresh()
        vm.refreshFavTree()
    }

    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxSize()) {
            if (inboxState.isSelecting) {
                SelectionTopBar(
                    count = inboxState.selection.size,
                    onSelectAll = { inboxVm.selectAll() },
                    onConfirm = { inboxVm.confirmSelection() },
                )
            } else {
                CenterTopBar(
                    title = "快捷目录",
                    menuContent = {
                        FavoritesOverflowMenu(
                            readOnly = state.readOnly,
                            onSelect = { inboxVm.enterSelection() },
                            onCreate = {
                                newName = ""
                                showCreate = true
                            },
                        )
                    },
                )
            }
            val refreshing = inboxState.loading && inboxState.entries.isNotEmpty()
            PullToRefreshBox(
                isRefreshing = refreshing,
                onRefresh = { refreshAll() },
                modifier = Modifier.weight(1f).fillMaxWidth(),
            ) {
                LazyColumn(Modifier.fillMaxSize()) {
                    // 收件箱置顶根行(恒在;选择模式不可选不导航)
                    item(key = "inbox-row") {
                        DirNodeRow(
                            title = "收件箱",
                            expanded = inboxExpanded,
                            selecting = inboxState.isSelecting,
                            onToggleExpand = {
                                if (inboxExpanded) {
                                    inboxVm.collapseAll()
                                    inboxExpanded = false
                                } else {
                                    inboxExpanded = true
                                }
                            },
                            onOpen = {
                                if (!inboxState.isSelecting) {
                                    onPush(inboxVm.deviceId, inboxVm.rootPath, "收件箱", true)
                                }
                            },
                        )
                    }
                    // 收件箱行内展开子级(iOS childrenArea:错误行/转圈行/树投影)
                    if (inboxExpanded) {
                        inboxChildrenArea(
                            inboxVm = inboxVm,
                            inboxState = inboxState,
                            onOpenPath = { path ->
                                // 子目录推入同携收件箱旗(SAF 真面接管推入页)
                                onPush(inboxVm.deviceId, path, FavoritesTree.dirName(path), true)
                            },
                            onDownload = { entry ->
                                vm.downloadTreeRow(inboxVm.deviceId, entry.path)
                            },
                            onForward = { entry ->
                                treeForwardPicked = TreeForwardPick(device = inboxVm.deviceId, entry = entry)
                            },
                            onDelete = { entry ->
                                deleteRequest = DeleteRequest(
                                    paths = listOf(entry.path),
                                    folders = if (entry.isDir) 1 else 0,
                                )
                            },
                        )
                    }
                    // 快捷行区段(空清单 = 引导行,非 readOnly;iOS favoritesSection)
                    if (state.favorites.isEmpty()) {
                        if (!state.readOnly) {
                            item(key = "favorites-hint") {
                                HintRow(
                                    text = "新建快捷目录,再把设备里的目录添加进来",
                                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 14.dp),
                                )
                            }
                        }
                    } else {
                        itemsIndexed(state.favorites, key = { _, f -> f.id }) { _, fav ->
                            FavoriteRowItem(
                                fav = fav,
                                state = state,
                                vm = vm,
                                selecting = inboxState.isSelecting,
                                onToggleExpand = { vm.toggleFavoriteExpand(fav.id) },
                                onManage = { manageTargetId = fav.id },
                                onDeleteFav = { favDeleteTarget = fav },
                                onOpenMember = onOpenMember,
                                onOpenGroup = onOpenGroup,
                                onTreeDownload = { device, entry -> vm.downloadTreeRow(device, entry.path) },
                                onTreeForward = { device, entry ->
                                    treeForwardPicked = TreeForwardPick(device = device, entry = entry)
                                },
                                onTreeDelete = { target ->
                                    treeDeleteTarget = target
                                    treeDeleteRequest = DeleteRequest(
                                        paths = listOf(target.path),
                                        folders = if (target.isDir) 1 else 0,
                                    )
                                },
                            )
                        }
                    }
                    // 尾注「N 项」(iOS ItemCountFooter(count: 1 + favorites.count))
                    item(key = "item-count-footer") {
                        Text(
                            "${1 + state.favorites.size} 项",
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

            // 选择模式底部动作条(本地档 = 分享/删除,iOS actions(isRemote:false))
            if (inboxState.isSelecting) {
                SelectionActionBar(
                    actions = BrowseSelectionAction.actions(isRemote = false, allowRemoteDelete = false)
                        .map { action ->
                            when (action) {
                                BrowseSelectionAction.SHARE -> SynshuSelectionAction.Share
                                BrowseSelectionAction.DELETE -> SynshuSelectionAction.Delete
                                else -> SynshuSelectionAction.Share
                            }
                        },
                    onAction = { action ->
                        when (action) {
                            SynshuSelectionAction.Share -> {
                                val items = inboxVm.selectedEntries
                                if (items.isNotEmpty()) {
                                    inboxVm.exitSelection()
                                    pickedForSend = items
                                }
                            }

                            SynshuSelectionAction.Delete -> {
                                val entries = inboxVm.selectedEntries
                                if (entries.isNotEmpty()) {
                                    deleteRequest = DeleteRequest(
                                        paths = entries.map { it.path },
                                        folders = entries.count { it.isDir },
                                    )
                                }
                            }

                            else -> Unit
                        }
                    },
                )
            }
        }

        // 动作发起反馈胶囊(底部悬浮,自动消隐)
        sendToast?.let { text ->
            ToastPill(text, Modifier.align(Alignment.BottomCenter).padding(bottom = 64.dp))
        }
        state.toast?.let { text ->
            ToastPill(text, Modifier.align(Alignment.BottomCenter).padding(bottom = 64.dp))
        }
    }

    // ── 弹窗/弹层群 ─────────────────────────────────────────────────────────

    // 收件箱删除确认(选择删除/行滑删同一收口;确认后走本机档既有流)
    deleteRequest?.let { request ->
        AlertDialog(
            onDismissRequest = { deleteRequest = null },
            title = { Text(request.alertTitle) },
            text = { Text(request.alertMessage) },
            confirmButton = {
                TextButton(onClick = {
                    deleteRequest = null
                    if (inboxVm.state.value.isSelecting) {
                        inboxVm.deleteSelected()
                    } else {
                        inboxVm.deletePaths(request.paths, reloadAfter = true)
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

    // 树行删除确认(§17.5;文件夹形文案点明整树语义,三分支复用 T7 面)
    treeDeleteRequest?.let { request ->
        val target = treeDeleteTarget
        AlertDialog(
            onDismissRequest = { treeDeleteRequest = null },
            title = { Text(request.alertTitle) },
            text = { Text(request.alertMessage) },
            confirmButton = {
                TextButton(onClick = {
                    treeDeleteRequest = null
                    if (target != null) {
                        vm.deleteTreeRow(
                            device = target.device,
                            path = target.path,
                            refreshDevice = target.refreshDevice,
                            refreshPath = target.refreshPath,
                        )
                    }
                }) {
                    Text("删除", color = SynshuSemantic.Fail)
                }
            },
            dismissButton = {
                TextButton(onClick = { treeDeleteRequest = null }) { Text("取消") }
            },
        )
    }

    // 删除快捷确认(左滑「删除快捷」;解除链接文案 = 管理屏同款逐字)
    favDeleteTarget?.let { fav ->
        AlertDialog(
            onDismissRequest = { favDeleteTarget = null },
            title = { Text("删除快捷目录") },
            text = { Text("只解除链接,不删除任何文件。删除 ${fav.name}?") },
            confirmButton = {
                TextButton(onClick = {
                    favDeleteTarget = null
                    listScope.launch { vm.deleteFavorite(fav.id) }
                }) {
                    Text("删除", color = SynshuSemantic.Fail)
                }
            },
            dismissButton = {
                TextButton(onClick = { favDeleteTarget = null }) { Text("取消") }
            },
        )
    }

    // 新建快捷目录(alert+TextField;空名禁提交——钮 disabled,词面与副文
    // 逐字对齐 iOS FavoritesView 新建对话框)
    if (showCreate) {
        AlertDialog(
            onDismissRequest = { showCreate = false },
            title = { Text("新建快捷目录") },
            text = {
                Column {
                    OutlinedTextField(
                        value = newName,
                        onValueChange = { newName = it },
                        placeholder = { Text("名称") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Text(
                        "创建后可把设备里的目录添加进来",
                        Modifier.padding(top = 10.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                    )
                }
            },
            confirmButton = {
                TextButton(
                    enabled = newName.trim().isNotEmpty(),
                    onClick = {
                        val name = newName.trim()
                        showCreate = false
                        listScope.launch { vm.createFavorite(name) }
                    },
                ) { Text("创建") }
            },
            dismissButton = {
                TextButton(onClick = { showCreate = false }) { Text("取消") }
            },
        )
    }

    // 快捷操作失败(新建/删除/重命名/成员;store op 返 Bool 无明细,plain)
    state.opError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { vm.clearFeedback() },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { vm.clearFeedback() }) { Text("好") }
            },
        )
    }

    // 动作失败(下载/转发/树行删除;iOS lastActionError alert 同构)
    state.lastActionError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { vm.clearActionFeedback() },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { vm.clearActionFeedback() }) { Text("好") }
            },
        )
    }

    // 分享发起失败(本地态;iOS lastActionError 同口)
    sendError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { sendError = null },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { sendError = null }) { Text("好") }
            },
        )
    }

    // 收件箱删除完成(部分失败明细;全成静默)
    inboxState.deleteNotice?.let { notice ->
        AlertDialog(
            onDismissRequest = { inboxVm.clearFeedback() },
            title = { Text("删除完成") },
            text = { Text(notice) },
            confirmButton = {
                TextButton(onClick = { inboxVm.clearFeedback() }) { Text("好") }
            },
        )
    }

    // 收件箱动作失败(分享/删除收口失败文案)
    inboxState.lastActionError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { inboxVm.clearFeedback() },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { inboxVm.clearFeedback() }) { Text("好") }
            },
        )
    }

    // 分享选设备弹层(选择流;发起 = 路径形态上传到对端,dir = 收件箱哨兵)
    pickedForSend?.let { items ->
        DevicePickerSheet(
            graph = graph,
            title = "发送 ${items.size} 项到…",
            excluding = null,
            onPick = { target ->
                val paths = items.map { it.path }
                listScope.launch {
                    graph.repo.uploadFiles(files = paths, to = target.deviceId, dir = MtdssRepository.INBOX_DIR)
                        .fold(
                            onSuccess = { sendToast = "已开始 ${paths.size} 项 · 进度见传输列表" },
                            onFailure = { e -> sendError = BrowseViewModel.userErrorText(e) },
                        )
                }
                pickedForSend = null
            },
        )
    }

    // 树行转发弹层(§17.5 左滑;来源设备前置滤 from==to)
    treeForwardPicked?.let { picked ->
        DevicePickerSheet(
            graph = graph,
            title = "转发 1 项到…",
            excluding = picked.device,
            onPick = { target ->
                vm.forwardTreeRow(picked.device, picked.entry.path, target)
                treeForwardPicked = null
            },
        )
    }

    // 快捷管理屏(T17 面;持 id 查最新,他端删除随整单跟随自关)
    manageTargetId?.let { favId ->
        FavoriteManageSheet(
            favId = favId,
            vm = vm,
            onDismiss = { manageTargetId = null },
        )
    }

    // 系统返回:选择中退选择(根页无浏览历史)
    BackHandler(enabled = inboxState.isSelecting) { inboxVm.exitSelection() }
}

// ── 「···」菜单(选择 + 新建快捷目录 + 排序保留形;iOS overflowMenu 同构)──

/** 「···」菜单 = 选择 + 新建快捷目录(readOnly 隐藏)+ 排序四项保留形(禁用)。 */
@Composable
private fun ColumnScope.FavoritesOverflowMenu(
    readOnly: Boolean,
    onSelect: () -> Unit,
    onCreate: () -> Unit,
) {
    val closeMenu = LocalTopBarMenuClose.current
    DropdownMenuItem(
        text = { Text("选择") },
        onClick = {
            closeMenu()
            onSelect()
        },
    )
    if (!readOnly) {
        // 新建快捷目录(readOnly 隐藏——v>1 前向守卫,store 拒写,编辑入口不示能)
        DropdownMenuItem(
            text = { Text("新建快捷目录") },
            onClick = {
                closeMenu()
                onCreate()
            },
        )
    }
    HorizontalDivider()
    // 排序保留形(根页 = 收件箱置顶 + 快捷清单,非文件浏览,排序无意义但
    // 保留形;iOS Picker .constant(.name) disabled 同构)
    BrowseSort.entries.forEach { field ->
        DropdownMenuItem(
            text = { Text(field.label) },
            trailingIcon = if (field == BrowseSort.NAME) {
                { MockGlyph(CHECK_BOLD_PATH, 14.dp, MaterialTheme.colorScheme.primary, strokeWidth = 3f) }
            } else {
                null
            },
            enabled = false,
            onClick = {},
        )
    }
}

// ── 行原子(DirNode 行/引导行/转圈行/胶囊;复用 common 字形件)──────────────

/**
 * DirNode 行(收件箱置顶行/快捷行同形;iOS RootFolderIcon 行同构):箭头 =
 * 行内展开(› 展开态旋转 90° 成 ∨,触区 28dp 负偏移还原 12dp 占位);行
 * 点按 = [onOpen](选择态 = 无操作,根行不可选 R5b 语义);通栏发丝线。
 */
@Composable
private fun DirNodeRow(
    title: String,
    expanded: Boolean,
    selecting: Boolean,
    onToggleExpand: () -> Unit,
    onOpen: () -> Unit,
    caretVisible: Boolean = true,
) {
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 58.dp)
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                enabled = !selecting,
                onClickLabel = title,
                onClick = onOpen,
            )
            .semantics { contentDescription = title },
    ) {
        Row(
            Modifier
                .fillMaxSize()
                .padding(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RowCaret(
                expanded = expanded,
                visible = caretVisible,
                onToggle = onToggleExpand,
            )
            FileTypeIcon(isDir = true)
            Text(
                title,
                color = MaterialTheme.colorScheme.onSurface,
                fontSize = 17.sp,
                lineHeight = 22.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
        }
        // 发丝分割线:0.5dp,行首内缩 20dp,通栏到右缘(iOS FullBleedRow 同构)
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = 20.dp)
                .fillMaxWidth()
                .height(0.5.dp)
                .background(MaterialTheme.colorScheme.outlineVariant),
        )
    }
}

/** 展开钮(iOS 同构:› 14sp 旋转 90°;触区外扩 28dp 方 + 负偏移还原 12dp
 * 占位;[visible] = false = 12dp 隐形占位,行名左缘对齐不漂)。 */
@Composable
private fun RowCaret(expanded: Boolean, visible: Boolean, onToggle: () -> Unit) {
    Box(Modifier.width(12.dp), contentAlignment = Alignment.Center) {
        if (visible) {
            Box(
                Modifier
                    .offset(x = (-8).dp)
                    .size(28.dp)
                    .clickable(onClickLabel = if (expanded) "收起" else "展开", onClick = onToggle),
                contentAlignment = Alignment.Center,
            ) {
                MockGlyph(
                    pathData = SynshuGlyphs.CHEVRON,
                    size = 14.dp,
                    strokeWidth = 2.6f,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.graphicsLayer { rotationZ = if (expanded) 90f else 0f },
                )
            }
        }
    }
}

/** 引导/注记行(muted 不可点;iOS emptyHintRow 同语言)。 */
@Composable
private fun HintRow(text: String, modifier: Modifier = Modifier, small: Boolean = false) {
    Box(modifier.fillMaxWidth()) {
        Text(
            text,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = if (small) 13.sp else 14.sp,
        )
    }
}

/** 转圈行(44dp;树子级懒加载在飞位,缩进随树级)。 */
@Composable
private fun TreeSpinnerRow(depth: Int) {
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 44.dp)
            .padding(start = 20.dp + 18.dp * depth),
    ) { CircularProgressIndicator(Modifier.size(20.dp)) }
}

/** 动作发起反馈胶囊(iOS toast 同构:accent 85% 胶囊白字,宿主自动消隐)。 */
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

// ── 收件箱行内展开子级(iOS childrenArea:错误行/转圈行/树投影 + 左滑同集)─

/** 收件箱展开子级区(错误行「无法加载目录」+重试/转圈行/树投影行;VM 恒在
 * ——T14 起数据面 = SAF 真面,无 recv_dir 投影窗)。 */
private fun LazyListScope.inboxChildrenArea(
    inboxVm: BrowseViewModel,
    inboxState: BrowseViewModel.UiState,
    onOpenPath: (String) -> Unit,
    onDownload: (DirEntry) -> Unit,
    onForward: (DirEntry) -> Unit,
    onDelete: (DirEntry) -> Unit,
) {
    when {
        inboxState.errorText != null && inboxState.entries.isEmpty() -> item(key = "inbox-error") {
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 44.dp)
                    .padding(start = 38.dp, end = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "无法加载目录",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 13.sp,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { inboxVm.reload() }) { Text("重试", fontSize = 13.sp) }
            }
        }

        inboxState.loading && inboxState.entries.isEmpty() -> item(key = "inbox-loading") {
            TreeSpinnerRow(depth = 1)
        }

        else -> itemsIndexed(inboxVm.visibleRows, key = { _, r -> r.id }) { _, row ->
            InboxChildRow(
                row = row,
                inboxVm = inboxVm,
                inboxState = inboxState,
                onOpenPath = onOpenPath,
                onDownload = onDownload,
                onForward = onForward,
                onDelete = onDelete,
            )
        }
    }
}

/** 收件箱子行(树行同集:本机恒 [下载, 转发, 删除];选择态翻空集;长按
 * 0.4s 进选择;点按 = 选择态翻选/常态目录推入整页;iOS swipeableChildRow)。 */
@Composable
private fun InboxChildRow(
    row: BrowseRow,
    inboxVm: BrowseViewModel,
    inboxState: BrowseViewModel.UiState,
    onOpenPath: (String) -> Unit,
    onDownload: (DirEntry) -> Unit,
    onForward: (DirEntry) -> Unit,
    onDelete: (DirEntry) -> Unit,
) {
    val entry = row.entry
    val swipes = if (inboxState.isSelecting) {
        emptyList()
    } else {
        listOf(SynshuSelectionAction.Download, SynshuSelectionAction.Forward, SynshuSelectionAction.Delete)
    }
    SwipeRevealRow(
        actions = swipes.map { it.toTreeSwipeSpec() },
        onAction = { i ->
            when (swipes[i]) {
                SynshuSelectionAction.Download -> onDownload(entry)
                SynshuSelectionAction.Forward -> onForward(entry)
                SynshuSelectionAction.Delete -> onDelete(entry)
                else -> Unit
            }
        },
    ) {
        Box(
            Modifier.pointerInput(entry.path, inboxState.isSelecting) {
                detectTapGestures(
                    onLongPress = {
                        if (!inboxVm.state.value.isSelecting) inboxVm.enterSelection()
                    },
                )
            },
        ) {
            SynshuRow(
                name = entry.name,
                isDir = entry.isDir,
                depth = row.depth + 1,
                expanded = entry.path in inboxState.expandedPaths,
                selecting = inboxState.isSelecting,
                selected = entry.path in inboxState.selection,
                onOpen = {
                    if (inboxVm.state.value.isSelecting) {
                        inboxVm.toggle(entry.path)
                    } else if (entry.isDir) {
                        onOpenPath(entry.path)
                    } // 文件行点按无导航面(iOS 同构:文件行为叶)
                },
                onToggleExpand = if (entry.isDir) {
                    { inboxVm.toggleExpand(entry.path) }
                } else {
                    null
                },
                sub = entrySubtitle(entry.mtimeMs, entry.children?.toInt(), entry.size, entry.isDir),
            )
        }
    }
}

// ── 快捷行 + 成员行树(V3-3 §16.3 成员行形态;iOS FavoritesRootList 同构)──

/** 单枚快捷条目(行 + 展开成员区;滑集/选择态收口在此)。 */
@Composable
private fun FavoriteRowItem(
    fav: Favorite,
    state: FavoritesViewModel.UiState,
    vm: FavoritesViewModel,
    selecting: Boolean,
    onToggleExpand: () -> Unit,
    onManage: () -> Unit,
    onDeleteFav: () -> Unit,
    onOpenMember: (String, String) -> Unit,
    onOpenGroup: (String, String) -> Unit,
    onTreeDownload: (String, DirEntry) -> Unit,
    onTreeForward: (String, DirEntry) -> Unit,
    onTreeDelete: (TreeDeleteTarget) -> Unit,
) {
    val expanded = fav.id in state.expandedFavIds
    // 左滑 = [管理, 删除快捷];选择态/readOnly 翻空集(iOS favoriteRow 同构)
    val swipes: List<FavSwipeAction> =
        if (selecting || state.readOnly) emptyList() else FavSwipeAction.entries.toList()
    Column(Modifier.fillMaxWidth()) {
        SwipeRevealRow(
            actions = swipes.map { it.toSpec() },
            onAction = { i ->
                when (swipes[i]) {
                    FavSwipeAction.MANAGE -> onManage()
                    FavSwipeAction.DELETE -> onDeleteFav()
                }
            },
        ) {
            // 行点按 = iOS 语义进入(R2 G1):多成员 = 推快捷分组页(iOS 混合
            // 页安卓落形);单成员在线 = 推该成员目录整页(唯一目标,不进分
            // 组页);单成员离线/零成员 = 翻展开(诚实披露)。展开/进页两条
            // 路并存 = iOS 同构(chevron 展开成员树行为不变)
            DirNodeRow(
                title = fav.name,
                expanded = expanded,
                selecting = selecting,
                onToggleExpand = onToggleExpand,
                onOpen = {
                    when (val target = FavoritesViewModel.favRowTapTarget(fav, state.devices)) {
                        is FavoritesViewModel.FavTapRoute.Member -> onOpenMember(target.device, target.path)
                        is FavoritesViewModel.FavTapRoute.Group -> onOpenGroup(fav.id, fav.name)
                        null -> onToggleExpand()
                    }
                },
                caretVisible = fav.members.isNotEmpty(),
            )
        }
        if (expanded) {
            MemberTreeArea(
                fav = fav,
                state = state,
                vm = vm,
                selecting = selecting,
                onOpenMember = onOpenMember,
                onTreeDownload = onTreeDownload,
                onTreeForward = onTreeForward,
                onTreeDelete = onTreeDelete,
            )
        }
    }
}

/** 快捷树成员区(零成员 = 引导占位行;否则每成员一行 + 展开子级)。 */
@Composable
private fun MemberTreeArea(
    fav: Favorite,
    state: FavoritesViewModel.UiState,
    vm: FavoritesViewModel,
    selecting: Boolean,
    onOpenMember: (String, String) -> Unit,
    onTreeDownload: (String, DirEntry) -> Unit,
    onTreeForward: (String, DirEntry) -> Unit,
    onTreeDelete: (TreeDeleteTarget) -> Unit,
) {
    val members = vm.memberRows(fav)
    if (members.isEmpty()) {
        HintRow(
            text = "新建快捷目录,再把设备里的目录添加进来",
            modifier = Modifier
                .padding(start = 38.dp, end = 20.dp)
                .heightIn(min = 44.dp),
            small = true,
        )
        return
    }
    members.forEachIndexed { index, member ->
        val nodeId = FavoritesViewModel.nodeIdOf(fav.id, index, member.deviceId, member.path)
        MemberRowItem(
            fav = fav,
            member = member,
            nodeId = nodeId,
            state = state,
            vm = vm,
            selecting = selecting,
            onOpenMember = onOpenMember,
            onTreeDownload = onTreeDownload,
            onTreeForward = onTreeForward,
            onTreeDelete = onTreeDelete,
        )
        if (nodeId in state.expandedNodes) {
            FavoritesChildrenArea(
                fav = fav,
                device = member.deviceId,
                path = member.path,
                depth = 2,
                parentId = nodeId,
                state = state,
                vm = vm,
                selecting = selecting,
                onOpenMember = onOpenMember,
                onTreeDownload = onTreeDownload,
                onTreeForward = onTreeForward,
                onTreeDelete = onTreeDelete,
            )
        }
    }
}

/**
 * 成员行(§16.3):caret + 文件夹图标 + 「设备名 - 目录名」(设备名 15sp·600,
 * 「 - 目录名」12.5sp 灰,单行内联)。**离线整行灰显禁点禁展开**(行名灰显
 * 即披露);在线 = 点行推成员目录整页浏览,箭头 = 懒加载子目录;左滑 = 树
 * 行同集。
 */
@Composable
private fun MemberRowItem(
    fav: Favorite,
    member: FavoritesTree.MemberRow,
    nodeId: String,
    state: FavoritesViewModel.UiState,
    vm: FavoritesViewModel,
    selecting: Boolean,
    onOpenMember: (String, String) -> Unit,
    onTreeDownload: (String, DirEntry) -> Unit,
    onTreeForward: (String, DirEntry) -> Unit,
    onTreeDelete: (TreeDeleteTarget) -> Unit,
) {
    val offline = member.offline
    val expanded = nodeId in state.expandedNodes
    // 成员行删除刷新键 = 成员根缓存键自身(iOS memberKey 同构)
    val entry = DirEntry(
        name = FavoritesTree.dirName(member.path),
        path = member.path,
        size = 0,
        mtimeMs = 0,
        isDir = true,
        children = null,
    )
    val deviceLabel = FavoritesTree.deviceName(state.devices, member.deviceId)
    val tail = FavoritesTree.dirName(member.path)
    val swipes = treeRowSwipes(state, selecting, device = member.deviceId, online = !offline)
    SwipeRevealRow(
        modifier = Modifier.fillMaxWidth(),
        actions = swipes.map { it.toTreeSwipeSpec() },
        onAction = { i ->
            handleTreeSwipe(
                action = swipes[i],
                device = member.deviceId,
                entry = entry,
                refreshDevice = member.deviceId,
                refreshPath = member.path,
                onDownload = onTreeDownload,
                onForward = onTreeForward,
                onDelete = onTreeDelete,
            )
        },
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 58.dp)
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    enabled = !offline && !selecting,
                    onClickLabel = "$deviceLabel - $tail" + if (offline) ",离线" else "",
                ) { onOpenMember(member.deviceId, member.path) }
                .padding(start = 20.dp),
        ) {
            Row(
                Modifier.weight(1f),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                // 成员级缩进 1 级(iOS padding(.leading, indentPerLevel))
                Box(Modifier.padding(start = 18.dp)) {
                    RowCaret(
                        expanded = expanded,
                        visible = !offline,
                        onToggle = { vm.toggleDirNode(fav.id, nodeId, null, member.deviceId, member.path) },
                    )
                }
                FileTypeIcon(isDir = true)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        deviceLabel,
                        color = if (offline) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.onSurface
                        },
                        fontSize = 15.sp,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        " - $tail",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 12.5.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Spacer(Modifier.width(20.dp))
            }
        }
    }
}

/**
 * 成员节点子级行区(懒加载;§17.5 子级含文件——缓存存全量,渲染层滤隐藏,
 * 目录恒前;目录行可展开可导航,文件行 = 叶仅展示;空目录 = 「空文件夹」
 * 注记行;未缓存在飞 = 转圈行;不在飞无缓存不出——失败已收展开态)。
 */
@Composable
private fun FavoritesChildrenArea(
    fav: Favorite,
    device: String,
    path: String,
    depth: Int,
    parentId: String,
    state: FavoritesViewModel.UiState,
    vm: FavoritesViewModel,
    selecting: Boolean,
    onOpenMember: (String, String) -> Unit,
    onTreeDownload: (String, DirEntry) -> Unit,
    onTreeForward: (String, DirEntry) -> Unit,
    onTreeDelete: (TreeDeleteTarget) -> Unit,
) {
    val key = device + "\u0000" + path
    val children = state.memberChildren[key]
    when {
        children != null -> {
            val rows = FavoritesViewModel.favTreeChildren(children, state.showHidden)
            if (rows.isEmpty()) {
                // 空目录注记(Windows emptyDir 同词;iOS 「空文件夹」同构)
                HintRow(
                    text = "空文件夹",
                    modifier = Modifier
                        .padding(start = 20.dp + 18.dp * depth, end = 20.dp)
                        .heightIn(min = 44.dp),
                    small = true,
                )
            } else {
                rows.forEach { entry ->
                    if (entry.isDir) {
                        ChildDirRow(
                            fav = fav,
                            device = device,
                            parentPath = path,
                            parentId = parentId,
                            entry = entry,
                            depth = depth,
                            state = state,
                            vm = vm,
                            selecting = selecting,
                            onOpenMember = onOpenMember,
                            onTreeDownload = onTreeDownload,
                            onTreeForward = onTreeForward,
                            onTreeDelete = onTreeDelete,
                        )
                    } else {
                        ChildFileRow(
                            device = device,
                            parentPath = path,
                            parentId = parentId,
                            entry = entry,
                            depth = depth,
                            state = state,
                            selecting = selecting,
                            onTreeDownload = onTreeDownload,
                            onTreeForward = onTreeForward,
                            onTreeDelete = onTreeDelete,
                        )
                    }
                }
            }
        }

        key in state.loadingKeys -> TreeSpinnerRow(depth)

        else -> Unit // 不在飞无缓存不出(失败已收展开态,行不存在)
    }
}

/** 树子目录行(SynshuRow 复用):点行 = 推成员目录整页浏览;箭头 = 行内
 * 展开递归子级;左滑 = 树行同集;删除后刷新键 = 父节点缓存键。 */
@Composable
private fun ChildDirRow(
    fav: Favorite,
    device: String,
    parentPath: String,
    parentId: String,
    entry: DirEntry,
    depth: Int,
    state: FavoritesViewModel.UiState,
    vm: FavoritesViewModel,
    selecting: Boolean,
    onOpenMember: (String, String) -> Unit,
    onTreeDownload: (String, DirEntry) -> Unit,
    onTreeForward: (String, DirEntry) -> Unit,
    onTreeDelete: (TreeDeleteTarget) -> Unit,
) {
    val childId = FavoritesViewModel.childNodeIdOf(parentId, device, entry.path)
    val swipes = treeRowSwipes(state, selecting, device = device, online = true)
    SwipeRevealRow(
        modifier = Modifier.fillMaxWidth(),
        actions = swipes.map { it.toTreeSwipeSpec() },
        onAction = { i ->
            handleTreeSwipe(
                action = swipes[i],
                device = device,
                entry = entry,
                refreshDevice = device,
                refreshPath = parentPath,
                onDownload = onTreeDownload,
                onForward = onTreeForward,
                onDelete = onTreeDelete,
            )
        },
    ) {
        SynshuRow(
            name = entry.name,
            isDir = true,
            depth = depth,
            expanded = childId in state.expandedNodes,
            onOpen = {
                if (!selecting) onOpenMember(device, entry.path)
            },
            onToggleExpand = {
                vm.toggleDirNode(fav.id, childId, parentId, device, entry.path)
            },
            sub = entrySubtitle(entry.mtimeMs, entry.children?.toInt(), entry.size, entry.isDir),
        )
    }
}

/** 树文件行(§17.5:叶,无展开无点按导航——行面与混合页文件行同形;左滑 =
 * 树行同集;删除后刷新键 = 所在父目录缓存键)。 */
@Composable
private fun ChildFileRow(
    device: String,
    parentPath: String,
    parentId: String,
    entry: DirEntry,
    depth: Int,
    state: FavoritesViewModel.UiState,
    selecting: Boolean,
    onTreeDownload: (String, DirEntry) -> Unit,
    onTreeForward: (String, DirEntry) -> Unit,
    onTreeDelete: (TreeDeleteTarget) -> Unit,
) {
    val swipes = treeRowSwipes(state, selecting, device = device, online = true)
    SwipeRevealRow(
        modifier = Modifier.fillMaxWidth(),
        actions = swipes.map { it.toTreeSwipeSpec() },
        onAction = { i ->
            handleTreeSwipe(
                action = swipes[i],
                device = device,
                entry = entry,
                refreshDevice = device,
                refreshPath = parentPath,
                onDownload = onTreeDownload,
                onForward = onTreeForward,
                onDelete = onTreeDelete,
            )
        },
    ) {
        SynshuRow(
            name = entry.name,
            isDir = false,
            depth = depth,
            onOpen = {},
            sub = entrySubtitle(entry.mtimeMs, entry.children?.toInt(), entry.size, entry.isDir),
        )
    }
}

// ── 树行左滑动作集(§17.5/X7;iOS FavoritesRootList.treeRowSwipes 同构)────

/**
 * 树行左滑动作集(「混合页滑集同源」;文件行与目录行同集):[下载, 转发]
 * + 删除?(本机行恒可删;远端行须「允许删除远端文件」门开)。选择态/离线
 * 行翻空集即收(离线 = 灰显禁点禁展开同旨禁滑)。
 */
internal fun treeRowSwipes(
    state: FavoritesViewModel.UiState,
    selecting: Boolean,
    device: String,
    online: Boolean,
): List<SynshuSelectionAction> {
    if (!online || selecting) return emptyList()
    val swipes = mutableListOf(SynshuSelectionAction.Download, SynshuSelectionAction.Forward)
    if (localRowOf(state, device)) {
        swipes.add(SynshuSelectionAction.Delete) // 本机行不设门恒在(spec §4.2)
    } else if (state.allowRemoteDelete) {
        swipes.add(SynshuSelectionAction.Delete) // 远端行受「允许删除远端文件」门(gates 流,翻开关即时换)
    }
    return swipes
}

/** 行本机判定(VM 单源;快照缺该 id 保守按远端计)。 */
internal fun localRowOf(state: FavoritesViewModel.UiState, device: String): Boolean =
    device == state.selfDeviceId || device == FavoritesViewModel.SELF_ALIAS

/** 底栏动作字形 → 左滑钮规格(下载/转发/删除;删除红底,白图标同底栏;
 * 字形 = 单源表直取,G3)。 */
@Composable
internal fun SynshuSelectionAction.toTreeSwipeSpec(): SwipeActionSpec = SwipeActionSpec(
    label = label,
    tint = if (this == SynshuSelectionAction.Delete) SynshuSemantic.Fail else MaterialTheme.colorScheme.primary,
    glyph = { ActionGlyph(toGlyphKind(), Color.White) },
)

/** 树行左滑分发(§17.5;单行 = 单行集):下载直达、转发捕获进弹层、删除入
 * 确认(携刷新键 = 删除后父缓存现拉键)。 */
internal fun handleTreeSwipe(
    action: SynshuSelectionAction,
    device: String,
    entry: DirEntry,
    refreshDevice: String,
    refreshPath: String,
    onDownload: (String, DirEntry) -> Unit,
    onForward: (String, DirEntry) -> Unit,
    onDelete: (TreeDeleteTarget) -> Unit,
) {
    when (action) {
        SynshuSelectionAction.Download -> onDownload(device, entry)
        SynshuSelectionAction.Forward -> onForward(device, entry)
        SynshuSelectionAction.Delete -> onDelete(
            TreeDeleteTarget(
                device = device,
                path = entry.path,
                isDir = entry.isDir,
                refreshDevice = refreshDevice,
                refreshPath = refreshPath,
            ),
        )

        else -> Unit // 树行集无 share/favorite 面
    }
}

/** 树行左滑转发捕获(须携来源设备——转发分档 upload/remote_copy 按 source)。 */
internal data class TreeForwardPick(val device: String, val entry: DirEntry)

/** 树行左滑删除待执行档(确认后执行;携完成后父缓存现拉刷新键)。 */
internal data class TreeDeleteTarget(
    val device: String,
    val path: String,
    val isDir: Boolean,
    val refreshDevice: String,
    val refreshPath: String,
)

/** 快捷行左滑动作(声明序末位贴缘 = 删除快捷,破坏面最右;iOS 同构)。 */
private enum class FavSwipeAction(val label: String) {
    MANAGE("管理"),
    DELETE("删除快捷"),
}

/** 快捷行左滑钮规格(管理蓝/删除快捷红;字形白 = 单源表直取,iOS swipeSpec 同构)。 */
@Composable
private fun FavSwipeAction.toSpec(): SwipeActionSpec = SwipeActionSpec(
    label = label,
    tint = if (this == FavSwipeAction.DELETE) SynshuSemantic.Fail else MaterialTheme.colorScheme.primary,
    glyph = {
        when (this@toSpec) {
            FavSwipeAction.MANAGE -> ActionGlyph(ActionGlyphKind.MANAGE, Color.White)
            FavSwipeAction.DELETE -> ActionGlyph(ActionGlyphKind.DELETE, Color.White)
        }
    },
)

// ── 快捷管理屏(T17 面;iOS FavoriteManageSheet 同构)──────────────────────

/**
 * 快捷目录管理屏:持 id 每渲染经 store 查最新——他端改名/增删成员随
 * data_changed 秒级跟线,查无即自关(他端删除 = 解除链接)。顶部 = 快捷名
 * 内联输入 + 保存(busy/空名/未变更 禁);成员列表 = 设备名 + 路径 + 移除
 * (免二次确认;设备不在已拉到快照 = 「设备已移除」灰标,快照未及 = 名退化
 * id);底部 = 「删除快捷目录」破坏钮(确认居中 alert,解除链接文案);
 * busy(store 在途编辑拒重入的 UI 半边)内容面全局禁点。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FavoriteManageSheet(
    favId: String,
    vm: FavoritesViewModel,
    onDismiss: () -> Unit,
) {
    val state by vm.state.collectAsState()
    val listScope = rememberCoroutineScope()
    val fav = state.favorites.firstOrNull { it.id == favId }

    // 查无即自关(推入窗内他端已删;sheet 持 id 查最新,iOS 同构)
    LaunchedEffect(fav) { if (fav == null) onDismiss() }

    // 编辑框内容(开屏 = 当前名;名真变更回写——无关整单更新如成员增删)
    var nameText by remember(favId) { mutableStateOf(fav?.name ?: "") }
    var committedName by remember(favId) { mutableStateOf(fav?.name) }
    LaunchedEffect(fav?.name) {
        val current = fav?.name
        if (current != null && committedName != current) {
            committedName = current
            nameText = current
        }
    }
    var confirmDelete by remember { mutableStateOf(false) }

    if (fav == null) return
    val trimmed = nameText.trim()

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        dragHandle = null, // 自绘把手(DevicePickerSheet 同语言)
        shape = RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp),
        containerColor = MaterialTheme.colorScheme.surface,
        contentWindowInsets = { WindowInsets(0, 0, 0, 0) },
    ) {
        Column(Modifier.fillMaxWidth().padding(bottom = 26.dp)) {
            // 自绘把手(DevicePickerSheet 同语言 36×5;T16 台账 T10 补画,
            // dragHandle = null 注释自此不失实)
            Box(
                Modifier
                    .align(Alignment.CenterHorizontally)
                    .padding(top = 8.dp)
                    .size(width = 36.dp, height = 5.dp)
                    .background(Color(0xFFD1D1D6), RoundedCornerShape(2.5.dp)),
            )
            // 头部(标题居中 + 完成右;iOS header 同语言)
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(start = 20.dp, end = 20.dp, top = 16.dp, bottom = 10.dp),
            ) {
                Text(
                    "管理快捷目录",
                    Modifier.align(Alignment.Center),
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                )
                Text(
                    "完成",
                    Modifier
                        .align(Alignment.CenterEnd)
                        .clickable(onClickLabel = "完成", onClick = onDismiss),
                    color = MaterialTheme.colorScheme.primary,
                    fontSize = 17.sp,
                )
            }
            // busy 全局禁点(store 在途编辑拒重入的 UI 半边;完成在门外)
            Column(
                Modifier
                    .fillMaxWidth()
                    .heightIn(max = 420.dp)
                    .verticalScroll(rememberScrollState()),
            ) {
                // 快捷名(内联输入 + 保存;busy/空名/未变更 禁)
                Column(Modifier.padding(horizontal = 20.dp, vertical = 6.dp)) {
                    Text(
                        "快捷名",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                    )
                    Row(
                        Modifier.padding(top = 8.dp).fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        OutlinedTextField(
                            value = nameText,
                            onValueChange = { nameText = it },
                            placeholder = { Text("名称") },
                            singleLine = true,
                            modifier = Modifier.weight(1f),
                        )
                        Text(
                            "保存",
                            Modifier.clickable(
                                enabled = !state.busy && trimmed.isNotEmpty() && trimmed != fav.name,
                                onClickLabel = "保存",
                            ) {
                                listScope.launch { vm.renameFavorite(fav.id, trimmed) }
                            },
                            color = MaterialTheme.colorScheme.primary,
                            fontSize = 17.sp,
                        )
                    }
                }
                // 成员列表(设备名 + 路径 + 移除;免二次确认)
                if (fav.members.isEmpty()) {
                    Text(
                        "暂无成员,到设备浏览中添加目录",
                        Modifier.padding(horizontal = 20.dp, vertical = 14.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                    )
                } else {
                    fav.members.forEach { member ->
                        ManageMemberRow(
                            member = member,
                            state = state,
                            onRemove = { listScope.launch { vm.removeMember(fav.id, member) } },
                        )
                    }
                }
            }
            // 删除快捷目录(底部 destructive;确认居中 alert)
            Text(
                "删除快捷目录",
                Modifier
                    .fillMaxWidth()
                    .clickable(enabled = !state.busy, onClickLabel = "删除快捷目录") { confirmDelete = true }
                    .padding(vertical = 14.dp),
                color = SynshuSemantic.Fail,
                fontSize = 17.sp,
                textAlign = TextAlign.Center,
            )
        }
    }

    // 删除确认(spec §3.1 解除链接文案;根列表同款逐字)
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("删除快捷目录") },
            text = { Text("只解除链接,不删除任何文件。删除 ${fav.name}?") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    listScope.launch {
                        if (vm.deleteFavorite(fav.id)) onDismiss()
                    }
                }) {
                    Text("删除", color = SynshuSemantic.Fail)
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmDelete = false }) { Text("取消") }
            },
        )
    }
}

/** 管理屏成员行:设备名(快照未拉到 = 退化 id 不诬「已移除」;已拉到而无此
 * 设备 = 「设备已移除」灰标)+ 路径原串小字灰 + 「移除」钮(直接执行)。 */
@Composable
private fun ManageMemberRow(
    member: FavMember,
    state: FavoritesViewModel.UiState,
    onRemove: () -> Unit,
) {
    val known = state.devices.firstOrNull { it.deviceId == member.device }
    Box(Modifier.fillMaxWidth().heightIn(min = 56.dp)) {
        Row(
            Modifier
                .fillMaxSize()
                .padding(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                when {
                    known != null -> Text(
                        known.name ?: known.deviceId,
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 16.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )

                    state.devicesLoaded -> Text(
                        "设备已移除",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                    )

                    else -> Text(
                        member.device,
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 16.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Text(
                    member.path,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.5.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                "移除",
                Modifier.clickable(enabled = !state.busy, onClickLabel = "移除", onClick = onRemove),
                color = MaterialTheme.colorScheme.primary,
                fontSize = 15.sp,
            )
        }
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = 20.dp)
                .fillMaxWidth()
                .height(0.5.dp)
                .background(MaterialTheme.colorScheme.outlineVariant),
        )
    }
}

// ── 「添加到快捷目录」弹层(V3-3 T18;iOS FavoritePickerSheet 同构)────────

/**
 * 「加入快捷」选择弹层(远端浏览「快捷」动作接线面,T7 的 onAddFavorite
 * 挂点):远端浏览目录行左滑「快捷」(单条)与选择底栏「快捷」(纯目录
 * 多选)统一入口。
 * - 清单 = 快捷条目(名 + 「N 个成员」灰注);点选 → 行内预去重(全重复
 *   直接「该目录已在其中」收口,不发回写)→ addMembers → 实际新增>0 =
 *   toast「已加入 {名}」;=0 = 失败 alert 留驻可重试。成功/已在其中 =
 *   toast 短读面后收弹层(0.9s);
 * - 空清单(非 readOnly)=「还没有快捷目录」+「新建快捷目录」行(alert
 *   空名禁提交)→ create → 认领取尾部 id(T14 创建序不变量)→ addMembers
 *   → toast → 收;
 * - readOnly(v>1 前向守卫)= 只读提示行,清单照渲不可加;busy/working 禁点
 *   (store 编辑重入门的 UI 半边 + 双击防线;T9 义务)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun FavoritePickerSheet(
    graph: Graph,
    entries: List<DirEntry>,
    sourceDevice: String,
    onDismiss: () -> Unit,
) {
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) { onDispose { scope.cancel() } }
    val vm = remember(graph) { FavoritesViewModel(graph.favorites, graph.repo, graph.hub, scope, graph.settings.gates) }
    val state by vm.state.collectAsState()
    val listScope = rememberCoroutineScope()

    var showCreate by remember { mutableStateOf(false) }
    var newName by remember { mutableStateOf("") }
    var toast by remember { mutableStateOf<String?>(null) }
    var working by remember { mutableStateOf(false) }
    LaunchedEffect(toast) {
        toast?.let {
            delay(2200)
            toast = null
        }
    }

    fun memberKey(m: FavMember): String = FavoritesViewModel.memberKeyOf(m)

    /** 点选加入:预去重(全重复收口不发回写)→ addMembers → toast → 收。 */
    fun addTo(fav: Favorite) {
        if (working || state.busy || state.readOnly) return
        val target = state.favorites.firstOrNull { it.id == fav.id } ?: run {
            onDismiss() // 他端删除随 data_changed 跟线,弹层照收
            return
        }
        val seen = HashSet(target.members.map { memberKey(it) })
        val fresh = entries.map { FavMember(device = sourceDevice, path = it.path) }
            .filter { seen.add(memberKey(it)) }
        listScope.launch {
            if (fresh.isEmpty()) {
                toast = "该目录已在其中"
                delay(900)
                onDismiss()
                return@launch
            }
            working = true
            val added = vm.addMembers(target.id, fresh)
            working = false
            if (added > 0) {
                toast = "已加入 ${target.name}"
                delay(900)
                onDismiss()
            } // =0:失败 alert 留驻可重试(VM opError 已置)
        }
    }

    /** 新建即加(空清单入口):新建落库后认领尾部 id 直加(T14 创建序不变量
     * ;认领 = store snapshot 同步读,VM 面 [FavoritesViewModel.createAndAddMembers])。 */
    fun createAndAdd(name: String) {
        if (working || state.busy || state.readOnly) return
        val members = entries.map { FavMember(device = sourceDevice, path = it.path) }
        listScope.launch {
            working = true
            val added = vm.createAndAddMembers(name, members)
            working = false
            if (added > 0) {
                // 新条目名 = store 尾条同步读(put 提交后即时,无投影窗)
                toast = "已加入 ${graph.favorites.snapshot().lastOrNull()?.name ?: name}"
                delay(900)
                onDismiss()
            } // 0:失败 alert 留驻可重试(VM opError 已置;新建成功后的成员写入
            // 失败不回滚新建——快捷已建成,可重开弹层再加,iOS 同口径)
        }
    }

    Box(Modifier.fillMaxWidth()) {
        ModalBottomSheet(
            onDismissRequest = onDismiss,
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            dragHandle = null,
            shape = RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp),
            containerColor = MaterialTheme.colorScheme.surface,
            contentWindowInsets = { WindowInsets(0, 0, 0, 0) },
        ) {
            Column(Modifier.fillMaxWidth().padding(bottom = 26.dp)) {
                // 自绘把手(DevicePickerSheet 同语言 36×5)
                Box(
                    Modifier
                        .align(Alignment.CenterHorizontally)
                        .padding(top = 8.dp)
                        .size(width = 36.dp, height = 5.dp)
                        .background(Color(0xFFD1D1D6), RoundedCornerShape(2.5.dp)),
                )
                // 头部(标题居中 + 完成;FavoriteManageSheet 头部同语言)
                Box(
                    Modifier
                        .fillMaxWidth()
                        .padding(start = 20.dp, end = 20.dp, top = 10.dp, bottom = 10.dp),
                ) {
                    Text(
                        "添加到快捷目录",
                        Modifier.align(Alignment.Center),
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                    )
                    Text(
                        "完成",
                        Modifier
                            .align(Alignment.CenterEnd)
                            .clickable(onClickLabel = "完成", onClick = onDismiss),
                        color = MaterialTheme.colorScheme.primary,
                        fontSize = 17.sp,
                    )
                }
                // 清单(readOnly 提示 + 空态新建入口;busy/working/readOnly 禁点)
                Column(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(max = 380.dp)
                        .verticalScroll(rememberScrollState()),
                ) {
                    if (state.readOnly) {
                        HintRow(
                            text = "快捷目录当前只读,暂不能添加",
                            modifier = Modifier.padding(horizontal = 20.dp, vertical = 14.dp),
                        )
                    }
                    if (state.favorites.isEmpty()) {
                        if (!state.readOnly) {
                            HintRow(
                                text = "还没有快捷目录",
                                modifier = Modifier.padding(horizontal = 20.dp, vertical = 14.dp),
                            )
                            // 空清单新建入口(整行 accent;Windows picker 同位)
                            Row(
                                Modifier
                                    .fillMaxWidth()
                                    .clickable(enabled = !working && !state.busy, onClickLabel = "新建快捷目录") {
                                        newName = ""
                                        showCreate = true
                                    }
                                    .padding(horizontal = 20.dp, vertical = 14.dp),
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                ActionGlyph(SynshuSelectionAction.Favorite, MaterialTheme.colorScheme.primary)
                                Text(
                                    "新建快捷目录",
                                    color = MaterialTheme.colorScheme.primary,
                                    fontSize = 17.sp,
                                )
                            }
                        }
                    } else {
                        state.favorites.forEach { fav ->
                            val enabled = !state.busy && !working && !state.readOnly
                            Box(
                                Modifier
                                    .fillMaxWidth()
                                    .heightIn(min = 56.dp)
                                    .clickable(
                                        enabled = enabled,
                                        onClickLabel = "${fav.name},${fav.members.size} 个成员",
                                    ) { addTo(fav) },
                            ) {
                                Row(
                                    Modifier
                                        .fillMaxSize()
                                        .padding(horizontal = 20.dp),
                                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    FileTypeIcon(isDir = true)
                                    Text(
                                        fav.name,
                                        color = MaterialTheme.colorScheme.onSurface,
                                        fontSize = 17.sp,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                        modifier = Modifier.weight(1f),
                                    )
                                    Text(
                                        "${fav.members.size} 个成员",
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        fontSize = 12.5.sp,
                                    )
                                }
                                Box(
                                    Modifier
                                        .align(Alignment.BottomStart)
                                        .padding(start = 20.dp)
                                        .fillMaxWidth()
                                        .height(0.5.dp)
                                        .background(MaterialTheme.colorScheme.outlineVariant),
                                )
                            }
                        }
                    }
                }
            }
        }
        // 结果反馈胶囊(「已加入 {名}」/「该目录已在其中」;短读面后收弹层)
        toast?.let { text ->
            ToastPill(text, Modifier.align(Alignment.BottomCenter).padding(bottom = 40.dp))
        }
    }

    // 新建快捷目录(命名对话框;空名禁提交——钮 disabled,词面与副文逐字
    // 对齐 iOS FavoritePickerSheet 新建对话框,T17 首页同款)
    if (showCreate) {
        AlertDialog(
            onDismissRequest = { showCreate = false },
            title = { Text("新建快捷目录") },
            text = {
                Column {
                    OutlinedTextField(
                        value = newName,
                        onValueChange = { newName = it },
                        placeholder = { Text("名称") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Text(
                        "创建后可把设备里的目录添加进来",
                        Modifier.padding(top = 10.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        fontSize = 13.sp,
                    )
                }
            },
            confirmButton = {
                TextButton(
                    enabled = newName.trim().isNotEmpty(),
                    onClick = {
                        val name = newName.trim()
                        showCreate = false
                        createAndAdd(name)
                    },
                ) { Text("创建") }
            },
            dismissButton = {
                TextButton(onClick = { showCreate = false }) { Text("取消") }
            },
        )
    }

    // 操作失败(addMembers put 失败/只读守卫;store op 返值无明细,plain)
    state.opError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { vm.clearFeedback() },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { vm.clearFeedback() }) { Text("好") }
            },
        )
    }
}
