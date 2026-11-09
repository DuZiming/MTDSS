package com.mtdss.android.send

import android.content.Intent
import com.mtdss.android.ipc.BridgeItem

/**
 * 选择器/分享 URI → 发送条目(纯逻辑,JVM 钉死,IO 经注入的 [SafFace]):
 * ref = **URI 串本身**(单文档形态,见 [SourceRefs] 注释)。两个条目化面:
 * [itemsFromUris] = SAF 选择器面(documentMeta 元数据,既有行为不变);
 * [itemsFromShareUris] = 分享取名链面(相册类 provider 纯数字垃圾名下沉,
 * 三级通道取真名——R5/J1)。
 *
 * 选择器的拉起(ACTION_OPEN_DOCUMENT 多选)在屏面经
 * ActivityResultContracts.OpenMultipleDocuments 完成,本类只吃结果 URI。
 * takePersistableUriPermission 在此**尽力而为**:OPEN_DOCUMENT 的授权可
 * 持久(这是选择器件可续传重发的根基);分享件(ACTION_SEND)的临时授权
 * 必抛 SecurityException——[SafFace.takePersistable] 内部吞掉,当场会话
 * 照常可发,进程死后授权失效(重发 = 「源已不存在」,spec 评审裁定③)。
 *
 * rel = 显示名;跨来源同名撞号加序号后缀(daemon 拒重复 rel,整批连带
 * 拒——两处选来同名文件是真实场景,不拒条目)。元数据查不到的 URI 跳过
 * 并计数、原因逐条带出(见 [Picked.skipReasons];打不开的源若硬塞进
 * items,清单构建时单条失败会废掉整单)。
 */
class PickerBridge(private val saf: SafFace) {

    /**
     * 条目化结果:items = 可发条目;skipped = 元数据不可得而跳过的 URI 数;
     * skipReasons = 逐 URI 的跳过原因(含 provider 异常词面——「不可读」
     * 文案带因,下次复现从词面即可定位;设备场 F9)。
     */
    data class Picked(val items: List<BridgeItem>, val skipped: Int, val skipReasons: List<String> = emptyList())

    fun itemsFromUris(uris: List<String>): Picked {
        val items = ArrayList<BridgeItem>(uris.size)
        val usedRels = HashSet<String>()
        var skipped = 0
        val skipReasons = ArrayList<String>()
        for (uri in uris) {
            saf.takePersistable(uri)
            var failure: String? = null
            val meta = saf.documentMeta(uri) { failure = it } ?: run {
                skipped++
                skipReasons += "${failure ?: "无此文档"}:$uri"
                continue
            }
            val name = meta.name
            if (name.isNullOrEmpty()) {
                skipped++
                skipReasons += "无显示名:$uri"
                continue
            }
            val rel = uniqueRel(name, usedRels)
            items += BridgeItem(ref = uri, rel = rel, size = meta.size, mtime = meta.mtimeMs)
        }
        return Picked(items, skipped, skipReasons)
    }

    /**
     * 分享条目化(R5/J1:分享路径专用取名链,不动 [itemsFromUris] 的 SAF
     * 选择器面):相册类 provider 经 DocumentsContract 投影回纯数字名
     * (用户实机:安卓名变「1、2、3、4、5」),取名链三级下沉取真名——
     * ① **OpenableColumns 主通道**(DISPLAY_NAME/SIZE;任意 content URI 的
     * 规范取法,MediaStore 相册 URI 即回 IMG_xxx 真名)→ ② **MediaStore
     * 直查**(media 形 URI 专属)→ ③ **DocumentsContract 通道**(既有
     * documentMeta)。逐级垃圾名判据:空 / 纯数字且无扩展名(有扩展即收,
     * 「1.jpg」是真实场景);无扩展但 MIME 可知按 MIME 补扩展名(唯一允许
     * 的「改名」),纯数字且无扩展且 MIME 不可知 = 仍当垃圾名走兜底链。
     * 全链无有效名 → 跳过(既有 skipReasons 语义,带实得词面便于定位)。
     * sanitizedName/uniqueRel 撞号既有语义不变。
     */
    fun itemsFromShareUris(uris: List<String>): Picked {
        val items = ArrayList<BridgeItem>(uris.size)
        val usedRels = HashSet<String>()
        var skipped = 0
        val skipReasons = ArrayList<String>()
        for (uri in uris) {
            saf.takePersistable(uri)
            var failure: String? = null
            val resolved = resolveShareName(uri) { failure = it } ?: run {
                skipped++
                skipReasons += "${failure ?: "无显示名"}:$uri"
                continue
            }
            val rel = uniqueRel(resolved.name, usedRels)
            items += BridgeItem(ref = uri, rel = rel, size = resolved.size, mtime = resolved.mtimeMs)
        }
        return Picked(items, skipped, skipReasons)
    }

