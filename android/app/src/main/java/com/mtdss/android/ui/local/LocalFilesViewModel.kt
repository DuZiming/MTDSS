package com.mtdss.android.ui.local

import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.send.RootTree
import com.mtdss.android.send.RootsStore
import com.mtdss.android.send.SafFace
import com.mtdss.android.send.StoredRoot
import com.mtdss.android.send.TreeChild
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 本机页 VM(收件箱枚举 + 授权根账本 + 树内导航/多选/展开发送):
 *
 * - **树导航**:path = 树内路径栈(名字 + 文档 id 成对);entries = 当前
 *   目录子项;选择集 = 当前目录内的 documentId 集(换目录即清,镜像浏览
 *   屏);当前目录的 rel = 栈中名字按 '/' 连接(树根 = "");
 * - **展开发送**:文件+目录混合选择(或「发送本目录」)经 [RootTree]
 *   展开成 BridgeItem(ref = roottree://rootId/rel);查询在 IO 线程
 *   (DocumentsContract query 是阻塞调用);
 * - 授权根增删直通 RootsStore(添加动作由屏面经 OPEN_DOCUMENT_TREE
 *   launcher 完成,VM 只收结果)。
 */
class LocalFilesViewModel(
    private val store: RootsStore,
    private val saf: SafFace,
    private val inbox: InboxListing,
    private val scope: CoroutineScope,
) {
    /** 树内一跳(目录名 + 该目录的文档 id)。 */
    data class PathSegment(val name: String, val documentId: String)

    data class UiState(
        // ── 收件箱面 ──
        val inboxEntries: List<InboxListing.InboxEntry> = emptyList(),
        val inboxLoading: Boolean = true,
        // ── 授权根面 ──
        val roots: List<StoredRoot> = emptyList(),
        /** 当前进入的根(null = 根列表层)。 */
        val currentRoot: StoredRoot? = null,
        val path: List<PathSegment> = emptyList(),
        val entries: List<TreeChild> = emptyList(),
        val treeLoading: Boolean = false,
        val treeError: String? = null,
        val selected: Set<String> = emptySet(),
        /** 展开结果错误(一次性;条目数护栏等)。 */
        val expandError: String? = null,
    )

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    init {
        refreshRoots()
        loadInbox()
    }

    private fun refreshRoots() {
        _state.value = _state.value.copy(roots = store.roots())
    }

    private fun loadInbox() {
        scope.launch {
            val entries = withContext(Dispatchers.IO) { inbox.query() }
            _state.value = _state.value.copy(inboxEntries = entries, inboxLoading = false)
        }
    }

    /** 收件箱刷新。 */
    fun refreshInbox() {
        _state.value = _state.value.copy(inboxLoading = true)
        loadInbox()
    }

    // ── 授权根 ─────────────────────────────────────────────────────────────

    /** 登记 OPEN_DOCUMENT_TREE 结果(提取失败返回 null,屏面提示)。 */
    fun addRoot(treeUri: String, displayName: String): StoredRoot? {
        val root = store.add(treeUri, displayName)
        refreshRoots()
        return root
    }

    /** 移除授权根(连带释放持久授权);当前正浏览该根则弹回根列表。 */
    fun removeRoot(rootId: String) {
        store.remove(rootId)
        val s = _state.value
        if (s.currentRoot?.rootId == rootId) {
            _state.value = s.copy(currentRoot = null, path = emptyList(), entries = emptyList(), selected = emptySet())
        }
        refreshRoots()
    }

    /** 进入授权根(根层)。 */
    fun openRoot(root: StoredRoot) {
        val mySeq = ++seq
        _state.value = _state.value.copy(
            currentRoot = root,
            path = emptyList(),
            selected = emptySet(),
            treeLoading = true,
            treeError = null,
            entries = emptyList(),
        )
        loadChildren(root, root.treeDocId, emptyList(), mySeq)
    }

    /** 进入子目录。 */
    fun openDir(segment: PathSegment) {
        val s = _state.value
        val root = s.currentRoot ?: return
        val mySeq = ++seq
        val path = s.path + segment
        _state.value = s.copy(path = path, selected = emptySet(), treeLoading = true, treeError = null)
        loadChildren(root, segment.documentId, path, mySeq)
    }

    /** 返回上级;在根层则弹回根列表。 */
    fun up() {
        val s = _state.value
        val root = s.currentRoot ?: return
        if (s.path.isEmpty()) {
            _state.value = s.copy(currentRoot = null, entries = emptyList(), selected = emptySet())
            return
        }
        val path = s.path.dropLast(1)
        val mySeq = ++seq
        val dirId = path.lastOrNull()?.documentId ?: root.treeDocId
        _state.value = s.copy(path = path, selected = emptySet(), treeLoading = true, treeError = null)
        loadChildren(root, dirId, path, mySeq)
    }

    private fun loadChildren(root: StoredRoot, documentId: String, path: List<PathSegment>, mySeq: Long) {
        scope.launch {
            val children = withContext(Dispatchers.IO) { saf.treeChildren(root.treeUri, documentId) }
            if (mySeq != seq) return@launch // 过期响应丢弃(导航竞速守卫)
            if (children == null) {
                _state.value = _state.value.copy(treeLoading = false, treeError = "目录读取失败")
            } else {
                _state.value = _state.value.copy(entries = children, treeLoading = false, treeError = null)
            }
        }
    }

    /** 选择切换(仅当前目录条目)。 */
    fun toggle(documentId: String) {
        val s = _state.value
        if (s.entries.none { it.documentId == documentId }) return
        _state.value = s.copy(
            selected = if (documentId in s.selected) s.selected - documentId else s.selected + documentId,
        )
    }

    /** 当前目录的 rel(相对授权根;根 = "")。 */
    private fun currentRel(): String = _state.value.path.joinToString("/") { it.name }

    /**
     * 展开选择集成发送条目(屏面「发送到设备」入口;查询在 IO)。空选择
     * 交给 [expandDirectory](发送本目录)。
     */
    fun expandSelection(onItems: (List<BridgeItem>) -> Unit) {
        val s = _state.value
        val root = s.currentRoot ?: return
        val prefix = currentRel()
        val selections = s.entries
            .filter { it.documentId in s.selected }
            .map { RootTree.TreeSelection(rel = if (prefix.isEmpty()) it.name else "$prefix/${it.name}", entry = it) }
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                RootTree.expand(root.rootId, selections) { id -> saf.treeChildren(root.treeUri, id) }
            }
            handleExpand(result, onItems)
        }
    }

    /** 展开当前目录整树(「发送本目录」入口)。 */
    fun expandDirectory(onItems: (List<BridgeItem>) -> Unit) {
        val s = _state.value
        val root = s.currentRoot ?: return
        val rel = currentRel()
        val dirId = s.path.lastOrNull()?.documentId ?: root.treeDocId
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                RootTree.expandDirectory(root.rootId, rel, dirId) { id -> saf.treeChildren(root.treeUri, id) }
            }
            handleExpand(result, onItems)
        }
    }

    private fun handleExpand(result: RootTree.ExpandResult, onItems: (List<BridgeItem>) -> Unit) {
        when (result) {
            is RootTree.ExpandResult.Items -> {
                _state.value = _state.value.copy(selected = emptySet(), expandError = null)
                onItems(result.items)
            }

            is RootTree.ExpandResult.Error ->
                _state.value = _state.value.copy(expandError = result.message)
        }
    }

    /** 清一次性展开错误。 */
    fun clearExpandError() {
        _state.value = _state.value.copy(expandError = null)
    }

    private var seq = 0L
}
