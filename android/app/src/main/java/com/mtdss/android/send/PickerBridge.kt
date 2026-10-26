package com.mtdss.android.send

import android.content.Intent
import com.mtdss.android.ipc.BridgeItem

/**
 * 选择器/分享 URI → 发送条目(SAF 元数据面;纯逻辑,JVM 钉死,IO 经注入
 * 的 [SafFace]):ref = **URI 串本身**(单文档形态,见 [SourceRefs] 注释)。
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
 * 并计数(打不开的源若硬塞进 items,清单构建时单条失败会废掉整单)。
 */
class PickerBridge(private val saf: SafFace) {

    /** 条目化结果:items = 可发条目;skipped = 元数据不可得而跳过的 URI 数。 */
    data class Picked(val items: List<BridgeItem>, val skipped: Int)

    fun itemsFromUris(uris: List<String>): Picked {
        val items = ArrayList<BridgeItem>(uris.size)
        val usedRels = HashSet<String>()
        var skipped = 0
        for (uri in uris) {
            saf.takePersistable(uri)
            val meta = saf.documentMeta(uri) ?: run {
                skipped++
                continue
            }
            val name = meta.name ?: run {
                skipped++
                continue
            }
            if (name.isEmpty()) {
                skipped++
                continue
            }
            val rel = uniqueRel(name, usedRels)
            items += BridgeItem(ref = uri, rel = rel, size = meta.size, mtime = meta.mtimeMs)
        }
        return Picked(items, skipped)
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
        /**
         * 分享意图的 URI 提取(ACTION_SEND 单件 / ACTION_SEND_MULTIPLE 多件;
         * 无流 URI 的纯文本分享不进发送链,返回空表由调用方提示)。
         */
        fun shareUrisFromIntent(intent: Intent?): List<String> {
            if (intent == null) return emptyList()
            @Suppress("DEPRECATION") // getParcelableExtra 泛型重载在 33+ 弃用;minSdk 29 仍须走此面
            val single: android.net.Uri? = intent.getParcelableExtra(Intent.EXTRA_STREAM)
            @Suppress("DEPRECATION")
            val multiple: List<android.net.Uri>? = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
            return (multiple ?: listOfNotNull(single)).map { it.toString() }
        }
    }
}
