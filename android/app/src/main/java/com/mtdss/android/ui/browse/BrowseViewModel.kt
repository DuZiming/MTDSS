package com.mtdss.android.ui.browse

import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.DirectoryDevice
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicLong

/**
 * 浏览屏 VM(对端目录树,list op;路径语义见 [RemotePath]):
 * - **seq 守卫**:快速连续导航时过期响应丢弃(镜像桌面 DirStates 的
 *   applyListing;seq 单调,旧响应不得覆盖新位置);
 * - 选择集:entry.path 集(文件与目录都可选——目录整树下载,协议原生
 *   支持;导航换目录即清空);
 * - 下载:transfer.start download 形态,dir 恒 "inbox"(repo 收口),
 *   策略 = 跳过(默认,断点续传语义)/ 覆盖(overwrite = 顶层名集,
 *   与桌面 conflict 决策表同源:策略名单按顶层 rel 命中)。
 */
class BrowseViewModel(
    val device: DirectoryDevice,
    private val repo: MtdssRepository,
    private val scope: CoroutineScope,
) {
    data class UiState(
        /** 当前目录路径;"" = 根卷列表。 */
        val path: String = "",
        val entries: List<DirEntry> = emptyList(),
        /** 目录超出引擎上限(10000 项)被截断。 */
        val truncated: Boolean = false,
        val loading: Boolean = false,
        val error: String? = null,
        val selected: Set<String> = emptySet(),
        /** 下载已开始(job_id;一次性反馈,消费后清)。 */
        val startedJob: String? = null,
        val downloadError: String? = null,
    )

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    private val seq = AtomicLong()

    /** 打开目录(导航入口;"" = 根卷)。 */
    fun open(path: String) {
        val mySeq = seq.incrementAndGet()
        _state.value = _state.value.copy(
            path = path,
            loading = true,
            error = null,
            selected = emptySet(),
            startedJob = null,
            downloadError = null,
        )
        scope.launch {
            repo.list(device.deviceId, path).fold(
                onSuccess = { r ->
                    if (mySeq == seq.get()) {
                        _state.value = _state.value.copy(
                            entries = r.entries,
                            truncated = r.truncated,
                            loading = false,
                        )
                    }
                },
                onFailure = { e ->
                    if (mySeq == seq.get()) {
                        _state.value = _state.value.copy(
                            entries = emptyList(),
                            truncated = false,
                            loading = false,
                            error = e.message ?: e.javaClass.simpleName,
                        )
                    }
                },
            )
        }
    }

    /** 上级目录;已在根卷则空操作(设备层由路由栈承担)。 */
    fun up() {
        val parent = RemotePath.parentOf(_state.value.path) ?: return
        open(parent)
    }

    /** 选择切换(仅收当前列表条目,防陈旧选择混入)。 */
    fun toggle(path: String) {
        val s = _state.value
        if (s.entries.none { it.path == path }) return
        _state.value = s.copy(
            selected = if (path in s.selected) s.selected - path else s.selected + path,
        )
    }

    /** 下载选择集到收件箱;overwrite=true 时选择集顶层名进 overwrite 名单。 */
    fun download(overwrite: Boolean) {
        val s = _state.value
        val paths = s.selected.toList()
        if (paths.isEmpty()) return
        // overwrite 名单 = 顶层名(collect_items 的 rel 语义:顶层输入名;
        // 目录名作为前缀天然覆盖其整树)
        val names = paths.map { p -> s.entries.firstOrNull { it.path == p }?.name ?: p }
        scope.launch {
            _state.value = _state.value.copy(downloadError = null, startedJob = null)
            repo.downloadToInbox(
                device = device.deviceId,
                paths = paths,
                skip = emptyList(),
                overwrite = if (overwrite) names else emptyList(),
            ).fold(
                onSuccess = { r ->
                    _state.value = _state.value.copy(selected = emptySet(), startedJob = r.jobId)
                },
                onFailure = { e ->
                    // 失败保留选择集(用户可重试/改选)
                    _state.value = _state.value.copy(
                        downloadError = e.message ?: e.javaClass.simpleName,
                    )
                },
            )
        }
    }

    /** 清一次性反馈(startedJob/downloadError)。 */
    fun clearFeedback() {
        _state.value = _state.value.copy(startedJob = null, downloadError = null)
    }
}
