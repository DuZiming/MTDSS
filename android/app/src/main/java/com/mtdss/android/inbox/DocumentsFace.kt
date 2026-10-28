package com.mtdss.android.inbox

import android.content.Context
import android.content.Intent
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.util.Log
import com.mtdss.android.send.TreeChild

/**
 * 收件面的 SAF 薄操作缝(P4a-r2:收件操作整体迁至 SAF 授权树,spec §7;
 * 口径镜像 send 链的 SafFace——收件侧唯一的 Android 框界面):树根解析/
 * 子项枚举/子项名查询、建目录/建文档、改名/删除、写句柄、持久授权收放
 * (读写双旗)、扫描器触发。决策逻辑([SafInboxStore] 的发布编排与在场
 * 查询)只依赖本抽象,JVM 单测注入伪件钉死;真实 provider 行为实机验证
 * (MIUI 点名保留性 = ExternalStorageProvider 直写文件系统,spec §7.7)。
 *
 * 契约:**文档 id(documentId)是唯一句柄**(树内操作全部以它定位);
 * 失败面统一:查询类失败返回 null(无此文档/查询被拒),变更类失败
 * 返回 null/false(调用方按词面收口),不抛未归类异常。
 */
interface DocumentsFace {

    /**
     * 持久化授权(读写双旗;ACTION_OPEN_DOCUMENT_TREE 结果必可持久——
     * 发送侧 RootsStore 模式在收件面的镜像,收件须写故带 WRITE)。
     * 失败(SecurityException 等)返回 false(当场会话仍可用,不阻断)。
     */
    fun takePersistableReadWrite(uri: String): Boolean

    /** 释放持久授权(授权已失时静默)。 */
    fun releasePersistable(uri: String)

    /** 根 URI → 树根文档 id(DocumentsContract.getTreeDocumentId;非法树 URI → null)。 */
    fun treeDocumentId(treeUri: String): String?

    /** 枚举树目录的子项(下钻/在场查询/撞名预查;查询失败 → null)。 */
    fun treeChildren(treeUri: String, parentDocumentId: String): List<TreeChild>?

    /** 查文档显示名(收件根归一化的根名甄别:根名已 = MTDSS 则根即收件箱;
     *  查询失败/无此文档 → null)。 */
    fun documentName(treeUri: String, documentId: String): String?

    /** 建子目录 → 新目录文档 id;失败(provider 拒/名被占)→ null。 */
    fun createDirectory(treeUri: String, parentDocumentId: String, displayName: String): String?

    /** 建子文档 → 新文档 id;失败 → null。 */
    fun createDocument(
        treeUri: String,
        parentDocumentId: String,
        displayName: String,
        mimeType: String,
    ): String?

    /**
     * 改名(renameDocument):**返回改名后的新句柄与实际显示名**——
     * rename 只改显示名不换父目录;provider 可能返回新 id(路径形
     * provider 改名即换 id)也可能保持原 id。实际显示名用于碰撞甄别
     * (返回名 ≠ 请求名 = 终名路径被占用而 provider 自动改 "(N)",F14b
     * 词面沿);provider 不支持改名/失败 → null。
     */
    fun renameDocument(treeUri: String, documentId: String, displayName: String): Renamed?

    /**
     * 移动文档(临时件从 `.mtdss` 挪到落位父目录——rename 不换父目录,
     * 发布的「落位」半步;移动以点文件临时名进行,随机名不撞终名)→
     * 新文档 id(移动即换路径,路径形 provider 必换 id;保持原 id 的
     * provider 原样返回);失败 → null。
     */
    fun moveDocument(
        treeUri: String,
        documentId: String,
        sourceParentDocumentId: String,
        targetParentDocumentId: String,
    ): String?

    /** 删文档(SAF 对授权树有删除权——GC 能力归 App 的根基);false = 失败。 */
    fun deleteDocument(treeUri: String, documentId: String): Boolean

    /** 开写句柄("wt" 截断写;发布拷贝写入面);失败抛底层异常(调用方收口)。 */
    fun openWrite(treeUri: String, documentId: String): ParcelFileDescriptor

    /** 触发系统扫描器索引(SAF 落盘不走 MediaStore,发布后可见性由扫描器
     *  自然接手,spec §7.6;实机面,JVM 不可验)。 */
    fun scanFile(absolutePath: String)
}

