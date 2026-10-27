package com.mtdss.android.inbox

/**
 * 收件箱协议决策核心(T6 fs.request_dest 的 App 侧状态机;纯逻辑,JVM 单测
 * 钉死)。IO 面全部经 [InboxStore] 注入——本类只做:
 *
 * - 请求分诊:resume=false → create(全新);resume=true → reopen(不截断),
 *   未命中 → 拒开(NOT_FOUND,daemon 清陈旧记录后重试轮全新重收);
 * - 在途表:request_id → handle(commit/失败收尾据此路由);
 * - 在场查询(F7 fs.query_dest):store.exists 行在且 size 吻合才算在场;
 *   异常按不在场收口——宁可重传,不可假完成;
 * - 终态收尾:0x03 done:true → reveal;done:false → discard;应答被
 *   daemon 拒(ack ok:false,请求已超时/未知)→ discard;发送失败 → discard。
 *   未知 request_id 的 0x03 静默忽略(request_id 跨轮全局单调不复用,对不
 *   上的 commit 只可能指向已出表的项——重复终态同样忽略,恰一次语义由
 *   在途表的弹出保证)。
 *
 * 「ack 超时但 fd 已出门」不在这里清理(保守保留):迟到的 0x03 仍能落定;
 * 若最终无 0x03,暂存条目停留 pending(仅本 App 可见;重收时被同名清理或
 * 重开复用)——清理决策归调用方([onAckRejected]/[onSendFailed] 显式通知)。
 */
class InboxLogic(private val store: InboxStore) {

    private val lock = Any()

    /** request_id → 在途 handle(应答已交付、尚未 commit)。 */
    private val open = HashMap<Long, InboxHandle>()

    /** fs.request_dest {request_id, rel, size, resume} 的处置:开暂存或拒开。 */
    fun onRequest(requestId: Long, rel: String, size: Long, resume: Boolean): InboxDecision =
        synchronized(lock) {
            if (resume) {
                val handle = try {
                    store.reopen(rel, size)
                } catch (e: InboxStoreException) {
                    return InboxDecision.Refuse("REOPEN_FAILED: ${e.message}")
                }
                // 素材已失:拒开(NOT_FOUND 词面对齐 daemon 词面锚,重试轮全新重收)
                    ?: return InboxDecision.Refuse("NOT_FOUND: 暂存已失(rel=$rel)")
                open[requestId] = handle
                InboxDecision.Answer(handle)
            } else {
                val handle = try {
                    store.create(rel, size)
                } catch (e: InboxStoreException) {
                    return InboxDecision.Refuse("CREATE_FAILED: ${e.message}")
                }
                open[requestId] = handle
                InboxDecision.Answer(handle)
            }
        }

    /** 0x02 应答被 daemon 拒(ack ok:false)→ 清暂存(fd 从未真正接手)。 */
    fun onAckRejected(requestId: Long) {
        val handle = synchronized(lock) { open.remove(requestId) } ?: return
        runCatching { handle.discard() }
    }

    /** 0x02 发送失败(连接断/IO 错)→ 清暂存。 */
    fun onSendFailed(requestId: Long) {
        val handle = synchronized(lock) { open.remove(requestId) } ?: return
        runCatching { handle.discard() }
    }

    /** 0x03 dest_commit {request_id, done}:done → reveal;否则 discard。未知 id 忽略。 */
    fun onCommit(requestId: Long, done: Boolean) {
        val handle = synchronized(lock) { open.remove(requestId) } ?: return
        runCatching { if (done) handle.reveal() else handle.discard() }
    }

    /** fs.query_dest {request_id, rel, size} 的处置:在场查询(行在且
     *  size 吻合 → true)。查询异常按不在场收口——daemon 侧随之 FULL
     *  重收(失败方向铁律:宁可重传,不可假完成)。request_id 不进决策
     *  (查询无状态面、无资源占用,超时由 daemon 侧兑底)。 */
    fun onQuery(rel: String, size: Long): Boolean = try {
        store.exists(rel, size)
    } catch (e: InboxStoreException) {
        false
    }
}

/** [InboxLogic.onRequest] 的处置结果:交付暂存 fd,或拒开(带稳定文案)。 */
sealed interface InboxDecision {
    data class Answer(val handle: InboxHandle) : InboxDecision

    /** reason 进 0x02 拒开 payload 的 error 字段(NOT_FOUND 前缀对齐 daemon 词面)。 */
    data class Refuse(val reason: String) : InboxDecision
}
