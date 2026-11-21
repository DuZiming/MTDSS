/// 预览路由纯函数面(V4-1 T25;钦定 mockup mobile-preview.html):文件名 →
/// 四容器路由 + HEIC 面 + 回环 URL 组装 + doc kind 词面 + open-as MIME +
/// iOS WKWebView 载体面(自管 scheme)。零 UI 依赖,宿主单测直驱
/// (PreviewRouteTests);词表与桌面 ui/src/lib/preview.ts KIND_BY_EXT /
/// 安卓 ui/preview/PreviewRoute.kt 同源(漂移先红本套)。

import Foundation

/// 预览容器四分(spec「四容器一分区」路由唯一依据;安卓 PreviewContainer
/// 同构):图片 = 自研 zoom 查看器;视频/音频 = AVPlayer 原流直连(移动端
/// 原生解码不经转码,spec 裁定);pdf/office/md/文本 = WKWebView(previewer
/// bundle);其余 = 类型页 + 下载引导。
public enum PreviewContainer: Equatable, Sendable {
    case image
    case video
    case audio
    case doc
    case unsupported
}

public enum PreviewRoute {

    /// 图片扩展名词表(filetype.ts img 类同源,小写存取;安卓同表)。
    static let imageExts: Set<String> = [
        "jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif",
    ]

    /// 视频扩展名词表(filetype.ts vid 类同源)。
    static let videoExts: Set<String> = [
        "mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp",
    ]

    /// 音频扩展名词表(filetype.ts 词表同源)。
    static let audioExts: Set<String> = [
        "mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus",
    ]

    /// 文档扩展名词表(pdf/office/md/文本;preview.ts text 高亮语言域同源)。
    static let docExts: Set<String> = [
        "pdf",
        "doc", "docx", "xls", "xlsx", "csv", "ppt", "pptx",
        "md", "txt", "log",
        "rs", "ts", "tsx", "js", "mjs", "cjs", "jsx", "py", "go", "java", "kt", "kts",
        "swift", "c", "h", "cpp", "cc", "cxx", "hpp", "cs", "rb", "php", "sh", "bash",
        "zsh", "ps1", "bat", "json", "jsonc", "yml", "yaml", "toml", "ini", "cfg",
        "conf", "xml", "html", "css", "sql",
    ]

    /// doc 扩展名 → previewer kind 词面(preview.ts KIND_BY_EXT office/md/
    /// text 段同源;非 doc 容器/未知扩展无键)。
    static let docKindByExt: [String: String] = {
        var map: [String: String] = [
            "pdf": "pdf",
            "doc": "docx", "docx": "docx",
            "xls": "xlsx", "xlsx": "xlsx", "csv": "xlsx",
            "ppt": "pptx", "pptx": "pptx",
            "md": "md",
        ]
        let textOnly = docExts.subtracting([
            "pdf", "doc", "docx", "xls", "xlsx", "csv", "ppt", "pptx", "md",
        ])
        for ext in textOnly { map[ext] = "text" }
        return map
    }()

    /// open-as MIME 表(系统打开器常识面;未知落 octet-stream)。
    static let mimeByExt: [String: String] = [
        "jpg": "image/jpeg", "jpeg": "image/jpeg", "png": "image/png",
        "gif": "image/gif", "bmp": "image/bmp", "webp": "image/webp",
        "heic": "image/heic", "svg": "image/svg+xml", "tif": "image/tiff",
        "tiff": "image/tiff", "ico": "image/x-icon", "avif": "image/avif",
        "mp4": "video/mp4", "m4v": "video/mp4", "mov": "video/quicktime",
        "webm": "video/webm", "mkv": "video/x-matroska", "avi": "video/x-msvideo",
        "3gp": "video/3gpp", "wmv": "video/x-ms-wmv", "flv": "video/x-flv",
        "mpg": "video/mpeg", "mpeg": "video/mpeg",
        "mp3": "audio/mpeg", "flac": "audio/flac", "wav": "audio/wav",
        "aac": "audio/aac", "ogg": "audio/ogg", "m4a": "audio/mp4",
        "wma": "audio/x-ms-wma", "opus": "audio/ogg",
        "pdf": "application/pdf",
        "doc": "application/msword",
        "docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls": "application/vnd.ms-excel",
        "xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "csv": "text/csv",
        "ppt": "application/vnd.ms-powerpoint",
        "pptx": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "txt": "text/plain", "log": "text/plain", "md": "text/plain",
        "json": "text/plain", "xml": "text/plain", "yml": "text/plain",
        "yaml": "text/plain", "toml": "text/plain", "ini": "text/plain",
        "html": "text/plain", "css": "text/plain", "js": "text/plain",
        "ts": "text/plain", "rs": "text/plain", "py": "text/plain",
        "sh": "text/plain", "kt": "text/plain", "java": "text/plain",
    ]

    /// 预览容器四分(spec「四容器一分区」;词表矩阵见上方扩展名集)。
    public static func containerOf(name: String) -> PreviewContainer {
        let ext = extOf(name)
        if imageExts.contains(ext) { return .image }
        if videoExts.contains(ext) { return .video }
        if audioExts.contains(ext) { return .audio }
        if docExts.contains(ext) { return .doc }
        return .unsupported
    }

    /// HEIC 源(jpeg 转码端点仅对 heic 装配——非 HEIC 恒 404,桌面同语义)。
    public static func isHeic(name: String) -> Bool {
        extOf(name) == "heic"
    }

