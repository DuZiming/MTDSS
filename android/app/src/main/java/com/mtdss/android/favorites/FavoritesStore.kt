package com.mtdss.android.favorites

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.JVal
import com.mtdss.android.ipc.RpcException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * 快捷目录 store + watcher(V3-4 T9,spec §4.10「快捷目录落位」;iOS Kit
 * FavoritesStore/MixedViewModel SWR 与 App 侧 T15 接线的 Kotlin 语义镜像,
 * TS favorites.ts/favstore.ts 同源):
 *
 * - **同步模型**(spec 2026-11-05-mtdss-v3p3-favorites §3.2 整单 LWW):
 *   kv 单键 [FavoritesStore.KV_KEY](ASCII 定死,恒不改);编辑统一走
 *   [FavoritesStore.put]「kv.get 取最新 → parse 前向守卫 → 纯改(transform)
 *   → kv.put 整单回写」,put 成功才落本地状态;put 失败保旧值、返回 false、
 *   不自动重试(用户重试动作)。updated_at 由 put 写回前统一盖 now(transform
 *   不动时间字段)。前向守卫:解析出 v>1 → readOnly,拒绝编辑且**绝不回写**
 *   (防旧版 schema 覆盖新版数据)。
 * - **解析容差**(iOS FavoritesParser/TS parseFavorites 逐条对齐):无键
 *   (not_found)/空串 → 空清单可编辑;解码失败/非对象/无 v 可读 → readOnly
 *   + 空;v>1 → readOnly + 已知字段尽力解析;updated_at→0、favorites→[]、
 *   created_at→0、members→[] 缺失补默认;id/name 非字符串的条目整条丢弃。
 * - **JSON 形**:kv 存值串与 iOS Codable/TS JSON.stringify **逐字节同构**
 *   (键序 = 插入序 v/updated_at/favorites → id/name/created_at/members →
 *   device/path;[encodeDoc] 直拼,不走 [JVal.toCanonicalJson] 的字母序——
 *   那是请求帧 serde_json 口径,kv 值串是跨端互读 payload)。
 * - **SWR 缓存**(iOS MixedViewModel favCache 同象,裁定「单例+登出清」):
 *   store 即 app 级单例(AppGraph 装配),其状态流 = 缓存本体——重进页
 *   [FavoritesStore.snapshot] 瞬时出种子、事件/登录翻转后台再校验整单替换;
 *   「members 签名失效」与 devices 快照迟到(树重算)归 T10 的成员行/成员
 *   列表消费面——文档级缓存由 data_changed{favorites} 整单重拉天然覆盖
 *   他端 members 编辑。[FavoritesStore.clearFavCache] 在登出 + 被踢清场
 *   (换账号后旧账号清单绝不可瞬时复用)。
 * - **watcher**([FavoritesWatcher]):登录态投影翻转拉全量/清缓存、
 *   event.data_changed{key==favorites} 增量重拉(他 key 忽略)、
 *   event.device_evicted 清缓存;挂 AppGraph,沿 MtdssEventHub 既有消费形态。
 */

// ── schema(TS favorites.ts/iOS FavoritesStore.swift 同源;wire snake_case)──

/** 快捷成员:设备 id + 目录路径原串(路径分隔符随目标平台,勿在本层切分)。 */
data class FavMember(
    val device: String,
    val path: String,
)

/** 单个快捷条目:id=客户端生成 16 hex;空成员(0 个)合法。 */
data class Favorite(
    val id: String,
    val name: String,
    val createdAt: Long,
    val members: List<FavMember>,
)

/** 整单文档(写回 v 恒为 1;更高 v 见 [FavoritesParser] 只读守卫)。 */
data class FavoritesDoc(
    val v: Int,
    val updatedAt: Long,
    val favorites: List<Favorite>,
)

// ── 解析(TS parseFavorites/iOS FavoritesParser 镜像;容差逐条对齐)──────────

