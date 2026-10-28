package com.mtdss.android.inbox

import android.os.ParcelFileDescriptor
import com.mtdss.android.send.TreeChild

/**
 * JVM 伪 SAF 树(DocumentsFace 域概念,内存节点树):授权收放/子项枚举/
 * 建删改名全查表;记录面(createdDirs、renamedCalls、deleted、scanned、
 * taken)供发布编排断言。真实 provider 行为(含 MIUI 点名改写)实机验证。
 */
class FakeDocumentsFace(rootName: String = "Download") : DocumentsFace {

    /** 树节点(name 可变 = rename 生效;size 仅文件有意义)。 */
    class Node(var name: String, val isDir: Boolean, var size: Long = 0) {
        val children = mutableListOf<Node>()

        /** 便捷建子目录(带子树构建闭包)。 */
        fun dir(name: String, build: (Node.() -> Unit)? = null): Node {
            val n = Node(name, isDir = true)
            children += n
            build?.invoke(n)
            return n
        }

        /** 便捷建文件。 */
        fun file(name: String, size: Long): Node {
            val n = Node(name, isDir = false, size = size)
            children += n
            return n
        }

        /** 按名取直接子项。 */
        operator fun get(name: String): Node? = children.firstOrNull { it.name == name }
    }

    /** 授权树 URI(treeDocumentId 只认它;非法授权 → null)。 */
    var treeUri: String = "content://tree/fake"

    /** 树根(名可配 = 授权 Download / 授权 Download/MTDSS 两形态)。 */
    val root: Node = Node(rootName, isDir = true)

    // ── 失败注入 ────────────────────────────────────────────────────────────

    /** treeDocumentId 直接失败(授权账本甄别的非法树形态)。 */
    var failTreeDocId: Boolean = false

    /** 这些父文档 id 的子项查询失败(children → null,下钻断链)。 */
    val failChildrenOf = mutableSetOf<String>()

    /** createDirectory 一律失败。 */
    var failCreateDir: Boolean = false

    /** 模拟并发 createDirectory 竞争败者(TOCTOU):节点照建(对手协程
     *  所建)但对本调用返回 null——create-or-get 败臂回查应复用之。 */
    var createDirLosesRace: Boolean = false

    /** createDocument 一律失败。 */
    var failCreateDoc: Boolean = false

    /** renameDocument 一律失败。 */
    var failRename: Boolean = false

    /** moveDocument 一律失败。 */
    var failMove: Boolean = false

    /** move 返回的新文档 id(null = 保持原 id)。 */
    var moveDocIdOverride: String? = null

    /** deleteDocument 一律失败。 */
    var failDelete: Boolean = false

    /** 这些文档 id 的 deleteDocument 失败(清扫 best-effort 的逐项失败形态)。 */
    val failDeleteOf = mutableSetOf<String>()

    /** documentName 一律失败(收件根归一化的根名查询失败臂)。 */
    var failDocumentName: Boolean = false

    /** rename 碰撞形态:实际落名(null = 落请求名;实机 "(N)" 自动改名)。 */
    var renameCollisionTo: String? = null

    /** rename 返回的新文档 id(null = 保持原 id;路径形 provider 改名即换 id)。 */
    var renameDocIdOverride: String? = null

    // ── 记录面 ──────────────────────────────────────────────────────────────

    val taken = mutableListOf<String>()
    val released = mutableListOf<String>()
    val childQueries = mutableListOf<String>()
    val createdDirs = mutableListOf<Pair<String, String>>() // (parentDocId, name)
    val createdDocs = mutableListOf<Triple<String, String, String>>() // (parentDocId, name, mime)
    val renamedCalls = mutableListOf<Pair<String, String>>() // (docId, requestedName)
    val movedCalls = mutableListOf<Triple<String, String, String>>() // (docId, from, to)
    val deleted = mutableListOf<String>()
    val openWrites = mutableListOf<String>()
    val scanned = mutableListOf<String>()

    // ── docId 分配 ──────────────────────────────────────────────────────────

    private var seq = 0
    private val ids = mutableMapOf<Node, String>()
    private val byId = mutableMapOf<String, Node>()

    init {
        idOf(root)
    }

    private fun idOf(n: Node): String =
        ids.getOrPut(n) { "d${++seq}".also { byId[it] = n } }

