package com.mtdss.android.ui.preview

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 预览路由纯函数 JVM 单测(V4-1 T23;桌面 ui/src/lib/preview.ts KIND_BY_EXT
 * 词表同源逐字对齐——两侧漂移先红本套):容器四分路由 / HEIC 面 / 回环
 * URL 形 / doc kind 词面 / open-as MIME 表。
 */
class PreviewRouteTest {

    @Test
    fun `图片词表路由到 IMAGE 容器(大小写不敏感)`() {
        for (ext in listOf("jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif")) {
            assertEquals(
                "ext=[$ext] → IMAGE",
                PreviewContainer.IMAGE,
                PreviewRoute.containerOf("photo.$ext"),
            )
        }
        assertEquals(PreviewContainer.IMAGE, PreviewRoute.containerOf("照片.JPG"))
        assertEquals(PreviewContainer.IMAGE, PreviewRoute.containerOf("photo.HeIc"))
    }

    @Test
    fun `视频词表路由到 VIDEO 容器`() {
        for (ext in listOf("mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp")) {
            assertEquals(
                "ext=[$ext] → VIDEO",
                PreviewContainer.VIDEO,
                PreviewRoute.containerOf("clip.$ext"),
            )
        }
        assertEquals(PreviewContainer.VIDEO, PreviewRoute.containerOf("录像.MP4"))
    }

    @Test
    fun `音频词表路由到 AUDIO 容器`() {
        for (ext in listOf("mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus")) {
            assertEquals(
                "ext=[$ext] → AUDIO",
                PreviewContainer.AUDIO,
                PreviewRoute.containerOf("song.$ext"),
            )
        }
    }

    @Test
    fun `文档词表路由到 DOC 容器(pdf office md 文本)`() {
        for (name in listOf(
            "a.pdf", "a.doc", "a.docx", "a.xls", "a.xlsx", "a.csv", "a.ppt", "a.pptx",
            "a.md", "a.txt", "a.log", "a.rs", "a.ts", "a.tsx", "a.js", "a.py", "a.go",
            "a.java", "a.kt", "a.swift", "a.c", "a.h", "a.cpp", "a.cs", "a.sh", "a.ps1",
            "a.json", "a.yml", "a.yaml", "a.toml", "a.ini", "a.xml", "a.html", "a.css", "a.sql",
            "README.MD", "main.TS",
        )) {
            assertEquals(
                "name=[$name] → DOC",
                PreviewContainer.DOC,
                PreviewRoute.containerOf(name),
            )
        }
    }

    @Test
    fun `未知扩展名与无扩展名落 UNSUPPORTED`() {
        for (name in listOf("archive.zip", "无扩展名", ".gitignore", "trailing.", "bin")) {
            assertEquals(
                "name=[$name] → UNSUPPORTED",
                PreviewContainer.UNSUPPORTED,
                PreviewRoute.containerOf(name),
            )
        }
    }

    @Test
    fun `isHeic 面`() {
        assertEquals(true, PreviewRoute.isHeic("a.heic"))
        assertEquals(true, PreviewRoute.isHeic("A.HEIC"))
        assertEquals(false, PreviewRoute.isHeic("a.jpg"))
        assertEquals(false, PreviewRoute.isHeic("heic"))
        assertEquals(false, PreviewRoute.isHeic("a.heic.jpg"))
    }

    @Test
    fun `HEIC 取数序 jpeg 优先于 raw 非HEIC 仅 raw`() {
        assertEquals(listOf("jpeg", "raw"), PreviewRoute.imageFaces("a.heic"))
        assertEquals(listOf("raw"), PreviewRoute.imageFaces("a.jpg"))
        assertEquals(listOf("raw"), PreviewRoute.imageFaces("a.png"))
    }

    @Test
    fun `回环 URL 形逐字(与桌面 previewLoopbackUrl 同形)`() {
        assertEquals(
            "http://127.0.0.1:8123/f/h1/raw?t=tok",
            PreviewRoute.rawUrl(port = 8123, handle = "h1", token = "tok"),
        )
        assertEquals(
            "http://127.0.0.1:8123/f/h1/jpeg?t=tok",
            PreviewRoute.jpegUrl(port = 8123, handle = "h1", token = "tok"),
        )
        assertEquals(
            "http://127.0.0.1:8123/f/h1/thumb?w=256&t=tok",
            com.mtdss.android.ui.browse.BrowseThumbs.thumbUrl(port = 8123, handle = "h1", token = "tok"),
        )
    }

