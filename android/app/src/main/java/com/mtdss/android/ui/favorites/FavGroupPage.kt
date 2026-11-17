package com.mtdss.android.ui.favorites

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.Graph
import com.mtdss.android.favorites.FavoritesTree
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ui.browse.DeleteRequest
import com.mtdss.android.ui.common.CenterTopBar
import com.mtdss.android.ui.common.DevicePickerSheet
import com.mtdss.android.ui.common.ExpandChevron
import com.mtdss.android.ui.common.SearchField
import com.mtdss.android.ui.common.SwipeRevealRow
import com.mtdss.android.ui.common.SynshuRow
import com.mtdss.android.ui.common.entrySubtitle
import com.mtdss.android.ui.theme.SynshuSemantic
import kotlinx.coroutines.delay

/**
 * 快捷分组页(R2 G1,快捷行点按多成员直推;iOS FavoritesMixedView 安卓
 * 落形,spec §16.1 分类恒定同词面):
 * - **标题 = 快捷名**(顶栏返回 = 弹回根页,无页内历史;名跟随 store 现
 *   值,路由档名仅兜底);搜索框 = 浏览模板同款(过滤当前已载内容,大小写
 *   不敏感);下拉刷新 = 全成员重拉(缓存命中也拉,SWR 再校验同口);
 * - **内容 = 按成员分节**(节序 = 成员添加序):节头 =「设备名 - 目录名」
 *   (§16.1 词面;44dp 行高,iOS 同度量),加载中…/无法访问 灰标注(iOS
 *   statusBadge 同词);节内 = 该成员目录列表(目录恒前 + 名称升序,文件
 *   行叶),行 = [SynshuRow] 同款;节内 truncated → 节尾「已截断」注记;
 *   **离线成员整节省略**(§14.1 废止注记后混合面同口径);节头点击 =
 *   收起/展开(R3 H2:iOS 混合页节头同构——收起节不渲染行、保留节头,
 *   右缘 ›/∨ 蓝箭头形随收起态;会话级,刷新/搜索不动);
 * - **行左滑 = 远端行 [下载, 转发] + 删除?(受「允许删除远端文件」门);
 *   本机行 [转发, 删除] 无下载面(V3-6 Task 22 同着陆页口径——Android
 *   local_copy 收件箱落点无解,哨兵被引擎预检拒「dir 非法」)**——与快捷
 *   树行同源单表([treeRowSwipes]/[handleTreeSwipe]);
 * - 目录行点按 = 钻取推成员目录整页浏览(文件行 = 叶,无点按面;iOS
 *   drill 同构);行内展开无(钻取替代,iOS 同构——静态箭头披露);
 * - 数据面 = [FavoritesViewModel] 组态(favfetch 渐进扇出:并行 list,
 *   应答逐成员落位,世代守卫;设备翻面软更新翻桶);快捷被删 → 弹回根页
 *   (解除链接不触碰文件,iOS 同构)。
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun FavGroupPage(
    graph: Graph,
    vm: FavoritesViewModel,
    favId: String,
    fallbackTitle: String,
    onBack: () -> Unit,
    onDrill: (String, String, String) -> Unit,
) {
    val state by vm.state.collectAsState()

    // 开页扇出(重开 = 缓存种子瞬时 + 静默再校验);离页收口(组态留 =
    // 弹回再进瞬时;被钻取顶掉同收口,返回重开扇出再校验)
    DisposableEffect(favId) {
        vm.openGroup(favId)
        onDispose { vm.closeGroup() }
    }
    // 快捷被删(他端解除链接随 store 整单跟线)→ 弹回根页(iOS 同构)。
    // 见过才算:VM 壳级新造/进程恢复窗内清单未及(空单)不弹也不空转——
    // 首见置位并补开页(恢复窗 openGroup 因清单未及早退,此处重试),其后
    // 缺席即弹(iOS followFavoriteChange 同旨)。
    var seenFav by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(state.favorites) {
        if (state.favorites.any { it.id == favId }) {
            seenFav = true
            if (vm.state.value.openGroupFavId != favId) vm.openGroup(favId)
        } else if (seenFav) {
            onBack()
        }
    }
    // 系统返回 = 弹回根页(本页无页内历史)
    BackHandler { onBack() }

    // 标题 = 快捷名跟随 store 现值(他端改名秒级跟线;路由档名兜底)
    val title = state.favorites.firstOrNull { it.id == favId }?.name ?: fallbackTitle
    val sections = state.groupStates[favId]?.let { states ->
        FavoritesViewModel.groupSections(states, state.groupSearchText, state.showHidden)
    }.orEmpty()
    // 收起节(R3 H2):节头保留、节内行不渲染;「N 项」尾注只数可见行
    // (收起节的行不进计数,与渲染面同口径)。
    val visibleCount = sections.sumOf { section ->
        if (vm.isGroupSectionCollapsed(favId, section.state.member)) 0 else section.rows.size
    }
    val anyPending = state.groupStates[favId]?.any { it.status == FavoritesViewModel.GroupMemberStatus.PENDING } == true

    // 转发捕获(行左滑;须携来源设备)/删除待确认档(根页树行同形)
    var forwardPicked by remember { mutableStateOf<TreeForwardPick?>(null) }
    var deleteTarget by remember { mutableStateOf<TreeDeleteTarget?>(null) }
    var deleteRequest by remember { mutableStateOf<DeleteRequest?>(null) }
    // 动作发起反馈胶囊(iOS toast 同构;VM 计数单源)
    var toast by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(state.startedCount) {
        state.startedCount?.let {
            toast = "已开始 $it 项 · 进度见传输列表"
            vm.clearActionFeedback()
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
            CenterTopBar(title = title, onBack = onBack)
            // 搜索框(浏览模板同款上下距;iOS searchBar top 4 / bottom 10,G2 度量)
            SearchField(
                text = state.groupSearchText,
                onTextChange = vm::setGroupSearchText,
                modifier = Modifier.padding(top = 4.dp, bottom = 10.dp),
            )
            when {
                // 首扇出空窗(种子全在途无行;iOS ProgressView 窗同构)
                anyPending && visibleCount == 0 -> Box(
                    Modifier.weight(1f).fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                else -> PullToRefreshBox(
                    isRefreshing = anyPending,
                    onRefresh = { vm.refreshGroup(favId) },
                    modifier = Modifier.weight(1f).fillMaxWidth(),
                ) {
                    LazyColumn(Modifier.fillMaxSize()) {
                        sections.forEach { section ->
                            val member = section.state.member
                            val deviceLabel = FavoritesTree.deviceName(state.devices, member.device)
                            val dirLabel = FavoritesTree.dirName(member.path)
                            // 节头收起/展开(R3 H2;iOS 混合页节头点击同构):
                            // 收起节 = 不渲染行、保留节头;刷新/搜索不动收起态
                            // (会话级收起集,VM 单源)
                            val collapsed = vm.isGroupSectionCollapsed(favId, member)
                            item(key = "head-${member.device}:${member.path}", contentType = "header") {
                                GroupSectionHeader(
                                    deviceLabel = deviceLabel,
                                    dirLabel = dirLabel,
                                    status = section.state.status,
                                    collapsed = collapsed,
                                    onToggleCollapse = { vm.toggleGroupSectionCollapsed(favId, member) },
                                )
                            }
                            if (!collapsed) {
                                items(section.rows, key = { "${member.device}:${it.path}" }) { entry ->
                                    GroupEntryRow(
                                        entry = entry,
                                        memberDevice = member.device,
                                        memberPath = member.path,
                                        state = state,
                                        onDownload = { device, e -> vm.downloadTreeRow(device, e.path) },
                                        onForward = { device, e -> forwardPicked = TreeForwardPick(device = device, entry = e) },
                                        onDelete = { target ->
                                            deleteTarget = target
                                            deleteRequest = DeleteRequest(
                                                paths = listOf(target.path),
                                                folders = if (target.isDir) 1 else 0,
                                            )
                                        },
                                        onDrillRow = { device, path -> onDrill(device, path, FavoritesTree.dirName(path)) },
                                    )
                                }
                                if (section.state.truncated) {
                                    item(key = "trunc-${member.device}:${member.path}", contentType = "note") {
                                        TruncatedNote()
                                    }
                                }
                            }
                        }
                        // 尾注「N 项」= 可见行(浏览模板同款;iOS X8 可见计数同构)
                        item(key = "item-count-footer") {
                            Text(
                                "$visibleCount 项",
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
        }

        // 动作发起反馈胶囊(底部悬浮,自动消隐)
        toast?.let { text ->
            ToastPill(text, Modifier.align(Alignment.BottomCenter).padding(bottom = 64.dp))
        }
    }

    // 树行删除确认(文件夹形文案点明整树语义;根页同款)
    deleteRequest?.let { request ->
        AlertDialog(
            onDismissRequest = {
                deleteRequest = null
                deleteTarget = null
            },
            title = { Text(request.alertTitle) },
            text = { Text(request.alertMessage) },
            confirmButton = {
                TextButton(onClick = {
                    deleteRequest = null
                    deleteTarget?.let { target ->
                        vm.deleteGroupRow(
                            favId = favId,
                            device = target.device,
                            path = target.path,
                            memberPath = target.refreshPath,
                        )
                    }
                    deleteTarget = null
                }) {
                    Text("删除", color = SynshuSemantic.Fail)
                }
            },
            dismissButton = {
                TextButton(onClick = {
                    deleteRequest = null
                    deleteTarget = null
                }) { Text("取消") }
            },
        )
    }

    // 动作失败(下载/转发/删除;iOS lastActionError alert 同构)
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

    // 行左滑转发弹层(X7;来源设备前置滤 from==to)
    forwardPicked?.let { picked ->
        DevicePickerSheet(
            graph = graph,
            title = "转发 1 项到…",
            excluding = picked.device,
            onPick = { target ->
                vm.forwardTreeRow(picked.device, picked.entry.path, target)
                forwardPicked = null
            },
        )
    }
}

/**
 * 分组节头(§16.1:44dp 行高、「设备名 - 目录名」词面;加载中…/无法访问
 * 灰标注;iOS statusBadge 同词;通栏发丝线,iOS FullBleedRow 同构)。
 * 整行可点 = 收起/展开节(R3 H2;iOS 混合页节头同构),右缘小箭头
 * ›/∨(蓝,形随收起态,0.2s 旋转与行展开钮同源件 [ExpandChevron])。
 */
