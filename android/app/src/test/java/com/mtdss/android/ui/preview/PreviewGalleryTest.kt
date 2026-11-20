package com.mtdss.android.ui.preview

import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ui.browse.BrowseRow
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 图库切换集几何 JVM 单测(V4-1 T23;mockup mobile-preview:‹› 切换 +
 * 底部页点):同深度连续切片 = 兄弟集(展开子树不串层),图库只收图片
 * 类文件;环绕索引纯函数。
 */
class PreviewGalleryTest {

    private fun entry(name: String, isDir: Boolean = false) =
        DirEntry(name = name, path = "/$name", size = 1, mtimeMs = 0, isDir = isDir)

    @Test
    fun `图库集只收图片类文件(目录与非图片不入)`() {
        val files = listOf(
            entry("a.jpg"),
            entry("notes.txt"),
            entry("sub", isDir = true),
            entry("b.PNG"),
            entry("c.mp4"),
            entry("d.heic"),
        )
        val gallery = PreviewGallery.imageSiblings(files)
        assertEquals(listOf("/a.jpg", "/b.PNG", "/d.heic"), gallery.map { it.path })
    }

    @Test
    fun `环绕索引(负数与越界双向收口)`() {
        assertEquals(2, PreviewGallery.wrapIndex(-1, 3))
        assertEquals(0, PreviewGallery.wrapIndex(0, 3))
        assertEquals(0, PreviewGallery.wrapIndex(3, 3))
        assertEquals(1, PreviewGallery.wrapIndex(4, 3))
        assertEquals(0, PreviewGallery.wrapIndex(0, 0))
        assertEquals(0, PreviewGallery.wrapIndex(5, 1))
    }

    @Test
    fun `兄弟集 = 同深度连续切片(展开子树不串层)`() {
        // 展示序:a.jpg(0) b.txt(0) [展开 sub] x.png(1) y.mov(1) c.pdf(0)
        val rows = listOf(
            BrowseRow(entry("a.jpg"), 0),
            BrowseRow(entry("b.txt"), 0),
            BrowseRow(entry("sub", isDir = true), 0),
            BrowseRow(entry("x.png"), 1),
            BrowseRow(entry("y.mov"), 1),
            BrowseRow(entry("c.pdf"), 0),
        )
        // 点子树内文件(index 3,depth 1)= 兄弟集仅该目录的连续段
        assertEquals(
            listOf("/x.png", "/y.mov"),
            PreviewGallery.siblingFiles(rows, 3).map { it.path },
        )
        // 点根级文件(index 0,depth 0)= 整段根级行(子树行为深度更深,连续包含)
        assertEquals(
            6,
            PreviewGallery.siblingFiles(rows, 0).size,
        )
    }

    @Test
    fun `兄弟集索引越界退单条`() {
        assertEquals(
            listOf("/only.txt"),
            PreviewGallery.siblingFiles(listOf(BrowseRow(entry("only.txt"), 0)), 9).map { it.path },
        )
    }
}