    /** 按名路径取节点("MTDSS/docs/a.bin";根 = "" → root)。 */
    fun nodeAt(path: String): Node? = path.split('/').filter { it.isNotEmpty() }
        .fold(root as Node?) { acc, seg -> acc?.takeIf { it.isDir }?.get(seg) }

    /** 按名路径取 docId。 */
    fun docIdAt(path: String): String? = nodeAt(path)?.let { idOf(it) }

    private fun node(documentId: String): Node? = byId[documentId]

    // ── DocumentsFace ───────────────────────────────────────────────────────

    override fun takePersistableReadWrite(uri: String): Boolean {
        taken += uri
        return true
    }

    override fun releasePersistable(uri: String) {
        released += uri
    }

    override fun treeDocumentId(treeUri: String): String? =
        if (!failTreeDocId && treeUri == this.treeUri) idOf(root) else null

    override fun treeChildren(treeUri: String, parentDocumentId: String): List<TreeChild>? {
        childQueries += parentDocumentId
        if (parentDocumentId in failChildrenOf) return null
        // 文件节点无子项(下钻断链形态);未知 id 同 null
        val parent = node(parentDocumentId)?.takeIf { it.isDir } ?: return null
        return parent.children.map { TreeChild(idOf(it), it.name, it.isDir, it.size, 0L) }
    }

    override fun documentName(treeUri: String, documentId: String): String? {
        if (failDocumentName) return null
        return node(documentId)?.name
    }

    override fun createDirectory(treeUri: String, parentDocumentId: String, displayName: String): String? {
        if (failCreateDir) return null
        val parent = node(parentDocumentId)?.takeIf { it.isDir } ?: return null
        if (parent.children.any { it.name == displayName }) return null // 名被占
        val n = Node(displayName, isDir = true)
        parent.children += n
        createdDirs += parentDocumentId to displayName
        return if (createDirLosesRace) null else idOf(n)
    }

    override fun createDocument(
        treeUri: String,
        parentDocumentId: String,
        displayName: String,
        mimeType: String,
    ): String? {
        if (failCreateDoc) return null
        val parent = node(parentDocumentId)?.takeIf { it.isDir } ?: return null
        if (parent.children.any { it.name == displayName }) return null // 名被占
        val n = Node(displayName, isDir = false)
        parent.children += n
        createdDocs += Triple(parentDocumentId, displayName, mimeType)
        return idOf(n)
    }

    override fun renameDocument(treeUri: String, documentId: String, displayName: String): Renamed? {
        renamedCalls += documentId to displayName
        if (failRename) return null
        val n = node(documentId) ?: return null
        n.name = renameCollisionTo ?: displayName
        // id 更换 = 移动语义(rename 换新 id,旧 id 随之失效——deleteDocument
        // 对旧 id 无害失败,镜像路径形 provider 行为)
        val newId = renameDocIdOverride
            ?.also { ids[n] = it; byId[it] = n; byId.remove(documentId) }
            ?: documentId
        return Renamed(documentId = newId, displayName = n.name)
    }

    override fun moveDocument(
        treeUri: String,
        documentId: String,
        sourceParentDocumentId: String,
        targetParentDocumentId: String,
    ): String? {
        movedCalls += Triple(documentId, sourceParentDocumentId, targetParentDocumentId)
        if (failMove) return null
        val n = node(documentId) ?: return null
        val from = node(sourceParentDocumentId) ?: return null
        val to = node(targetParentDocumentId)?.takeIf { it.isDir } ?: return null
        if (!from.children.remove(n)) return null
        to.children += n
        // id 更换 = 移动语义(旧 id 失效,deleteDocument 对旧 id 无害失败)
        return moveDocIdOverride
            ?.also { ids[n] = it; byId[it] = n; byId.remove(documentId) }
            ?: documentId
    }

    override fun deleteDocument(treeUri: String, documentId: String): Boolean {
        deleted += documentId
        if (failDelete) return false
        if (documentId in failDeleteOf) return false
        val n = node(documentId) ?: return false
        fun Node.removeChild(target: Node): Boolean {
            if (children.remove(target)) return true
            return children.any { it.removeChild(target) }
        }
        return root.removeChild(n)
    }

    override fun openWrite(treeUri: String, documentId: String): ParcelFileDescriptor {
        openWrites += documentId
        return FakePfd()
    }

    override fun scanFile(absolutePath: String) {
        scanned += absolutePath
    }

    /** 伪 fd:JVM 桩面下 close 无操作。 */
    class FakePfd : ParcelFileDescriptor(null) {
        override fun close() {}
    }
}
