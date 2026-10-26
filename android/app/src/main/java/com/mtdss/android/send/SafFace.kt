package com.mtdss.android.send

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import java.io.FileDescriptor

/**
 * SAF/ContentResolver 薄面(发送链唯一的 Android 框界面):持久授权收放、
 * 文档元数据查询、树子项枚举、URI 重建、源 fd 打开。全部行为集中在一个
 * 接口后面——决策逻辑(SourceRefs/RootsStore/RootTree/PickerBridge/
 * SourceProvider)只依赖本抽象,JVM 单测注入伪实现;真实行为实机验证。
 */
interface SafFace {

    /** 尽力持久化授权(OPEN_DOCUMENT/OPEN_DOCUMENT_TREE 的授权可持久;分享件的临时授权必抛 SecurityException——调用方忽略结果即可,见 ShareActivity 注释)。 */
    fun takePersistable(uri: String): Boolean

    /** 释放持久授权(RootsStore.remove 用;授权已失时静默)。 */
    fun releasePersistable(uri: String)

    /** 文档元数据;查询失败/无此文档 → null(调用方按「跳过该项」或「拒开」收口)。 */
    fun documentMeta(uri: String): DocMeta?

    /** 打开源读句柄("r",fd 桥上行只读);失败(授权被撤/文档消失/provider 不支持)抛 [SourceOpenException]。 */
    fun openRead(uri: String): OpenedSource

    /** 根 URI → 树根文档 id(DocumentsContract.getTreeDocumentId;非法树 URI → null)。 */
    fun treeDocumentId(treeUri: String): String?

    /** 树子文档 URI 重建(DocumentsContract.buildDocumentUriUsingTree;SourceRefs.childDocumentId 的产物在此成 URI)。 */
    fun buildChildUri(treeUri: String, documentId: String): String

    /** 枚举树目录的子项(浏览与发送展开用;查询失败 → null)。 */
    fun treeChildren(treeUri: String, parentDocumentId: String): List<TreeChild>?
}

/** 文档元数据投影(name 缺失 = provider 未给,调用方跳过该条目)。 */
data class DocMeta(val name: String?, val size: Long, val mtimeMs: Long)

/** 树目录的一个子项(documentId = 该子项的文档 id,树内唯一)。 */
data class TreeChild(
    val documentId: String,
    val name: String,
    val isDir: Boolean,
    val size: Long,
    val mtimeMs: Long,
)

/** 已打开的源句柄:fd 交 FdPassChannel(SCM_RIGHTS 在 sendmsg 时复制),本地副本随后 [release]。 */
class OpenedSource(val fd: FileDescriptor, private val onClose: () -> Unit) {
    fun release() = onClose()
}

/** 源打开失败(授权被撤/文档消失/provider 拒开;message 进 0x01 拒开文案)。 */
class SourceOpenException(message: String, cause: Throwable? = null) : Exception(message, cause)

/** 实机面:ContentResolver/DocumentsContract 直连(薄,无逻辑)。 */
class ContentResolverSafFace(private val context: Context) : SafFace {

    private val resolver get() = context.contentResolver

    override fun takePersistable(uri: String): Boolean = try {
        resolver.takePersistableUriPermission(Uri.parse(uri), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        true
    } catch (_: SecurityException) {
        false
    }

    override fun releasePersistable(uri: String) {
        try {
            resolver.releasePersistableUriPermission(Uri.parse(uri), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
            // 授权已失(用户在系统设置 revoke 等):释放即达目的,静默
        }
    }

    override fun documentMeta(uri: String): DocMeta? {
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        )
        return try {
            resolver.query(Uri.parse(uri), projection, null, null, null)?.use { c ->
                if (!c.moveToFirst()) return null
                val name = if (c.isNull(0)) null else c.getString(0)
                val size = if (c.isNull(1)) 0L else c.getLong(1)
                val mtime = if (c.isNull(2)) 0L else c.getLong(2)
                DocMeta(name, size, mtime)
            }
        } catch (_: Exception) {
            null
        }
    }

    override fun openRead(uri: String): OpenedSource {
        val pfd = try {
            resolver.openFileDescriptor(Uri.parse(uri), "r")
        } catch (e: SecurityException) {
            throw SourceOpenException("授权被撤或不可读(${e.message})", e)
        } catch (e: Exception) {
            throw SourceOpenException("文档不可开(${e.message})", e)
        } ?: throw SourceOpenException("文档不可开(openFileDescriptor 返回 null)")
        return OpenedSource(pfd.fileDescriptor) { runCatching { pfd.close() } }
    }

    override fun treeDocumentId(treeUri: String): String? = try {
        DocumentsContract.getTreeDocumentId(Uri.parse(treeUri))
    } catch (_: IllegalArgumentException) {
        null
    }

    override fun buildChildUri(treeUri: String, documentId: String): String =
        DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), documentId).toString()

    override fun treeChildren(treeUri: String, parentDocumentId: String): List<TreeChild>? {
        val childrenUri =
            DocumentsContract.buildChildDocumentsUriUsingTree(Uri.parse(treeUri), parentDocumentId)
        val projection = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        )
        return try {
            resolver.query(childrenUri, projection, null, null, null)?.use { c ->
                val out = ArrayList<TreeChild>(c.count)
                while (c.moveToNext()) {
                    val name = if (c.isNull(1)) continue else c.getString(1) // 无名子项跳过(不可成为 rel 段)
                    out += TreeChild(
                        documentId = c.getString(0),
                        name = name,
                        isDir = DocumentsContract.Document.MIME_TYPE_DIR == c.getString(2),
                        size = if (c.isNull(3)) 0L else c.getLong(3),
                        mtimeMs = if (c.isNull(4)) 0L else c.getLong(4),
                    )
                }
                out
            }
        } catch (_: Exception) {
            null
        }
    }
}
