package com.mtdss.android.ui.tasks

import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.Contract
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.Ops
import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.ipc.TransferListResult
import com.mtdss.android.ipc.TransferProgress
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.SafFace
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * 任务列表 VM(**app 级**(AppGraph 装配,与屏面生命周期解耦)——KEEP_SCREEN_ON
 * 须在任意屏面可见时驱动,且任务的活态追踪不随屏面进出重置)。
 *
 * 数据面(镜像桌面 App.tsx 的任务收敛):
 * - **首连**:transfer.list 全量**重置**行集(只此一次;fast-fail 自发起连,
 *   冷启动「引擎未起 → socket 可连」间隙由 IpcClient 自愈覆盖);
 * - [IpcClient.transferListRefresh](重连补拉,replay=1)→ 直接消费携带的
 *   结果重置行集(零追加 RPC;首连失败/中途断开的重拉兜底同在此面);
 * - event.transfer 增量 → [TasksReducer.applyEvent](终态粘滞/会话行终态即
 *   消失/事件帧不带 name·src·dst 保留前值);
 * - 行操作五枚 → busy 防连点;reinitiate 经 [ReinitiateRouter] 路由
 *   (bridge 上传 App 侧重建,其余 daemon op);clear/reinitiate 成功后
 *   重置重拉对账(摘行/新行入场即刻反映)。
 *
 * 事件订阅纪律(先订阅后动作):本 VM 构造即订阅 ipc.events
 * (onSubscription 闸,镜像 InboxProvider/SourceProvider;hub 的 transfer 面
 * 即为此设计为不经 hub——各自生命周期与闸门不同)。
 */
