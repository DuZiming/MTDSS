/// WebView boot 注入单点(V4-1 T25;可测纯函数,宿主单测直驱
/// PreviewBootTests):注入形状与桌面独立预览窗逐字对齐——src-tauri
/// popout.rs `window.__MTDSS_PREVIEW_BOOT__ = {req, handle}`(initialization_
/// script)+ previewer-main.ts 消费面。安卓 PreviewBoot.kt 逐字同构。
/// iOS WKWebView 宿主页(mtdsspreview://localhost/preview.html,自管 scheme
/// 同源面)在模块脚本就绪后经 `mtdss-boot` 事件消费同一形状;App 侧经
/// WKUserScript(atDocumentEnd,宿主页模块脚本已挂监听)注入。
///
/// req = previewer PreviewRequest JSON `{kind, urls:{raw}, meta:{name,size}}`
/// (桌面 buildPreviewRequest 文档族同形;iOS doc 容器只走 pdf/office/md/
/// 文本四类,raw 恒携 `?t=` 查询令牌——bundle 内 fetch 不带头,T20 面)。
/// 载荷全为本 App 自产(路径/名/回环 URL),转义面只需 JSON 字符串双件
/// (引号/反斜杠 + 控制字符),``escape`` 单点收口。

import Foundation

public enum PreviewBoot {

    /// boot 载荷全局名(桌面 popout.rs 注入名逐字同)。
    public static let bootGlobal = "__MTDSS_PREVIEW_BOOT__"

    /// 宿主页挂载事件名(App 注入 JS 派发,宿主页模块脚本监听)。
    public static let event = "mtdss-boot"

    /// PreviewRequest JSON(kind = ``PreviewRoute/docKindOf(name:)`` 词面;
    /// raw 恒携 ?t=;meta = 面板文案数据源,office 帽面尺寸自门消费)。
    public static func requestJson(
        kind: String,
        rawUrl: String,
        name: String,
        size: Int64,
        hlsUrl: String? = nil,
        thumbUrl: String? = nil,
        directBlob: Bool = false,
        /// r45 封面持久缓存键(`path|mtime|size`);缺席=meta.key 不携。
        entryKey: String? = nil,
        /// r49 #2 视频自动播旗(相册化海报点按进播放):true = req 根携
        /// autoPlay: true(previewer loadedmetadata 即试 play;WebKit 手势
        /// 策略拒则控制条单击)。缺省 false = 现状。
        autoPlay: Bool = false,
    ) -> String {
        var urls = "\"raw\":\"\(escape(rawUrl))\""
        if let hlsUrl {
            urls += ",\"hls\":\"\(escape(hlsUrl))\""
        }
        if let thumbUrl {
            urls += ",\"thumb\":\"\(escape(thumbUrl))\""
        }
        // r38:iOS 直读形态旗——fetch→blob 喂 <video>(媒体进程经自定义
        // scheme 加载远端流挂起;fetch 网页进程已证畅通)
        let blobFlag = directBlob ? ",\"directBlob\":true" : ""
        // r49 #2:视频自动播旗(req 根)
        let playFlag = autoPlay ? ",\"autoPlay\":true" : ""
        // r45:封面持久缓存键(previewer IndexedDB;path|mtime|size)
        let keyField = entryKey.map { ",\"key\":\"\(escape($0))\"" } ?? ""
        return "{\"kind\":\"\(escape(kind))\"," +
            "\"urls\":{\(urls)}," +
            "\"meta\":{\"name\":\"\(escape(name))\",\"size\":\(size)\(keyField)}\(blobFlag)\(playFlag)}"
    }

    /// boot 载荷 JSON `{req, handle}`(桌面 boot 载荷键形同)。
    public static func bootJson(reqJson: String, handle: String) -> String {
        "{\"req\":\(reqJson),\"handle\":\"\(escape(handle))\"}"
    }

    /// 注入 JS 片段(WKUserScript 单点注入):赋值全局后派发 `mtdss-boot`
    /// 事件——宿主页两路消费(boot 先到 = 模块脚本装配时直挂;后到 = 事件
    /// 监听挂),顺序无竞态。
    public static func injectionJs(_ bootJson: String) -> String {
        "window.\(bootGlobal) = \(bootJson);document.dispatchEvent(new Event(\"\(event)\"));"
    }

    /// JSON 字符串转义(引号/反斜杠/控制字符;载荷全为可信自产面)。
    /// 形与安卓 PreviewBoot.escape 同构(%04x 小写十六进制)。
    public static func escape(_ s: String) -> String {
        var out = ""
        out.reserveCapacity(s.count)
        for c in s {
            switch c {
            case "\"": out.append("\\\"")
            case "\\": out.append("\\\\")
            case "\n": out.append("\\n")
            case "\r": out.append("\\r")
            case "\t": out.append("\\t")
            case "\u{08}": out.append("\\b")
            case "\u{0C}": out.append("\\f")
            default:
                if let scalar = c.unicodeScalars.first, scalar.value < 0x20 {
                    out.append(String(format: "\\u%04x", scalar.value))
                } else {
                    out.append(c)
                }
            }
        }
        return out
    }
}
