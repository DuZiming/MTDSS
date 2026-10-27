package com.mtdss.android.ui.tasks

import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.TransferJob
import com.mtdss.android.send.BridgeLimits
import com.mtdss.android.send.ResolvedSource
import com.mtdss.android.send.RootLookup
import com.mtdss.android.send.SafFace
import com.mtdss.android.send.SourceRef
import com.mtdss.android.send.SourceRefs

/**
 * 重新发起路由(任务列表最微妙契约的唯一实现面;分支与词面勿轻动)。
 *
 * ## 为什么 bridge 上传不能走 daemon 的 transfer.reinitiate
 *
 * daemon 对 bridge 形态上传记录(P2 语义原样承袭,daemon jobs.rs
 * `reinitiate` 的 upload 臂)把 src_paths 里的 `bridge://{ref}` 串**当本地
 * 路径预扫**——引用不是路径,partial 预检全 missing → 「无可传内容」失败,
 * 记录回插保留(不删)。即:bridge 上传的重发若转发 daemon op,用户只会
 * 看到恒定失败。
 *
 * ## App 侧重建(本路由的 Rebuild 臂)
 *
 * ref 本就是为重发设计的稳定编码(T10 SourceRefs):授权根条目
 * `roottree://<rootId>/<rel>`(持久授权 + 账本跨重启稳定)、选择器件/分享件
 * = URI 串本身。重建 = 解析任务记录 src 字段(daemon 的 src_paths 序列化:
 * `bridge://{ref}, bridge://{ref}`,逗号+空格连接)→ 逐 ref 重解析 +
 * 查新元数据 → 组新 [BridgeItem] 批 → 发**新的** transfer.start(upload
 * bridge 形态)。skip/overwrite 双空 = 跳过对端已收完 + 断点续传(熄屏中断
 * 续传的 headline 路径:对端 .mtdss 位图命中即跳过已收块)。
 *
 * 重发件源失效(分享件临时授权随进程死亡、授权根被移除、文档被删)→
 * 「源已不存在:…」(spec 评审裁定③的产品语义,词面前缀钉死)。
 *
 * ## 其余 kind 走 daemon op
 *
 * download / remote_copy 的路径在对端/本机文件系统,daemon 侧
 * transfer.reinitiate 原样可用(翻新 + 摘旧记录 + old_rels 重发编排);
 * 非 bridge 形态的 upload(本 App 恒 bridge 发起,防御面:其他客户端经
 * 本 daemon 发的路径形态)同样 daemon 侧可用 → 一并 Daemon 臂。
 *
 * ## 旧记录为何不清(与 daemon reinitiate 的行为差)
 *
 * daemon reinitiate 摘旧记录;App 侧重建发的是新 start,**旧失败行保留**
 * ——transfer.clear 的 upload 臂会先让对端 cleanup_outbox(删 .mtdss 记录
 * 与 .part),那正是新任务赖以续传的位图,清除即毁。在途期由
 * [TasksReducer.actionsFor] 的同签名在途抑制兜住(旧行按钮隐藏);新行
 * 终态后用户可再清(与桌面各清各的语义一致)。
 */
object ReinitiateRouter {

    /** src 字段的 bridge 条目前缀(daemon jobs.rs `format!("bridge://{}", ref)`)。 */
    private const val BRIDGE_PREFIX = "bridge://"

    /** src_paths 的连接符(daemon `src_paths.join(", ")`)。 */
    private const val SRC_SEPARATOR = ", "

    /** 路由产物(VM 按臂执行;[Reject] 的 reason 直达 UI)。 */
    sealed interface Plan {
        /** bridge 上传:App 侧重建条目后发新 transfer.start(items/to/dir 已备)。 */
        data class Rebuild(val items: List<BridgeItem>, val to: String, val dir: String) : Plan

        /** daemon 侧 reinitiate 原样可用(download/remote_copy/路径形态 upload)。 */
        data class Daemon(val id: String) : Plan

        /** 不可重发(源不存在/记录缺字段/条目超限):reason 呈现,零 RPC。 */
        data class Reject(val reason: String) : Plan
    }

    /** 按 kind+src 路由(调用方已保证 failed/interrupted 行;见 actionsFor 资格面)。 */
    fun plan(job: TransferJob, roots: RootLookup, saf: SafFace): Plan {
        if (job.kind != "upload") {
            return if (job.kind == "download" || job.kind == "remote_copy") Plan.Daemon(job.id)
            else Plan.Reject("任务种类不可重新发起:${job.kind}")
        }
        // upload:bridge 形态才需 App 侧重建;空/路径形态 → daemon 臂
        val refs = bridgeRefs(job.src) ?: return Plan.Daemon(job.id)
        val (to, dir) = parseDst(job.dst)
            ?: return Plan.Reject("任务记录缺少目标信息,无法重新发起")
        return rebuildItems(refs, roots, saf).fold(
            onSuccess = { items -> Plan.Rebuild(items, to, dir) },
            onFailure = { e -> Plan.Reject(e.message ?: "源已不存在") },
        )
    }

