package com.mtdss.android.ui.tasks

import androidx.compose.ui.graphics.Color
import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.ipc.TransferProgress
import com.mtdss.android.ui.tasks.TasksReducer.TaskAction
import com.mtdss.android.ui.tasks.TasksReducer.TransferFilter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Calendar

/**
 * 任务列表纯逻辑层 JVM 单测:快照合并(事件增量/终态粘滞/会话行终态即
 * 消失)、操作可用性矩阵(含同签名在途抑制)、呈现投影、KEEP_SCREEN_ON
 * 派生。语义锚 = 桌面 ui/src/lib/transfer.test.ts 的收敛用例族。
 */
class TasksReducerTest {

    private fun job(
        id: String,
        kind: String = "upload",
        state: String = "running",
        name: String = "样本.bin",
        src: String? = "srcA:/样本.bin",
        dst: String? = "devB:/inbox",
        error: String? = null,
        filesTotal: Long = 1,
        bytesTotal: Long = 4096,
        relay: Boolean = false,
        skipped: Long = 0,
    ) = TransferJob(
        id = id, kind = kind, name = name, state = state,
        filesDone = 0, filesTotal = filesTotal, bytesDone = 0, bytesTotal = bytesTotal,
        speedBps = 0, skipped = skipped, relay = relay, src = src, dst = dst, error = error,
    )

    private fun evt(
        id: String,
        kind: String = "upload",
        state: String = "running",
        bytesDone: Long = 0,
        bytesTotal: Long = 4096,
        speedBps: Long = 0,
        relay: Boolean = false,
        error: String? = null,
    ) = TransferProgress(
        id = id, kind = kind, state = state,
        filesDone = 1, filesTotal = 1, bytesDone = bytesDone, bytesTotal = bytesTotal,
        speedBps = speedBps, skipped = 0, relay = relay, error = error,
    )

    // ── seed:全量重置 ──────────────────────────────────────────────────────

    @Test
    fun `seed 保留终态历史行,滤除终态会话行,反转最新在上`() {
        val rows = TasksReducer.seed(
            listOf(
                job("1", state = "failed"),
                job("2", kind = "download", state = "done"),
                job("3", kind = "incoming", state = "done"), // 会话卡终态 → 不进列表
                job("4", kind = "incoming", state = "running"), // 活态会话行保留
                job("5", kind = "local_copy", state = "cancelled"), // 会话卡终态 → 滤除
                job("6", state = "interrupted"), // store 历史行保留(重发/清除入口)
            ),
        )
        // daemon 快照 = job_seq 升序(最旧在前)→ 反转即最新在最上
        assertEquals(listOf("6", "4", "2", "1"), rows.map { it.id })
    }

    @Test
    fun `seed 对重复 id 防御去重(首见胜)`() {
        val rows = TasksReducer.seed(
            listOf(job("1", state = "running"), job("1", state = "done")),
        )
        assertEquals(1, rows.size)
        assertEquals("running", rows[0].state)
    }

    // ── applyEvent:事件增量合并 ─────────────────────────────────────────────

    @Test
    fun `事件增量更新计数与状态,保留 name src dst`() {
        val rows = listOf(job("1", bytesTotal = 8192))
        val out = TasksReducer.applyEvent(rows, evt("1", bytesDone = 4096, bytesTotal = 8192, speedBps = 1024, relay = true))
        assertEquals(1, out.size)
        val r = out[0]
        assertEquals("running", r.state)
        assertEquals(4096, r.bytesDone)
        assertEquals(1024, r.speedBps)
        assertTrue(r.relay)
        // 事件帧不带的面保留前值
        assertEquals("样本.bin", r.name)
        assertEquals("srcA:/样本.bin", r.src)
        assertEquals("devB:/inbox", r.dst)
    }

    @Test
    fun `全新 id 事件建行,显示名兜底 id`() {
        val out = TasksReducer.applyEvent(emptyList(), evt("9"))
        assertEquals(1, out.size)
        assertEquals("9", out[0].name)
        assertNull(out[0].src)
        assertNull(out[0].dst)
    }

