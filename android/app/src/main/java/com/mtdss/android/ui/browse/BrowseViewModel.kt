package com.mtdss.android.ui.browse

import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * 浏览页视图模型(iOS MTDSSKit BrowseViewModel 语义镜像,spec §3-4 远端浏览
 * 页/§4.1 浏览模板;远端/本地/成员目录通用——本地档 T10 复用同模板,动作
 * 集按浏览档出):
 * - **seq 守卫**:快速连续导航时过期响应丢弃(seq 单调,旧响应不得覆盖新
 *   位置;iOS/android 同款);
 * - **浏览器式历史**:返回/前进双箭头(spec §2.2 顶栏);open 截断前进
 *   分支追加历史,back/forward 只移动游标;
 * - **导航即时反馈**(R12 ②):open/goBack/goForward 标题先行 + 列表即刻
 *   清空进 loading(旧目录条目不得在拉取窗内残留);原地 reload 不清
 *   (下拉刷新/删除后重载保内容语义不变);
 * - 排序:文件夹恒前(所有字段),默认 名称 升序;日期 最新在前、大小
 *   大者在前、种类 按扩展名(iOS 文件 App 惯例);
 * - 隐藏项(dotfiles)默认过滤——「显示隐藏项」开关归 T11 设置,VM 只挂
 *   [UiState.showHidden] 输入;搜索框按名称过滤当前列表(只在根级,搜索
 *   起手收起全部展开、清空不恢复);
 * - **行内展开**(R3 #8):文件夹行箭头 = 行内展开([visibleRows] 树投影),
 *   行点击 = 进整页;一次展开一次拉取(缓存,收起释放),导航换页重置,
 *   选择跨层级(全选/收集含展开子级);
 * - **确认语义**(Ruling O):confirmSelection = **只退出选择模式**,无论
 *   选中与否、无副作用——动作仅自底栏/左滑发起(先捕获 selectedEntries
 *   再退出)。
 */
class BrowseViewModel(
    /** 浏览目标设备(本机浏览 = 本机 id「self」,远端 = 对端 id;iOS 同构)。 */
    val deviceId: String,
    /** 浏览树根(远端 = "" 卷列表,契约 list path 空串语义;本地 = recv_dir)。 */
    val rootPath: String,
    /** 根目录标题(远端 = 设备名;本地 = 收件箱等)。 */
    val rootTitle: String,
    /** 对端设备名(远端标题 = 「设备名 | 目录名」;本地 null = 单段口径)。 */
    val deviceName: String?,
    private val repo: MtdssRepository,
    private val scope: CoroutineScope,
) {

    data class UiState(
        /** 当前目录路径(初始即 rootPath,标题先行就位)。 */
        val path: String,
        /** daemon 原始列表(未滤;可见面走 [BrowseViewModel.visibleEntries])。 */
        val entries: List<DirEntry> = emptyList(),
        /** 目录超出引擎上限(10000 项)被截断(尾注「N 项(已截断)」)。 */
        val truncated: Boolean = false,
        val loading: Boolean = false,
        val errorText: String? = null,
        /** 搜索框:按名称过滤当前列表(空 = 不过滤)。 */
        val searchText: String = "",
        /** 排序(默认 名称 升序;文件夹恒前)。 */
        val sort: BrowseSort = BrowseSort.NAME,
        /** 隐藏项显示开关(默认过滤 dotfiles;T11 设置页接线)。 */
        val showHidden: Boolean = false,
        /** 选择模式开关(顶栏换 全选/N 项/蓝圈确认,壳藏胶囊底栏)。 */
        val isSelecting: Boolean = false,
        /** 选中路径集(行渲染选中态;键 = entry.path)。 */
        val selection: Set<String> = emptySet(),
        /** 展开目录集(键 = 绝对路径;页级状态,导航即重置)。 */
        val expandedPaths: Set<String> = emptySet(),
        /** 浏览器式历史游标面(双箭头可用态)。 */
        val canGoBack: Boolean = false,
        val canGoForward: Boolean = false,
        /** 最近一次动作失败文案(删除/下载/转发等;UI 弹「操作失败」后清)。 */
        val lastActionError: String? = null,
        /** 删除完成提示(部分失败明细;全成静默;UI 弹「删除完成」后清)。 */
        val deleteNotice: String? = null,
        /** 动作已发起条数(一次性;UI 呈「已开始 N 项 · 进度见传输列表」)。 */
        val startedCount: Int? = null,
    )

    private val _state = MutableStateFlow(UiState(path = rootPath))

    val state: StateFlow<UiState> = _state.asStateFlow()

    private val seq = AtomicLong()

    /** 浏览器式历史(游标制;open 截断前进分支)。 */
    private var history = listOf(rootPath)
    private var historyIndex = 0

    // ── 行内展开(R3 #8:一次展开一次拉取,缓存收起释放)────────────────────

    /** 子项缓存(键 = 绝对路径;页生命周期内复用,收起释放,导航重置一并清)。 */
    private val childrenCache = HashMap<String, List<DirEntry>>()
    /** 子项拉取在飞(防重复;同路径再点吞掉)。 */
    private val loadingChildren = HashSet<String>()
    /** 在飞子项拉取仍被需要的路径(单路径收起后迟到应答不得复活展开)。 */
    private val wantedChildren = HashSet<String>()
    /** 展开/收起世代(在飞子项拉取的作废面;搜索收起/导航重置 +1)。 */
    private var expandGeneration = 0

    // ── 派生面 ──────────────────────────────────────────────────────────────

    /** 顶栏标题(浏览根 = rootTitle;深于根且 deviceName 在场 = 「设备名 | 目录名」)。 */
    val title: String
        get() {
            val s = _state.value
            return composeTitle(rootTitle, s.path, rootPath, deviceName)
        }

    /** 可见条目 = 隐藏过滤 → 搜索过滤 → 排序(文件夹恒前)。 */
    val visibleEntries: List<DirEntry>
        get() {
            val s = _state.value
            return applyFilters(s.entries, s.searchText, s.showHidden, s.sort)
        }

    /**
     * 行内展开投影 = 根可见行 + 逐层展开子树(每层走同一 隐藏过滤 → 排序
     * 管线,depth = 父 +1;搜索过滤只在根级——搜索已收起全部展开,子级不
     * 投影)。
     */
    val visibleRows: List<BrowseRow>
        get() {
            val s = _state.value
            val rows = ArrayList<BrowseRow>()
            fun walk(list: List<DirEntry>, depth: Int) {
                var leveled = levelEntries(list, s.showHidden, s.sort)
                if (depth == 0 && s.searchText.isNotEmpty()) {
                    leveled = leveled.filter { it.name.contains(s.searchText, ignoreCase = true) }
                }
                for (entry in leveled) {
                    rows.add(BrowseRow(entry, depth))
                    if (entry.isDir && s.searchText.isEmpty() && entry.path in s.expandedPaths) {
                        walk(childrenCache[entry.path] ?: emptyList(), depth + 1)
                    }
                }
            }
            walk(s.entries, 0)
            return rows
        }

    /** 底部计数「N 项」(可见面)。 */
    val itemCount: Int get() = visibleEntries.size

    /** 单层可见化(展开子级同管线;搜索只在根级滤——子级仅在无搜索时投影)。 */
    private fun levelEntries(list: List<DirEntry>, showHidden: Boolean, sort: BrowseSort): List<DirEntry> {
        var l = list
        if (!showHidden) l = l.filter { !it.name.startsWith(".") }
        return sortEntries(l, sort)
    }

    /** 选中条目(可见行序——含展开子级;取消勾选/已不可见项不收)。 */
    val selectedEntries: List<DirEntry>
        get() {
            val s = _state.value
            return visibleRows.map { it.entry }.filter { it.path in s.selection }
        }

    // ── UI 输入(搜索/排序/隐藏)────────────────────────────────────────────

    /**
     * 搜索起手 = 收起全部行内展开(R3 #8 裁定;清空不恢复)——无条件收口:
     * 即使展开集暂空,也要作废在飞子项拉取(R3 评审 I-1:防迟到应答复活展开)。
     */
    fun setSearchText(text: String) {
        val old = _state.value.searchText
        if (text == old) return
        if (text.isNotEmpty()) collapseAll()
        _state.value = _state.value.copy(searchText = text)
    }

    fun setSort(sort: BrowseSort) {
        _state.value = _state.value.copy(sort = sort)
    }

    fun setShowHidden(show: Boolean) {
        _state.value = _state.value.copy(showHidden = show)
    }

    // ── 选择模式(spec §4.2;Ruling O)───────────────────────────────────────

    /** 进入选择模式(···菜单「选择」/行长按两入口同落此)。 */
    fun enterSelection() {
        _state.value = _state.value.copy(isSelecting = true, selection = emptySet())
    }

    /** 退出选择模式(取消/完成后的收尾;选择集清空)。 */
    fun exitSelection() {
        _state.value = _state.value.copy(isSelecting = false, selection = emptySet())
    }

    /** 蓝圈确认(Ruling O):**只 = 退出选择模式**,无论选中与否、无副作用。 */
    fun confirmSelection() {
        exitSelection()
    }

    /** 行点按切换选中态(选择模式下点按不再打开目录)。 */
    fun toggle(path: String) {
        val s = _state.value
        _state.value = s.copy(
            selection = if (path in s.selection) s.selection - path else s.selection + path,
        )
    }

    /** 全选(可见行集——含展开子级,R3 #8 选择跨层级);已全选再点 = 清空。 */
    fun selectAll() {
        val all = visibleRows.map { it.entry.path }
        val s = _state.value
        if (all.isNotEmpty() && all.all { it in s.selection }) {
            _state.value = s.copy(selection = emptySet())
        } else {
            _state.value = s.copy(selection = all.toSet())
        }
    }

    // ── 导航(浏览器式历史)─────────────────────────────────────────────────

    /**
     * 打开目录(导航入口;历史截断前进分支后追加;标题先行——失败/迟到
     * 应答由 seq 收口)。换页 = 展开态重置 + 搜索清空 + 列表即刻清空进
     * loading(R12 ②;原地 reload 不在此列,下拉刷新保内容语义不变)。
     */
    fun open(target: String) {
        val mySeq = seq.incrementAndGet()
        if (target != history[historyIndex]) {
            val truncated = history.subList(0, historyIndex + 1) + target
            history = truncated
            historyIndex = history.size - 1
        }
        syncNavFlags()
        // 标题先行(iOS open 同构:path 同步就位,列表即刻清空进 loading)
        _state.value = _state.value.copy(path = target, searchText = "")
        collapseAll()
        clearListForNavigation()
        fetch(target, mySeq)
    }

    /** 初次加载浏览根(收件箱/远端卷列表)。 */
    fun loadRoot() {
        open(rootPath)
    }

    /** 返回(游标左移;已在浏览根为空操作——弹回标签根由 UI 层承担)。 */
    fun goBack() {
        if (historyIndex <= 0) return
        val mySeq = seq.incrementAndGet()
        historyIndex -= 1
        syncNavFlags()
        _state.value = _state.value.copy(path = history[historyIndex], searchText = "")
        collapseAll()
        clearListForNavigation()
        fetch(history[historyIndex], mySeq)
    }

    /** 前进(游标右移;返回后可再进入)。 */
    fun goForward() {
        if (historyIndex + 1 >= history.size) return
        val mySeq = seq.incrementAndGet()
        historyIndex += 1
        syncNavFlags()
        _state.value = _state.value.copy(path = history[historyIndex], searchText = "")
        collapseAll()
        clearListForNavigation()
        fetch(history[historyIndex], mySeq)
    }

    /**
     * 原地重载当前目录(错误重试/下拉刷新/删除后重载;不动历史)。成功后
     * 已展开子级缓存随后刷新(删除等原地重载不收起展开态,过期子项不残留)。
     * **不清列表**(R12 ②:reload = 原地刷新语义)。
     */
    fun reload() {
        val mySeq = seq.incrementAndGet()
        scope.launch {
            fetchSuspend(history[historyIndex], mySeq)
            if (mySeq == seq.get() && _state.value.errorText == null) {
                refreshExpandedChildren(mySeq)
            }
        }
    }

    // ── 行内展开 ────────────────────────────────────────────────────────────

    /**
     * 展开/收起目录行。展开 = list 拉子项一次并缓存(已有缓存即时展开);
     * 收起 = 出展开集并释放缓存。拉取失败 = 不展开(行仍在,可再点;错误
     * 不打断浏览主面)。在飞拉取遇导航(seq)/收起(世代/单路径)即整包丢弃
     * ——迟到的应答不得写缓存/展开。
     */
    fun toggleExpand(path: String) {
        scope.launch { toggleExpandSuspend(path) }
    }

    private suspend fun toggleExpandSuspend(path: String) {
        val expanded = _state.value.expandedPaths
        if (path in expanded) {
            collapseOne(path)
            return
        }
        if (childrenCache.containsKey(path)) {
            _state.value = _state.value.copy(expandedPaths = expanded + path)
            return
        }
        if (path in loadingChildren) return // 在飞:本次点按吞掉
        loadingChildren.add(path)
        wantedChildren.add(path)
        val mySeq = seq.get()
        val myGen = expandGeneration
        repo.list(deviceId, path).fold(
            onSuccess = { r ->
                loadingChildren.remove(path)
                if (mySeq != seq.get() || myGen != expandGeneration || path !in wantedChildren) return@fold
                wantedChildren.remove(path)
                childrenCache[path] = r.entries
                _state.value = _state.value.copy(expandedPaths = _state.value.expandedPaths + path)
            },
            onFailure = {
                loadingChildren.remove(path)
                if (mySeq != seq.get() || myGen != expandGeneration) return@fold
                wantedChildren.remove(path)
                // 不展开(行仍在,可再点;错误不打断浏览主面)
                _state.value = _state.value.copy(expandedPaths = _state.value.expandedPaths - path)
            },
        )
    }

    /** 收起全部展开(搜索起手/导航换页同一收口;缓存一并释放,在飞作废)。 */
    fun collapseAll() {
        expandGeneration += 1
        wantedChildren.clear()
        childrenCache.clear()
        _state.value = _state.value.copy(expandedPaths = emptySet())
    }

    /** 收起单路径(收起释放缓存;在飞应答经 wanted 集作废)。 */
    private fun collapseOne(path: String) {
        wantedChildren.remove(path)
        childrenCache.remove(path)
        _state.value = _state.value.copy(expandedPaths = _state.value.expandedPaths - path)
    }

    /** 重载后刷新已展开目录的子项缓存(逐个现拉;失败该级收起;孤儿缓存不写)。 */
    private suspend fun refreshExpandedChildren(mySeq: Long) {
        val myGen = expandGeneration
        for (path in _state.value.expandedPaths.sorted()) {
            if (!childrenCache.containsKey(path) || path in loadingChildren) continue
            repo.list(deviceId, path).fold(
                onSuccess = { r ->
                    if (mySeq != seq.get() || myGen != expandGeneration) return@fold
                    childrenCache[path] = r.entries
                },
                onFailure = {
                    if (mySeq != seq.get() || myGen != expandGeneration) return@fold
                    childrenCache.remove(path)
                    _state.value = _state.value.copy(expandedPaths = _state.value.expandedPaths - path)
                },
            )
        }
    }

    // ── 动作(远端档:下载/转发/删除;iOS BrowseView 同收口)──────────────────

    /** 动作受理互斥(iOS sending guard 同构:发起窗内再点吞掉)。 */
    private val starting = AtomicBoolean(false)

    /** 下载条目到本机收件箱(repo 收口 dir = 收件箱哨兵,R12 ① android 口径)。 */
    fun download(paths: List<String>) {
        if (paths.isEmpty() || !starting.compareAndSet(false, true)) return
        scope.launch {
            try {
                repo.downloadToInbox(deviceId, paths).fold(
                    onSuccess = {
                        _state.value = _state.value.copy(startedCount = paths.size, lastActionError = null)
                    },
                    onFailure = { e ->
                        _state.value = _state.value.copy(lastActionError = userErrorText(e))
                    },
                )
            } finally {
                starting.set(false)
            }
        }
    }

    /** 转发选中项到设备 B(remote_copy 调度,数据不经操作端;B 端 dir =
     * 收件箱哨兵;B 离线在弹层面拦截,不可达错误走 [UiState.lastActionError])。 */
    fun forward(paths: List<String>, to: DirectoryDevice) {
        if (paths.isEmpty() || !starting.compareAndSet(false, true)) return
        scope.launch {
            try {
                repo.forward(from = deviceId, paths = paths, to = to.deviceId).fold(
                    onSuccess = {
                        _state.value = _state.value.copy(startedCount = paths.size, lastActionError = null)
                    },
                    onFailure = { e ->
                        _state.value = _state.value.copy(lastActionError = userErrorText(e))
                    },
                )
            } finally {
                starting.set(false)
            }
        }
    }

    /** 删除选中项(成功后退出选择模式并原地重载;失败保持选择走 lastActionError)。 */
    fun deleteSelected() {
        val paths = selectedEntries.map { it.path }
        if (paths.isEmpty()) return
        deletePaths(paths, exitSelection = true, reloadAfter = true)
    }

    /**
     * 删除给定路径集(delete {device,paths,force:true};device = VM 浏览目标
     * ——本机浏览 = 本机 id,远端浏览 = 对端 id,iOS 同构)。不退选不重载
     * (调用方定收口);失败置 [UiState.lastActionError] 保持选择;成功全成
     * 静默,部分失败给明细进 [UiState.deleteNotice]。
     */
    fun deletePaths(paths: List<String>, exitSelection: Boolean = false, reloadAfter: Boolean = false) {
        if (paths.isEmpty()) return
        scope.launch {
            repo.delete(deviceId, paths, force = true).fold(
                onSuccess = { result ->
                    _state.value = _state.value.copy(
                        deleteNotice = if (result.failed.isEmpty()) {
                            null
                        } else {
                            "已删除 ${result.deleted} 项,${result.failed.size} 项失败:${result.failed[0].message}"
                        },
                    )
                    if (exitSelection) exitSelection()
                    if (reloadAfter) reload()
                },
                onFailure = { e ->
                    // 失败保持选择(iOS 同构:用户可重试/改选)
                    _state.value = _state.value.copy(lastActionError = userErrorText(e))
                },
            )
        }
    }

    /** 清一次性反馈(「已开始」计数/动作错误/删除完成明细)。 */
    fun clearFeedback() {
        _state.value = _state.value.copy(startedCount = null, lastActionError = null, deleteNotice = null)
    }

    // ── 内部 ────────────────────────────────────────────────────────────────

    /** 导航换页的列表即刻清空(R12 ②);只属导航三口,reload 不调此。 */
    private fun clearListForNavigation() {
        _state.value = _state.value.copy(entries = emptyList(), truncated = false)
    }

    private fun syncNavFlags() {
        _state.value = _state.value.copy(
            canGoBack = historyIndex > 0,
            canGoForward = historyIndex + 1 < history.size,
        )
    }

    /** 拉 listing(seq 不符的迟到应答整包丢弃;错误 = 清空列表转错误态)。 */
    private fun fetch(target: String, mySeq: Long) {
        _state.value = _state.value.copy(loading = true, errorText = null)
        scope.launch { fetchSuspend(target, mySeq) }
    }

    private suspend fun fetchSuspend(target: String, mySeq: Long) {
        _state.value = _state.value.copy(loading = true, errorText = null)
        repo.list(deviceId, target).fold(
            onSuccess = { r ->
                if (mySeq != seq.get()) return@fold
                _state.value = _state.value.copy(
                    path = target,
                    entries = r.entries,
                    truncated = r.truncated,
                    loading = false,
                )
            },
            onFailure = { e ->
                if (mySeq != seq.get()) return@fold
                _state.value = _state.value.copy(
                    entries = emptyList(),
                    truncated = false,
                    loading = false,
                    errorText = userErrorText(e),
                )
            },
        )
    }

    // ── 纯函数面(companion;宿主可测,iOS 同构)──────────────────────────────

    companion object {

        /**
         * 标题组合(纯函数;R4 真机轮 #5 组合口径):
         * - 浏览根 = rootTitle 单段(远端 = 设备名,本地 = 收件箱等);
         * - 深于根:远端(deviceName 在场)= 「设备名 | 目录名」(分隔留白,
         *   裸 `|` 两段粘连难读);本地(deviceName null)保持单段口径 = 目录名。
         */
        fun composeTitle(rootTitle: String, path: String, rootPath: String, deviceName: String?): String {
            if (path == rootPath) return rootTitle
            val folder = displayName(path)
            if (deviceName.isNullOrEmpty()) return folder
            return "$deviceName | $folder"
        }

        /** 路径显示名 = 最后一段(正反斜杠都切;远端 Windows 路径同形)。 */
        fun displayName(path: String): String =
            RemotePath.segments(path).lastOrNull() ?: path

        /** 可见化管线(隐藏过滤 → 搜索过滤 → 排序;纯函数,宿主可测)。 */
        internal fun applyFilters(
            entries: List<DirEntry>,
            searchText: String,
            showHidden: Boolean,
            sort: BrowseSort,
        ): List<DirEntry> {
            var list = entries
            if (!showHidden) list = list.filter { !it.name.startsWith(".") }
            if (searchText.isNotEmpty()) list = list.filter { it.name.contains(searchText, ignoreCase = true) }
            return sortEntries(list, sort)
        }

        /**
         * 统一排序(文件夹恒前;字段内主键、名称兜底;iOS BrowseViewModel.sorted
         * 同构):名称 = 大小写不敏感升序;日期 = 最新在前;大小 = 大者在前;
         * 种类 = 按扩展名(目录 "" 恒前)。
         */
        internal fun sortEntries(items: List<DirEntry>, sort: BrowseSort): List<DirEntry> =
            items.sortedWith { a, b -> if (less(a, b, sort)) -1 else if (less(b, a, sort)) 1 else 0 }

        /** 单条比较器:文件夹恒前;字段内主键、名称兜底(iOS BrowseViewModel.less 同构)。 */
        private fun less(a: DirEntry, b: DirEntry, sort: BrowseSort): Boolean {
            if (a.isDir != b.isDir) return a.isDir
            return when (sort) {
                BrowseSort.NAME -> nameLess(a.name, b.name)
                BrowseSort.DATE -> if (a.mtimeMs != b.mtimeMs) a.mtimeMs > b.mtimeMs else nameLess(a.name, b.name)
                BrowseSort.SIZE -> if (a.size != b.size) a.size > b.size else nameLess(a.name, b.name)
                BrowseSort.KIND -> {
                    val ka = kindKey(a)
                    val kb = kindKey(b)
                    if (ka != kb) ka < kb else nameLess(a.name, b.name)
                }
            }
        }

        /** 名称兜底比较:大小写不敏感(与系统文件 App 口径一致);同形 = 非小者。 */
        private fun nameLess(a: String, b: String): Boolean =
            a.compareTo(b, ignoreCase = true) < 0

        /** 种类键:目录 ""(恒前),文件取小写扩展名(无扩展名 "" 殿后于目录)。 */
        private fun kindKey(e: DirEntry): String {
            if (e.isDir) return ""
            val comps = e.name.split(".")
            return if (comps.size > 1) comps.last().lowercase() else ""
        }

        /** 错误用户文案(daemon 消息本就面向用户;无名异常给常识兜底文案)。 */
        internal fun userErrorText(e: Throwable): String =
            e.message?.takeIf { it.isNotBlank() } ?: "请求失败:连接中断"
    }
}

// ── 排序字段(spec §2.2 「···」菜单单选;iOS BrowseSort 同构)─────────────────

/** 排序字段(文件夹恒前由比较器统一保证);label = 菜单文案(名称/日期/大小/种类)。 */
enum class BrowseSort(val label: String) {
    NAME("名称"),
    DATE("日期"),
    SIZE("大小"),
    KIND("种类"),
}

// ── 行内展开投影行(R3 #8)───────────────────────────────────────────────────

/**
 * 行内展开投影行:根可见行 + 展开目录子树的扁平序列;[depth] = 缩进层级
 * (根级 0,子级 = 父 +1),视图按 depth × 18dp 缩进。id = entry.path。
 */
data class BrowseRow(val entry: DirEntry, val depth: Int) {
    val id: String get() = entry.path
}

// ── 删除确认(R5b #9/R11 ③;iOS SelectionBar.DeleteRequest 逐字镜像)─────────

/**
 * 待确认删除请求(条数入确认文案;选择删除与行滑删同一收口)。目录计数入
 * 请求——文件夹删除 = **整树语义**(引擎 delete 目录 = remove_dir_all 整树
 * 删),确认文案点明「含内容」:单目录形标题 = 「删除文件夹及内容?」;多选
 * 含目录标题保持「删除 N 项?」而副文明示文件夹连同内容;纯文件形副文
 * 「所选项目将被永久删除,不可恢复。」(iOS 逐字面)。
 */
data class DeleteRequest(val paths: List<String>, val folders: Int = 0) {

    /** 确认弹窗标题(单目录形特称;其余按条数)。 */
    val alertTitle: String
        get() = if (folders >= 1 && paths.size == 1) "删除文件夹及内容?" else "删除 ${paths.size} 项?"

    /** 确认弹窗副文(含目录时点明整树,破坏性口径不变)。 */
    val alertMessage: String
        get() = if (folders >= 1) {
            "所选文件夹及其全部内容将被永久删除,不可恢复。"
        } else {
            "所选项目将被永久删除,不可恢复。"
        }
}

// ── 行左滑动作(R5b #7/R11 ③/V3-3 T18;iOS BrowseSwipeAction 同构)───────────

/**
 * 行左滑动作(上下文式——动作集按浏览档出集;R11 ③ 目录行同文件行同集
 * ——目录发送/下载/转发 = collect_items 递归展开整树,删除 = remove_dir_all
 * 整树删,均为引擎既有面):本地行 = [发送, 删除];远端行 = [下载, 转发],
 * 门开追加 删除([BrowseGates] 默认关)。目录行追加「快捷」恒簇末
 * ([dirActions])。
 */
enum class BrowseSwipeAction(val label: String) {
    SEND("发送"),
    DELETE("删除"),
    DOWNLOAD("下载"),
    FORWARD("转发"),
    FAVORITE("快捷"),
    ;

    companion object {

        /** 行动集(`isRemote` 分浏览档;远端删除受 [allowRemoteDelete] 门,
         * 缺省关 = 今日集——调用方为本地档时传 false 与缺省同义)。 */
        fun actions(isRemote: Boolean, allowRemoteDelete: Boolean = false): List<BrowseSwipeAction> =
            if (isRemote) {
                if (allowRemoteDelete) listOf(DOWNLOAD, FORWARD, DELETE) else listOf(DOWNLOAD, FORWARD)
            } else {
                listOf(SEND, DELETE)
            }

        /** 目录行动作集(V3-3 T18):= [actions] 出集 + (entry.isDir 时追加
         * [FAVORITE],恒簇末;文件行/两档非目录输入与旧集逐字同)。 */
        fun dirActions(entry: DirEntry, isRemote: Boolean, allowRemoteDelete: Boolean = false): List<BrowseSwipeAction> =
            actions(isRemote, allowRemoteDelete) + if (entry.isDir) listOf(FAVORITE) else emptyList()
    }
}

// ── 选择底栏动作集(R9 #3/V3-3 T18;iOS BrowseSelectionAction 同构)──────────

/**
 * 选择底栏动作集(与行左滑同一远端删除门——两表面单源出集):本地 =
 * [分享, 删除](自有文件,不设门);远端 = [下载, 转发](门关)/ [下载,
 * 转发, 删除](门开)。纯目录多选追加「快捷」([dirSelectionActions],
 * 含文件或混合选择不出)。
 */
enum class BrowseSelectionAction(val label: String) {
    SHARE("分享"),
    DELETE("删除"),
    DOWNLOAD("下载"),
    FORWARD("转发"),
    FAVORITE("快捷"),
    ;

    companion object {

        /** 动作集按浏览档 + 远端删除门出集(顺序即底栏自左向右,破坏面恒簇末)。 */
        fun actions(isRemote: Boolean, allowRemoteDelete: Boolean = false): List<BrowseSelectionAction> =
            if (isRemote) {
                if (allowRemoteDelete) listOf(DOWNLOAD, FORWARD, DELETE) else listOf(DOWNLOAD, FORWARD)
            } else {
                listOf(SHARE, DELETE)
            }

        /** 纯目录多选动作集(V3-3 T18):= [actions] 出集 + (allDirs 时追加
         * [FAVORITE],favorite 恒簇末;既有出集对非 allDirs 输入逐字不变)。 */
        fun dirSelectionActions(
            isRemote: Boolean,
            allDirs: Boolean,
            allowRemoteDelete: Boolean = false,
        ): List<BrowseSelectionAction> =
            actions(isRemote, allowRemoteDelete) + if (allDirs) listOf(FAVORITE) else emptyList()
    }
}

// ── 浏览档门控单源(spec §4.2 删除门;T11 设置页落地)────────────────────────

/**
 * 浏览档门控单源:远端删除受「允许删除远端文件」设置门——门只收 UI 面
 * (入口显隐),引擎 delete 契约不变;本地删除不设门。设置项随 T11 落地,
 * 本波默认关 = 远端行左滑/选择底栏不出「删除」。
 */
object BrowseGates {

    /** 「允许删除远端文件」(默认关;T11 接设置开关后改读设置面)。 */
    const val ALLOW_REMOTE_DELETE: Boolean = false
}