    /**
     * 解析 src 字段的 bridge 引用集:全部条目均为 `bridge://{ref}` 形态 →
     * ref 列表;否则(空/路径形态/混形)→ null(调用方转 daemon 臂)。
     * 注:SAF URI 按契约编码(空格/逗号转义),ref 内不含裸 ", ",切分无歧义。
     */
    internal fun bridgeRefs(src: String?): List<String>? {
        if (src.isNullOrBlank()) return null
        val tokens = src.split(SRC_SEPARATOR)
        if (tokens.any { !it.startsWith(BRIDGE_PREFIX) || it.length == BRIDGE_PREFIX.length }) return null
        return tokens.map { it.removePrefix(BRIDGE_PREFIX) }
    }

    /**
     * 解析 upload 记录 dst(`{to_dev}:{target_dir}`,首个 ':' 切分;设备 id
     * 为十六进制设备号不含 ':')。null = 缺失/畸形。
     */
    internal fun parseDst(dst: String?): Pair<String, String>? {
        if (dst.isNullOrBlank()) return null
        val idx = dst.indexOf(':')
        return if (idx <= 0 || idx == dst.length - 1) null
        else dst.substring(0, idx) to dst.substring(idx + 1)
    }

    /**
     * 逐 ref 重建发送条目(决策核心 = [SourceRefs.resolve];元数据现查 =
     * 文件已改动时以当前状态为准,身份不吻合对端自然重收):
     * - 授权根条目:rel 直接取自 ref(rel 编码在 ref 里,断点续传的落位
     *   锚逐字保真——rel 变了位图就 miss);
     * - 单文档条目:rel = 显示名,同名撞号加序号(镜像 PickerBridge 的
     *   " (1)" 方案,同批同序重演一致);
     * - 任一 ref 不可解析/查不到元数据 → 整批拒(「源已不存在:…」);
     *   拒开不静默——半批重发会造成「部分源已不存在」的隐性丢件。
     */
    private fun rebuildItems(refs: List<String>, roots: RootLookup, saf: SafFace): Result<List<BridgeItem>> {
        if (refs.isEmpty()) return Result.failure(IllegalStateException("无可传内容"))
        BridgeLimits.itemCountError(refs.size)?.let { return Result.failure(IllegalStateException(it)) }
        val items = ArrayList<BridgeItem>(refs.size)
        val usedRels = HashSet<String>(refs.size)
        for (ref in refs) {
            when (val parsed = SourceRefs.parse(ref)) {
                null -> return Result.failure(IllegalStateException("源已不存在:引用格式非法($ref)"))

                is SourceRef.RootTree -> when (val resolved = SourceRefs.resolve(ref, roots.roots())) {
                    is ResolvedSource.TreeChild -> {
                        val uri = saf.buildChildUri(resolved.treeUri, resolved.documentId)
                        val meta = saf.documentMeta(uri)
                            ?: return Result.failure(IllegalStateException("源已不存在:${parsed.rel}"))
                        items += BridgeItem(ref = ref, rel = parsed.rel, size = meta.size, mtime = meta.mtimeMs)
                    }

                    is ResolvedSource.Unresolvable ->
                        return Result.failure(IllegalStateException("源已不存在:${resolved.reason}"))

                    is ResolvedSource.Document ->
                        // ref 已 parse 为 RootTree,resolve 不可能落 Document 臂(防御面)
                        return Result.failure(IllegalStateException("源已不存在:引用形态不一致($ref)"))
                }

                is SourceRef.Document -> {
                    val meta = saf.documentMeta(parsed.uri)
                        ?: return Result.failure(IllegalStateException("源已不存在:源不可读"))
                    val name = meta.name?.takeIf { it.isNotEmpty() }
                        ?: return Result.failure(IllegalStateException("源已不存在:文档无名"))
                    items += BridgeItem(
                        ref = ref,
                        rel = uniqueRel(name, usedRels),
                        size = meta.size,
                        mtime = meta.mtimeMs,
                    )
                }
            }
        }
        return Result.success(items)
    }

    /** 同名撞号加序号(daemon 拒重复 rel;方案与 PickerBridge 逐字一致,勿漂移)。 */
    private fun uniqueRel(base: String, used: MutableSet<String>): String {
        var rel = base
        var n = 1
        while (rel in used) rel = "$base (${n++})"
        used.add(rel)
        return rel
    }
}
