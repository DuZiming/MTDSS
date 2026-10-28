package com.mtdss.android.inbox

/**
 * 收件箱协议决策核心(P4a 起仅存查询与发布的薄包装;fd 桥时代的请求分诊/
 * 在途表/终态收尾状态机已随 0x02/0x03 退役)。IO 面全部经 [InboxStore]
 * 注入——本类只做:
 * - 在场查询(fs.query_dest):store.exists 真开 fd 验证通过且 size 吻合才
 *   算在场(行仅定位,F10);异常按不在场收口——宁可重传,不可假完成;
 * - 发布(fs.publish):store.publish 全失败面收口为 [PublishResult.Failed]
 *   (接口契约不抛);store 契约外的未归类异常再兜一层网(appScope 无
 *   CoroutineExceptionHandler,逸出即崩 App——镜像 onQuery 的收口纪律)。
 */
class InboxLogic(private val store: InboxStore) {

    /** fs.query_dest {request_id, rel, size} 的处置:在场查询(真开 fd 验证
     *  通过且 size 吻合 → true;行仅为定位,见 [InboxStore.exists])。查询异常
     *  按不在场收口——daemon 侧随之 FULL 重收(失败方向铁律:宁可重传,
     *  不可假完成)。request_id 不进决策(查询无状态面、无资源占用,超时由
     *  daemon 侧兑底)。 */
    fun onQuery(rel: String, size: Long): Boolean = try {
        store.exists(rel, size)
    } catch (e: InboxStoreException) {
        false
    }

    /** fs.publish {request_id, rel, staging_path, size} 的处置:发布并折算
     *  结果(防御网兜 store 契约外异常;request_id 不进决策——应答由
     *  调用方回携,发布本身无状态面,重复/迟到事件按 daemon 侧摘表拒收)。 */
    fun onPublish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult = try {
        store.publish(rel, stagingPath, size, policy)
    } catch (e: Exception) {
        PublishResult.Failed("发布异常(${e.javaClass.simpleName}): ${e.message}")
    }
}
