package com.mtdss.android.ui.tasks

import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.ipc.TransferProgress

/**
 * 任务列表纯逻辑层(镜像桌面 ui/src/lib/transfer.ts 的收敛语义;字段面 =
 * Contract 的 [TransferJob]/[TransferProgress],JVM 钉死,VM 只做编排)。
 *
 * 数据面契约(与 daemon jobs.rs/api.rs 对齐):
 * - `event.transfer` 事件帧**不带 name/src/dst**(daemon emit_locked 只带计数
 *   与状态)——合并时保留前值;全新 id 以 id 兜底显示名,待 transfer.list
 *   重置对账补齐(桌面 mergeTransfers 的等价面 = 任务屏打开即重拉)。
 * - `error` 恒在场(null = 无错);`relay` 中继兜底角标。
 * - **终态粘滞**:done/failed/cancelled/interrupted 之后迟到的进度帧整体
 *   忽略(与 daemon 侧 is_terminal_state + 桌面 reducer 同口径;interrupted
 *   是 store 独有态——daemon 重启标记,无 live 驱动,按历史终态呈现)。
 * - **会话级行**(kind = incoming 被动来件 / local_copy,daemon 无持久记录)
 *   :活态照常合并(transfer.list 的 live 快照含它们),**终态即消失**
 *   (App 侧摘行;daemon 注册表终态常驻但无记录面,不清即幽灵)。
 */
object TasksReducer {

    /** 终态集合(daemon is_terminal_state + store 独有 interrupted)。 */
    private val TERMINAL_STATES = setOf("done", "failed", "cancelled", "interrupted")

    /** 会话级 kind(daemon 侧无持久记录;终态动作必报「任务不存在」)。 */
    private val SESSION_KINDS = setOf("incoming", "local_copy")

    /** 有持久记录的 kind(终态动作资格面,镜像桌面 RECORD_KINDS)。 */
    private val RECORD_KINDS = setOf("upload", "download", "remote_copy")

    fun isTerminalState(state: String): Boolean = state in TERMINAL_STATES

    fun isSessionKind(kind: String): Boolean = kind in SESSION_KINDS

    fun isRecordKind(kind: String): Boolean = kind in RECORD_KINDS

    // ── 快照合并 ─────────────────────────────────────────────────────────────

    /**
     * transfer.list 全量 → 行集(重置语义:连接建立/重连补拉/手动刷新/
     * reinitiate·clear 成功后的对账)。终态会话行滤除(终态即消失);
     * 记录行含中断/失败/完成等历史,照收不隐(操作可用性在 [actionsFor])。
     * distinctBy 防御:daemon 合并面已保 id 唯一,契约违规时 UI 不至于因
     * LazyColumn 重复 key 崩(首见胜)。
     */
    fun seed(jobs: List<TransferJob>): List<TransferJob> =
        jobs.filter { !(isSessionKind(it.kind) && isTerminalState(it.state)) }.distinctBy { it.id }

    /**
     * event.transfer 增量合并(纯函数):
     * - 既有行:终态粘滞(终态后迟到帧整体忽略);会话行终态 → 摘行;
     *   其余整体以事件帧为准,仅 name/src/dst 保留前值(事件帧不带)。
     * - 全新 id:建行(显示名兜底 = id);会话 kind 的终态帧不建行。
     */
    fun applyEvent(rows: List<TransferJob>, evt: TransferProgress): List<TransferJob> {
        val idx = rows.indexOfFirst { it.id == evt.id }
        val prev = if (idx >= 0) rows[idx] else null
        if (prev != null && isTerminalState(prev.state)) return rows // 终态粘滞
        if (isSessionKind(evt.kind) && isTerminalState(evt.state)) {
            return if (prev == null) rows else rows.filterIndexed { i, _ -> i != idx } // 会话卡终态即消失
        }
        val merged = TransferJob(
            id = evt.id,
            kind = evt.kind,
            name = prev?.name?.takeIf { it.isNotEmpty() } ?: evt.id,
            state = evt.state,
            filesDone = evt.filesDone,
            filesTotal = evt.filesTotal,
            bytesDone = evt.bytesDone,
            bytesTotal = evt.bytesTotal,
            speedBps = evt.speedBps,
            skipped = evt.skipped,
            relay = evt.relay,
            src = prev?.src,
            dst = prev?.dst,
            error = evt.error,
        )
        return if (prev == null) rows + merged else rows.toMutableList().also { it[idx] = merged }
    }

    // ── 操作可用性矩阵(spec §7.2,镜像桌面 actionsFor)──────────────────────

    /** 行操作五枚(pause/resume/cancel/reinitiate/clear)。 */
    enum class TaskAction { PAUSE, RESUME, CANCEL, REINITIATE, CLEAR }