    @Test
    fun `终态粘滞,迟到进度帧整体忽略`() {
        val rows = listOf(job("1", state = "done", bytesTotal = 4096))
        val out = TasksReducer.applyEvent(rows, evt("1", state = "running", bytesDone = 100))
        assertEquals("done", out[0].state)
        assertEquals(0, out[0].bytesDone) // 迟到帧的计数不落
    }

    @Test
    fun `会话行终态即消失`() {
        val rows = listOf(job("1", kind = "incoming", state = "running"), job("2"))
        val out = TasksReducer.applyEvent(rows, evt("1", kind = "incoming", state = "done"))
        assertEquals(listOf("2"), out.map { it.id })
    }

    @Test
    fun `会话 kind 的终态事件不建行`() {
        val out = TasksReducer.applyEvent(emptyList(), evt("9", kind = "incoming", state = "failed", error = "x"))
        assertTrue(out.isEmpty())
    }

    @Test
    fun `终态事件带 error 落行`() {
        val rows = listOf(job("1"))
        val out = TasksReducer.applyEvent(rows, evt("1", state = "failed", error = "信令未在线"))
        assertEquals("failed", out[0].state)
        assertEquals("信令未在线", out[0].error)
    }

    // ── 操作可用性矩阵 ──────────────────────────────────────────────────────

    @Test
    fun `操作矩阵全态遍历`() {
        fun acts(state: String, kind: String = "upload") =
            TasksReducer.actionsFor(job("x", kind = kind, state = state), listOf(job("x", kind = kind, state = state)))

        assertEquals(listOf(TaskAction.PAUSE, TaskAction.CANCEL), acts("queued"))
        assertEquals(listOf(TaskAction.PAUSE, TaskAction.CANCEL), acts("running"))
        // remote_copy 无本机数据面拒 pause,仅取消
        assertEquals(listOf(TaskAction.CANCEL), acts("running", kind = "remote_copy"))
        assertEquals(listOf(TaskAction.RESUME, TaskAction.CANCEL), acts("paused"))
        assertEquals(listOf(TaskAction.REINITIATE, TaskAction.CLEAR), acts("failed"))
        assertEquals(listOf(TaskAction.REINITIATE, TaskAction.CLEAR), acts("interrupted"))
        assertEquals(listOf(TaskAction.CLEAR), acts("cancelled"))
        assertEquals(emptyList<TaskAction>(), acts("done"))
        assertEquals(emptyList<TaskAction>(), acts("weird-state")) // 未知状态防御
        // 会话卡:无记录面,终态动作必报「任务不存在」→ 不出键;活态照常暂停/取消
        assertEquals(listOf(TaskAction.PAUSE, TaskAction.CANCEL), acts("running", kind = "incoming"))
        assertEquals(emptyList<TaskAction>(), acts("failed", kind = "incoming"))
        assertEquals(emptyList<TaskAction>(), acts("failed", kind = "local_copy"))
        assertEquals(emptyList<TaskAction>(), acts("cancelled", kind = "incoming"))
    }

    @Test
    fun `同签名在途行抑制重发与清除`() {
        val old = job("1", state = "failed", src = "bridge://a", dst = "devB:/inbox")
        val running = job("2", state = "running", src = "bridge://a", dst = "devB:/inbox")

        // 在途同签名(典型:bridge 上传 App 侧重建后的旧行 + 新行)
        assertEquals(emptyList<TaskAction>(), TasksReducer.actionsFor(old, listOf(old, running)))
        // 在途行终态 → 旧行动作恢复
        val done = running.copy(state = "done")
        assertEquals(
            listOf(TaskAction.REINITIATE, TaskAction.CLEAR),
            TasksReducer.actionsFor(old, listOf(old, done)),
        )
        // 签名不同(dst/src/kind 任一)→ 不抑制
        val otherDst = running.copy(dst = "devB:/other")
        assertEquals(
            listOf(TaskAction.REINITIATE, TaskAction.CLEAR),
            TasksReducer.actionsFor(old, listOf(old, otherDst)),
        )
        // 已取消行同样被在途同签名抑制(清除会删对端 resume 位图)
        val cancelled = job("3", state = "cancelled", src = "bridge://a", dst = "devB:/inbox")
        assertEquals(emptyList<TaskAction>(), TasksReducer.actionsFor(cancelled, listOf(cancelled, running)))
    }

