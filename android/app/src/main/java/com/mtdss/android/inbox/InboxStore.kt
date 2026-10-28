package com.mtdss.android.inbox

/**
 * 收件箱面(P4a-r2 起为 SAF 授权树侧抽象:发布协议 + 0x04 在场查询;
 * 协议编排见 [InboxLogic],SAF 实现见 SafInboxStore)。JVM 单测注入伪
 * 实现——SAF provider 行为不可 JVM 验证,本接口把「可验的决策逻辑」与
 * 「须实机验的树操作面」切开。
 *
 * rel 语义(与 daemon collect_items/intake 同源):顶层文件 = 文件名;
 * 顶层目录 = `目录名/内部相对路径`(`/` 分隔)。
 */
interface InboxStore {

    /**
     * 终件在场查询(fs.query_dest/0x04 的树侧,spec §7.4):rel 目录段在
     * 授权树内下钻,**同名文件子文档真实存在且 SIZE 与 [size] 吻合** →
     * true;未授权/下钻断链/查询失败/尺寸不符/同名是目录/路径非法 →
     * false(F10 铁律沿:宁可重传,不可假完成;SAF 直读文件系统,无
     * MediaStore 行与扫描器时延,僵尸行家族在接收面整体退场)。
     * 实现不得逸出异常(provider 协程无 CoroutineExceptionHandler)。
     */
    fun exists(rel: String, size: Long): Boolean

    /**
     * 发布(fs.publish/0x05 的树侧,spec §7.3/§7.5):把暂存终件经
     * `MTDSS/.mtdss/` 隐藏目录的点文件临时名落位(createDocument → 流拷贝
     * 等长对账 → moveDocument 移入落位父目录 → renameDocument 落正式名),
     * 撞名沿 [policy](skip 命中弃件记 [PublishResult.Skipped];其余 SAF
     * delete 驱逐后落位);rename 返回名 ≠ 请求名 = 碰撞,沿 F14b 词面
     * [PublishResult.Failed] 诚实告知(改名后的正式文件保留)。活进程内
     * 任何失败 SAF 删临时件收 Failed。**全失败面收口返回值,不抛**(逸出
     * 即崩 App);staging 文件本体不归本面处置(发布 ok 后 daemon 删暂存
     * + 记录打 published,§6.2;Skipped 同样 ok:true 走该收尾)。
     */
    fun publish(rel: String, stagingPath: String, size: Long, policy: TransferPolicy): PublishResult
}

/**
 * 发布撞名策略(rel 名单命中语义与引擎 intake 的 TransferPolicy 同源:精确
 * 或段边界前缀;数据类镜像 crates/core/src/transport/intake.rs)。发布面
 * 判定(spec §7.5 + 缺省钉):skip 命中 → 弃暂存记 Skipped;其余(overwrite
 * 命中或双名单未命中 = 缺省 overwrite「落最新」)→ SAF 删既有件 + 落位。
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
 * 树侧异常收口(SAF 实现的 exists 按不在场自收口不再上浮;保留本类为
 * [InboxLogic] 防御网的既有契约形态,store 契约外异常的兜底词面载体)。
 */
class InboxStoreException(message: String, cause: Throwable? = null) : Exception(message, cause)