    @Test
    fun `doc kind 词面(pdf office md text)`() {
        assertEquals("pdf", PreviewRoute.docKindOf("a.pdf"))
        assertEquals("docx", PreviewRoute.docKindOf("a.doc"))
        assertEquals("docx", PreviewRoute.docKindOf("a.docx"))
        assertEquals("xlsx", PreviewRoute.docKindOf("a.xls"))
        assertEquals("xlsx", PreviewRoute.docKindOf("a.xlsx"))
        assertEquals("xlsx", PreviewRoute.docKindOf("a.csv"))
        assertEquals("pptx", PreviewRoute.docKindOf("a.ppt"))
        assertEquals("pptx", PreviewRoute.docKindOf("a.pptx"))
        assertEquals("md", PreviewRoute.docKindOf("a.md"))
        assertEquals("text", PreviewRoute.docKindOf("a.txt"))
        assertEquals("text", PreviewRoute.docKindOf("a.rs"))
        assertEquals("text", PreviewRoute.docKindOf("a.json"))
        assertNull("非 doc 容器无 kind", PreviewRoute.docKindOf("a.jpg"))
        assertNull("非 doc 容器无 kind", PreviewRoute.docKindOf("a.mp4"))
        assertNull("未知扩展无 kind", PreviewRoute.docKindOf("a.zip"))
    }

    @Test
    fun `导航守卫 仅 file 放行(特权 WebView 锁面)`() {
        assertEquals(
            true,
            PreviewRoute.linkNavigable("file:///android_asset/previewer/preview.html"),
        )
        for (url in listOf(
            "https://example.com/a",
            "http://127.0.0.1:8123/f/x/raw?t=t",
            "intent://foo",
            "javascript:void(0)",
            "data:text/html,x",
            "about:blank",
            "",
        )) {
            assertEquals("url=[$url] 拒于特权面", false, PreviewRoute.linkNavigable(url))
        }
    }

    @Test
    fun `open-as MIME 表(未知落 octet-stream)`() {
        assertEquals("image/jpeg", PreviewRoute.mimeOf("a.jpg"))
        assertEquals("image/heic", PreviewRoute.mimeOf("a.heic"))
        assertEquals("image/png", PreviewRoute.mimeOf("a.png"))
        assertEquals("video/mp4", PreviewRoute.mimeOf("a.mp4"))
        assertEquals("video/quicktime", PreviewRoute.mimeOf("a.mov"))
        assertEquals("video/x-matroska", PreviewRoute.mimeOf("a.mkv"))
        assertEquals("audio/mpeg", PreviewRoute.mimeOf("a.mp3"))
        assertEquals("audio/flac", PreviewRoute.mimeOf("a.flac"))
        assertEquals("audio/mp4", PreviewRoute.mimeOf("a.m4a"))
        assertEquals("application/pdf", PreviewRoute.mimeOf("a.pdf"))
        assertEquals("text/plain", PreviewRoute.mimeOf("a.txt"))
        assertEquals("text/plain", PreviewRoute.mimeOf("a.md"))
        assertEquals(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            PreviewRoute.mimeOf("a.docx"),
        )
        assertEquals(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            PreviewRoute.mimeOf("a.xlsx"),
        )
        assertEquals(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            PreviewRoute.mimeOf("a.pptx"),
        )
        assertEquals("application/msword", PreviewRoute.mimeOf("a.doc"))
        assertEquals("text/csv", PreviewRoute.mimeOf("a.csv"))
        assertEquals(
            "未知扩展 → octet-stream",
            "application/octet-stream",
            PreviewRoute.mimeOf("a.zip"),
        )
        assertEquals(
            "无扩展名 → octet-stream",
            "application/octet-stream",
            PreviewRoute.mimeOf("MIME"),
        )
    }
}