    // ── 呈现投影 ────────────────────────────────────────────────────────────

    @Test
    fun `显示名单文件与多项`() {
        // final review I2:name 由 daemon 单源计算(多项已带「等 N 项」单
        // 后缀),显示名直读不再自拼「· N 项」双后缀;空名回退 id
        assertEquals("样本.bin", TasksReducer.displayName(job("1", filesTotal = 1)))
        assertEquals(
            "报告.pdf 等 3 项",
            TasksReducer.displayName(job("1", name = "报告.pdf 等 3 项", filesTotal = 3)),
        )
        assertEquals("7", TasksReducer.displayName(job("7", name = "", filesTotal = 1))) // 空名回退 id
    }

    @Test
    fun `徽章分类`() {
        assertEquals(TasksReducer.TaskBadge.UPLOAD, TasksReducer.badgeFor("upload"))
        assertEquals(TasksReducer.TaskBadge.DOWNLOAD, TasksReducer.badgeFor("download"))
        assertEquals(TasksReducer.TaskBadge.DOWNLOAD, TasksReducer.badgeFor("incoming"))
        assertEquals(TasksReducer.TaskBadge.SCHEDULE, TasksReducer.badgeFor("remote_copy"))
        assertEquals(TasksReducer.TaskBadge.OTHER, TasksReducer.badgeFor("local_copy"))
        assertEquals(TasksReducer.TaskBadge.OTHER, TasksReducer.badgeFor("weird"))
    }

    @Test
    fun `进度比与速率文案`() {
        assertNull(TasksReducer.progressRatio(10, 0)) // 总量未知 → 不确定态
        assertNull(TasksReducer.progressRatio(10, -1))
        assertEquals(0.5f, TasksReducer.progressRatio(512, 1024)!!, 1e-6f)
        assertEquals(1.0f, TasksReducer.progressRatio(2048, 1024)!!, 1e-6f) // 越界夹取
        assertEquals(0.0f, TasksReducer.progressRatio(-5, 1024)!!, 1e-6f)

        assertEquals("0 B/s", TasksReducer.speedText(0))
        assertEquals("0 B/s", TasksReducer.speedText(-3))
        assertEquals("512 B/s", TasksReducer.speedText(512))
        assertEquals("2.0 KB/s", TasksReducer.speedText(2048))
        assertEquals("1.5 MB/s", TasksReducer.speedText(1024 * 1024 + 512 * 1024))
        assertEquals("1.0 GB/s", TasksReducer.speedText(1024L * 1024 * 1024))
    }

    // ── publishing(保存中,P4a 发布窗)────────────────────────────────────

    @Test
    fun `publishing 渲染保存中 非终态 动作只余取消`() {
        // 发布窗(finalize 完成、发布确认前):非终态(终态粘滞/会话卡摘行
        // 均不触发);文案「保存中」;无网络可暂停,取消合法(协调器不中断)
        assertFalse(TasksReducer.isTerminalState("publishing"))
        assertEquals("保存中", TasksReducer.stateText("publishing"))
        assertEquals(
            listOf(TaskAction.CANCEL),
            TasksReducer.actionsFor(job("1", state = "publishing"), listOf(job("1", state = "publishing"))),
        )
    }

    @Test
    fun `publishing 含接收行语义 活态保留且占熄屏`() {
        // 会话级接收行(incoming)进 publishing:活态保留(终态才摘),
        // KEEP_SCREEN_ON 占位(保存中不应熄屏,沿 P2 接收行语义)
        val out = TasksReducer.applyEvent(
            listOf(job("1", kind = "incoming", state = "running")),
            evt("1", kind = "incoming", state = "publishing"),
        )
        assertEquals(1, out.size)
        assertEquals("publishing", out[0].state)
        assertTrue(TasksReducer.keepScreenOn(listOf(job("1", kind = "incoming", state = "publishing"))))
        assertTrue(TasksReducer.keepScreenOn(listOf(job("1", kind = "download", state = "publishing"))))
    }

