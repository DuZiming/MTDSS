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
 *   树行远端删除旗 = 门控单源 gates 流即时并入(T13 接设置开关)。
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
        /** 本机收件箱绝对路径(本机源组下载 local_copy 的 dir;会话投影)。 */
        val recvDir: String? = null,
        /** 动作已发起条数(一次性;UI 呈「已开始 N 项 · 进度见传输列表」)。 */
        val startedCount: Int? = null,
        /** 最近一次动作失败文案(下载/转发/删除;UI 弹「操作失败」后清)。 */
        val lastActionError: String? = null,
    )

    /** 根行投影项:收件箱置顶恒在 + 快捷行。 */
    sealed interface RootRow {
        /** 收件箱根行(恒在;展开/点入数据面 = 本机浏览,归屏面)。 */
        data object Inbox : RootRow

        /** 快捷条目行(创建序)。 */
        data class Fav(val favorite: Favorite) : RootRow
    }

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
        // event.devices(hub replay 流)活体跟随 → 树重算(T9 铺定消费面)
        scope.launch {
            hub.awaitSubscribed()
            hub.devices.collect { devs -> _state.value = _state.value.copy(devices = devs) }
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

    /** 注入收件箱绝对路径(会话投影;本机源组下载 local_copy 的 dir)。 */
    fun setRecvDir(path: String?) {
        _state.value = _state.value.copy(recvDir = path)
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
     * 树行下载(X7 口径:本机源 = local_copy dir=收件箱绝对路径——哨兵被
     * 引擎 local_copy 预检拒;远端源 = download dir=收件箱哨兵)。
     */
    fun downloadTreeRow(device: String, path: String) {
        val isLocal = isLocalRow(device)
        if (isLocal && _state.value.recvDir.isNullOrEmpty()) {
            _state.value = _state.value.copy(lastActionError = DOWNLOAD_START_FAIL)
            return
        }
        if (!starting.compareAndSet(false, true)) return
        scope.launch {
            try {
                val result = if (isLocal) {
                    repo.localCopy(listOf(path), _state.value.recvDir!!)
                } else {
                    repo.downloadToInbox(device, listOf(path))
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

        /** 新快捷条目 id:安全随机 8 字节 → 16 位小写 hex(iOS newId 同象)。 */
        private fun newFavId(): String {
            val bytes = ByteArray(8)
            SecureRandom().nextBytes(bytes)
            return bytes.joinToString("") { String.format("%02x", it) }
        }
    }
}
