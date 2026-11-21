package com.mtdss.android.ui.preview

/**
 * WebView boot 注入单点(V4-1 T23;可测纯函数,宿主 JVM 单测直驱
 * [PreviewBootTest]):注入形状与桌面独立预览窗逐字对齐——src-tauri
 * popout.rs `window.__MTDSS_PREVIEW_BOOT__ = {req, handle}`(initialization_
 * script)+ previewer-main.ts 消费面。安卓宿主页(assets/previewer/
 * preview.html)在模块脚本就绪后经 `mtdss-boot` 事件消费同一形状。
 *
 * req = previewer PreviewRequest JSON `{kind, urls:{raw}, meta:{name,size}}`
 * (桌面 buildPreviewRequest 文档族同形;安卓 doc 容器只走 pdf/office/md/
 * 文本四类,raw 恒携 `?t=` 查询令牌——bundle 内 fetch 不带头,T20 面)。
 * 载荷全为本 App 自产(路径/名/回环 URL),转义面只需 JSON 字符串双件
 * (引号/反斜杠 + 控制字符),[escape] 单点收口。
 */
object PreviewBoot {

    /** boot 载荷全局名(桌面 popout.rs 注入名逐字同)。 */
    const val BOOT_GLOBAL = "__MTDSS_PREVIEW_BOOT__"

    /** 宿主页挂载事件名(App 注入 JS 派发,宿主页模块脚本监听)。 */
    const val EVENT = "mtdss-boot"

    /**
     * PreviewRequest JSON(kind = [PreviewRoute.docKindOf] 词面;raw 恒携
     * ?t=;meta = 面板文案数据源,office 帽面尺寸自门消费)。
     */
    fun requestJson(kind: String, rawUrl: String, name: String, size: Long): String =
        "{\"kind\":\"${escape(kind)}\"," +
            "\"urls\":{\"raw\":\"${escape(rawUrl)}\"}," +
            "\"meta\":{\"name\":\"${escape(name)}\",\"size\":$size}}"

    /** boot 载荷 JSON `{req, handle}`(桌面 boot 载荷键形同)。 */
    fun bootJson(reqJson: String, handle: String): String =
        "{\"req\":$reqJson,\"handle\":\"${escape(handle)}\"}"

    /**
     * 注入 JS 片段(evaluateJavascript 于 onPageFinished 单点调用):赋值
     * 全局后派发 `mtdss-boot` 事件——宿主页两路消费(boot 先到 = 模块脚本
     * 装配时直挂;后到 = 事件监听挂),顺序无竞态。
     */
    fun injectionJs(bootJson: String): String =
        "window.$BOOT_GLOBAL = $bootJson;document.dispatchEvent(new Event(\"$EVENT\"));"

    /** JSON 字符串转义(引号/反斜杠/控制字符;载荷全为可信自产面)。 */
    fun escape(s: String): String = buildString {
        for (c in s) {
            when (c) {
                '"' -> append("\\\"")
                '\\' -> append("\\\\")
                '\n' -> append("\\n")
                '\r' -> append("\\r")
                '\t' -> append("\\t")
                '\b' -> append("\\b")
                '\u000C' -> append("\\f")
                else -> if (c < ' ') append("\\u%04x".format(java.util.Locale.ROOT, c.code)) else append(c)
            }
        }
    }
}