@Composable
private fun GroupSectionHeader(
    deviceLabel: String,
    dirLabel: String,
    status: FavoritesViewModel.GroupMemberStatus,
    collapsed: Boolean,
    onToggleCollapse: () -> Unit,
) {
    val dim = status != FavoritesViewModel.GroupMemberStatus.OK
    Box(Modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 44.dp)
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    onClickLabel = if (collapsed) "展开" else "收起",
                    onClick = onToggleCollapse,
                )
                .padding(horizontal = 20.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                deviceLabel,
                color = if (dim) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                " - $dirLabel",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 12.5.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            val badge = when (status) {
                FavoritesViewModel.GroupMemberStatus.PENDING -> "加载中…"
                FavoritesViewModel.GroupMemberStatus.FAILED -> "无法访问"
                else -> null
            }
            badge?.let {
                Text(
                    it,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 12.5.sp,
                )
            }
            // 右缘小箭头(› = 收起态/∨ = 展开态;蓝,形随收起态)
            ExpandChevron(expanded = !collapsed)
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

/**
 * 分组节内行([SynshuRow] 同款;X7 左滑 = 树行同源单表):目录行点按 =
 * 钻取推成员目录整页,文件行 = 叶;删除后刷新键 = 所在成员根路径(节现拉)。
 */
@Composable
private fun GroupEntryRow(
    entry: DirEntry,
    memberDevice: String,
    memberPath: String,
    state: FavoritesViewModel.UiState,
    onDownload: (String, DirEntry) -> Unit,
    onForward: (String, DirEntry) -> Unit,
    onDelete: (TreeDeleteTarget) -> Unit,
    onDrillRow: (String, String) -> Unit,
) {
    val swipes = treeRowSwipes(state, selecting = false, device = memberDevice, online = true)
    SwipeRevealRow(
        modifier = Modifier.fillMaxWidth(),
        actions = swipes.map { it.toTreeSwipeSpec() },
        onAction = { i ->
            handleTreeSwipe(
                action = swipes[i],
                device = memberDevice,
                entry = entry,
                refreshDevice = memberDevice,
                refreshPath = memberPath,
                onDownload = onDownload,
                onForward = onForward,
                onDelete = onDelete,
            )
        },
    ) {
        SynshuRow(
            name = entry.name,
            isDir = entry.isDir,
            depth = 0,
            onOpen = {
                if (entry.isDir) onDrillRow(memberDevice, entry.path)
            }, // 文件行点按无导航面(叶;iOS 同构)
            onToggleExpand = null, // 行内展开无(钻取替代;iOS 混合页同构)
            sub = entrySubtitle(entry.mtimeMs, entry.children?.toInt(), entry.size, entry.isDir),
        )
    }
}

/** 节尾「已截断」注记(成员目录超出引擎上限;浏览模板尾注同语言)。 */
@Composable
private fun TruncatedNote() {
    Text(
        "已截断",
        Modifier
            .fillMaxWidth()
            .padding(top = 4.dp, bottom = 6.dp),
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontSize = 13.sp,
        textAlign = TextAlign.Center,
    )
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
