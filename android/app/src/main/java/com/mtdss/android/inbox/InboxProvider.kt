package com.mtdss.android.inbox

import android.content.Context
import android.util.Log
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.fdpass.FdPassTimeoutException
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asBoolean
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.req
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch

/**
 * 收件箱应答编排(T6 fs.request_dest 的 App 侧胶水;决策在 [InboxLogic],
 * MediaStore 行为在 [MediaStoreInboxStore],两者 JVM/实机分面验证)。
 *
 * 职责链(每事件一个协程,并发请求互不阻塞;应答窗口 daemon 侧 10s):
 * 1. 订阅 `fs.request_dest {request_id, rel, size, resume}`(构造即订阅——
 *    App 生命周期内恒早于任何下载动作;**对端也可能主动 upload 到我们的
 *    收件箱**,不经本 App 的浏览/下载界面,订阅面必须是 app 级);
 * 2. 事件 → [InboxLogic.onRequest] 开暂存 → `0x02 dest_fd`(+fd)或拒开;
 * 3. ack ok:false(daemon 未受理:请求已超时/未知)→ 清暂存;**ack 超时
 *    保守保留**(fd 可能已接手,迟到的 0x03 仍能落定;发送失败则清理);
 * 4. `0x03 dest_commit {request_id, done}`(与 ack 在同一 fdpass 连接交错
 *    到达,读循环分拣)→ done:true 揭开 / false 清项。
 * 5. `fs.query_dest {request_id, rel, size}`(F7 在场查询,同一订阅面)→
 *    [InboxLogic.onQuery] 查行 → `0x04 dest_query {exists, size}`(无 fd);
 *    应答失败只记日志——daemon 侧超时自会保守按「不在场」重传。
 */
class InboxProvider(
    context: Context,
    ipc: IpcClient,
    private val channel: FdPassChannel,
    private val scope: CoroutineScope,
    store: InboxStore = MediaStoreInboxStore(context),
) {
    private val logic = InboxLogic(store)

    private val subscribed = CompletableDeferred<Unit>()

    /** 0x03 订阅落位闸(应答前 await——0x03 可能紧随 0x02 交错到达)。 */
    private val commitsSubscribed = CompletableDeferred<Unit>()

    init {
        // fs.request_dest 订阅(no-replay 事件流:先订阅后动作由 AppGraph
        // 构造序保证——本 Provider 与屏面同生,远早于首个传输动作)
        scope.launch {
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    when (frame.method) {
                        EV_REQUEST_DEST -> onRequest(frame)
                        EV_QUERY_DEST -> onQueryDest(frame)
                    }
                }
        }
        // 0x03 落定/失败收尾(daemon 事后原路推送,与 ack 交错)
        scope.launch {
            channel.commits
                .onSubscription { commitsSubscribed.complete(Unit) }
                .collect { commit ->
                    Log.i(TAG, "dest_commit request_id=${commit.requestId} done=${commit.done}")
                    logic.onCommit(commit.requestId, commit.done)
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    private fun onRequest(frame: EventFrame) {
        // 解析容错:畸形事件帧不得杀死订阅协程(镜像 IpcClient 读循环的
        // 丢弃口径;request_id 单调不复用,丢一帧即该文件按超时失败)
        val p = try {
            RequestParams(
                requestId = frame.params.req("request_id").asLong(),
                rel = frame.params.req("rel").asString(),
                size = frame.params.req("size").asLong(),
                resume = frame.params.req("resume").asBoolean(),
            )
        } catch (e: Exception) {
            Log.w(TAG, "request_dest 帧畸形,丢弃: ${e.message}")
            return
        }
        Log.i(TAG, "request_dest id=${p.requestId} rel=${p.rel} size=${p.size} resume=${p.resume}")
        // 每请求一协程:并发文件互不阻塞(MediaStore 开销毫秒级,10s 窗口充裕)
        scope.launch(Dispatchers.IO) { answer(p.requestId, p.rel, p.size, p.resume) }
    }

    private data class RequestParams(val requestId: Long, val rel: String, val size: Long, val resume: Boolean)

    private data class QueryParams(val requestId: Long, val rel: String, val size: Long)

    private suspend fun answer(requestId: Long, rel: String, size: Long, resume: Boolean) {
        commitsSubscribed.await() // 先订阅后动作(0x03 与 ack 同连接交错)
        val decision = try {
            logic.onRequest(requestId, rel, size, resume)
        } catch (e: Exception) {
            // store 抛出未归类异常:按拒开收口(daemon 侧任务失败/降级重收)
            InboxDecision.Refuse("OPEN_FAILED: ${e.message}")
        }
        when (decision) {
            is InboxDecision.Answer -> {
                val r = channel.sendDestFd(requestId, decision.handle.rawFd)
                // 本地 fd 副本使命完成(SCM_RIGHTS 在 sendmsg 时已复制;此后
                // daemon 持自己的副本读写)
                decision.handle.release()
                r.fold(
                    onSuccess = { ack ->
                        if (!ack.ok) {
                            Log.w(TAG, "dest_fd 被拒(请求已超时/未知)id=$requestId: ${ack.error}")
                            logic.onAckRejected(requestId)
                        }
                    },
                    onFailure = { e ->
                        if (e is FdPassTimeoutException) {
                            // 超时:结果未知,保守保留暂存(迟到 0x03 仍可落定;
                            // 若终无 0x03,pending 条目停留不可见态,重收时清理)
                            Log.w(TAG, "dest_fd ack 超时 id=$requestId,暂存保留", e)
                        } else {
                            Log.w(TAG, "dest_fd 发送失败 id=$requestId,清暂存", e)
                            logic.onSendFailed(requestId)
                        }
                    },
                )
            }

            is InboxDecision.Refuse -> {
                Log.w(TAG, "拒开 id=$requestId rel=$rel: ${decision.reason}")
                channel.sendDestFd(requestId, null, decision.reason)
            }
        }
    }

    /** fs.query_dest(F7 在场查询):查 MediaStore 真实行 → 0x04 dest_query
     *  应答(exists 仅当行在且 size 吻合)。应答被拖/发送失败只记日志——
     *  daemon 侧超时自会保守传输,无需清理面(查询无资源占用)。 */
    private fun onQueryDest(frame: EventFrame) {
        val p = try {
            QueryParams(
                requestId = frame.params.req("request_id").asLong(),
                rel = frame.params.req("rel").asString(),
                size = frame.params.req("size").asLong(),
            )
        } catch (e: Exception) {
            Log.w(TAG, "query_dest 帧畸形,丢弃: ${e.message}")
            return
        }
        Log.i(TAG, "query_dest id=${p.requestId} rel=${p.rel} size=${p.size}")
        // 每查询一协程:与 0x02 应答同样不阻塞订阅循环
        scope.launch(Dispatchers.IO) {
            val exists = logic.onQuery(p.rel, p.size)
            val reported = if (exists) p.size else 0L
            channel.sendDestQuery(p.requestId, exists, reported)
                .onFailure {
                    Log.w(TAG, "dest_query 应答失败 id=${p.requestId}(daemon 超时自会保守传输)", it)
                }
        }
    }

    private companion object {
        const val TAG = "InboxProvider"

        /** fs.request_dest 事件方法名(daemon jobs.rs EV_REQUEST_DEST 同串)。 */
        const val EV_REQUEST_DEST = "fs.request_dest"

        /** fs.query_dest 事件方法名(daemon jobs.rs EV_QUERY_DEST 同串)。 */
        const val EV_QUERY_DEST = "fs.query_dest"
    }
}