/** rename 结果:改名后的文档 id(rename 移动语义下旧 id 失效;保持原 id 的 provider 则原样)+ 实际显示名。 */
data class Renamed(val documentId: String, val displayName: String)

/** 实机面:ContentResolver/DocumentsContract 直连(薄,无逻辑)。 */
class ContentResolverDocumentsFace(private val context: Context) : DocumentsFace {

    private companion object {
        const val TAG = "InboxProvider"

        /** 文档显示名查询投影。 */
        val NAME_PROJECTION = arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
    }

    private val resolver get() = context.contentResolver

    override fun takePersistableReadWrite(uri: String): Boolean = try {
        resolver.takePersistableUriPermission(
            Uri.parse(uri),
            Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
        )
        true
    } catch (_: SecurityException) {
        false
    }

    override fun releasePersistable(uri: String) {
        try {
            resolver.releasePersistableUriPermission(
                Uri.parse(uri),
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
        } catch (_: SecurityException) {
            // 授权已失(系统设置 revoke 等):释放即达目的,静默
        }
    }

    override fun treeDocumentId(treeUri: String): String? = try {
        DocumentsContract.getTreeDocumentId(Uri.parse(treeUri))
    } catch (_: IllegalArgumentException) {
        null
    }

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

    override fun documentName(treeUri: String, documentId: String): String? = try {
        resolver.query(
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), documentId),
            NAME_PROJECTION,
            null,
            null,
            null,
        )?.use { c ->
            if (c.moveToFirst() && !c.isNull(0)) c.getString(0) else null
        }
    } catch (_: Exception) {
        null
    }

    override fun createDirectory(treeUri: String, parentDocumentId: String, displayName: String): String? =
        create(treeUri, parentDocumentId, displayName, DocumentsContract.Document.MIME_TYPE_DIR)

    override fun createDocument(
        treeUri: String,
        parentDocumentId: String,
        displayName: String,
        mimeType: String,
    ): String? = create(treeUri, parentDocumentId, displayName, mimeType)

    private fun create(treeUri: String, parentDocumentId: String, displayName: String, mimeType: String): String? =
        try {
            DocumentsContract.createDocument(
                resolver,
                DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), parentDocumentId),
                mimeType,
                displayName,
            )?.let(DocumentsContract::getDocumentId)
        } catch (_: Exception) {
            null
        }

    override fun renameDocument(treeUri: String, documentId: String, displayName: String): Renamed? = try {
        val newUri = DocumentsContract.renameDocument(
            resolver,
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), documentId),
            displayName,
        ) ?: return null
        val newDocId = DocumentsContract.getDocumentId(newUri)
        // 实际显示名回查(碰撞甄别:provider 可能把名自动改 "(N)");回查
        // 失败按请求名兜底(rename 本身已成,provider 以返回新 URI 为成功凭据)
        val actualName = documentName(treeUri, newDocId)
        if (actualName == null) Log.w(TAG, "rename 后显示名回查失败,按请求名兜底 docId=$newDocId")
        Renamed(documentId = newDocId, displayName = actualName ?: displayName)
    } catch (_: Exception) {
        null
    }

    override fun deleteDocument(treeUri: String, documentId: String): Boolean = try {
        DocumentsContract.deleteDocument(
            resolver,
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), documentId),
        )
    } catch (_: Exception) {
        false
    }

    override fun moveDocument(
        treeUri: String,
        documentId: String,
        sourceParentDocumentId: String,
        targetParentDocumentId: String,
    ): String? = try {
        DocumentsContract.moveDocument(
            resolver,
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), documentId),
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), sourceParentDocumentId),
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), targetParentDocumentId),
        )?.let(DocumentsContract::getDocumentId)
    } catch (_: Exception) {
        null
    }

    override fun openWrite(treeUri: String, documentId: String): ParcelFileDescriptor =
        resolver.openFileDescriptor(
            DocumentsContract.buildDocumentUriUsingTree(Uri.parse(treeUri), documentId),
            "wt",
        ) ?: throw java.io.FileNotFoundException("openFileDescriptor(wt) 返回 null")

    override fun scanFile(absolutePath: String) {
        MediaScannerConnection.scanFile(context, arrayOf(absolutePath), null, null)
    }
}
