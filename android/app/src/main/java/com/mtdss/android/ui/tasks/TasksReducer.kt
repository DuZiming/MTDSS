package com.mtdss.android.ui.tasks

import androidx.compose.ui.graphics.Color
import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.ipc.TransferProgress
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuSemantic

/**
 * 任务列表纯逻辑层(镜像桌面 ui/src/lib/transfer.ts 的收敛语义;字段面 =
 * Contract 的 [TransferJob]/[TransferProgress],JVM 钉死,VM 只做编排)。
 *
 * V3-4 传输列表重写追加呈现面(spec §3-5;iOS TransfersFormat 镜像):
 * 行集**最新在最上**(seed 反转/事件插顶)、徽章换向([TaskBadge.color])、
 * 分段过滤([TransferFilter])、左滑出集按状态([swipeActionsFor])、
 * 红因/跳过/方向设备/完成时间格式器与「清除已完成」出集。
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
     * **反转**(V3-4 §3-5 最新在上):daemon 快照 = job_seq 升序(最旧
     * 在前),反转即「最新在最上」(iOS TransfersViewModel.project 同构)。
     */
    fun seed(jobs: List<TransferJob>): List<TransferJob> =
        jobs.filter { !(isSessionKind(it.kind) && isTerminalState(it.state)) }.distinctBy { it.id }.reversed()

    /**
     * event.transfer 增量合并(纯函数):
     * - 既有行:终态粘滞(终态后迟到帧整体忽略);会话行终态 → 摘行;
     *   其余整体以事件帧为准,仅 name/src/dst 保留前值,**原位更新不挪位**
     *   (最新仍在前);
     * - 全新 id:建行(显示名兜底 = id)并**插顶**(iOS rows.insert(at: 0)
     *   同构);会话 kind 的终态帧不建行。
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
        return when {
            prev == null -> listOf(merged) + rows // 新行插顶(最新在上)
            else -> rows.toMutableList().also { it[idx] = merged } // 原位更新
        }
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

        "publishing" ->
            // P4a 发布窗:网络面已收妥,无传输可暂停;取消合法(协调器不
            // 中断发布,§6.1——已落定件照常发布,终态翻转自然无操作)
            listOf(TaskAction.CANCEL)

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

    /**
     * 左滑动作出集按状态(V3-4 §3-5 传输列表;iOS TransfersFormat.actions
     * 同构):进行中(queued/running/paused/publishing)→ [取消](pause/resume
     * 契约在而 UI 不上此页);中断/失败 → [重新发起, 清除];完成/已取消 →
     * [清除](T5b Ruling K:done 恢复清除入口)。终态动作只对有记录面的
     * kind 开放(会话卡无记录面);**同签名在途抑制**同 [actionsFor] 口径
     * (终态动作一并隐藏);空集行不示能(无滑面)。
     */
    fun swipeActionsFor(job: TransferJob, rows: List<TransferJob>): List<TaskAction> = when (job.state) {
        "queued", "running", "paused", "publishing" -> listOf(TaskAction.CANCEL)
        "interrupted", "failed" ->
            if (!isRecordKind(job.kind) || hasInFlightSibling(job, rows)) emptyList()
            else listOf(TaskAction.REINITIATE, TaskAction.CLEAR)
        "cancelled", "done" ->
            if (!isRecordKind(job.kind) || hasInFlightSibling(job, rows)) emptyList()
            else listOf(TaskAction.CLEAR)
        else -> emptyList() // 未知状态:无动作(防御渲染)
    }

    /**
     * 「清除已完成」可清集(spec §3-5 「···」;iOS clearCompleted 同构):
     * 全部**有记录面的终态行**(done/中断/失败/已取消同权,引擎侧 done 已
     * 放开),同签名在途抑制同单行口径(清除会删对端 resume 位图)。
     */
    fun clearCompletedIds(rows: List<TransferJob>): List<String> =
        rows.filter { isTerminalState(it.state) && isRecordKind(it.kind) }
            .filter { !hasInFlightSibling(it, rows) }
            .map { it.id }

    /** 「清除已完成」使能面(行集存在任一终态行;iOS 同构,资格面在出集)。 */
    fun hasClearable(rows: List<TransferJob>): Boolean = rows.any { isTerminalState(it.state) }

    // ── 呈现投影(镜像桌面 jobDisplayName/stateKey/badgeKind)─────────────────

    /**
     * 徽章四档(V3-4 §3-5 **换向**:接收(下载)= 蓝 `#007AFF`、发送(上传)
     * = 绿 `#34A853`、调度 = 紫灰 `#8E8AA0`,spec §2.1 语义色恒定,旧桌面
     * 同款 发送蓝/接收绿/调度橙 全退役;iOS TransferRowStyle.tint 同构;
     * 未知灰档防御渲染)。行徽章 = 词面着色式(iOS badgeLabel 同形:字面 +
     * 同色 12% 底,无方向箭头字形)。
     */
    enum class TaskBadge(val label: String, val color: Color) {
        UPLOAD("发送", SynshuSemantic.Send),
        DOWNLOAD("接收", SynshuSemantic.Receive),
        SCHEDULE("调度", SynshuSemantic.Schedule),
        OTHER("其他", SynshuColors.TextSecondary),
    }

    fun badgeFor(kind: String): TaskBadge = when (kind) {
        "upload" -> TaskBadge.UPLOAD
        "download", "incoming" -> TaskBadge.DOWNLOAD
        "remote_copy" -> TaskBadge.SCHEDULE
        else -> TaskBadge.OTHER
    }

    /**
     * 分段过滤四档(spec §3-5,序 = 全部/接收/发送/调度;iOS TransferFilter
     * 同序同面)。段过滤:「接收」含被动来件 incoming(与 [badgeFor] 口径
     * 同源);「发送」= upload;「调度」= remote_copy。
     */
    enum class TransferFilter(val label: String) {
        ALL("全部"),
        RECEIVE("接收"),
        SEND("发送"),
        SCHEDULE("调度"),
        ;

        fun matches(kind: String): Boolean = when (this) {
            ALL -> true
            RECEIVE -> kind == "download" || kind == "incoming"
            SEND -> kind == "upload"
            SCHEDULE -> kind == "remote_copy"
        }
    }

    /** 按分段过滤(保序——最新仍在前;越界兜底「全部」由调用面 ordinal 取值)。 */
    fun filterRows(rows: List<TransferJob>, filter: TransferFilter): List<TransferJob> =
        rows.filter { filter.matches(it.kind) }

    /** 任务名直读(V3-1 Task 10 起 name 由 daemon 单源计算,多项已带
     * 「等 N 项」后缀;final review I2 去移动端自拼「· N 项」双后缀,镜像
     * 桌面 jobDisplayName;空名回退 id)。 */
    fun displayName(job: TransferJob): String =
        job.name.takeIf { it.isNotEmpty() } ?: job.id

    /**
     * 状态文案(queued/running 归并「进行中」;未知状态原样,不崩)。
     * - publishing(P4a 发布窗,finalize 完成、发布确认前)→「保存中」;
     * - interrupted →「已中断」(iOS 同面;红因空兜底同词面,见 [causeText]);
     * - failed 且 error 为发布失败词面(daemon publish_coordinator 的
     *   「保存失败(N 项): …」,词面即协议呈现契约)→「保存失败」,
     *   其余失败仍「失败」(error 首因在行内错误区呈现)。
     */
    fun stateText(state: String, error: String? = null): String = when {
        state == "publishing" -> "保存中"
        state == "failed" && error?.startsWith("保存失败") == true -> "保存失败"
        state == "queued" || state == "running" -> "进行中"
        state == "paused" -> "已暂停"
        state == "interrupted" -> "已中断"
        state == "failed" -> "失败"
        state == "done" -> "完成"
        state == "cancelled" -> "已取消"
        else -> state
    }

    /**
     * 中断/失败红因文案(spec §3-5「中断失败红字原因」;iOS TransfersFormat
     * .causeText 同构):引擎 error 在场即原因(如设备离线),空则兜底
     * 「已中断」/「失败」;非终态 nil(进度面承担)。
     */
    fun causeText(state: String, error: String?): String? {
        val reason = error?.takeIf { it.isNotEmpty() } ?: return when (state) {
            "interrupted" -> "已中断"
            "failed" -> "失败"
            else -> null
        }
        return when (state) {
            "interrupted" -> "已中断 · $reason"
            "failed" -> reason
            else -> null
        }
    }

    /**
     * 跳过段(「跳过 N 项」,跳过的重复机制可见化;spec §3-5 词面带
     * 「(同名已存在)」括注,系转写自 iOS TransfersView.swift:8 未随 R7
     * 同步的页头注释——iOS 现网 TransfersFormat.skippedText 已去括注
     * (跳过成因不止同名,语境中立),fix round 1 裁定 iOS live 逐字为准):
     * skipped > 0 → 词面;0/负值 → nil(不显示)。
     */
    fun skippedText(skipped: Long): String? {
        if (skipped <= 0) return null
        return "跳过 $skipped 项"
    }

    /**
     * 方向设备文案(spec §3-5「方向设备 A → B」;iOS TransfersFormat
     * .directionText 同构):src/dst 设备段解析(upload dst = `{to}:{dir}`、
     * download src = `{dev}:{paths}`、remote_copy 两侧同形、incoming src =
     * 裸设备 id;首冒号前即设备段,Windows 路径盘符冒号不受影响);目录名
     * 解析([resolve])缺省回退裸 id,缺面「…」。
     */
    fun directionText(job: TransferJob, resolve: (String) -> String): String {
        fun dev(side: String?): String? {
            val s = side?.takeIf { it.isNotEmpty() } ?: return null
            // 首冒号前即设备段;无冒号(裸设备 id,incoming src 常态)= 原串
            val id = s.substringBefore(":")
            return id.takeIf { it.isNotEmpty() }?.let(resolve)
        }
        return when (job.kind) {
            "upload" -> "本机 → ${dev(job.dst) ?: "…"}"
            "download", "incoming" -> "${dev(job.src) ?: "…"} → 本机"
            "remote_copy" -> "${dev(job.src) ?: "…"} → ${dev(job.dst) ?: "…"}"
            else -> "…"
        }
    }

    /**
     * 完成时间文案(iOS TransferRowStyle.timeText 同构:`MM-dd HH:mm`,
     * 本地时区零填充)。时间源 = VM 现场观测的 done 翻转(契约 transfer.list/
     * event.transfer 均无时间戳,种子已完成的历史行不编造、不显示)。
     */
    fun timeText(ms: Long): String {
        val cal = java.util.Calendar.getInstance()
        cal.timeInMillis = ms
        return String.format(
            java.util.Locale.ROOT,
            "%02d-%02d %02d:%02d",
            cal.get(java.util.Calendar.MONTH) + 1,
            cal.get(java.util.Calendar.DAY_OF_MONTH),
            cal.get(java.util.Calendar.HOUR_OF_DAY),
            cal.get(java.util.Calendar.MINUTE),
        )
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
     * KEEP_SCREEN_ON 派生面:存在任一**非终态**任务(排队/运行/暂停/发布窗
     * publishing;含会话级接收行——中断是 Android 常态,接收/保存中同样不应
     * 熄屏)。纯终态历史面不占。
     */
    fun keepScreenOn(rows: List<TransferJob>): Boolean = rows.any { !isTerminalState(it.state) }
}