    @Test
    fun `failed 保存失败词面与普通失败分叉`() {
        // 发布失败终因(daemon publish_coordinator 词面「保存失败(N 项): 首因」)
        // → 状态列呈「保存失败」;其余失败仍「失败」
        assertEquals("保存失败", TasksReducer.stateText("failed", "保存失败(1 项): App 发布失败: 磁盘满"))
        assertEquals("失败", TasksReducer.stateText("failed", "信令未在线"))
        assertEquals("失败", TasksReducer.stateText("failed"))
    }

    // ── KEEP_SCREEN_ON 派生 ────────────────────────────────────────────────

    @Test
    fun `keepScreenOn 只看非终态存在性`() {
        assertFalse(TasksReducer.keepScreenOn(emptyList()))
        assertFalse(TasksReducer.keepScreenOn(listOf(job("1", state = "done"), job("2", state = "failed"))))
        assertTrue(TasksReducer.keepScreenOn(listOf(job("1", state = "done"), job("2", state = "running"))))
        // 排队/暂停都算活;会话级接收行同样占位(接收中不应熄屏)
        assertTrue(TasksReducer.keepScreenOn(listOf(job("1", state = "queued"))))
        assertTrue(TasksReducer.keepScreenOn(listOf(job("1", state = "paused"))))
        assertTrue(TasksReducer.keepScreenOn(listOf(job("1", kind = "incoming", state = "running"))))
    }

    // ── V3-4 传输列表重写(spec §3-5;iOS TransfersFormat 镜像)───────────────

    @Test
    fun `徽章换向色 接收蓝 发送绿 调度紫灰`() {
        // spec §2.1 语义色恒定 + §3-5 徽章换向(iOS TransferRowStyle.tint 同构;
        // 旧桌面同款配色 发送蓝/接收绿/调度橙 全退役)
        assertEquals(Color(0xFF34A853), TasksReducer.TaskBadge.UPLOAD.color) // 发送 = 绿
        assertEquals(Color(0xFF007AFF), TasksReducer.TaskBadge.DOWNLOAD.color) // 接收 = 蓝
        assertEquals(Color(0xFF8E8AA0), TasksReducer.TaskBadge.SCHEDULE.color) // 调度 = 紫灰
        assertEquals(Color(0xFF8E8E93), TasksReducer.TaskBadge.OTHER.color) // 未知灰档防御
        // 徽章词面(iOS badgeLabel 同面)
        assertEquals("发送", TasksReducer.TaskBadge.UPLOAD.label)
        assertEquals("接收", TasksReducer.TaskBadge.DOWNLOAD.label)
        assertEquals("调度", TasksReducer.TaskBadge.SCHEDULE.label)
    }

    @Test
    fun `最新在上 事件新行插顶,既有行原位更新不挪位`() {
        val rows = TasksReducer.seed(listOf(job("1"), job("2"), job("3")))
        assertEquals(listOf("3", "2", "1"), rows.map { it.id })
        // 全新 id 事件 → 插顶(iOS rows.insert(at: 0) 同构)
        val withNew = TasksReducer.applyEvent(rows, evt("9"))
        assertEquals(listOf("9", "3", "2", "1"), withNew.map { it.id })
        // 既有行进度帧 → 原位更新,序不变(最新仍在前)
        val updated = TasksReducer.applyEvent(withNew, evt("2", bytesDone = 100))
        assertEquals(listOf("9", "3", "2", "1"), updated.map { it.id })
    }

