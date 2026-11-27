package com.mtdss.android.ui.preview

import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ui.browse.BrowseRow

/**
 * 图库切换几何纯函数(V4-1 T23;钦定 mockup mobile-preview.html Android 列
 * ‹› 切换 + 底部页点),宿主 JVM 单测直驱([PreviewGalleryTest])。
 */
object PreviewGallery {

    /**
     * 图库切换集 = 文件行中的图片类(目录/非图片不入;展示序不变)——
     * ‹› 只在图片容器出(Android 列 mockup:图片查看器页点;视频/文档
     * 单件预览无切换面)。
     */
    fun imageSiblings(files: List<DirEntry>): List<DirEntry> =
        files.filter { !it.isDir && PreviewRoute.containerOf(it.name) == PreviewContainer.IMAGE }


    /** 视频切换集(r60 #3:视频相册滑切面取视频类)。 */
    fun videoSiblings(files: List<DirEntry>): List<DirEntry> =
        files.filter { !it.isDir && PreviewRoute.containerOf(it.name) == PreviewContainer.VIDEO }


    /** 音频切换集(r60 #3:音频滑切面取音频类)。 */
    fun audioSiblings(files: List<DirEntry>): List<DirEntry> =
        files.filter { !it.isDir && PreviewRoute.containerOf(it.name) == PreviewContainer.AUDIO }

    /**
     * 兄弟集 = 展示序同深度连续切片(展开子树不串层):浏览投影行树中,
     * index 行所在目录的连续段(左右两侧扫过深度 ≥ 本行的行,遇更浅深度
     * 收口);index 越界退单条。行树结构:父行(depth d)后随子行(d+1),
     * 切片即所在目录列表。
     */
    fun siblingFiles(rows: List<BrowseRow>, index: Int): List<DirEntry> {
        if (index !in rows.indices) {
            return rows.firstOrNull()?.let { listOf(it.entry) } ?: emptyList()
        }
        val depth = rows[index].depth
        var lo = index
        var hi = index
        while (lo > 0 && rows[lo - 1].depth >= depth) lo--
        while (hi + 1 < rows.size && rows[hi + 1].depth >= depth) hi++
        return rows.subList(lo, hi + 1).map { it.entry }
    }

    /** 环绕索引(‹› 首尾相接;size 0 守卫 = 恒 0)。 */
    fun wrapIndex(index: Int, size: Int): Int =
        if (size <= 0) 0 else ((index % size) + size) % size
}
