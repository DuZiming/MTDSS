package com.mtdss.android.inbox

import android.util.Log
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.asLong
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.req
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch

/**
 * 收件箱发布器(P4a 发布协议的 App 侧胶水;决策在 [InboxLogic],SAF
 * 授权树行为在 [SafInboxStore],两者 JVM/实机分面验证;0x04 在场查询面同舱)。
 *
 * 职责链(每事件一个协程,并发发布互不阻塞;应答窗口 daemon 侧发布 30s/
 * 查询 3s):
 * 1. 订阅 `fs.publish {request_id, rel, staging_path, size}` 与
 *    `fs.query_dest {request_id, rel, size}`(构造即订阅——先订阅后动作由
 *    AppGraph 构造序保证;订阅面必须是 app 级:接收任务的发布请求不经本
 *    App 界面即可到达);
 * 2. fs.publish → policy 解析(任务列表内存快照缝 [policyFor],rel 同任务
 *    匹配;取不到缺省 overwrite,见下「policy 缺省」钉)→
 *    [InboxLogic.onPublish] 发布 → `0x05 publish_result {error?, ok,
 *    request_id}`(无 fd;Skipped 亦 ok:true——文件已在场,daemon 删暂存+
 *    打标收尾照常);应答失败只记日志——0x05 帧多半已在途(daemon 收到即
 *    落定),丢帧时 daemon 30s 超时自会「保存失败」收口(暂存保留);
 * 3. fs.query_dest → [InboxLogic.onQuery] 查行 → `0x04 dest_query {exists,
 *    request_id, size}`(无 fd);应答失败同只记日志(daemon 超时保守按
 *    不在场传输)。
 *
 * **通道实例纪律(P4a 改二)**:0x04 与 0x05 共用本类持有的**查询通道**
 * ——daemon 侧 DestQueries 单 resolver 单计数器(spec §6.5),App 侧单通道
 * 单表天然无错路;0x02 dest 通道已随 fd 桥退役。
 *
 * **policy 缺省 overwrite(钉)**:fs.publish 不携带 policy(T1 协议定),
 * [policyFor] 即任务列表内存快照的查询缝(现装配恒缺省 null——直接收件
 * 任务的 policy 已在发起时生效于对端〔引擎侧 unique_target 改名/skip 计数〕,
 * 发布撞名是本端第二次撞,缺省 overwrite 与收件箱「落最新」直觉一致;
 * skip 名单命中仍尊弃件语义,见 [TransferPolicy])。
 */
class InboxProvider(
    ipc: IpcClient,
    /** 查询通道(0x04 在场查询 + 0x05 发布应答;与 daemon DestQueries 同一
     *  id 计数空间,单通道单表,见类注释「通道实例纪律」)。 */
    private val channel: FdPassChannel,
    private val scope: CoroutineScope,
    store: InboxStore,
    /** 发布撞名策略快照(rel → policy;null = 快照无匹配 → 缺省 overwrite)。 */
    private val policyFor: (rel: String) -> TransferPolicy? = { null },
) {
    private val logic = InboxLogic(store)

    private val subscribed = CompletableDeferred<Unit>()

    init {
        // fs.publish / fs.query_dest 订阅(no-replay 事件流:先订阅后动作
        // 由 AppGraph 构造序保证——本 Provider 与屏面同生,远早于首个传输动作)
        scope.launch {
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame ->
                    when (frame.method) {
                        EV_PUBLISH -> onPublish(frame)
                        EV_QUERY_DEST -> onQueryDest(frame)
                    }
                }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    private fun onPublish(frame: EventFrame) {
        // 解析容错:畸形事件帧不得杀死订阅协程(镜像 IpcClient 读循环的
        // 丢弃口径;request_id 单调不复用,丢一帧即该文件按发布超时失败)
        val p = try {
            PublishParams(
                requestId = frame.params.req("request_id").asLong(),
                rel = frame.params.req("rel").asString(),
                stagingPath = frame.params.req("staging_path").asString(),
                size = frame.params.req("size").asLong(),
            )
        } catch (e: Exception) {
            Log.w(TAG, "fs.publish 帧畸形,丢弃: ${e.message}")
            return
        }
        Log.i(TAG, "fs.publish id=${p.requestId} rel=${p.rel} size=${p.size}")
        // 每发布一协程:并发文件互不阻塞(spec §6.1 逐文件并发;MediaStore
        // 开销毫秒级 + 拷贝,30s 窗口充裕)
        scope.launch(Dispatchers.IO) { publish(p) }
    }

    private data class PublishParams(
        val requestId: Long,
        val rel: String,
        val stagingPath: String,
        val size: Long,
    )

    private data class QueryParams(val requestId: Long, val rel: String, val size: Long)

    private suspend fun publish(p: PublishParams) {
        // policy 解析:快照命中透传;取不到缺省 overwrite(类注释「policy
        // 缺省」钉——发布撞名是本端第二次撞)
        val policy = policyFor(p.rel) ?: TransferPolicy()
        val result = logic.onPublish(p.rel, p.stagingPath, p.size, policy)
        // Skipped 亦 ok:true——文件已在场(弃件),daemon 收尾照常
        val ok = result !is PublishResult.Failed
        val error = (result as? PublishResult.Failed)?.reason
        Log.i(TAG, "发布落定 id=${p.requestId} rel=${p.rel} ok=$ok error=$error")
        channel.sendPublishResult(p.requestId, ok, error)
            .onFailure { e ->
                Log.w(TAG, "publish_result 应答失败 id=${p.requestId}(daemon 超时自会收口)", e)
            }
    }

    /** fs.query_dest(F7 在场查询):查授权树内真实文档 → 0x04 dest_query
     *  应答(exists 仅当文档在且 size 吻合)。应答被拖/发送失败只记日志——
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
        // 每查询一协程:与发布应答同样不阻塞订阅循环
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

        /** fs.publish 事件方法名(daemon jobs.rs EV_PUBLISH 同串)。 */
        const val EV_PUBLISH = "fs.publish"

        /** fs.query_dest 事件方法名(daemon jobs.rs EV_QUERY_DEST 同串)。 */
        const val EV_QUERY_DEST = "fs.query_dest"
    }
}
