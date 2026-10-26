package com.mtdss.android.send

import com.mtdss.android.ipc.BridgeItem

/**
 * 授权根树展开(纯逻辑,JVM 钉死;树查询经注入的 children 闭包,SafFace
 * 的实机行为实机验证):把「同目录多选」(文件 + 目录混合)展开成发送条目。
 *
 * **rel 语义**:恒为「相对授权根」的路径('/' 分段),树根自身名字不进
 * rel(选择根下文件 `a.bin` → rel `a.bin`;选择子目录 `docs` → 其内文件
 * `docs/x.bin`,目录结构保真——与收件箱落位、daemon 清单校验同源)。
 * 调用方(VM)持导航位,负责把当前目录前缀拼进 [TreeSelection.rel]。
 *
 * **护栏**:
 * - [BridgeLimits.MAX_BRIDGE_ITEMS] 条即止(提前停走,不整树扫完再拒
 *   ——大目录上万项时免无谓查询风暴);超限错误与 daemon 词面一致;
 * - **环与退化防御**:documentId 已访问集(树按契约无环,防护性 provider
 *   自含环/重复条目);空名子项跳过(空串不成 rel 段)。无这两道,退化
 *   children 闭包可让 BFS 无限走(rel 逐级变长永不重复,条目计数不涨)。
 */
object RootTree {

    sealed interface ExpandResult {
        data class Items(val items: List<BridgeItem>) : ExpandResult

        data class Error(val message: String) : ExpandResult
    }

    /** 一个选中项:rel = 相对授权根的完整路径;entry = 树子项元数据。 */
    data class TreeSelection(val rel: String, val entry: TreeChild)

    /**
     * 展开选择集。目录 BFS 收集(子项按名排序,展开序确定);rel 去重
     * (防御性——兄弟选择不可能撞号,树内文件系统同名约束天然保证)。
     */
    fun expand(
        rootId: String,
        selections: List<TreeSelection>,
        maxItems: Int = BridgeLimits.MAX_BRIDGE_ITEMS,
        children: (documentId: String) -> List<TreeChild>?,
    ): ExpandResult {
        val items = ArrayList<BridgeItem>()
        val seenRels = HashSet<String>()
        val visitedDocs = HashSet<String>()
        // BFS 队列:(目录 rel, 目录 documentId)
        val queue = ArrayDeque<Pair<String, String>>()

        fun addItem(rel: String, entry: TreeChild) {
            if (!seenRels.add(rel)) return
            items += BridgeItem(
                ref = SourceRefs.rootRef(rootId, rel),
                rel = rel,
                size = entry.size,
                mtime = entry.mtimeMs,
            )
            if (items.size > maxItems) throw Overflow(items.size)
        }

        try {
            for (sel in selections) {
                if (!visitedDocs.add(sel.entry.documentId)) continue
                if (sel.entry.isDir) queue += sel.rel to sel.entry.documentId else addItem(sel.rel, sel.entry)
            }
            while (queue.isNotEmpty()) {
                val (dirRel, dirId) = queue.removeFirst()
                val kids = children(dirId) ?: return ExpandResult.Error("目录读取失败:${dirRel.ifEmpty { "(根)" }}")
                for (kid in kids.sortedBy { it.name }) {
                    if (kid.name.isEmpty()) continue
                    if (!visitedDocs.add(kid.documentId)) continue
                    // 根目录自身 rel 为空:子项 rel 无前导 '/'
                    val kidRel = if (dirRel.isEmpty()) kid.name else "$dirRel/${kid.name}"
                    if (kid.isDir) queue += kidRel to kid.documentId else addItem(kidRel, kid)
                }
            }
        } catch (e: Overflow) {
            return ExpandResult.Error(BridgeLimits.itemCountError(e.count)!!)
        }
        return ExpandResult.Items(items)
    }

    /** 展开「整目录」(当前目录递归全量;「发送本目录」入口用)。 */
    fun expandDirectory(
        rootId: String,
        dirRel: String,
        dirDocumentId: String,
        maxItems: Int = BridgeLimits.MAX_BRIDGE_ITEMS,
        children: (documentId: String) -> List<TreeChild>?,
    ): ExpandResult = expand(
        rootId = rootId,
        selections = listOf(
            TreeSelection(
                rel = dirRel,
                entry = TreeChild(dirDocumentId, dirRel.substringAfterLast('/'), isDir = true, size = 0, mtimeMs = 0),
            ),
        ),
        maxItems = maxItems,
        children = children,
    )

    private class Overflow(val count: Int) : Exception()
}
