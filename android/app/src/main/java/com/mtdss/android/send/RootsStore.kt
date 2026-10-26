package com.mtdss.android.send

import android.content.Context
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.asArray
import com.mtdss.android.ipc.asString
import com.mtdss.android.ipc.opt
import com.mtdss.android.ipc.toCanonicalJson
import java.util.UUID

/** 一条已授权根(OPEN_DOCUMENT_TREE 持久授权;treeDocId 与 treeUri 同源提取,见 [RootsStore.add])。 */
data class StoredRoot(
    val rootId: String,
    val treeUri: String,
    val treeDocId: String,
    val displayName: String,
)

/** 授权根查询面(SourceProvider 逐请求重查——根可随时增删,不能缓存陈旧表)。 */
fun interface RootLookup {
    fun roots(): List<StoredRoot>
}

/**
 * 授权根持久化(ACTION_OPEN_DOCUMENT_TREE 的 App 侧账本):list/add/remove,
 * 序列化经 [RootsCodec](JVal JSON 单串)。
 *
 * **存储选型:SharedPreferences(单键 JSON 串)而非 DataStore**——数据量
 * 恒小(用户授权的根是个位数),同步小读写零额外依赖;DataStore 的异步
 * 生命周期与协程域换来的是本场景不需要的规模能力,不值得引入。
 *
 * remove 连带释放持久授权(releasePersistableUriPermission)——不释放的
 * 授权会在系统设置的「授权应用」面残留,违背「移除即回收」的用户预期。
 */
class RootsStore(
    private val persistence: RootsPersistence,
    private val saf: SafFace,
) : RootLookup {

    /** 实机入口:SharedPreferences 账本。 */
    constructor(context: Context, saf: SafFace) : this(prefsPersistence(context), saf)

    /** 全量读取(损坏 JSON → 空表重来:账本丢了只是要重新授权,不致命)。 */
    override fun roots(): List<StoredRoot> =
        RootsCodec.decode(persistence.load() ?: "[]") ?: emptyList()

    /**
     * 登记授权根:提取树文档 id + 尽力持久授权 + 落账。treeDocId 提取失败
     * (非法树 URI)→ null(调用方提示重选)。重复 treeUri → 复用既有条目
     * (幂等,防重复授权同一目录刷屏)。
     */
    fun add(treeUri: String, displayName: String): StoredRoot? {
        val treeDocId = saf.treeDocumentId(treeUri) ?: return null
        val current = roots().toMutableList()
        current.find { it.treeUri == treeUri }?.let { return it }
        saf.takePersistable(treeUri) // OPEN_DOCUMENT_TREE 授权必可持久;失败也不阻断(当场会话仍可用)
        val root = StoredRoot(
            rootId = RootsCodec.newRootId(current.map { it.rootId }),
            treeUri = treeUri,
            treeDocId = treeDocId,
            displayName = displayName.ifBlank { "未命名根" },
        )
        current += root
        persistence.save(RootsCodec.encode(current))
        return root
    }

    /** 移除授权根(连带释放持久授权);不存在 → false。 */
    fun remove(rootId: String): Boolean {
        val current = roots()
        val root = current.find { it.rootId == rootId } ?: return false
        saf.releasePersistable(root.treeUri)
        persistence.save(RootsCodec.encode(current.filter { it.rootId != rootId }))
        return true
    }

    companion object {
        private const val PREFS_NAME = "mtdss_roots"
        private const val KEY_ROOTS = "roots"

        private fun prefsPersistence(context: Context): RootsPersistence {
            val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            return object : RootsPersistence {
                override fun load(): String? = prefs.getString(KEY_ROOTS, null)
                override fun save(json: String) = prefs.edit().putString(KEY_ROOTS, json).apply()
            }
        }
    }
}

/** 存储缝(JVM 单测注入内存实现;两方法,非 fun interface)。 */
interface RootsPersistence {
    fun load(): String?
    fun save(json: String)
}

/** 授权根表序列化(纯函数;字段名即持久化契约)。 */
object RootsCodec {

    /** 编码:JSON 数组,元素 {"id","name","tree_doc_id","tree_uri"}。 */
    fun encode(roots: List<StoredRoot>): String = JVal.Arr(
        roots.map { r ->
            JVal.Obj(
                "id" to JVal.str(r.rootId),
                "name" to JVal.str(r.displayName),
                "tree_doc_id" to JVal.str(r.treeDocId),
                "tree_uri" to JVal.str(r.treeUri),
            )
        },
    ).toCanonicalJson()

    /** 解码;畸形(非数组/元素缺字段/字段非字符串)→ null(整表弃置)。 */
    fun decode(json: String): List<StoredRoot>? = try {
        val arr = JVal.parse(json).asArray()
        arr.map { el ->
            StoredRoot(
                rootId = el.opt("id")?.asString() ?: return null,
                treeUri = el.opt("tree_uri")?.asString() ?: return null,
                treeDocId = el.opt("tree_doc_id")?.asString() ?: return null,
                displayName = el.opt("name")?.asString() ?: return null,
            )
        }
    } catch (_: Exception) {
        null
    }

    /** 新 rootId:"r" + 12 位十六进制(随机,无 '/'——SourceRefs.rootRef 的编码约束);与既有 id 去重。 */
    fun newRootId(existing: Collection<String> = emptySet()): String {
        repeat(8) {
            val id = "r" + UUID.randomUUID().toString().replace("-", "").take(12)
            if (id !in existing) return id
        }
        // 8 连撞(2^-48 量级事件)仍不中:加长兜底,必出
        return "r" + UUID.randomUUID().toString().replace("-", "")
    }
}
