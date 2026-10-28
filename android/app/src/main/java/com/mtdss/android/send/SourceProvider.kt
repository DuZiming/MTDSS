package com.mtdss.android.send

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
 * 发送源应答编排(T4 fs.request_source 的 App 侧胶水;决策核心在
 * [SourceRefs.resolve](ref 方案 + 重解析),SAF 行为在 [SafFace],两者
 * JVM/实机分面验证)。镜像 InboxProvider 的构造即订阅纪律:
 *
 * 1. 订阅 `fs.request_source {request_id, ref}`(App 级——上传任务的逐文件
 *    请求不经本 App 界面即可到达,且须先于任何 startUpload 落位订阅);
 * 2. 事件 → 每请求一协程(并发文件互不阻塞;daemon 应答窗 10s)→
 *    [SourceRefs.resolve] 按 ref 重解析(授权根查 [RootLookup] 实时表);
 *    授权根条目打开走**两段式**(见 [answerTreeChild];设备场 F8);
 * 3. 可解析 → SafFace 开只读 fd → `0x01 source_fd`(+fd)→ 0x81 ack 为
 *    回执(本地 fd 副本随即 release:SCM_RIGHTS 在 sendmsg 时已复制);
 *    ack ok:false = 请求已超时/未知(daemon 摘了等待方),记日志即毕;
 * 4. 不可解析/打开失败 → 0x01 无 fd(payload `{"ok":false,"error":…}`);
 *    daemon 收口「源不可用: App 拒开源(…)」,任务级失败随协议闭环
 *    (T4 语义:拒开不等超时,立即失败)。
 *
 * **通道实例**:0x01 专用一条 FdPassChannel(P4a 实例纪律改二:与收件
 * 查询通道〔0x04/0x05〕分立——daemon 两个 resolver 的 request_id 各自独立
 * 自增,单通道无法区分同号的两类 ack;详见 [FdPassChannel] 类注释)。
 */
class SourceProvider(
    private val roots: RootLookup,
    ipc: IpcClient,
    private val channel: FdPassChannel,
    private val saf: SafFace,
    private val scope: CoroutineScope,
) {
    private val subscribed = CompletableDeferred<Unit>()

    init {
        scope.launch {
            ipc.events
                .onSubscription { subscribed.complete(Unit) }
                .collect { frame -> if (frame.method == EV_REQUEST_SOURCE) onRequest(frame) }
        }
    }

    /** 订阅落位闸(测试/复用方显式同步;运行时构造序已保证)。 */
    suspend fun awaitSubscribed() = subscribed.await()

    private fun onRequest(frame: EventFrame) {
        // 解析容错:畸形事件帧不得杀死订阅协程(镜像 InboxProvider 口径;
        // request_id 单调不复用,丢一帧即该文件按超时失败)
        val requestId = try {
            frame.params.req("request_id").asLong()
        } catch (e: Exception) {
            Log.w(TAG, "request_source 帧缺 request_id,丢弃: ${e.message}")
            return
        }
        val ref = try {
            frame.params.req("ref").asString()
        } catch (e: Exception) {
            Log.w(TAG, "request_source id=$requestId 帧缺 ref,丢弃: ${e.message}")
            return
        }
        Log.i(TAG, "request_source id=$requestId ref=$ref")
        scope.launch(Dispatchers.IO) { answer(requestId, ref) }
    }

    private suspend fun answer(requestId: Long, ref: String) {
        when (val resolved = SourceRefs.resolve(ref, roots.roots())) {
            is ResolvedSource.TreeChild -> answerTreeChild(requestId, resolved)

            is ResolvedSource.Document -> openAndAnswer(requestId, resolved.uri)

            is ResolvedSource.Unresolvable -> refuse(requestId, resolved.reason)
        }
    }

    /**
     * 授权根条目两段式打开(设备场 F8,决策见 [SourceRefs] 类注释):
     * 1. **快路径**:路径形子 docId 直开一次(ExternalStorageProvider 等
     *    路径形 provider 零额外子文档查询);
     * 2. **回退**:快开抛(任何异常)→ [SourceRefs.drillChildDocumentId]
     *    逐段下钻(经 [SafFace.treeChildren] 同一注入缝)命中真实 docId
     *    再开——MIUI 把 Download 根挂 DownloadsProvider(纯数字 docId)时
     *    路径形拼接被拒,必经此臂。
     * 两段顺序执行(无双重打开竞争);回退结果不写回(ref/账本契约不变),
     * 每次解析都两段式。双失败 → 拒开,词面承快开异常(provider 的真实抱怨)。
     */
    private suspend fun answerTreeChild(requestId: Long, resolved: ResolvedSource.TreeChild) {
        val fastUri = saf.buildChildUri(resolved.treeUri, resolved.documentId)
        var fastFailure: Exception? = null
        val fastOpened = try {
            saf.openRead(fastUri)
        } catch (e: Exception) {
            fastFailure = e
            Log.i(TAG, "快开失败转回退 id=$requestId rel=${resolved.rel}: ${e.message}")
            null
        }
        if (fastOpened != null) {
            deliver(requestId, fastOpened)
            return
        }
        val drilledId = SourceRefs.drillChildDocumentId(resolved.treeDocId, resolved.rel) { parent ->
            saf.treeChildren(resolved.treeUri, parent)
        }
        if (drilledId == null) {
            refuse(requestId, fastFailure?.message ?: "文档不可开(子文档逐段解析失败:${resolved.rel})")
            return
        }
        val opened = try {
            saf.openRead(saf.buildChildUri(resolved.treeUri, drilledId))
        } catch (e: Exception) {
            refuse(requestId, e.message ?: "文档不可开")
            return
        }
        deliver(requestId, opened)
    }

    private suspend fun openAndAnswer(requestId: Long, uri: String) {
        val opened = try {
            saf.openRead(uri)
        } catch (e: Exception) {
            // 授权被撤/文档消失/provider 拒开:立即拒开(daemon 不等超时)
            refuse(requestId, e.message ?: "打开失败")
            return
        }
        deliver(requestId, opened)
    }

    /** 已开 fd 的应答:0x01(+fd)→ 0x81 ack 回执;本地 fd 副本随即 release。 */
    private suspend fun deliver(requestId: Long, opened: OpenedSource) {
        val result = channel.sendSourceFd(requestId, opened.fd)
        // 本地 fd 副本使命完成(SCM_RIGHTS 在 sendmsg 时已复制)
        opened.release()
        result.fold(
            onSuccess = { ack ->
                if (!ack.ok) Log.w(TAG, "source_fd 被拒(请求已超时/未知)id=$requestId: ${ack.error}")
            },
            onFailure = { e -> Log.w(TAG, "source_fd 发送失败 id=$requestId", e) },
        )
    }

    private suspend fun refuse(requestId: Long, reason: String) {
        Log.w(TAG, "拒开源 id=$requestId: $reason")
        channel.sendSourceFd(requestId, null, reason)
    }

    private companion object {
        const val TAG = "SourceProvider"

        /** fs.request_source 事件方法名(daemon jobs.rs EV_REQUEST_SOURCE 同串)。 */
        const val EV_REQUEST_SOURCE = "fs.request_source"
    }
}