    /**
     * 操作可用性矩阵:进行中 = 暂停·取消(remote_copy 无本机数据面拒 pause,
     * 仅取消);已暂停 = 继续·取消;中断/失败 = 重新发起·清除;已取消 = 清除;
     * 完成 = 无。终态动作只对有持久记录的 kind 开放(会话卡无记录面)。
     *
     * **同签名在途抑制**:中断/失败/已取消行若存在另一**非终态**行同
     * kind+src+dst(典型 = bridge 上传经 App 侧重建后的旧失败行 + 新在途行,
     * 两行序列化面逐字相同),重新发起/清除一并隐藏——前者会双发同 rels
     * (对端 .part 并发写坏),后者 transfer.clear 的 cleanup_outbox 会删掉
     * 对端 resume 位图(新行正依赖它续传)。在途行终态后动作恢复。
     */
    fun actionsFor(job: TransferJob, rows: List<TransferJob>): List<TaskAction> = when (job.state) {
        "queued", "running" ->
            if (job.kind == "remote_copy") listOf(TaskAction.CANCEL)
            else listOf(TaskAction.PAUSE, TaskAction.CANCEL)

        "paused" -> listOf(TaskAction.RESUME, TaskAction.CANCEL)

        "interrupted", "failed" ->
            if (!isRecordKind(job.kind) || hasInFlightSibling(job, rows)) emptyList()
            else listOf(TaskAction.REINITIATE, TaskAction.CLEAR)

        "cancelled" ->
            if (!isRecordKind(job.kind) || hasInFlightSibling(job, rows)) emptyList()
            else listOf(TaskAction.CLEAR)

        else -> emptyList() // done / 未知状态:无动作(防御渲染)
    }

    /** 同签名在途行存在性(见 [actionsFor] 注;签名 = [signatureOf])。 */
    private fun hasInFlightSibling(job: TransferJob, rows: List<TransferJob>): Boolean {
        val sig = signatureOf(job)
        return rows.any { it.id != job.id && !isTerminalState(it.state) && signatureOf(it) == sig }
    }

    /**
     * 行签名(操作互斥键):同 kind+src+dst = 同落位面(对端同 rels/同目标)。
     * [hasInFlightSibling] 的抑制判定与 VM 的签名级在途防连点(busySigs)
     * 共用本定义——双终态同签名行(失败重发再失败)互为双发隐患,两道
     * 防线必须看到同一个签名。
     */
    data class RowSignature(val kind: String, val src: String?, val dst: String?)

    fun signatureOf(job: TransferJob): RowSignature = RowSignature(job.kind, job.src, job.dst)

    // ── 呈现投影(镜像桌面 jobDisplayName/stateKey/badgeKind)─────────────────

    /** 三色徽章四档:发送/接收(含被动 incoming)/调度/其他(防御渲染)。 */
    enum class TaskBadge(val glyph: String, val label: String) {
        UPLOAD("↑", "发送"),
        DOWNLOAD("↓", "接收"),
        SCHEDULE("⇄", "调度"),
        OTHER("•", "其他"),
    }

    fun badgeFor(kind: String): TaskBadge = when (kind) {
        "upload" -> TaskBadge.UPLOAD
        "download", "incoming" -> TaskBadge.DOWNLOAD
        "remote_copy" -> TaskBadge.SCHEDULE
        else -> TaskBadge.OTHER
    }

    /** 任务名(单文件 = 名;多文件 = 首项名 + 「· N 项」;空名回退 id)。 */
    fun displayName(job: TransferJob): String {
        val base = job.name.takeIf { it.isNotEmpty() } ?: job.id
        return if (job.filesTotal > 1) "$base · ${job.filesTotal} 项" else base
    }

    /** 状态文案(queued/running 归并「进行中」;未知状态原样,不崩)。 */
    fun stateText(state: String): String = when (state) {
        "queued", "running" -> "进行中"
        "paused" -> "已暂停"
        "interrupted" -> "中断"
        "failed" -> "失败"
        "done" -> "完成"
        "cancelled" -> "已取消"
        else -> state
    }

    /** 进度比:总量未知(≤0,起步前/远端计数面)→ null(不确定态动画条)。 */
    fun progressRatio(bytesDone: Long, bytesTotal: Long): Float? =
        if (bytesTotal <= 0) null
        else (bytesDone.toDouble() / bytesTotal).coerceIn(0.0, 1.0).toFloat()

    /** 速率文案(镜像桌面 fmtSpeed:0 → 「0 B/s」;<1024 整数;此后 1024 进制一位小数)。 */
    fun speedText(bps: Long): String {
        if (bps <= 0) return "0 B/s"
        if (bps < 1024) return "$bps B/s"
        val units = arrayOf("KB", "MB", "GB", "TB")
        var v = bps / 1024.0
        var i = 0
        while (v >= 1024 && i < units.size - 1) {
            v /= 1024
            i++
        }
        return String.format(java.util.Locale.ROOT, "%.1f %s/s", v, units[i])
    }

    /**
     * KEEP_SCREEN_ON 派生面:存在任一**非终态**任务(排队/运行/暂停;含会话级
     * 接收行——中断是 Android 常态,接收中同样不应熄屏)。纯终态历史面不占。
     */
    fun keepScreenOn(rows: List<TransferJob>): Boolean = rows.any { !isTerminalState(it.state) }
}
