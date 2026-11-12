package com.mtdss.android.ui.favorites

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.favorites.Favorite
import com.mtdss.android.favorites.FavMember
import com.mtdss.android.favorites.FavoritesStore
import com.mtdss.android.favorites.FavoritesTree
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ui.browse.BrowseGates
import com.mtdss.android.ui.browse.BrowseViewModel
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 快捷目录 tab 视图模型(V3-4 T10,spec §3-2;iOS FavoritesView
 * FavoritesRootList 消费面语义镜像,数据面 = T9 [FavoritesStore]/[FavoritesTree]):
 *
 * - **根行投影**([rootRows]):收件箱置顶**恒在**(行永远在位,iOS 根行
 *   同位;展开/点入数据面 = 本机浏览,归屏面),快捷行按创建序跟随清单;
 * - **成员行投影**([memberRows]):[FavoritesTree.members] devices 快照
 *   join——「设备名 - 目录名」+ 离线旗;快照 = 进页 repo.devices 现拉 +
 *   `event.devices`(hub replay 流)活体跟随,树随重算(未及窗恒保守
 *   离线,刷新归位);
 * - **成员展开状态机**(镜像 iOS FavoritesRootList「快捷树展开态」):
 *   快捷行展开 = 成员行现拼(零拉取);节点展开 = 乐观入集 + 懒加载
 *   (缓存键 device␁path 一次一拉,在飞吞点,应答落地守卫 fav 仍展开 /
 *   父链仍在展开,失败收回可再点重试,收起按引用释放缓存——跨快捷/成员
 *   根键仍引用则留);清单变更清理已删除条目的残留展开态与孤儿缓存;
 * - **编辑面**(iOS FavoritesStore 五 op 的 put transform 落形,T9 store
 *   单口):新建(空名禁提交,trim 后落名,尾部追加创建序,16hex id)/
 *   删除快捷 = 只解除链接(绝不 ctrl delete)/重命名/成员增删(memberKey
 *   终审去重,零新增不回写);全部 **busy 闸双击禁用**(T9 义务:store
 *   put 并发排队不拒,UI 层必须在途拒入)失败保旧值走 [UiState.opError];
 * - readOnly(v>1 前向守卫)编辑全拒(零 kvPut,store 守卫 + VM 入口
 *   双面);[UiState.showHidden] 树行隐藏过滤 + [UiState.allowRemoteDelete]
 *   树行远端删除旗 = 门控单源 gates 流即时并入(T13 接设置开关);
 * - **快捷分组页组态**(R2 G1,行点按多成员直推的页面数据面;iOS
 *   FavoritesMixedView/favfetch 渐进扇出同构):[openGroup] 种子 + 全量
 *   扇出(世代守卫,应答逐成员落位;离线成员不发请求),设备翻面软更新
 *   翻桶补发,搜索框过滤当前已载内容;节投影 = [groupSections](纯函数);
 *   节头收起/展开(R3 H2)= 会话级收起集([toggleGroupSectionCollapsed],
 *   刷新/搜索不动)。
 */