    @Test
    fun `分段过滤四档 接收含 incoming`() {
        val rows = listOf(
            job("u", kind = "upload"),
            job("d", kind = "download"),
            job("i", kind = "incoming"),
            job("r", kind = "remote_copy"),
            job("l", kind = "local_copy"),
        )
        assertEquals(
            listOf("u", "d", "i", "r", "l"),
            TasksReducer.filterRows(rows, TransferFilter.ALL).map { it.id },
        )
        assertEquals(
            listOf("d", "i"),
            TasksReducer.filterRows(rows, TransferFilter.RECEIVE).map { it.id },
        )
        assertEquals(listOf("u"), TasksReducer.filterRows(rows, TransferFilter.SEND).map { it.id })
        assertEquals(listOf("r"), TasksReducer.filterRows(rows, TransferFilter.SCHEDULE).map { it.id })
        // 段序与词面(spec §3-5 = 全部/接收/发送/调度;iOS TransferFilter 同序同面)
        assertEquals(listOf("全部", "接收", "发送", "调度"), TransferFilter.entries.map { it.label })
    }

    @Test
    fun `左滑出集按状态矩阵`() {
        fun swipe(state: String, kind: String = "upload"): List<TaskAction> {
            val j = job("x", kind = kind, state = state)
            return TasksReducer.swipeActionsFor(j, listOf(j))
        }
        // 进行中(queued/running/paused/publishing)→ 仅取消(pause/resume 契约
        // 在而 UI 不上此页,iOS 同构);remote_copy/活态会话行同取消
        assertEquals(listOf(TaskAction.CANCEL), swipe("queued"))
        assertEquals(listOf(TaskAction.CANCEL), swipe("running"))
        assertEquals(listOf(TaskAction.CANCEL), swipe("paused"))
        assertEquals(listOf(TaskAction.CANCEL), swipe("publishing"))
        assertEquals(listOf(TaskAction.CANCEL), swipe("running", kind = "remote_copy"))
        assertEquals(listOf(TaskAction.CANCEL), swipe("running", kind = "incoming"))
        // 中断/失败 → 重新发起+清除
        assertEquals(listOf(TaskAction.REINITIATE, TaskAction.CLEAR), swipe("interrupted"))
        assertEquals(listOf(TaskAction.REINITIATE, TaskAction.CLEAR), swipe("failed"))
        // 完成/已取消 → 清除(iOS T5b Ruling K:done 恢复清除入口)
        assertEquals(listOf(TaskAction.CLEAR), swipe("done"))
        assertEquals(listOf(TaskAction.CLEAR), swipe("cancelled"))
        // 会话卡终态无动作(无记录面,空集行不示能);未知状态防御
        assertEquals(emptyList<TaskAction>(), swipe("failed", kind = "incoming"))
        assertEquals(emptyList<TaskAction>(), swipe("done", kind = "local_copy"))
        assertEquals(emptyList<TaskAction>(), swipe("cancelled", kind = "incoming"))
        assertEquals(emptyList<TaskAction>(), swipe("weird-state"))
        // 同签名在途抑制:终态行(含 done)动作一并隐藏
        val failed = job("f", state = "failed", src = "bridge://a", dst = "d:/in")
        val running = job("r2", state = "running", src = "bridge://a", dst = "d:/in")
        assertEquals(emptyList<TaskAction>(), TasksReducer.swipeActionsFor(failed, listOf(failed, running)))
        val doneRow = job("d2", state = "done", src = "bridge://a", dst = "d:/in")
        assertEquals(emptyList<TaskAction>(), TasksReducer.swipeActionsFor(doneRow, listOf(doneRow, running)))
    }

    @Test
    fun `跳过段文案`() {
        // spec §3-5 词面「跳过 N 项(同名已存在)」;0/负值不显示(iOS skippedText 同构)
        assertNull(TasksReducer.skippedText(0))
        assertNull(TasksReducer.skippedText(-1))
        assertEquals("跳过 2 项(同名已存在)", TasksReducer.skippedText(2))
        assertEquals("跳过 1 项(同名已存在)", TasksReducer.skippedText(1))
    }

