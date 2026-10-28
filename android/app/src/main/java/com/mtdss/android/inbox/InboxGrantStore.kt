package com.mtdss.android.inbox

import android.content.Context

/**
 * 收件箱 SAF 授权账本(P4a-r2,spec §7.1):保存用户经
 * ACTION_OPEN_DOCUMENT_TREE 授权给收件箱的树 URI(授 `Download/MTDSS`
 * 最优,或授 `Download` 由 App 在树内自管 MTDSS 子目录——归一化在
 * [SafInboxStore] 的收件根解析)。持久授权收放沿发送侧 RootsStore 模式
 * (takePersistableUriPermission,收件面读写双旗),但**独立存储键**
 * (发送根账本与收件授权互不相扰)。
 *
 * 存储选型同 RootsStore:SharedPreferences 单值(数据量恒一个 URI)。
 */
class InboxGrantStore(
    private val persistence: InboxGrantPersistence,
    private val face: DocumentsFace,
) {

    /** 实机入口:SharedPreferences 账本。 */
    constructor(context: Context, face: DocumentsFace) : this(prefsPersistence(context), face)

    /** 已授权树 URI;未授权/账本空 → null(发布与在场查询按无授权收口)。 */
    fun treeUri(): String? = persistence.load()?.takeIf { it.isNotBlank() }

    /**
     * 登记授权(选择器件结果):树 URI 合法性经 [DocumentsFace.treeDocumentId]
     * 甄别(非法 → null 不落账);尽力持久授权(失败不阻断——当场会话仍
     * 可用,重启后失效,下次发布/查询按无授权收口);落账返回 treeUri。
     * 重复授权同一树 = 幂等覆盖。
     */
    fun grant(treeUri: String): String? {
        if (face.treeDocumentId(treeUri) == null) return null
        face.takePersistableReadWrite(treeUri)
        persistence.save(treeUri)
        return treeUri
    }

    /** 撤销授权(连带释放持久授权;系统设置面的授权列表不留残项)。 */
    fun revoke() {
        treeUri()?.let(face::releasePersistable)
        persistence.clear()
    }

    companion object {
        private const val PREFS_NAME = "mtdss_inbox_grant"
        private const val KEY_TREE_URI = "inbox_tree_uri"

        private fun prefsPersistence(context: Context): InboxGrantPersistence {
            val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            return object : InboxGrantPersistence {
                override fun load(): String? = prefs.getString(KEY_TREE_URI, null)
                override fun save(uri: String) = prefs.edit().putString(KEY_TREE_URI, uri).apply()
                override fun clear() = prefs.edit().remove(KEY_TREE_URI).apply()
            }
        }
    }
}

/** 存储缝(JVM 单测注入内存实现)。 */
interface InboxGrantPersistence {
    fun load(): String?
    fun save(uri: String)
    fun clear()
}