class FavoritesViewModel(
    private val store: FavoritesStore,
    private val repo: MtdssRepository,
    hub: MtdssEventHub,
    private val scope: CoroutineScope,
    /**
     * 门控单源流(设置页双开关状态流投影,SettingsViewModel.gates;缺省 =
     * 双门全关——测试与既有调用面零破坏):init 收集即时并入 UiState
     * ([UiState.showHidden] 树行过滤 + [UiState.allowRemoteDelete] 树行
     * 远端删除旗),翻开关即时换,免重拉(T13)。
     */
    gates: StateFlow<BrowseGates> = MutableStateFlow(BrowseGates.DEFAULT),
) {

    data class UiState(
        /** 快捷清单(store 投影;创建序)。 */
        val favorites: List<Favorite> = emptyList(),
        /** 前向守卫旗(store 投影;true = 编辑入口全禁,清单照渲)。 */
        val readOnly: Boolean = false,
        /** 编辑在途(新建/删除/重命名/成员;UI 双击禁用半边)。 */
        val busy: Boolean = false,
        /** 设备快照(成员行 join;进页现拉 + event.devices 跟随)。 */
        val devices: List<DirectoryDevice> = emptyList(),
        /** 设备快照已拉到过(管理面「设备已移除」灰标的判据;失败名退化)。 */
        val devicesLoaded: Boolean = false,
        /** 已展开的快捷条目(fav.id;收起 = 成员子树整片收拢)。 */
        val expandedFavIds: Set<String> = emptySet(),
        /** 已展开的树节点(键 = favId␁成员位次␁device␁path 起,下级续接
         * device␁path;乐观展开,失败/收起在飞应答不复活)。 */
        val expandedNodes: Set<String> = emptySet(),
        /** 该设备某路径子级全量缓存(键 = device␁path;渲染层滤隐藏)。 */
        val memberChildren: Map<String, List<DirEntry>> = emptyMap(),
        /** 拉取在飞(键 = device␁path;防重复,在飞点按吞掉)。 */
        val loadingKeys: Set<String> = emptySet(),
        /** 树行隐藏过滤(门控单源 gates 投影,默认滤 dotfiles;数据在列表仅滤显示)。 */
        val showHidden: Boolean = false,
        /** 允许删除远端文件(默认关;树行远端删除入口旗,门控单源 gates 投影
         * ——本机行删除不设门恒在,T13)。 */
        val allowRemoteDelete: Boolean = false,
        /** 快捷操作失败文案(新建/删除/重命名/成员;plain)。 */
        val opError: String? = null,
        /** 成功轻反馈(「已删除快捷目录」「已重命名」;一次性)。 */
        val toast: String? = null,
        /** 本机设备 id(树行本机判定;会话投影就位后由屏面注入)。 */
        val selfDeviceId: String? = null,
        /** 动作已发起条数(一次性;UI 呈「已开始 N 项 · 进度见传输列表」)。 */
        val startedCount: Int? = null,
        /** 最近一次动作失败文案(下载/转发/删除;UI 弹「操作失败」后清)。 */
        val lastActionError: String? = null,
        /** 分组页成员拉取态(键 = favId;离页状态留 = 重开瞬时种子,R2 G1)。 */
        val groupStates: Map<String, List<GroupMemberState>> = emptyMap(),
        /** 当前开着的分组页 favId(null = 无开页;设备翻面软更新窗)。 */
        val openGroupFavId: String? = null,
        /** 分组页搜索框(单开单值;开页即清)。 */
        val groupSearchText: String = "",
        /** 分组页收起节集(R3 H2;键 = favId␁memberKey,会话级进程内存——
         * 刷新/搜索/清单变更不动,收起节 = 页面不渲染行、保留节头)。 */
        val collapsedGroupSections: Set<String> = emptySet(),
    )

    /** 根行投影项:收件箱置顶恒在 + 快捷行。 */
    sealed interface RootRow {
        /** 收件箱根行(恒在;展开/点入数据面 = 本机浏览,归屏面)。 */
        data object Inbox : RootRow

        /** 快捷条目行(创建序)。 */
        data class Fav(val favorite: Favorite) : RootRow
    }

    /** 快捷行点按路由(R2 G1;iOS 语义 = 行点按进入,展开/进页两条路并存)。 */
    sealed interface FavTapRoute {
        /** 单成员在线直推目标(推该成员目录整页;device + path,设备 id 取
         * 成员原值——推入页按 id 浏览,与成员行点入同构)。 */
        data class Member(val device: String, val path: String) : FavTapRoute

        /** 多成员 → 快捷分组页(离线成员整节省略,页内如实呈现)。 */
        data object Group : FavTapRoute
    }

    /** 分组成员拉取状态机(favfetch MemberState 同象;iOS MixedViewModel 同构)。 */
    enum class GroupMemberStatus { PENDING, OK, FAILED, OFFLINE }

    /** 分组页单成员状态(entries/truncated 仅 OK 态承载;离线节渲染层整节省略)。 */
    data class GroupMemberState(
        val member: FavMember,
        val status: GroupMemberStatus,
        val entries: List<DirEntry> = emptyList(),
        val truncated: Boolean = false,
    )

    /** 分组页节投影(节序 = 成员添加序;rows = 节内可见行)。 */
    data class GroupSection(val state: GroupMemberState, val rows: List<DirEntry>)

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    /** 编辑在途闸(双击禁用;store put 并发排队不拒,拒入归本层)。 */
    private val busyGate = AtomicBoolean(false)

    init {
        scope.launch {
            store.favorites.collect { favs ->
                _state.value = pruneStale(_state.value, favs)
            }
        }
        scope.launch {
            store.readOnly.collect { v -> _state.value = _state.value.copy(readOnly = v) }
        }
        // 门控单源收集(设置页翻开关即时换;缓存存全量渲染层滤,免重拉)
        scope.launch {
            gates.collect { g ->
                _state.value = _state.value.copy(showHidden = g.showHidden, allowRemoteDelete = g.allowRemoteDelete)
            }
        }
        // 设备快照:进页现拉(失败保旧值——成员行恒保守离线,刷新归位)
        scope.launch { loadDevices() }
        // event.devices(hub replay 流)活体跟随 → 树重算(T9 铺定消费面);
        // 开页分组组态软更新(favfetch 翻桶同构,R2 G1)
        scope.launch {
            hub.awaitSubscribed()
            hub.devices.collect { devs ->
                _state.value = _state.value.copy(devices = devs)
                rebucketOpenGroup(devs)
            }
        }
    }

    // ── 投影 ────────────────────────────────────────────────────────────────

    /** 根行投影:收件箱置顶恒在 + 快捷行(创建序)。 */
    val rootRows: List<RootRow>
        get() = buildList {
            add(RootRow.Inbox)
            _state.value.favorites.forEach { add(RootRow.Fav(it)) }
        }

    /** 单快捷的成员行投影(devices join;行序 = 成员添加序)。 */
    fun memberRows(favorite: Favorite): List<FavoritesTree.MemberRow> =
        FavoritesTree.members(favorite, _state.value.devices)

    // ── 输入(会话投影/反馈;双门归门控单源 gates 流,T13)─────────────────

    /** 注入本机设备 id(会话投影;树行本机判定源)。 */
    fun setSelfDeviceId(id: String?) {
        _state.value = _state.value.copy(selfDeviceId = id)
    }

    /** 清一次性反馈(操作失败/成功轻反馈)。 */
    fun clearFeedback() {
        _state.value = _state.value.copy(opError = null, toast = null)
    }

    /** 清一次性成功轻反馈(读面停后;动作错误/计数另口)。 */
    fun clearToast() {
        _state.value = _state.value.copy(toast = null)
    }

    /** 清一次性动作反馈(动作失败/已发起计数)。 */
    fun clearActionFeedback() {
        _state.value = _state.value.copy(lastActionError = null, startedCount = null)
    }

    /** 重拉(store 整单 + 设备快照;下拉刷新/重进页;失败各自保旧值)。 */
    fun refresh() {
        scope.launch { store.refresh() }
        scope.launch { loadDevices() }
    }

    private suspend fun loadDevices() {
        repo.devices().fold(
            onSuccess = { r ->
                _state.value = _state.value.copy(devices = r.devices, devicesLoaded = true)
            },
            onFailure = { /* 保旧值;快照未及窗恒保守离线,刷新归位 */ },
        )
    }

    // ── 快捷树展开状态机 ─────────────────────────────────────────────────────

    /**
     * 快捷行箭头点按:展开 = 成员行现拼(零预拉,行即现);收起 = 成员子树
     * 整片收拢 + 缓存按引用释放(跨快捷共享仍引用则留)。
     */
    fun toggleFavoriteExpand(favId: String) {
        val s = _state.value
        if (favId in s.expandedFavIds) {
            collapseFavorite(favId)
        } else {
            _state.value = s.copy(expandedFavIds = s.expandedFavIds + favId)
        }
    }

    /**
     * 树节点箭头点按(成员节点/子目录节点同口):展开 = 乐观入集(缓存命中
     * 即时子行;未缓存即拉,应答落地守卫 fav 仍展开且父链仍在,失败收回
     * ——行仍在可再点重试);在飞点按吞掉;收起 = 出集 + 缓存按引用释放。
     */
    fun toggleDirNode(favId: String, nodeId: String, parentRowId: String?, device: String, path: String) {
        val key = device + NUL + path
        val s = _state.value
        if (nodeId in s.expandedNodes) {
            // 收起:出集 + 单键按引用释放(成员根键仍引用则留)
            val nodes = s.expandedNodes - nodeId
            _state.value = s.copy(
                expandedNodes = nodes,
                memberChildren = releaseUnreferenced(s.memberChildren, favorites = s.favorites, expandedFavIds = s.expandedFavIds, expandedNodes = nodes),
            )
            return
        }
        _state.value = s.copy(expandedNodes = s.expandedNodes + nodeId) // 乐观展开
        if (key in s.memberChildren || key in s.loadingKeys) return // 缓存命中即时 / 在飞吞点
        _state.value = _state.value.copy(loadingKeys = _state.value.loadingKeys + key)
        scope.launch {
            val result = repo.list(device, path).getOrNull()
            val cur = _state.value
            val loading = cur.loadingKeys - key
            when {
                // fav 已收起:展开态不复活,缓存不写
                favId !in cur.expandedFavIds ->
                    _state.value = cur.copy(loadingKeys = loading, expandedNodes = cur.expandedNodes - nodeId)
                result == null ->
                    // 失败:收回乐观展开(行仍在,可再点;错误不打断主面)
                    _state.value = cur.copy(loadingKeys = loading, expandedNodes = cur.expandedNodes - nodeId)
                else -> {
                    _state.value = cur.copy(
                        loadingKeys = loading,
                        memberChildren = cur.memberChildren + (key to result.entries),
                    )
                    // 父链收起在飞:子链展开态不复活(缓存已写,引用面收口)
                    if (parentRowId != null && parentRowId !in _state.value.expandedNodes) {
                        _state.value = _state.value.copy(expandedNodes = _state.value.expandedNodes - nodeId)
                    }
                }
            }
        }
    }

    /** 收起快捷(成员子树整片收拢):节点出集 + 引用键缓存释放。 */
    private fun collapseFavorite(favId: String) {
        val s = _state.value
        val collapsed = s.expandedNodes.filter { it.startsWith(favId + NUL) }
        val nodes = s.expandedNodes - collapsed.toSet()
        _state.value = s.copy(
            expandedFavIds = s.expandedFavIds - favId,
            expandedNodes = nodes,
            memberChildren = releaseUnreferenced(s.memberChildren, favorites = s.favorites, expandedFavIds = s.expandedFavIds - favId, expandedNodes = nodes),
        )
    }

    /** 清单变更收口:已删除条目的展开态随 id 移除 + 孤儿缓存释放。 */
    private fun pruneStale(s: UiState, favs: List<Favorite>): UiState {
        val liveIds = favs.map { it.id }.toSet()
        val staleIds = s.expandedFavIds - liveIds
        if (staleIds.isEmpty() && s.favorites === favs) return s
        val stalePrefixes = staleIds.map { it + NUL }
        val nodes = s.expandedNodes.filterNot { node -> stalePrefixes.any { node.startsWith(it) } }.toSet()
        return s.copy(
            favorites = favs,
            expandedFavIds = s.expandedFavIds - staleIds,
            expandedNodes = nodes,
            memberChildren = releaseUnreferenced(s.memberChildren, favorites = favs, expandedFavIds = s.expandedFavIds - staleIds, expandedNodes = nodes),
        )
    }

    /** 现存缓存键引用面(已展开快捷的成员根键 + 已展开节点的缓存键)。 */
    private fun releaseUnreferenced(
        children: Map<String, List<DirEntry>>,
        favorites: List<Favorite>,
        expandedFavIds: Set<String>,
        expandedNodes: Set<String>,
    ): Map<String, List<DirEntry>> {
        val referenced = referencedKeys(favorites, expandedFavIds, expandedNodes)
        return children.filterKeys { it in referenced }
    }

    private fun referencedKeys(
        favorites: List<Favorite>,
        expandedFavIds: Set<String>,
        expandedNodes: Set<String>,
    ): Set<String> {
        val keys = HashSet<String>()
        for (fav in favorites) {
            if (fav.id in expandedFavIds) {
                fav.members.forEach { keys.add(memberKeyOf(it)) }
            }
        }
        for (node in expandedNodes) {
            // 节点链身份末两段 = device␁path(路径不含 NUL)
            keys.add(node.split(NUL).takeLast(2).joinToString(NUL))
        }
        return keys
    }

    // ── 编辑面(iOS FavoritesStore 五 op 的 put transform 落形)──────────────

    /**
     * 新建快捷(命名对话框提交):空名禁提交(trim 后空 = 零请求);尾部
     * 追加(创建序),空成员合法,新条目 id = 客户端生成 16 hex。失败保旧
     * 值走 [UiState.opError]。
     */
    suspend fun createFavorite(name: String): Boolean {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return false
        if (!busyGate.compareAndSet(false, true)) return false
        _state.value = _state.value.copy(busy = true)
        try {
            val ok = store.put { cur ->
                cur + Favorite(id = newFavId(), name = trimmed, createdAt = System.currentTimeMillis(), members = emptyList())
            }
            if (!ok) _state.value = _state.value.copy(opError = OP_ERROR)
            return ok
        } finally {
            _state.value = _state.value.copy(busy = false)
            busyGate.set(false)
        }
    }

    /**
     * 删除快捷 = 解除链接(spec §3.1,绝不触碰远端文件;调用方须先确认):
     * put transform 删该条目,成功 toast。失败保旧值走 [UiState.opError]。
     */
    suspend fun deleteFavorite(id: String): Boolean {
        if (!busyGate.compareAndSet(false, true)) return false
        _state.value = _state.value.copy(busy = true)
        try {
            val ok = store.put { cur -> cur.filterNot { it.id == id } }
            if (ok) {
                _state.value = _state.value.copy(toast = "已删除快捷目录")
            } else {
                _state.value = _state.value.copy(opError = OP_ERROR)
            }
            return ok
        } finally {
            _state.value = _state.value.copy(busy = false)
            busyGate.set(false)
        }
    }

    /** 重命名:只改目标条目名;成功 toast「已重命名」。失败保旧值。 */
    suspend fun renameFavorite(id: String, name: String): Boolean {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return false
        if (!busyGate.compareAndSet(false, true)) return false
        _state.value = _state.value.copy(busy = true)
        try {
            val ok = store.put { cur ->
                cur.map { fav ->
                    if (fav.id == id) fav.copy(name = trimmed) else fav
                }
            }
            if (ok) {
                _state.value = _state.value.copy(toast = "已重命名")
            } else {
                _state.value = _state.value.copy(opError = OP_ERROR)
            }
            return ok
        } finally {
            _state.value = _state.value.copy(busy = false)
            busyGate.set(false)
        }
    }

    /**
     * 添加成员:目标内按 memberKey 去重(含入参数组内部重复),新成员尾部
     * 追加(添加序),返回实际新增数;零新增 = 不回写;put 失败返回 0。
     */
    suspend fun addMembers(id: String, members: List<FavMember>): Int {
        if (members.isEmpty()) return 0
        if (!busyGate.compareAndSet(false, true)) return 0
        _state.value = _state.value.copy(busy = true)
        try {
            val (ok, added) = addMembersLocked(id, members)
            if (!ok) _state.value = _state.value.copy(opError = OP_ERROR)
            return if (ok) added else 0
        } finally {
            _state.value = _state.value.copy(busy = false)
            busyGate.set(false)
        }
    }

    /**
     * 新建即加(picker 空清单入口;Windows T11 Final ruling 同源):新建落库
     * 后**认领取尾部 id** 直加成员,全程同一 busy 闸(闸中途不释放,新建与
     * 直加不可被并发编辑插入)。**认领 = [FavoritesStore.snapshot] 同步读**
     * ——T14 创建序不变量(create 恒尾部追加且 put 成功才落状态,create 返
     * true 后 snapshot 尾条必为本端新条目;snapshot 为 put 提交后的同步读,
     * 无 VM 投影窗——评审 fix round 1:state 流经收集器投影差一拍,同步续
     * 体读之恒旧值,空单路径静默丢失直加)。返回实际新增数(0 = 新建失败
     * 或成员写入失败,失败 alert 归 [UiState.opError];新建成功后的成员写
     * 入失败不回滚新建——快捷已建成,可重开弹层再加,iOS 同口径)。
     */
    suspend fun createAndAddMembers(name: String, members: List<FavMember>): Int {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return 0
        if (!busyGate.compareAndSet(false, true)) return 0
        _state.value = _state.value.copy(busy = true)
        try {
            // 新建(尾部追加,创建序;iOS create 同形)
            val created = store.put { cur ->
                cur + Favorite(id = newFavId(), name = trimmed, createdAt = System.currentTimeMillis(), members = emptyList())
            }
            if (!created) {
                _state.value = _state.value.copy(opError = OP_ERROR)
                return 0
            }
            val fresh = store.snapshot().lastOrNull() ?: run {
                _state.value = _state.value.copy(opError = OP_ERROR)
                return 0
            }
            val (ok, added) = addMembersLocked(fresh.id, members)
            if (!ok) _state.value = _state.value.copy(opError = OP_ERROR)
            return if (ok) added else 0
        } finally {
            _state.value = _state.value.copy(busy = false)
            busyGate.set(false)
        }
    }

    /** 成员直加内部形(闸由调用方持;返回 = 回写成功 + 实际新增数)。 */
    private suspend fun addMembersLocked(id: String, members: List<FavMember>): Pair<Boolean, Int> {
        if (members.isEmpty()) return false to 0
        var added = 0
        val ok = store.put { cur ->
            var addedLocal = 0
            val next = cur.map { fav ->
                if (fav.id != id) {
                    fav
                } else {
                    val seen = HashSet(fav.members.map { memberKeyOf(it) })
                    val fresh = members.filter { seen.add(memberKeyOf(it)) }
                    addedLocal += fresh.size
                    if (fresh.isEmpty()) fav else fav.copy(members = fav.members + fresh)
                }
            }
            added = addedLocal
            if (addedLocal == 0) null else next // 零新增 → 不回写
        }
        return ok to (if (ok) added else 0)
    }

    /** 移除成员(解除链接):只删目标内匹配 memberKey 的那一项。失败保旧值。 */
    suspend fun removeMember(id: String, member: FavMember): Boolean {
        if (!busyGate.compareAndSet(false, true)) return false
        _state.value = _state.value.copy(busy = true)
        try {
            val target = memberKeyOf(member)
            val ok = store.put { cur ->
                cur.map { fav ->
                    if (fav.id == id) fav.copy(members = fav.members.filter { memberKeyOf(it) != target }) else fav
                }
            }
            if (!ok) _state.value = _state.value.copy(opError = OP_ERROR)
            return ok
        } finally {
            _state.value = _state.value.copy(busy = false)
            busyGate.set(false)
        }
    }

    // ── 树行动作(§17.5/X7 口径;iOS FavoritesRootList 同源)────────────────

    /** 动作受理互斥(发起窗内再点吞掉;iOS sending guard 同构)。 */
    private val starting = AtomicBoolean(false)

    /** 行本机判定:设备 id = 本机 id,或 = 引擎 self 别名(ctrl 短路同口);
     * 其余保守按远端计。 */
    fun isLocalRow(device: String): Boolean {
        val s = _state.value
        return device == s.selfDeviceId || device == SELF_ALIAS
    }

    /**
     * 树行下载(V3-6 Task 22 收口,X7 口径):**远端源** = transfer.start
     * download,dir = 收件箱哨兵(着陆页 [BrowseViewModel.download] 同形,
     * paths = 树行原形 path——树/着陆同参零形变);**本机源行无引擎下载面**
     * ——Android 收件箱 = SAF 桥面非引擎路径,status.recv_dir 投影恒哨兵
     * (api.rs recv_dir_display android 臂),被引擎 local_copy absolute_form
     * 预检拒「dir 非法」,着陆页本机档(收件箱推入页/本机成员页)本就不出
     * 下载,树行滑集同口径收口([FavoritesScreen.treeRowSwipes] 本机行不出
     * [下载])——此守卫为 VM 单源双保险:误呼诚实失败,**不发必败 RPC**。
     */
    fun downloadTreeRow(device: String, path: String) {
        if (isLocalRow(device)) {
            _state.value = _state.value.copy(lastActionError = DOWNLOAD_START_FAIL)
            return
        }
        if (!starting.compareAndSet(false, true)) return
        scope.launch {
            try {
                repo.downloadToInbox(device, listOf(path)).fold(
                    onSuccess = { _state.value = _state.value.copy(startedCount = 1, lastActionError = null) },
                    onFailure = { e -> _state.value = _state.value.copy(lastActionError = BrowseViewModel.userErrorText(e)) },
                )
            } finally {
                starting.set(false)
            }
        }
    }

    /**
     * 树行转发(选设备弹层点选后):来源设备分档——本机源 = upload(数据自
     * 引擎侧本机路径出),远端源 = remote_copy 数据不经操作端;dir = 收件箱
     * 哨兵全平台口径(目标 = 来源设备自身由弹层前置滤除)。
     */
    fun forwardTreeRow(device: String, path: String, to: DirectoryDevice) {
        val isLocal = isLocalRow(device)
        if (!starting.compareAndSet(false, true)) return
        scope.launch {
            try {
                val result = if (isLocal) {
                    repo.uploadFiles(files = listOf(path), to = to.deviceId, dir = MtdssRepository.INBOX_DIR)
                } else {
                    repo.forward(from = device, paths = listOf(path), to = to.deviceId)
                }
                result.fold(
                    onSuccess = { _state.value = _state.value.copy(startedCount = 1, lastActionError = null) },
                    onFailure = { e -> _state.value = _state.value.copy(lastActionError = BrowseViewModel.userErrorText(e)) },
                )
            } finally {
                starting.set(false)
            }
        }
    }

    /**
     * 树行删除(确认后执行):ctrl delete force=true(引擎恒永久删);
     * 全成 = 父缓存键现拉刷新(行随缓存更新消失,失败保旧值零扰动);
     * 部分失败/调用失败 = [UiState.lastActionError]。
     */
    fun deleteTreeRow(device: String, path: String, refreshDevice: String, refreshPath: String) {
        scope.launch {
            repo.delete(device, listOf(path), force = true).fold(
                onSuccess = { result ->
                    if (result.failed.isNotEmpty()) {
                        _state.value = _state.value.copy(lastActionError = result.failed[0].message)
                    } else {
                        refreshListing(refreshDevice, refreshPath)
                    }
                },
                onFailure = { e ->
                    _state.value = _state.value.copy(lastActionError = BrowseViewModel.userErrorText(e))
                },
            )
        }
    }

    /** 单键现拉刷新(缓存命中也拉;失败保旧值——缓存不写,行面零变化)。 */
    fun refreshListing(device: String, path: String) {
        scope.launch {
            repo.list(device, path).fold(
                onSuccess = { r ->
                    _state.value = _state.value.copy(
                        memberChildren = _state.value.memberChildren + ((device + NUL + path) to r.entries),
                    )
                },
                onFailure = { /* 保旧值;行面零变化 */ },
            )
        }
    }

    /**
     * 下拉刷新跟随(iOS refreshFavTree 同构):已展开快捷的**在线成员键**
     * 全量现拉——缓存命中也重拉(刷新语义),失败保旧值(行不消失);离线
     * 成员零请求;在飞键去重。已展节点的缓存键一并现拉(成员轮在飞键天然
     * 跳过)。
     */
    fun refreshFavTree() {
        val s = _state.value
        val memberKeys = HashSet<String>()
        for (fav in s.favorites) {
            if (fav.id !in s.expandedFavIds) continue
            for (m in fav.members) {
                val key = memberKeyOf(m)
                memberKeys.add(key)
                if (!FavoritesTree.isOnline(s.devices, m.device) || key in s.loadingKeys) continue
                refreshListing(m.device, m.path)
            }
        }
        for (node in s.expandedNodes) {
            val parts = node.split(NUL)
            if (parts.size < 2) continue
            val key = parts.takeLast(2).joinToString(NUL)
            if (key in memberKeys || key in s.loadingKeys) continue // 成员轮在飞:跳过
            refreshListing(parts[parts.size - 2], parts.last())
        }
    }

    // ── 快捷分组页(R2 G1;iOS FavoritesMixedView/favfetch 渐进扇出同构)────

    /** 扇出世代(重开/刷新 +1;旧世代迟到应答整体丢弃,favfetch 同构)。 */
    private var groupGeneration = 0L

    /**
     * 开分组页:种子(缓存命中 = 瞬时整表,SWR 同构)+ 全量扇出(在线成员
     * 静默再校验);搜索框即清。
     */
    fun openGroup(favId: String) {
        val s = _state.value
        val fav = s.favorites.firstOrNull { it.id == favId } ?: return
        val states = s.groupStates[favId] ?: seedGroupStates(fav, s.devices)
        _state.value = s.copy(
            openGroupFavId = favId,
            groupSearchText = "",
            groupStates = s.groupStates + (favId to states),
        )
        fanoutGroup(favId)
    }

    /** 关分组页(组态留 = 重开瞬时;iOS 弹回同构)。 */
    fun closeGroup() {
        _state.value = _state.value.copy(openGroupFavId = null)
    }

    /** 分组页搜索框(过滤当前已载内容;沿浏览模板口径,大小写不敏感)。 */
    fun setGroupSearchText(text: String) {
        _state.value = _state.value.copy(groupSearchText = text)
    }

    /**
     * 分组页节头收起/展开(R3 H2;iOS 混合页节头点击同构):翻收起集——
     * 收起节页面只保留节头不渲染行。**会话级**(进程内存;持久化留候选),
     * 下拉刷新/搜索/清单变更均不动收起态(各收口 .copy 保字段)。
     */
    fun toggleGroupSectionCollapsed(favId: String, member: FavMember) {
        val key = favId + NUL + memberKeyOf(member)
        val s = _state.value
        _state.value = s.copy(
            collapsedGroupSections = if (key in s.collapsedGroupSections) {
                s.collapsedGroupSections - key
            } else {
                s.collapsedGroupSections + key
            },
        )
    }

    /** 节收起态查询(页面渲染门;键面同 [toggleGroupSectionCollapsed])。 */
    fun isGroupSectionCollapsed(favId: String, member: FavMember): Boolean =
        favId + NUL + memberKeyOf(member) in _state.value.collapsedGroupSections

    /** 下拉刷新:非离线成员全量重拉(缓存命中也拉,刷新语义),离线零请求。 */
    fun refreshGroup(favId: String) {
        fanoutGroup(favId)
    }

    /** 快照种子(序 = 成员添加序;离线判定 = devices 快照 join,保守离线)。 */
    private fun seedGroupStates(fav: Favorite, devices: List<DirectoryDevice>): List<GroupMemberState> =
        fav.members.map { m ->
            GroupMemberState(
                member = m,
                status = if (FavoritesTree.isOnline(devices, m.device)) {
                    GroupMemberStatus.PENDING
                } else {
                    GroupMemberStatus.OFFLINE
                },
            )
        }

    /**
     * 扇出(favfetch 同构):世代 +1;非离线成员并行 list,应答逐成员到达
     * 逐次落位(不等齐);FAILED 翻回 PENDING(重试);OK 成员行面不动、
     * 静默再校验;快照判离线的成员初始即离线桶,从不发 list。
     */
    private fun fanoutGroup(favId: String) {
        val gen = ++groupGeneration
        val s = _state.value
        val current = s.groupStates[favId] ?: return
        val next = current.map { st ->
            when {
                st.status == GroupMemberStatus.OFFLINE &&
                    FavoritesTree.isOnline(s.devices, st.member.device) -> st.copy(status = GroupMemberStatus.PENDING)
                st.status == GroupMemberStatus.FAILED -> st.copy(status = GroupMemberStatus.PENDING)
                else -> st
            }
        }
        _state.value = s.copy(groupStates = s.groupStates + (favId to next))
        next.forEach { st ->
            if (st.status != GroupMemberStatus.OFFLINE) fetchGroupMember(favId, st.member, gen)
        }
    }

    /** 单成员拉取落位:应答逐成员 upsert(世代过期丢弃;翻面离线后迟到应答
     * 不复活;再校验失败静默保旧不闪失败面)。 */
    private fun fetchGroupMember(favId: String, member: FavMember, gen: Long) {
        scope.launch {
            val result = repo.list(member.device, member.path).getOrNull()
            if (gen != groupGeneration) return@launch
            upsertGroupMember(favId, member) { st ->
                when {
                    result == null ->
                        if (st.status == GroupMemberStatus.PENDING) {
                            st.copy(status = GroupMemberStatus.FAILED, entries = emptyList(), truncated = false)
                        } else {
                            st // 再校验失败静默保旧
                        }

                    st.status == GroupMemberStatus.OFFLINE -> st // 翻面离线:迟到应答不复活
                    else -> st.copy(status = GroupMemberStatus.OK, entries = result.entries, truncated = result.truncated)
                }
            }
        }
    }

    /** 按成员键原位 upsert(保成员添加序;无该组状态零扰动)。 */
    private fun upsertGroupMember(
        favId: String,
        member: FavMember,
        transform: (GroupMemberState) -> GroupMemberState,
    ) {
        val s = _state.value
        val states = s.groupStates[favId] ?: return
        val key = memberKeyOf(member)
        _state.value = s.copy(
            groupStates = s.groupStates + (favId to states.map { if (memberKeyOf(it.member) == key) transform(it) else it }),
        )
    }

    /**
     * 设备快照翻面软更新(favfetch 同构;仅开页组):offline→pending 翻桶
     * 补发,pending→offline 翻桶(迟到应答经现状态守卫就地丢弃);已定成员
     * (ok/failed)不动、不重拉。
     */
    private fun rebucketOpenGroup(devices: List<DirectoryDevice>) {
        val favId = _state.value.openGroupFavId ?: return
        val states = _state.value.groupStates[favId] ?: return
        val gen = groupGeneration
        var changed = false
        val next = states.map { st ->
            val online = FavoritesTree.isOnline(devices, st.member.device)
            when {
                st.status == GroupMemberStatus.OFFLINE && online -> {
                    changed = true
                    fetchGroupMember(favId, st.member, gen)
                    st.copy(status = GroupMemberStatus.PENDING)
                }

                st.status == GroupMemberStatus.PENDING && !online -> {
                    changed = true
                    st.copy(status = GroupMemberStatus.OFFLINE, entries = emptyList(), truncated = false)
                }

                else -> st
            }
        }
        if (changed) _state.value = _state.value.copy(groupStates = _state.value.groupStates + (favId to next))
    }

    /**
     * 分组页行删除(确认后执行;ctrl delete force = 引擎恒永久删):全成 =
     * 该成员节现拉刷新 + 树缓存同键刷新(返回根页展开即位);部分失败/调用
     * 失败 = [UiState.lastActionError]。
     */
    fun deleteGroupRow(favId: String, device: String, path: String, memberPath: String) {
        scope.launch {
            repo.delete(device, listOf(path), force = true).fold(
                onSuccess = { result ->
                    if (result.failed.isNotEmpty()) {
                        _state.value = _state.value.copy(lastActionError = result.failed[0].message)
                    } else {
                        refreshListing(device, memberPath)
                        refreshGroupMember(favId, device, memberPath)
                    }
                },
                onFailure = { e ->
                    _state.value = _state.value.copy(lastActionError = BrowseViewModel.userErrorText(e))
                },
            )
        }
    }

    /** 分组页单成员节现拉(删除后刷新;失败保旧值——节面零变化)。 */
    fun refreshGroupMember(favId: String, device: String, path: String) {
        val gen = groupGeneration
        scope.launch {
            val result = repo.list(device, path).getOrNull() ?: return@launch
            upsertGroupMember(favId, FavMember(device = device, path = path)) { st ->
                st.copy(status = GroupMemberStatus.OK, entries = result.entries, truncated = result.truncated)
            }
        }
    }

    // ── 纯函数面(companion;宿主可测)───────────────────────────────────────

    companion object {

        /** memberKey 分隔符(NUL;防 device/path 拼接歧义,iOS 同源)。 */
        private const val NUL: String = "\u0000"

        /** 引擎 self 别名(ctrl 对本机短路,Contract SELF_DEVICE_ALIAS 同值)。 */
        const val SELF_ALIAS: String = "self"

        /** 快捷操作失败文案(store op 返 Bool 无明细面,plain;iOS 同词面)。 */
        const val OP_ERROR: String = "操作失败,请重试。"

        /** 本机源下载发起失败(recv_dir 未投影;iOS 同词面)。 */
        const val DOWNLOAD_START_FAIL: String = "下载发起失败,请重试。"

        /** 成员去重键(device␁path;iOS FavoritesStore.memberKey 同象)。 */
        fun memberKeyOf(m: FavMember): String = m.device + NUL + m.path

        /** 节点链身份前缀(favId 起;子级续接 device␁path)。 */
        fun nodeIdOf(favId: String, index: Int, device: String, path: String): String =
            listOf(favId, index.toString(), device, path).joinToString(NUL)

        fun childNodeIdOf(parentNodeId: String, device: String, path: String): String =
            "$parentNodeId$NUL$device$NUL$path"

        /**
         * 树子级投影(§17.5 子级含文件;目录恒前 + 名称大小写不敏感升序,
         * 隐藏项随 [showHidden] 滤——缓存存全量,渲染层滤,iOS 同裁定)。
         */
        fun favTreeChildren(entries: List<DirEntry>, showHidden: Boolean): List<DirEntry> {
            var list = entries
            if (!showHidden) list = list.filter { !it.name.startsWith(".") }
            return list.sortedWith { a, b ->
                when {
                    a.isDir != b.isDir -> if (a.isDir) -1 else 1
                    else -> a.name.compareTo(b.name, ignoreCase = true)
                }
            }
        }

        /**
         * 快捷行点按路由(R2 G1;iOS 语义 = 行点按进入,展开/进页两条路并存
         * = iOS 同构):多成员 = 推**快捷分组页**(iOS 混合页安卓落形,恒推
         * ——离线成员整节省略,页内如实呈现);单成员且在线 = 直推该成员目
         * 录整页(单成员有唯一目标,不进分组页);单成员离线/快照未及保守
         * 按离线 = null(翻展开见灰显行,禁进);零成员 = null(翻展开见
         * 引导行)。
         */
        fun favRowTapTarget(favorite: Favorite, devices: List<DirectoryDevice>): FavTapRoute? =
            when {
                favorite.members.size > 1 -> FavTapRoute.Group
                favorite.members.size == 1 -> {
                    val m = favorite.members.first()
                    if (FavoritesTree.isOnline(devices, m.device)) FavTapRoute.Member(m.device, m.path) else null
                }

                else -> null
            }

        /**
         * 分组页节投影(R2 G1;iOS §16.1 分类恒定/§15.6 搜索同构):节序 =
         * states 序(成员添加序);**离线成员整节省略**(§14.1 废止注记后
         * 混合面同口径);节内 = 隐藏门滤 + 搜索滤(大小写不敏感子串,只滤
         * 当前已载内容)+ 目录恒前排序;非 ok 节无行(节头单行,iOS 同形)。
         */
        fun groupSections(
            states: List<GroupMemberState>,
            searchText: String,
            showHidden: Boolean,
        ): List<GroupSection> =
            states.filter { it.status != GroupMemberStatus.OFFLINE }.map { st ->
                val rows = if (st.status == GroupMemberStatus.OK) {
                    favTreeChildren(st.entries, showHidden)
                        .filter { it.name.contains(searchText, ignoreCase = true) }
                } else {
                    emptyList()
                }
                GroupSection(st, rows)
            }

        /** 分组节头词面(「设备名 - 目录名」;§16.1 词面,FavoritesTree 单源 join)。 */
        fun groupSectionTitle(devices: List<DirectoryDevice>, member: FavMember): String =
            FavoritesTree.deviceName(devices, member.device) + " - " + FavoritesTree.dirName(member.path)

        /** 新快捷条目 id:安全随机 8 字节 → 16 位小写 hex(iOS newId 同象)。 */
        private fun newFavId(): String {
            val bytes = ByteArray(8)
            SecureRandom().nextBytes(bytes)
            return bytes.joinToString("") { String.format("%02x", it) }
        }
    }
}
