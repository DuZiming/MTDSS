package com.mtdss.android.share

import android.util.Log
import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.send.SourceRefs
import com.mtdss.android.ui.tasks.ShareFailureFace
import com.mtdss.android.ui.tasks.ShareFailureRow
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max

/**
 * 主 App 分享队列消费面(V3-4 spec §4.4 T15;iOS ShareQueueConsumer +
 * ShareCenter 语义镜像,发送形态为安卓裁定分叉——桥式):
 *
 * - **消费**([ShareQueueConsumer.consumeOnce]):scan → ready 项逐文件
 *   复验(实际尺寸对声明,缺件/截短拒发「文件可能已损坏」——坏字节不上线)
 *   → 桥式发送(stage ref 条目,App 作 0x01 源,dir = 收件箱哨兵)→ 受理
 *   成功翻 sent(文件保暂存供引擎逐文件拉取);受理失败折 failed(原因落盘,
 *   暂存件原样保留——重试不重拷)。**幂等**:sent 项不再进发送面,反复调用
 *   安全。
 * - **登录门**:`loggedIn == false` 整轮不动队列(保「登录后自动发送」的
 *   等待态,而非把整队折失败;iOS consumeIfNeeded 同口径)。
 * - **重试**([ShareQueueConsumer.retryOnce]):失败行的用户动作(重选目标
 *   设备即重发,targetDeviceId 可覆盖);ready/sent 项不属重试面。
 * - **失败行**([ShareFailureController]):scan → failed 投影传输列表
 *   「分享转交失败」区(置顶区,随分段 全部/发送 出行);触发面 = 登录翻转 +
 *   前台(AppGraph 挂 Activity resumed 回调)+ 屏面打开。
 *
 * 宿主可测:纯 JVM,send 闭包注入(不依赖活引擎)。
 */

/** 消费面(逻辑核心;触发/呈现绑定归 [ShareFailureController])。 */
class ShareQueueConsumer(
    private val queue: ShareQueueStore,
    private val send: suspend (items: List<BridgeItem>, targetDeviceId: String) -> String,
    private val now: () -> Long = { System.currentTimeMillis() },
    private val copyGraceMs: Long = ShareQueueStore.DEFAULT_COPY_GRACE_MS,
) {

    data class Outcome(
        /** 受理进发送的项数(发送受理成功即算;完成与否归引擎/传输列表)。 */
        val consumed: Int,
    )

    /**
     * 一轮消费(scan → 分档处理;幂等,反复调用安全;队列 IO 恒 IO 线程,
     * 调用方可在任意线程挂):
     * - **登录前整轮不动队列**(等待态保真;扫描会折算失败态,同样不动);
     * - copying(宽限窗外)→ 已被 scan 折算 failed,跳过;
     * - ready → 逐文件复验 → 桥式发送 → 翻 sent;
     * - sent → 跳过(已受理;进度归引擎任务列表);
     * - failed → 跳过(呈现面上浮,重试是用户动作 [retryOnce])。
     *
     * at-least-once 窗口:受理成功与翻 sent 之间进程被杀 = 下轮重发(对端
     * 断点续传/跳过已收完兜底,不产生重复收件;iOS 同窗口语义)。
     */
    suspend fun consumeOnce(loggedIn: Boolean): Outcome = withContext(Dispatchers.IO) {
        if (!loggedIn) return@withContext Outcome(consumed = 0) // 登录前不动队列(保等待态)
        val entries = queue.scan(now(), copyGraceMs)
        var consumed = 0
        for (entry in entries) {
            if (entry.meta.status == ShareQueueStatus.READY && consume(entry, overrideTarget = null)) {
                consumed++
            }
        }
        Outcome(consumed = consumed)
    }

    /**
     * 用户重试单条失败项(传输列表「分享转交失败」行 → 重选目标设备 → 本
     * 入口;[overrideTargetDeviceId] 非空即覆盖原目标):暂存件逐文件复验
     * (完整件过,残件拒)→ 重发 → 成功翻 sent。true = 发送已受理。
     */
    suspend fun retryOnce(id: String, overrideTargetDeviceId: String?): Boolean =
        withContext(Dispatchers.IO) {
            val entries = queue.scan(now(), copyGraceMs)
            val entry = entries.firstOrNull { it.id == id } ?: return@withContext false
            if (entry.meta.status != ShareQueueStatus.FAILED) return@withContext false // 重试只属失败行
            consume(entry, overrideTarget = overrideTargetDeviceId?.takeIf { it.isNotEmpty() })
        }

    /** 单项消费;true = 发送已受理。 */
    private suspend fun consume(entry: ShareQueueEntry, overrideTarget: String?): Boolean {
        val target = overrideTarget ?: entry.meta.targetDeviceId
        // ⓪ 防御:空条目/空目标不进发送面(评审 fix-1 同轮:孤儿折算行的
        //    重试在此让路——「文件拷贝中断」原因保留,不被「发送(空条目)」
        //    的困惑错误覆写;该等行的用户动作 = 清除)
        if (entry.meta.files.isEmpty() || target.isBlank()) return false
        // ① 逐文件尺寸复验(缺件/截短拒发——盘损/篡改字节不上线)
        val staged = ArrayList<File>(entry.meta.files.size)
        for (file in entry.meta.files) {
            val f = queue.stagedFile(entry.id, file.name)
            if (f == null || f.length() != file.size) {
                queue.markFailed(entry.id, ERROR_SIZE_MISMATCH)
                return false
            }
            staged += f
        }
        // ② 桥式发送(暂存件作源;App 侧 0x01 应答,dir = 收件箱哨兵)
        val items = staged.map { f ->
            BridgeItem(
                ref = SourceRefs.stageRef(entry.id, f.name),
                rel = f.name,
                size = f.length(),
                mtime = entry.meta.enqueuedAt,
            )
        }
        try {
            send(items, target)
        } catch (e: Exception) {
            // 不静默丢:暂存件原样保留 + failed 态(重试不重拷)
            queue.markFailed(entry.id, errorText(e))
            return false
        }
        // ③ 受理成功翻 sent + sent_at 落盘(清扫唯一锚——休眠件受理后暂存件
        //    保真;文件保暂存供引擎拉取,清扫交 scan 按 sent_at 超窗)
        queue.markSent(entry.id, now())
        Log.i(TAG, "分享项已受理 id=${entry.id} target=$target files=${items.size}")
        return true
    }

    companion object {

        private const val TAG = "ShareConsumer"

        /** 拷贝中断折算文案(scan 折算落盘用;iOS errorCopyInterrupted 同字面)。 */
        const val ERROR_COPY_INTERRUPTED = "文件拷贝中断"

        /** 尺寸守卫拒发文案(盘损/缺件;iOS errorSizeMismatch 的任务钉字面)。 */
        const val ERROR_SIZE_MISMATCH = "文件可能已损坏"

        /** 错误词面透传(message 优先;空兜底类名——不静默丢)。 */
        fun errorText(e: Throwable): String =
            e.message?.takeIf { it.isNotBlank() } ?: e.javaClass.simpleName
    }
}

