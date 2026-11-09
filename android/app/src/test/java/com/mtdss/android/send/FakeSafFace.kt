package com.mtdss.android.send

import java.io.FileDescriptor
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList

/**
 * JVM 单测伪 SAF 面:元数据/树/授权收放全部查表,打开行为可注入。
 * 记录面(taken/released/opened/childUris)供调用序断言。
 */
class FakeSafFace : SafFace {

    /** takePersistable 调用过的 URI(按序)。 */
    val taken = CopyOnWriteArrayList<String>()

    /** releasePersistable 调用过的 URI(按序)。 */
    val released = CopyOnWriteArrayList<String>()

    /** openRead 调用过的 URI(按序)。 */
    val opened = CopyOnWriteArrayList<String>()

    /** buildChildUri 调用记录:(treeUri, documentId) 按序。 */
    val childUris = CopyOnWriteArrayList<Pair<String, String>>()

    /** 元数据表:uri → meta(缺项 = documentMeta 返回 null)。 */
    val metas = ConcurrentHashMap<String, DocMeta>()

    /** 元数据失败注入:uri → 失败词面(命中 = 回调 metaFailure + 返回 null;
     *  复现 ContentResolverSafFace 的 provider 异常路径,设备场 F9)。 */
    val metaFailures = ConcurrentHashMap<String, String>()

    /** OpenableColumns 主通道表(R5/J1 取名链):uri → meta(缺项 = null)。 */
    val openableMetas = ConcurrentHashMap<String, DocMeta>()

    /** MediaStore 直查表(取名链 ② 通道):uri → meta(缺项 = null)。 */
    val mediaMetas = ConcurrentHashMap<String, MediaMeta>()

    /** mediaStoreMeta 查询记录(按序;media 形 gating 断言用)。 */
    val mediaQueries = CopyOnWriteArrayList<String>()

    /** MIME 探测表:uri → mime(缺项 = mimeOf 返回 null)。 */
    val mimes = ConcurrentHashMap<String, String>()

    /** MIME → 扩展名表:mime → ext(缺项 = mimeExtension 返回 null)。 */
    val mimeExtensions = ConcurrentHashMap<String, String>()

    /** 树根表:treeUri → 树根文档 id(缺项 = treeDocumentId 返回 null)。 */
    val treeDocIds = ConcurrentHashMap<String, String>()

    /** 子项表:documentId → children(缺项 = treeChildren 返回 null)。 */
    val children = ConcurrentHashMap<String, List<TreeChild>>()

    /** treeChildren 查询记录:parentDocumentId 按序(两段式回退下钻断言用)。 */
    val childQueries = CopyOnWriteArrayList<String>()

    /** 打开失败注入:uri → 异常(命中即抛;null = 成功)。 */
    val openErrors = ConcurrentHashMap<String, Exception>()

    override fun takePersistable(uri: String): Boolean {
        taken += uri
        return true
    }

    override fun releasePersistable(uri: String) {
        released += uri
    }

    override fun documentMeta(uri: String, metaFailure: ((String) -> Unit)?): DocMeta? {
        metaFailures[uri]?.let {
            metaFailure?.invoke(it)
            return null
        }
        return metas[uri]
    }

    override fun openableMeta(uri: String): DocMeta? = openableMetas[uri]

    override fun mediaStoreMeta(uri: String): MediaMeta? {
        mediaQueries += uri
        return mediaMetas[uri]
    }

    override fun mimeOf(uri: String): String? = mimes[uri]

    override fun mimeExtension(mime: String): String? = mimeExtensions[mime]

    override fun openRead(uri: String): OpenedSource {
        opened += uri
        openErrors[uri]?.let { throw it }
        return OpenedSource(FileDescriptor()) { /* 伪 fd 无需关闭 */ }
    }

    override fun treeDocumentId(treeUri: String): String? = treeDocIds[treeUri]

    override fun buildChildUri(treeUri: String, documentId: String): String {
        childUris += treeUri to documentId
        return "fake://child/$treeUri/$documentId"
    }

    override fun treeChildren(treeUri: String, parentDocumentId: String): List<TreeChild>? {
        childQueries += parentDocumentId
        return children[parentDocumentId]
    }

    /** 便捷装树:children[doc] = list。 */
    fun givenChildren(doc: String, vararg entries: TreeChild) {
        children[doc] = entries.toList()
    }
}