    /// 图片查看器取数序(桌面 image.ts 渐进序镜像):thumb 恒先显(快速
    /// 首帧),主图 heic = jpeg 端点优先 → raw 兜底;其余恒 raw。
    public static func imageFaces(name: String) -> [String] {
        isHeic(name: name) ? ["jpeg", "raw"] : ["raw"]
    }

    /// 原流回环 URL:`http://127.0.0.1:{port}/f/{handle}/raw?t={token}`
    /// (`?t=` 查询令牌与 Bearer 双缝等价,T20 面;仅本机进程可达)。
    public static func rawUrl(port: Int64, handle: String, token: String) -> String {
        "http://127.0.0.1:\(port)/f/\(handle)/raw?t=\(token)"
    }

    /// HEIC 转码 JPEG 回环 URL(形同 raw;非 HEIC 恒 404)。
    public static func jpegUrl(port: Int64, handle: String, token: String) -> String {
        "http://127.0.0.1:\(port)/f/\(handle)/jpeg?t=\(token)"
    }

    /// doc 容器的 previewer kind 词面(非 doc 容器/未知扩展 = nil)。
    public static func docKindOf(name: String) -> String? {
        docKindByExt[extOf(name)]
    }

    /// open-as MIME(未知/无扩展名 → application/octet-stream 兜底)。
    public static func mimeOf(name: String) -> String {
        mimeByExt[extOf(name)] ?? "application/octet-stream"
    }

    // MARK: WKWebView 载体面(T25;安卓 file:// 特权面的 iOS 同位)

    /// 宿主页自管 scheme。为什么不是 file://:WebKit 无安卓
    /// allowUniversalAccessFromFileURLs 等价开关——file:// 页 fetch 回环口
    /// 恒被 CORS 拒(daemon 不发 Access-Control 头,零 daemon 改动纪律)。
    /// 自管 scheme 经 WKURLSchemeHandler 供页 + 供回环代理 = 宿主页/资产/
    /// 代理**全同源**,fetch/XHR/Range 预检全零,previewer bundle 零改动。
    public static let proxyScheme = "mtdsspreview"

    /// 宿主页 URL(scheme handler 资产臂供页,App PreviewProxySchemeHandler)。
    public static let hostPageUrl = "mtdsspreview://localhost/preview.html"

    /// doc 容器 boot raw URL(自管 scheme 面;形状与 ``rawUrl`` 同构,仅
    /// scheme 换——反解单点 ``loopbackUrl(from:)``)。
    public static func proxyRawUrl(port: Int64, handle: String, token: String) -> String {
        "\(proxyScheme)://127.0.0.1:\(port)/f/\(handle)/raw?t=\(token)"
    }

    /// scheme 请求 → daemon 回环 URL 反解(App 侧 PreviewProxySchemeHandler
    /// 代理臂单点)。收紧面:仅本 scheme + 127.0.0.1 host + 显式数字 port
    /// + /f/ 路径接受(handler 不做任意出口);宿主页/bundle 资产路径返回
    /// nil(走 handler 资产臂)。
    public static func loopbackUrl(from url: URL?) -> URL? {
        guard let url,
              let comps = URLComponents(url: url, resolvingAgainstBaseURL: false),
              comps.scheme?.lowercased() == proxyScheme,
              comps.host?.lowercased() == "127.0.0.1",
              let port = comps.port,
              comps.path.hasPrefix("/f/")
        else { return nil }
        var out = URLComponents()
        out.scheme = "http"
        out.host = "127.0.0.1"
        out.port = port
        out.path = comps.path
        out.query = comps.query
        return out.url
    }

    /// scheme 请求 → daemon 回环 URLRequest(代理臂**出站**单点;App 侧
    /// handler 用它发起 URLSession 调用)。fix round 1 Important #1:Range
    /// 头必须转发——响应侧广告 Accept-Ranges: bytes,出站不带 Range 则
    /// daemon 恒 200 全量,pdf.js 按 206 校验即抛「Unexpected server
    /// response」。除 Range 外不透传其余头(宿主页同源面无凭据语义,
    /// Origin/Referer 等浏览器面头不外泄;cachePolicy = 回环短命件不落盘)。
    public static func proxyRequest(for request: URLRequest?) -> URLRequest? {
        guard let incoming = request, let target = loopbackUrl(from: incoming.url) else { return nil }
        var out = URLRequest(url: target)
        out.cachePolicy = .reloadIgnoringLocalCacheData
        if let range = incoming.value(forHTTPHeaderField: "Range"), !range.isEmpty {
            out.setValue(range, forHTTPHeaderField: "Range")
        }
        return out
    }

    /// WebView 导航守卫(T23 fix round 1 Important #2 的 iOS 面:特权面
    /// 永不导外部内容)。仅自管资产面放行:本 scheme(宿主页载体)+ file:
    /// (安卓载体同位,守卫语义单源);http(s) 外链点击交系统浏览器、其余
    /// scheme(intent:/javascript:/data: 等)拒。空串拒。
    public static func linkNavigable(_ url: String) -> Bool {
        url.hasPrefix("file:") || url.hasPrefix("\(proxyScheme):")
    }

    /// 取小写扩展名(与桌面 filetype.ts extOf / ``BrowseThumbs`` 同口径):
    /// 无点/点在首位(点文件)/点在末尾 → 空串(视为无扩展名)。
    static func extOf(_ name: String) -> String {
        guard let i = name.lastIndex(of: "."), i != name.startIndex, i != name.index(before: name.endIndex) else {
            return ""
        }
        return String(name[name.index(after: i)...]).lowercased()
    }
}