class TasksViewModel(
    private val ipc: IpcClient,
    private val repo: MtdssRepository,
    private val roots: RootLookup,
    private val saf: SafFace,
    private val scope: CoroutineScope,
) {
    data class State(
        /** 行集(首见序稳定;重置语义见类注)。 */
        val rows: List<TransferJob> = emptyList(),
        val loading: Boolean = false,
        /** 最近一次全量拉取失败(成功清空;行集保留旧值)。 */
        val error: String? = null,
        /** 操作在途的行 id(防连点;按钮置灰)。 */
        val busy: Set<String> = emptySet(),
        /** 操作失败一次性反馈(snackbar 呈现后清)。 */
        val opError: String? = null,
    ) {
        /** KEEP_SCREEN_ON 派生面(存在任一非终态任务;纯函数派生,勿另行存储)。 */
        val keepScreenOn: Boolean get() = TasksReducer.keepScreenOn(rows)
    }

    private val _state = MutableStateFlow(State())

    val state: StateFlow<State> = _state.asStateFlow()

    private val subscribed = CompletableDeferred<Unit>()

    init {
        // 事件订阅(app 级单订阅,构造即落位;闸后动作)
        scope.launch {
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame -> if (frame.method == Ops.EVENT_TRANSFER) onTransferFrame(frame) }
        }
        // 重连补拉结果(replay=1):IpcClient 自愈重连后**已拉好** transfer.list,
        // 直接消费携带的结果(零追加 RPC)——重连语义的承载面
        scope.launch {
            ipc.transferListRefresh.collect { r -> applyListResult(r) }
        }
        // 首连拉取(只此一次):冷启动「引擎未起 → socket 可连」间隙由
        // IpcClient 自愈覆盖,Connected 终会到来;首连若失败/中途断开,
        // 重连侧的 transferListRefresh 兜底重拉。**不做每次 Connected 都
        // 拉的双触发**——与 refresh 补拉并发会引入快照响应与事件帧的
        // 竞态窗口(同连接上后到的快照把先到的事件增量覆盖回旧值)
        scope.launch {
            var pulled = false
            repo.connectionState.collect { st ->
                if (st == ConnState.Connected && !pulled) {
                    pulled = true
                    pullList()
                }
            }
        }
        // 首连发起(fast-fail;本 VM 自足,不依赖 SessionViewModel 的连接触发)
        scope.launch {
            subscribed.await()
            ipc.connect()
        }
    }

    /** 手动刷新(全量重置;任务屏打开时亦调——事件新建行补 name/src/dst)。 */
    fun reseed() {
        pullList()
    }

    // ── 行操作(操作可用性矩阵在 [TasksReducer.actionsFor],屏面出键)──────

    fun pause(id: String) = control(id) { repo.transferPause(id) }

    fun resume(id: String) = control(id) { repo.transferResume(id) }

    fun cancel(id: String) = control(id) { repo.transferCancel(id) }

    /** 清除(记录摘除 + 残留回收;成功后重拉对账让行出列)。 */
    fun clear(id: String) = control(id, reseedAfter = true) { repo.transferClear(id) }

    /**
     * 重新发起:路由见 [ReinitiateRouter](bridge 上传 = App 侧重建 + 新
     * transfer.start;download/remote_copy/路径形态 = daemon op)。两臂成功
     * 均重拉对账;Rebuild 臂的旧记录有意保留(见 Rebuild 注)。
     */
    fun reinitiate(id: String) {
        val job = _state.value.rows.firstOrNull { it.id == id } ?: return
        control(id, reseedAfter = true) {
            when (val plan = withContext(Dispatchers.IO) { ReinitiateRouter.plan(job, roots, saf) }) {
                is ReinitiateRouter.Plan.Rebuild ->
                    // 断点续传:skip/overwrite 双空(跳过对端已收完 + 位图续传)
                    repo.startUpload(items = plan.items, to = plan.to, dir = plan.dir).map { }

                is ReinitiateRouter.Plan.Daemon -> repo.transferReinitiate(id)

                is ReinitiateRouter.Plan.Reject -> Result.failure(IllegalStateException(plan.reason))
            }
        }
    }

    /** 清一次性操作反馈。 */
    fun clearOpError() {
        _state.update { it.copy(opError = null) }
    }

    // ── 内部:拉取与事件收敛 ─────────────────────────────────────────────────

    private fun pullList() {
        scope.launch {
            _state.update { it.copy(loading = true) }
            applyListResult(repo.transferList())
        }
    }

    private fun applyListResult(r: Result<TransferListResult>) {
        r.fold(
            onSuccess = { list ->
                _state.update {
                    it.copy(rows = TasksReducer.seed(list.jobs), loading = false, error = null)
                }
            },
            onFailure = { e ->
                _state.update { it.copy(loading = false, error = e.message ?: e.javaClass.simpleName) }
            },
        )
    }

    private fun onTransferFrame(frame: EventFrame) {
        // 畸形事件帧防御:解析失败丢弃,不杀订阅协程(镜像 SourceProvider 口径)
        val evt: TransferProgress = try {
            Contract.parseTransferProgress(frame.params)
        } catch (_: Exception) {
            return
        }
        _state.update { s -> s.copy(rows = TasksReducer.applyEvent(s.rows, evt)) }
    }

    /**
     * 操作统一执行面:busy 防连点 → RPC(经 [run] 路由)→ 失败落一次性
     * 反馈;成功可选重拉。**重拉在 busy 窗口内行内完成**——对账结果(新
     * 在途行入场 → 同签名在途抑制生效)先于按键解禁,重新发起的双发窗口
     * 由此闭合(busy 只盖 RPC 的话,两次连点可在新行入场前各发一次
     * transfer.start,同 rels 并发写坏对端 .part)。
     */
    private fun control(id: String, reseedAfter: Boolean = false, run: suspend () -> Result<Unit>) {
        if (id in _state.value.busy) return
        _state.update { it.copy(busy = it.busy + id) }
        scope.launch {
            run().fold(
                onSuccess = { if (reseedAfter) applyListResult(repo.transferList()) },
                onFailure = { e ->
                    _state.update { it.copy(opError = e.message ?: e.javaClass.simpleName) }
                },
            )
            _state.update { it.copy(busy = it.busy - id) }
        }
    }
}