    /** 取名链单 URI 求解产物(名字已过垃圾名判据;size/mtime 供条目元数据)。 */
    private data class ShareName(val name: String, val size: Long, val mtimeMs: Long)

    /**
     * 取名链求解:① openable → ② MediaStore(media 形)→ ③ DocumentsContract。
     * 全链垃圾 → null,跳过词面优先带实得垃圾名([junkLiteral] 有值),其次
     * provider 异常词面([onFailure] 旁路),最后「无显示名」既有词面。
     */
    private fun resolveShareName(uri: String, onFailure: (String) -> Unit): ShareName? {
        var junkLiteral: String? = null

        fun accept(name: String?, size: Long, mtimeMs: Long, mime: String?): ShareName? {
            if (name.isNullOrEmpty()) return null
            if (hasExtension(name)) return ShareName(name, size, mtimeMs)
            // 无扩展:MIME 可知 → 按 MIME 补扩展名(唯一允许的「改名」)
            val ext = mime?.let { saf.mimeExtension(it) }
            if (!ext.isNullOrEmpty()) return ShareName("$name.$ext", size, mtimeMs)
            // MIME 不可知:纯数字仍当垃圾名走兜底链;其余(如 README)即收
            if (name.all { it.isDigit() }) {
                junkLiteral = name
                return null
            }
            return ShareName(name, size, mtimeMs)
        }

        // ① OpenableColumns 主通道(相册 MediaStore URI 即回真名)
        saf.openableMeta(uri)?.let { meta ->
            accept(meta.name, meta.size, meta.mtimeMs, saf.mimeOf(uri))?.let { return it }
        }
        // ② MediaStore 直查(media 形 URI 专属;mime 优先行内 MIME_TYPE 列)
        if (uri.startsWith(MEDIA_URI_PREFIX)) {
            saf.mediaStoreMeta(uri)?.let { meta ->
                accept(meta.name, meta.size, 0L, meta.mimeType ?: saf.mimeOf(uri))?.let { return it }
            }
        }
        // ③ DocumentsContract 既有通道(metaFailure 词面旁路,全链垃圾时带出)
        saf.documentMeta(uri) { onFailure(it) }?.let { meta ->
            accept(meta.name, meta.size, meta.mtimeMs, saf.mimeOf(uri))?.let { return it }
        }
        junkLiteral?.let { onFailure("垃圾名(实得:$it)") }
        return null
    }

    /** 垃圾名判据的扩展名面(「.jpg」首字符点不算扩展;末字符点同)。 */
    private fun hasExtension(name: String): Boolean {
        val i = name.lastIndexOf('.')
        return i > 0 && i < name.length - 1
    }

    /** 同名撞号加序号(daemon 拒重复 rel;两处选来同名文件是真实场景)。 */
    private fun uniqueRel(base: String, used: MutableSet<String>): String {
        var rel = base
        var n = 1
        while (rel in used) {
            rel = "$base (${n++})"
        }
        used.add(rel)
        return rel
    }

    companion object {
        /** MediaStore 媒体形 URI 前缀(分享取名链 ② 通道的 gating:仅此形态直查 MediaStore)。 */
        private const val MEDIA_URI_PREFIX = "content://media/"

        /**
         * 分享流条目合并:EXTRA_STREAM(单/多件)× ClipData 条目的**并集**,
         * 保序(stream 在前)、去重(同一 URI 两处都填只留一次)。纯字符串面,
         * JVM 钉死;MIUI 各分享源存在只填 ClipData 的形态(授权随 ClipData
         * 附挂),漏 ClipData 即「未收到可发送的文件」假象(设备场 F9)。
         */
        fun mergeShareUris(stream: List<String>, clip: List<String>): List<String> =
            (stream + clip).distinct()

        /**
         * 分享意图的 URI 提取(ACTION_SEND 单件 / ACTION_SEND_MULTIPLE 多件;
         * EXTRA_STREAM 与 ClipData 并集见 [mergeShareUris];无流 URI 的纯文本
         * 分享不进发送链,返回空表由调用方提示)。
         */
        fun shareUrisFromIntent(intent: Intent?): List<String> {
            if (intent == null) return emptyList()
            @Suppress("DEPRECATION") // getParcelableExtra 泛型重载在 33+ 弃用;minSdk 29 仍须走此面
            val single: android.net.Uri? = intent.getParcelableExtra(Intent.EXTRA_STREAM)
            @Suppress("DEPRECATION")
            val multiple: List<android.net.Uri>? = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
            val stream = (multiple ?: listOfNotNull(single)).map { it.toString() }
            val clipData = intent.clipData ?: return stream.distinct()
            val clip = buildList {
                for (i in 0 until clipData.itemCount) {
                    clipData.getItemAt(i)?.uri?.toString()?.let { add(it) }
                }
            }
            return mergeShareUris(stream, clip)
        }
    }
}
