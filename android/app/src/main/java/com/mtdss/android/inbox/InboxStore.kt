package com.mtdss.android.inbox

/**
 * 收件箱面(P4a 发布协议 + F7 在场查询的磁盘侧抽象;协议编排见 [InboxLogic],
 * MediaStore 实现见 MediaStoreInboxStore)。JVM 单测注入伪实现——MediaStore
 * 行为不可 JVM 验证,本接口把「可验的决策逻辑」与「须实机验的磁盘面」切开。
 *
 * rel 语义(与 daemon collect_items/intake 同源):顶层文件 = 文件名;
 * 顶层目录 = `目录名/内部相对路径`(`/` 分隔)。
 */
interface InboxStore {

    /**
     * 终件在场查询(fs.query_dest/0x04 的磁盘侧):**真开 fd 为准,行仅用于
     * 定位**(F10)——rel 对应落位行在、fd 真开成功且 fstat SIZE 与 [size] 吻合 →
     * true;无行/fd 开失败(含 stale 行的 FileNotFoundException)/尺寸不符/
     * 路径非法 → false(MIUI 文件管理器裸删后 MediaStore 行残留、SIZE 字段
     * 照旧,行查询会撒谎成假完成);行定位查询失败一律收口为抛
     * [InboxStoreException](实现须包住未归类异常,复审 Important B)
     * → [InboxLogic.onQuery] 转 false——
     * false 使 daemon 对账不 Skip、引擎走 FULL(失败方向铁律:宁可重传,
     * 不可假完成;实机 = MediaStore 行定位 Uri 后 openFileDescriptor("r")
     * + statSize 对账)。
     */
    fun exists(rel: String, size: Long): Boolean

    /**
     * 发布(fs.publish/0x05 的磁盘侧,spec §3.2/§6.4):把暂存终件落位进
     * MediaStore(尾杠 RELATIVE_PATH insert + IS_PENDING 包裹拷贝瞬间 +
     * 流拷贝 + 揭开),落位撞名沿 [policy](F11 驱逐机器复用:僵尸行验尸
     * 驱逐;真文件在场 → skip 命中弃件记 [PublishResult.Skipped],其余驱逐
     * +插入);同落位自家 pending 半行在场(发布中死 App 残留)则复用该行
     * 截断重拷,不删行不重插(F14:MIUI 删行不删文件——删行孤儿半拷件、
     * 重插撞残件被改名 "(1)")。**全失败面收口返回值,不抛**
     * ([PublishResult.Failed] 携带词面——provider 协程无 CoroutineExceptionHandler,
     * 逸出即崩 App;实现不得逸出异常)。staging 文件本体不归本面处置
     * (发布 ok 后 daemon 删暂存 + 记录打 published,§6.2;Skipped 同样
     * ok:true 走该收尾)。
     */
    fun publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult
}

/**
 * 发布撞名策略(rel 名单命中语义与引擎 intake 的 TransferPolicy 同源:精确
 * 或段边界前缀;数据类镜像 crates/core/src/transport/intake.rs)。发布面
 * 判定(spec §6.4 + 缺省钉):skip 命中 → 弃暂存记 Skipped;其余(overwrite
 * 命中或双名单未命中 = 缺省 overwrite「落最新」)→ 驱逐 + 插入。
 */
data class TransferPolicy(
    val skip: List<String> = emptyList(),
    val overwrite: List<String> = emptyList(),
) {

    /** rel 是否命中 skip 名单(命中即弃件:尊重既有文件,暂存由 daemon 清)。 */
    fun skips(rel: String): Boolean = hits(skip, rel)

    companion object {

        /** 名单命中:rel == 名单项,或 rel 以「名单项/」为段边界前缀(目录整树)。 */
        internal fun hits(list: List<String>, rel: String): Boolean =
            list.any { rel == it || rel.startsWith("$it/") }
    }
}

/** 发布结果(全失败面收口;[Failed.reason] 进 0x05 的 error 词面 → 任务「保存失败」终因)。 */
sealed interface PublishResult {

    /** 落位 + 拷贝 + 揭开全成。 */
    data object Published : PublishResult

    /** skip 命中弃件(既有文件保留;对 daemon 与成功同义,0x05 ok:true)。 */
    data object Skipped : PublishResult

    /** 失败(词面透传 daemon → 「保存失败(N 项): 首因」;暂存保留待重发)。 */
    data class Failed(val reason: String) : PublishResult
}

/**
 * MediaStore 面异常收口(查询面 [InboxStore.exists] 的带因上浮词面;
 * 发布面已全收口为 [PublishResult.Failed],不再走异常)。
 */
class InboxStoreException(message: String, cause: Throwable? = null) : Exception(message, cause)