/** kv 存值解析:raw 为 null/空串 → null(无键);v=1 → 可编辑;v>1 → 只读 +
 * 已知字段尽力解析;解码失败/非对象/无 v 可读 → 只读 + 空文档;字段缺失按
 * 容差补默认。解析恒成功(除非无键返回 null),失败态全部折进 readOnly。 */
object FavoritesParser {

    data class Parsed(
        val doc: FavoritesDoc,
        val readOnly: Boolean,
    )

    /** 空文档:kv 无值时的合法初始态(404=空清单,spec §3.2)。 */
    val empty = FavoritesDoc(v = 1, updatedAt = 0, favorites = emptyList())

    fun parse(raw: String?): Parsed? {
        if (raw.isNullOrEmpty()) return null
        val value = try {
            JVal.parse(raw)
        } catch (_: IllegalArgumentException) {
            return Parsed(empty, readOnly = true)
        }
        if (value !is JVal.Obj) return Parsed(empty, readOnly = true)
        // v 必须是可读整数(缺失/非数值 → 只读 + 空;TS typeof v !== "number"
        // 同象;整数口径:schema v 恒整)
        val v = (value["v"] as? JVal.Num)?.raw?.toLongOrNull()
            ?: return Parsed(empty, readOnly = true)
        val favorites = (value["favorites"] as? JVal.Arr)?.items?.mapNotNull(::parseFavorite)
            ?: emptyList()
        val updatedAt = (value["updated_at"] as? JVal.Num)?.raw?.toLongOrNull() ?: 0
        // 文档 v 规范化为 1(TS 同象):v>1 仅用于 readOnly 裁定,绝不回写
        return Parsed(
            doc = FavoritesDoc(v = 1, updatedAt = updatedAt, favorites = favorites),
            readOnly = v > 1,
        )
    }

    /** 尽力解析单个条目:id/name 非字符串(身份与展示缺失)即丢弃;created_at
     * 缺 → 0(非整数值同弃 0——schema 时间戳恒整毫秒,小数为契约外脏值);
     * members 缺/含残项 → 逐项过滤(双字符串字段)。 */
    private fun parseFavorite(value: JVal): Favorite? {
        if (value !is JVal.Obj) return null
        val id = (value["id"] as? JVal.Str)?.v ?: return null
        val name = (value["name"] as? JVal.Str)?.v ?: return null
        val members = (value["members"] as? JVal.Arr)?.items?.mapNotNull(::parseMember)
            ?: emptyList()
        val createdAt = (value["created_at"] as? JVal.Num)?.raw?.toLongOrNull() ?: 0
        return Favorite(id = id, name = name, createdAt = createdAt, members = members)
    }

    private fun parseMember(value: JVal): FavMember? {
        if (value !is JVal.Obj) return null
        val device = (value["device"] as? JVal.Str)?.v ?: return null
        val path = (value["path"] as? JVal.Str)?.v ?: return null
        return FavMember(device = device, path = path)
    }
}

// ── 整单 JSON(键序 = iOS Codable/TS 插入序,逐字节同构)────────────────────

/**
 * 整单 JSON 串(直拼不走 [JVal.toCanonicalJson]:那是 IPC 请求帧的 serde_json
 * 字母序口径;kv 值串是跨端互读 payload,键序须与 iOS JSONEncoder 的 CodingKeys
 * 声明序 / TS 对象插入序一致——iOS FavoritesStoreTests 内嵌期望 JSON 即金标准)。
 * 转义面与 serde_json 一致:仅 `"`、`\` 与 <0x20 控制字符(`\b\f\n\r\t` 快捷式,
 * 其余 `\u00xx` 小写十六进制);非 ASCII 原样直出。
 */
