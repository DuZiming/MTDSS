package com.mtdss.android.ui.browse

import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.PreviewOpenResult
import kotlinx.coroutines.ensureActive
import kotlin.coroutines.coroutineContext

/**
 * 预览回环句柄池(V4-1 T22):浏览行/磁贴缩略图的 handle 生命周期收口。
 *
 * 生命周期语义(防泄漏是底线;daemon 侧 handles 60s 空闲回收为终极兜底):
 * - **在场 open**:条目入视口(行/磁贴组合)经 [urlFor] 取缩略图 URL——
 *   池内已有 = 引用计数 +1 复用同 handle;没有 = preview_open 一枚(IPC
 *   无批量 op,逐条目开, briefed 批量面如实记录)。
 * - **离场 close**:条目出视口(滚动出屏/换页/切形态/退屏)经 [release]
 *   减计数,归零即 preview_close;[releaseAll] = 屏面卸载全量收口。
 * - **离场竞态闭合**:open 在途遇行离场 = LaunchedEffect 协程被取消,
 *   [urlFor] 在入账前 `ensureActive()`——取消态不入账,半开槽不留
 *   (配合 job.cancel,在途 IPC 应答即弃)。
 * - **降级面**:open 失败(源不可达/文件已变更 Stale/未授权)→ null,
 *   调用方落类型图标(缩略图缺位只是降级,daemon 同语义);close 失败
 *   静默(daemon 60s 空闲回收兜底)。
 *
 * 线程纪律:槽表单线程消费——行/磁贴组合面全在主 dispatcher(Compose
 * effect 调度),测试面 runBlocking 单线程,不加锁。
 *
 * @param open 条目 → preview_open(注入缝:BrowseScreen 按浏览档装
 *   device/size/mtime;测试面注入假件)
 * @param close handle → preview_close(池内 runCatching 包裹,失败静默)
 */
class PreviewHandlePool(
    private val open: suspend (DirEntry) -> Result<PreviewOpenResult>,
    private val close: suspend (String) -> Unit,
) {

    private class Slot(val result: PreviewOpenResult, var refs: Int)

    /** 槽表(键 = entry.path;单线程纪律见类注)。 */
    private val slots = LinkedHashMap<String, Slot>()

    /** 活槽数(单测锚)。 */
    internal val liveCount: Int get() = slots.size

    /**
     * 取条目缩略图 URL(引用计数 +1;无 handle 可用 = null → 图标占位)。
     * open 在途遇取消(行离场)= 不入账直返,调用方协程同亡。
     */
    suspend fun urlFor(entry: DirEntry): String? {
        slots[entry.path]?.let {
            it.refs++
            return BrowseThumbs.thumbUrl(it.result.port, it.result.handle, it.result.token)
        }
        val result = open(entry).getOrNull() ?: return null
        // 离场竞态:open 已回但协程已收到取消(行出视口)→ 不入账,泄漏闭合
        coroutineContext.ensureActive()
        slots[entry.path] = Slot(result, 1)
        return BrowseThumbs.thumbUrl(result.port, result.handle, result.token)
    }

    /** 引用计数 −1(条目离场);归零 = close 并摘槽。未知路径幂等静默。 */
    suspend fun release(entry: DirEntry) {
        val slot = slots[entry.path] ?: return
        if (--slot.refs > 0) return
        slots.remove(entry.path)
        runCatching { close(slot.result.handle) } // close 失败静默:daemon 60s 空闲回收兜底
    }

    /** 全量收口(屏面卸载;逐槽 release,槽内 close 同静默面)。 */
    suspend fun releaseAll() {
        for (path in slots.keys.toList()) {
            release(entryByPath(path) ?: continue)
        }
    }

    /** releaseAll 的路径 → 条目投影(槽只存 path;release 以 path 键,构造
     *  影子条目即可——size/mtime 不参与 release 面)。 */
    private fun entryByPath(path: String): DirEntry =
        DirEntry(name = path, path = path, size = 0, mtimeMs = 0, isDir = false)
}
