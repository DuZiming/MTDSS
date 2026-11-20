package com.mtdss.android.ui.browse

import android.content.Context

/**
 * 浏览页视图形态(V4-1 T22;钦定 mockup mobile-browse.html **列表默认** +
 * mobile-tiles-v2.html 「⋮」切平铺,SharedPreferences 跨次进页记忆):
 * - [LIST] 列表:通栏行(图片/视频 = 44dp 缩略图方图行 + 视频时长角标,
 *   文件夹/文档沿用现行行图标);
 * - [TILES] 平铺:LazyVerticalGrid 4 列磁贴(图标槽/缩略图/时长角标/名字
 *   一行居中截断)。
 */
enum class BrowseViewMode(val key: String) {
    LIST("list"),
    TILES("tiles"),
    ;

    /** 切往的另一形态(「⋮」菜单项词面 = 本函数的返回形态名)。 */
    fun other(): BrowseViewMode = if (this == LIST) TILES else LIST

    companion object {
        /** 缺省形态 = 列表(钦定;档面缺席/损坏同归此)。 */
        val DEFAULT: BrowseViewMode = LIST

        /** 档面值 → 形态(只认 `list|tiles` 逐字;其余一律缺省列表,不猜)。 */
        fun fromKey(raw: String?): BrowseViewMode =
            entries.firstOrNull { it.key == raw } ?: DEFAULT
    }
}

/** 视图形态持久化缝(JVM 单测注入内存实现;实机 = SharedPreferences 单键)。 */
interface BrowseViewPersistence {
    fun load(): String?
    fun save(value: String)
}

/**
 * 浏览视图形态记忆(V4-1 T22):SharedPreferences "browse" 单键
 * `browse_view_mode`(值 `list|tiles`),「⋮」切换即时落档,下次进页构造
 * 即读。存储选型沿 RootsStore 同裁定:单值同步小读写,SharedPreferences
 * 零额外依赖,不引 DataStore。
 */
class BrowseViewModeStore(private val persistence: BrowseViewPersistence) {

    /** 实机入口:SharedPreferences 账本(独立 "browse" 档,与登录/授权根分立)。 */
    constructor(context: Context) : this(prefsPersistence(context))

    /** 当前形态(构造即读;档面损坏降级列表默认,不炸面)。 */
    val mode: BrowseViewMode = BrowseViewMode.fromKey(persistence.load())

    /** 切换落档(「⋮」消费;写失败 = 静默——记忆缺位只是降级面)。 */
    fun save(mode: BrowseViewMode) {
        runCatching { persistence.save(mode.key) }
    }

    companion object {
        /** SharedPreferences 档名(与 "login"/"mtdss_roots" 等既有档分立)。 */
        const val PREFS = "browse"

        /** 档键(brief 钉死逐字)。 */
        const val KEY = "browse_view_mode"

        private fun prefsPersistence(context: Context): BrowseViewPersistence {
            val prefs = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            return object : BrowseViewPersistence {
                override fun load(): String? = prefs.getString(KEY, null)
                override fun save(value: String) {
                    prefs.edit().putString(KEY, value).apply()
                }
            }
        }
    }
}