internal fun encodeDoc(doc: FavoritesDoc): String {
    val sb = StringBuilder()
    sb.append("{\"v\":").append(doc.v)
    sb.append(",\"updated_at\":").append(doc.updatedAt)
    sb.append(",\"favorites\":[")
    doc.favorites.forEachIndexed { i, fav ->
        if (i > 0) sb.append(',')
        sb.append("{\"id\":"); writeQuoted(sb, fav.id)
        sb.append(",\"name\":"); writeQuoted(sb, fav.name)
        sb.append(",\"created_at\":").append(fav.createdAt)
        sb.append(",\"members\":[")
        fav.members.forEachIndexed { j, m ->
            if (j > 0) sb.append(',')
            sb.append("{\"device\":"); writeQuoted(sb, m.device)
            sb.append(",\"path\":"); writeQuoted(sb, m.path)
            sb.append('}')
        }
        sb.append("]}")
    }
    sb.append("]}")
    return sb.toString()
}

/** JSON 字符串字面量写出(转义面见 [encodeDoc] 注)。 */
private fun writeQuoted(sb: StringBuilder, v: String) {
    sb.append('"')
    for (c in v) {
        when {
            c == '"' -> sb.append("\\\"")
            c == '\\' -> sb.append("\\\\")
            c == '\b' -> sb.append("\\b")
            c == '\u000C' -> sb.append("\\f")
            c == '\n' -> sb.append("\\n")
            c == '\r' -> sb.append("\\r")
            c == '\t' -> sb.append("\\t")
            c.code < 0x20 -> sb.append("\\u").append(String.format("%04x", c.code))
            else -> sb.append(c)
        }
    }
    sb.append('"')
}

// ── store(TS createFavoritesCore/iOS FavoritesStore 镜像)───────────────────

/**
 * 快捷目录 store:编辑统一 [put]「kv.get 取最新 → 前向守卫 → 纯改 → kv.put
 * 整单回写」,put 成功才落状态流;失败保旧值、返 false、不自动重试。app 级
 * 单例(AppGraph 装配)= SWR 缓存本体(重进瞬时/首绘缓存种子;登出+被踢
 * [clearFavCache] 清场)。refresh/put 共用一把互斥体:单一 get→put 周期不被
 * 并发周期交错(iOS busy 门同位的**数据层**防线;UI 双击禁用归 T10)。
 */
class FavoritesStore(private val repo: MtdssRepository) {

    /** 快捷条目清单(创建序;refresh 整单替换,编辑 put 成功才落)。 */
    val favorites: StateFlow<List<Favorite>> get() = _favorites.asStateFlow()

    /** 前向守卫旗(v>1 或解析失败;true 时 UI 禁编辑入口,store 绝不回写)。 */
    val readOnly: StateFlow<Boolean> get() = _readOnly.asStateFlow()

    private val _favorites = MutableStateFlow<List<Favorite>>(emptyList())
    private val _readOnly = MutableStateFlow(false)

    private val mutex = Mutex()

    /** 当前整单快照(瞬时非挂起;重进页首绘缓存种子——iOS SWR 种子同象)。 */
    fun snapshot(): List<Favorite> = _favorites.value

    /**
     * 重拉(服务端为准整单替换):无键(not_found)= 合法空单可编辑;解析
     * 失败 → readOnly=true + 空;v>1 → readOnly=true + 已知字段尽力解析;
     * 其他失败(离线/会话失效)保旧值,等下次信号。
     */
    suspend fun refresh(): Unit = mutex.withLock {
        val fetched = repo.kvGet(KV_KEY)
        val raw: String = when {
            fetched.isSuccess -> fetched.getOrThrow().value
            (fetched.exceptionOrNull() as? RpcException)?.kind == KIND_NOT_FOUND -> {
                commit(favorites = emptyList(), readOnly = false) // 无键 = 合法空单
                return
            }
            else -> return // 拉取失败:保旧值,不重试(连接态事件会再触发)
        }
        val parsed = FavoritesParser.parse(raw)
        if (parsed == null) {
            commit(favorites = emptyList(), readOnly = false) // 空串存值:合法空单
        } else {
            commit(favorites = parsed.doc.favorites, readOnly = parsed.readOnly)
        }
    }

