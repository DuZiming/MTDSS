package com.mtdss.android.ui.preview

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * WebView boot 注入单点 JVM 单测(V4-1 T23):注入形状与桌面 preview_popout
 * `window.__MTDSS_PREVIEW_BOOT__ = {req, handle}`(src-tauri popout.rs +
 * previewer-main.ts 消费面)逐字对齐——req = PreviewRequest JSON
 * {kind, urls{raw}, meta{name,size}},raw 恒携 `?t=` 查询令牌(T20 面)。
 */
class PreviewBootTest {

    private val req = PreviewBoot.requestJson(
        kind = "pdf",
        rawUrl = "http://127.0.0.1:8000/f/h1/raw?t=tk",
        name = "说明书.pdf",
        size = 123L,
    )

    @Test
    fun `常量锚(全局名与事件名)`() {
        assertEquals("__MTDSS_PREVIEW_BOOT__", PreviewBoot.BOOT_GLOBAL)
        assertEquals("mtdss-boot", PreviewBoot.EVENT)
    }

    @Test
    fun `requestJson kind urls raw 携 t meta name size 形`() {
        assertEquals(
            "{\"kind\":\"pdf\"," +
                "\"urls\":{\"raw\":\"http://127.0.0.1:8000/f/h1/raw?t=tk\"}," +
                "\"meta\":{\"name\":\"说明书.pdf\",\"size\":123}}",
            req,
        )
        assertTrue("raw 恒携 ?t= 查询令牌", req.contains("raw?t="))
    }

    @Test
    fun `requestJson JSON 字符串转义(引号反斜杠)`() {
        val escaped = PreviewBoot.requestJson(
            kind = "text",
            rawUrl = "http://127.0.0.1:8000/f/h1/raw?t=tk",
            name = "a\"b\\c",
            size = 1L,
        )
        assertEquals(
            "{\"kind\":\"text\"," +
                "\"urls\":{\"raw\":\"http://127.0.0.1:8000/f/h1/raw?t=tk\"}," +
                "\"meta\":{\"name\":\"a\\\"b\\\\c\",\"size\":1}}",
            escaped,
        )
    }

    @Test
    fun `bootJson req handle 键形(桌面 boot 载荷同形)`() {
        val boot = PreviewBoot.bootJson(reqJson = req, handle = "h1")
        assertEquals(
            "{\"req\":$req,\"handle\":\"h1\"}",
            boot,
        )
    }

    @Test
    fun `injectionJs 赋值全局并派发事件`() {
        val js = PreviewBoot.injectionJs(PreviewBoot.bootJson(req, "h1"))
        assertTrue(
            "赋值 window.__MTDSS_PREVIEW_BOOT__",
            js.startsWith("window.__MTDSS_PREVIEW_BOOT__ = {\"req\":"),
        )
        assertTrue("句柄键入载荷", js.contains("\"handle\":\"h1\""))
        assertTrue(
            "派发 mtdss-boot 事件到 window(r54 #2:document 上不冒泡事件到不了 window 监听器)",
            js.endsWith(";window.dispatchEvent(new Event(\"mtdss-boot\"));"),
        )
    }

    @Test
    fun `bootJson handle 转义`() {
        val boot = PreviewBoot.bootJson(reqJson = req, handle = "h\"1")
        assertTrue(boot.contains("\"handle\":\"h\\\"1\""))
    }
}