    @Test
    fun `中断失败红因文案 error 空兜底已中断`() {
        // 状态词面:interrupted = 已中断(iOS 同面,旧「中断」退役)
        assertEquals("已中断", TasksReducer.stateText("interrupted"))
        // 红因文案(spec §3-5:中断/失败 = 红字原因,error 空兜底「已中断」)
        assertEquals("已中断", TasksReducer.causeText("interrupted", null))
        assertEquals("已中断", TasksReducer.causeText("interrupted", ""))
        assertEquals("已中断 · 设备离线", TasksReducer.causeText("interrupted", "设备离线"))
        assertEquals("失败", TasksReducer.causeText("failed", null))
        assertEquals("信令未在线", TasksReducer.causeText("failed", "信令未在线"))
        // 非中断/失败态无红因(进度面承担)
        assertNull(TasksReducer.causeText("running", null))
        assertNull(TasksReducer.causeText("done", null))
    }

    @Test
    fun `方向设备文案 本机锚定`() {
        // upload:本机 → 对端({to}:{dir} 首冒号前即设备段,Windows 盘符冒号不受影响)
        val names = mapOf("devB" to "公司台式机")
        fun dir(kind: String, src: String?, dst: String?, resolve: (String) -> String = { it }) =
            TasksReducer.directionText(job("x", kind = kind, src = src, dst = dst), resolve)
        assertEquals("本机 → devB", dir("upload", "srcA:/样本.bin", "devB:/inbox"))
        assertEquals("本机 → 公司台式机", dir("upload", null, "devB:/inbox") { names[it] ?: it })
        // download/incoming:源设备 → 本机
        assertEquals("devA → 本机", dir("download", "devA:/远端/文档.pdf", "inbox"))
        assertEquals("公司台式机 → 本机", dir("download", "devB:/远端/文档.pdf", "inbox") { names[it] ?: it })
        assertEquals("devA → 本机", dir("incoming", "devA", null))
        // remote_copy:A → B;缺面/空 = 「…」;未知 id 缺名回退裸 id
        assertEquals("devA → devB", dir("remote_copy", "devA:/a", "devB:/b"))
        assertEquals("… → …", dir("remote_copy", null, null))
        assertEquals("本机 → …", dir("upload", null, ""))
        assertEquals("devZ → 本机", dir("download", "devZ:/x", "inbox") { names[it] ?: it })
        // 未知 kind 防御(iOS default → 「…」)
        assertEquals("…", dir("local_copy", "a", "b"))
    }

    @Test
    fun `完成时间文案 月日时分格式`() {
        // iOS TransferRowStyle.timeText 同构(en_US_POSIX,本地时区,零填充)
        val cal = Calendar.getInstance()
        cal.clear()
        cal.set(2026, Calendar.NOVEMBER, 8, 9, 5, 0)
        assertEquals("11-08 09:05", TasksReducer.timeText(cal.timeInMillis))
        val cal2 = Calendar.getInstance()
        cal2.clear()
        cal2.set(2026, Calendar.MARCH, 5, 23, 59, 0)
        assertEquals("03-05 23:59", TasksReducer.timeText(cal2.timeInMillis))
    }

    @Test
    fun `清除已完成出集与使能面`() {
        val rows = listOf(
            job("1", state = "failed"),
            job("2", kind = "download", state = "done"),
            job("3", kind = "incoming", state = "done"), // 会话卡无记录面
            job("4", state = "running", src = "devX:/a", dst = "devY:/b"), // 独立签名
            job("5", state = "cancelled"),
        )
        // 可清集 = 有记录面的终态行(done/cancelled/interrupted/failed 同权,
        // iOS Ruling K;会话卡与活态行不出集)
        assertEquals(listOf("1", "2", "5"), TasksReducer.clearCompletedIds(rows))
        assertTrue(TasksReducer.hasClearable(rows))
        assertFalse(TasksReducer.hasClearable(listOf(job("4", state = "running"))))
        assertFalse(TasksReducer.hasClearable(emptyList()))
        // 同签名在途抑制同单行口径(清除会删对端 resume 位图)
        val failed = job("f", state = "failed", src = "b://a", dst = "d:/in")
        val running = job("r", state = "running", src = "b://a", dst = "d:/in")
        assertEquals(emptyList<String>(), TasksReducer.clearCompletedIds(listOf(failed, running)))
    }
}