    /**
     * 编辑统一入口(LWW;iOS mutate 镜像):前置 kv.get 必取最新(他端可能
     * 刚改)→ parse → readOnly 拒(落 readOnly 旗,绝不 kvPut)→ [transform]
     * 改最新整单的清单(null = 无可写变更,不回写)→ 盖 updated_at=now、v=1
     * → kvPut 整单 → put 成功才落状态流。失败状态保旧值、返回 false、不自动
     * 重试。transform 收到的清单即服务端最新(并发他端条目天然并入回写)。
     */
    suspend fun put(transform: (List<Favorite>) -> List<Favorite>?): Boolean = mutex.withLock {
        val fetched = repo.kvGet(KV_KEY) // 编辑前置必取最新
        val raw: String? = when {
            fetched.isSuccess -> fetched.getOrThrow().value
            (fetched.exceptionOrNull() as? RpcException)?.kind == KIND_NOT_FOUND ->
                null // 无键 = 空单起点(TS parsed null → EMPTY 同象)
            else -> return false // 编辑前置 kv.get 失败 = 本次编辑动作失败
        }
        val parsed = FavoritesParser.parse(raw)
        if (parsed != null && parsed.readOnly) {
            _readOnly.value = true // 前向守卫:v>1 只读
            return false
        }
        val next = transform(parsed?.doc?.favorites ?: emptyList()) ?: return false
        val stamped = FavoritesDoc(v = 1, updatedAt = System.currentTimeMillis(), favorites = next)
        if (!repo.kvPut(KV_KEY, encodeDoc(stamped)).isSuccess) {
            return false // put 失败:保旧值,不重试
        }
        commit(favorites = stamped.favorites, readOnly = false) // put 成功才落状态
        true
    }

    /**
     * 登出/被踢清场(watcher 接线;换账号后旧账号的清单绝不可瞬时复用)。
     * 清快照 + 复位只读旗(下次登录全量重拉)。
     */
    fun clearFavCache() {
        _favorites.value = emptyList()
        _readOnly.value = false
    }

    private fun commit(favorites: List<Favorite>, readOnly: Boolean) {
        _favorites.value = favorites
        _readOnly.value = readOnly
    }

    companion object {
        /** account_kv 键名(spec §3.1,ASCII 定死;恒 "favorites",不改)。 */
        const val KV_KEY: String = "favorites"

        /** kv.get 未命中 typed kind(daemon 稳定码;golden kv.get.resp.notfound 同值)。 */
        const val KIND_NOT_FOUND: String = "not_found"
    }
}

// ── watcher(iOS MTDSSApp T15 接线环镜像;挂 AppGraph 常驻)─────────────────

/**
 * 快捷目录增量消费环(app 生命周期常驻;沿 MtdssEventHub 既有消费形态):
 * - 登录态投影翻转(iOS refreshStatus「登录态才拉」同径):true → 拉全量
 *   (冷启动已登录同径——status 首投影即翻转);false → [FavoritesStore.clearFavCache]
 *   登出清缓存;null(冷启动未知)→ 等权威投影。
 * - event.data_changed:key == [FavoritesStore.KV_KEY] → 整单重拉(他端编辑
 *   秒级跟线);他 key 忽略。
 * - event.device_evicted → 清缓存(被踢;daemon 已清会话)。
 *
 * devices 快照迟到(设备名未及/离线翻面)由成员行消费面(T10 VM 订阅
 * hub.devices 重算树)承担——文档级缓存不依赖 devices。
 */
class FavoritesWatcher(
    private val store: FavoritesStore,
    loggedIn: Flow<Boolean?>,
    hub: MtdssEventHub,
    scope: CoroutineScope,
) {
    init {
        scope.launch {
            loggedIn.distinctUntilChanged().collect { value ->
                when (value) {
                    true -> store.refresh()
                    false -> store.clearFavCache()
                    null -> Unit
                }
            }
        }
        scope.launch {
            hub.dataChanged.collect { ev ->
                if (ev.key == FavoritesStore.KV_KEY) store.refresh()
            }
        }
        scope.launch {
            hub.evicted.collect { store.clearFavCache() }
        }
    }
}