/**
 * 分享转交控制器(app 级绑定;iOS ShareCenter 同构):消费触发(登录翻转/
 * 前台 resumed/屏面打开)+ 失败行投影(传输列表「分享转交失败」区,T8 渲染位
 * 接真源)+ 重试/清除出键。
 *
 * [ShareFailureFace] 实现面:
 * - [rows] 失败行流(scan 驱动;消费/重试/清除后刷新);
 * - [onRetry] 置重选设备请求([retryRequest] 流出键)→ 屏面挂既有
 *   DevicePickerSheet → [retryWith](重选即重发,目标覆盖);
 * - [onClear] 队列项整移除(暂存件一并回收);
 * - [refresh] 打开/回前台触发(幂等;登录门在消费面)。
 */
class ShareFailureController(
    private val store: ShareQueueStore,
    private val loggedIn: StateFlow<Boolean?>,
    private val scope: CoroutineScope,
    private val send: suspend (items: List<BridgeItem>, targetDeviceId: String) -> String,
    private val now: () -> Long = { System.currentTimeMillis() },
) : ShareFailureFace {

    private val consumer = ShareQueueConsumer(store, send, now)

    /** 消费在途闸(前台/登录双触发合并为一轮;幂等面)。 */
    private val consuming = AtomicBoolean(false)

    private val _rows = MutableStateFlow<List<ShareFailureRow>>(emptyList())

    private val _retryRequest = MutableStateFlow<String?>(null)

    init {
        // 登录翻转触发(false/null → true 翻转;冷启动首值 true 同样触发——
        // 队列里可能有上个会话暂存的等待件)
        scope.launch {
            var last: Boolean? = null
            loggedIn.collect { v ->
                if (v == true && last != true) consumeIfNeeded()
                last = v
            }
        }
    }

    // ── 消费触发 ──────────────────────────────────────────────────────────

    /** 前台/登录触发(异步一轮;在飞消费窗合并。行刷新随轮内收口——不在
     *  调用线程做盘扫,Main 调用零磁盘 IO)。 */
    override fun refresh() {
        consumeIfNeeded()
    }

    fun consumeIfNeeded() {
        if (!consuming.compareAndSet(false, true)) return
        scope.launch {
            try {
                consumer.consumeOnce(loggedIn.value == true)
            } finally {
                consuming.set(false)
            }
            refreshRows()
        }
    }

    /**
     * 等待版一轮消费(分享面板入队后的收口判定:调用方随后读 meta 分支
     * 「已开始发送/已暂存等待/转交失败」)。在飞消费在途时让路返回 false
     * (面板按等待态呈现,失败行自随触发出)。
     */
    suspend fun consumeAwaiting(): Boolean {
        if (!consuming.compareAndSet(false, true)) return false
        try {
            consumer.consumeOnce(loggedIn.value == true)
        } finally {
            consuming.set(false)
        }
        withContext(Dispatchers.IO) { refreshRows() }
        return true
    }

    // ── 失败行面(ShareFailureFace)────────────────────────────────────────

    override fun rows(): StateFlow<List<ShareFailureRow>> = _rows.asStateFlow()

    override fun retryRequest(): StateFlow<String?> = _retryRequest.asStateFlow()

    override fun onRetry(id: String) {
        _retryRequest.value = id
    }

    /** 重选设备回调:清请求 → 换目标重发(重试不重拷)→ 行随刷新。 */
    override fun retryWith(id: String, targetDeviceId: String) {
        _retryRequest.value = null
        scope.launch {
            consumer.retryOnce(id, overrideTargetDeviceId = targetDeviceId)
            refreshRows()
        }
    }

    /** 重选层取消(下滑收层):请求清位,失败行原样保留。 */
    override fun cancelRetry() {
        _retryRequest.value = null
    }

    override fun onClear(id: String) {
        _retryRequest.value = null
        scope.launch {
            store.removeItem(id)
            refreshRows()
        }
    }

    /** 失败行刷新(scan → failed 投影;iOS ShareCenter.refreshRows 同构)。 */
    fun refreshRows() {
        _rows.value = store.scan(now())
            .filter { it.meta.status == ShareQueueStatus.FAILED }
            .map { entry ->
                ShareFailureRow(
                    id = entry.id,
                    name = entry.meta.sourceName.ifEmpty { "分享内容" },
                    filesTotal = max(entry.meta.files.size.toLong(), 1L),
                    reason = entry.meta.error,
                )
            }
    }
}
