package com.mtdss.android.send

/**
 * 源引用方案(fs.request_source 的 ref 编解码 + 重解析;**这是持久化契约**,
 * 改动即断已发任务的续传重发能力,须保持向后兼容):
 *
 * ref 是 App 侧放进 transfer.start(bridge) items[] 的不透明串,daemon 只在
 * fs.request_source 事件里原样送回 + 落 taskstore(`bridge://{ref}` 串)。
 * 两种形态(spec §5.2「重发时按 ref 重新解析 URI 再开」):
 *
 * - **授权根条目**:`roottree://<rootId>/<rel>` —— rootId 查 RootsStore 得
 *   (treeUri, treeDocId),子文档解析**两段式**(设备场 F8):快路径 = 路径形
 *   拼接 `treeDocId + "/" + rel`([childDocumentId];DocumentsContract 树
 *   子文档在路径形 provider 上的既定编码:父 id + '/' + 显示名,逐段累积,
 *   ExternalStorageProvider 遵守,零额外查询)——**这不是全 provider 契约**:
 *   MIUI 文件管理器把 Download 目录授权落在 DownloadsProvider 上,其 docId
 *   是纯数字,路径形拼接被 `Integer.parseInt` 拒 → openFileDescriptor 必抛
 *   → SourceProvider 回退 [drillChildDocumentId] 逐段下钻取真实 docId 再
 *   buildDocumentUriUsingTree 打开。**跨重启/跨会话稳定**:授权根的
 *   持久授权 + RootsStore 记录都在,rel 不含设备路径假设;下钻结果不写回
 *   任何账本(每次解析都两段式,ref/账本契约不动)。
 * - **单文档条目**(选择器件/分享件):ref = **持久化 URI 串本身**
 *   (content://…,选择器件已 takePersistableUriPermission,重启后可原样
 *   重开;分享件的临时授权随进程死亡失效 → 重发按「源已不存在」收口,
 *   这是 spec 评审裁定③的产品语义,由 UI 文案兜底)。
 *
 * 两形态以 `roottree://` 前缀区分;content:// 串与该前缀天然无碰撞。
 * rel 语义(与收件箱/清单同源):'/' 分段、段非空、无首尾 '/'(编码侧由
 * 树枚举保证,解析侧防御校验)。
 */
object SourceRefs {

    /** 授权根条目前缀(与 content:// scheme 无碰撞的私有前缀)。 */
    const val ROOT_SCHEME: String = "roottree://"

    /** 编码授权根条目 ref(rootId 不含 '/',rel 非空且无首尾 '/';违规抛 IllegalArgumentException = 编码侧 bug)。 */
    fun rootRef(rootId: String, rel: String): String {
        require(rootId.isNotEmpty() && '/' !in rootId) { "rootId 非法: $rootId" }
        require(isValidRel(rel)) { "rel 非法: $rel" }
        return "$ROOT_SCHEME$rootId/$rel"
    }

    /** 解析 ref;malformed(空/前缀后无 rootId 或无 rel/rel 非法)→ null。 */
    fun parse(ref: String): SourceRef? {
        if (!ref.startsWith(ROOT_SCHEME)) {
            if (ref.isEmpty()) return null
            return SourceRef.Document(ref)
        }
        val rest = ref.removePrefix(ROOT_SCHEME)
        val slash = rest.indexOf('/')
        if (slash <= 0) return null // 无 rootId 或无 rel
        val rootId = rest.substring(0, slash)
        val rel = rest.substring(slash + 1)
        if (!isValidRel(rel)) return null
        return SourceRef.RootTree(rootId, rel)
    }

    /** rel 校验:非空、无首尾 '/'、无空段。 */
    private fun isValidRel(rel: String): Boolean {
        if (rel.isEmpty() || rel.startsWith("/") || rel.endsWith("/")) return false
        return rel.split('/').none { it.isEmpty() }
    }

    // ── 重解析(决策核心:ref + 授权根表 → 可开面;纯函数,JVM 钉死)──────

    /**
     * 重解析结果:[TreeChild] 携 (treeUri, 快路径子文档 id) 待 Android 面重建
     * URI 并打开;[Document] 的 URI 串原样可开;[Unresolvable] 转拒开文案。
     */
    fun resolve(ref: String, roots: List<StoredRoot>): ResolvedSource = when (val parsed = parse(ref)) {
        null -> ResolvedSource.Unresolvable("引用格式非法")
        is SourceRef.RootTree -> {
            val root = roots.firstOrNull { it.rootId == parsed.rootId }
            if (root == null) {
                ResolvedSource.Unresolvable("授权根已移除(${parsed.rootId})")
            } else {
                ResolvedSource.TreeChild(
                    treeUri = root.treeUri,
                    documentId = childDocumentId(root.treeDocId, parsed.rel),
                    treeDocId = root.treeDocId,
                    rel = parsed.rel,
                )
            }
        }

        is SourceRef.Document -> ResolvedSource.Document(parsed.uri)
    }

    /** 树子文档 id:父文档 id + '/' + rel(DocumentsContract 树的既定累积编码,见类注释)。 */
    fun childDocumentId(treeDocId: String, rel: String): String = "$treeDocId/$rel"

    /**
     * 逐段下钻解析子文档 id(两段式的回退臂,设备场 F8):从 treeDocId 出发,
     * 对 rel 的每一段查其父目录子项,按显示名 == 段名 命中该段的真实 docId,
     * 逐层累积。任一段查询失败(children 返回 null)或段名无匹配 → null
     * (调用方按拒开收口)。
     *
     * 对一切 provider 成立:不依赖「父 id + '/' + 显示名」的路径形编码
     * (DownloadsProvider 等纯数字 docId 的 provider 不认该拼接;
     * ExternalStorageProvider 走此路同样可达,只是多几跳)。children 闭包 =
     * [SafFace.treeChildren] 注入缝(JVM 单测钉死,实机行为实机验证)。
     */
    fun drillChildDocumentId(
        treeDocId: String,
        rel: String,
        children: (parentDocumentId: String) -> List<TreeChild>?,
    ): String? {
        var docId = treeDocId
        for (segment in rel.split('/')) {
            docId = children(docId)?.firstOrNull { it.name == segment }?.documentId ?: return null
        }
        return docId
    }
}

/** 解析后的 ref 形态([SourceRefs.parse])。 */
sealed interface SourceRef {
    data class RootTree(val rootId: String, val rel: String) : SourceRef

    /** 单文档(ref = URI 串本身;选择器件 = 持久授权,分享件 = 临时授权)。 */
    data class Document(val uri: String) : SourceRef
}

/** 重解析结果([SourceRefs.resolve])。 */
sealed interface ResolvedSource {
    /**
     * 授权根条目:documentId = 快路径路径形拼接子文档 id;treeDocId/rel =
     * 回退下钻([SourceRefs.drillChildDocumentId])的输入(快开抛时用,
     * 见 [SourceRefs] 类注释的两段式)。
     */
    data class TreeChild(
        val treeUri: String,
        val documentId: String,
        val treeDocId: String,
        val rel: String,
    ) : ResolvedSource

    data class Document(val uri: String) : ResolvedSource

    /** 拒开文案(进 0x01 无 fd 应答的 error;daemon 收口「源不可用: App 拒开源(…)」)。 */
    data class Unresolvable(val reason: String) : ResolvedSource
}
