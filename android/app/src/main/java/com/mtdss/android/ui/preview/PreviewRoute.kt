package com.mtdss.android.ui.preview

/**
 * 预览路由纯函数面(V4-1 T23;钦定 mockup mobile-preview.html):文件名 →
 * 四容器路由 + HEIC 面 + 回环 URL 组装 + doc kind 词面 + open-as MIME。
 * 零 Android/Compose 依赖,宿主 JVM 单测直驱([PreviewRouteTest]);
 * 词表与桌面 ui/src/lib/preview.ts KIND_BY_EXT 同源(漂移先红本套)。
 */
object PreviewRoute {

    /** 图片扩展名词表(filetype.ts img 类同源,小写存取)。 */
    private val IMAGE_EXTS =
        setOf("jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif")

    /** 视频扩展名词表(filetype.ts vid 类同源)。 */
    private val VIDEO_EXTS =
        setOf("mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp")

    /** 音频扩展名词表(filetype.ts 词表同源)。 */
    private val AUDIO_EXTS =
        setOf("mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus")

    /** 文档扩展名词表(pdf/office/md/文本;preview.ts text 高亮语言域同源)。 */
    private val DOC_EXTS =
        setOf(
            "pdf",
            "doc", "docx", "xls", "xlsx", "csv", "ppt", "pptx",
            "md", "txt", "log",
            "rs", "ts", "tsx", "js", "mjs", "cjs", "jsx", "py", "go", "java", "kt", "kts",
            "swift", "c", "h", "cpp", "cc", "cxx", "hpp", "cs", "rb", "php", "sh", "bash",
            "zsh", "ps1", "bat", "json", "jsonc", "yml", "yaml", "toml", "ini", "cfg",
            "conf", "xml", "html", "css", "sql",
        )

    /** doc 扩展名 → previewer kind 词面(preview.ts KIND_BY_EXT office/md/text 段同源)。 */
    private val DOC_KIND_BY_EXT: Map<String, String> = buildMap {
        put("pdf", "pdf")
        put("doc", "docx"); put("docx", "docx")
        put("xls", "xlsx"); put("xlsx", "xlsx"); put("csv", "xlsx")
        put("ppt", "pptx"); put("pptx", "pptx")
        put("md", "md")
        for (ext in DOC_EXTS - setOf("pdf", "doc", "docx", "xls", "xlsx", "csv", "ppt", "pptx", "md")) {
            put(ext, "text")
        }
    }

    /** open-as MIME 表(系统打开器常识面;未知落 octet-stream)。 */
    private val MIME_BY_EXT: Map<String, String> = mapOf(
        "jpg" to "image/jpeg", "jpeg" to "image/jpeg", "png" to "image/png",
        "gif" to "image/gif", "bmp" to "image/bmp", "webp" to "image/webp",
        "heic" to "image/heic", "svg" to "image/svg+xml", "tif" to "image/tiff",
        "tiff" to "image/tiff", "ico" to "image/x-icon", "avif" to "image/avif",
        "mp4" to "video/mp4", "m4v" to "video/mp4", "mov" to "video/quicktime",
        "webm" to "video/webm", "mkv" to "video/x-matroska", "avi" to "video/x-msvideo",
        "3gp" to "video/3gpp", "wmv" to "video/x-ms-wmv", "flv" to "video/x-flv",
        "mpg" to "video/mpeg", "mpeg" to "video/mpeg",
        "mp3" to "audio/mpeg", "flac" to "audio/flac", "wav" to "audio/wav",
        "aac" to "audio/aac", "ogg" to "audio/ogg", "m4a" to "audio/mp4",
        "wma" to "audio/x-ms-wma", "opus" to "audio/ogg",
        "pdf" to "application/pdf",
        "doc" to "application/msword",
        "docx" to "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" to "application/vnd.ms-excel",
        "xlsx" to "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "csv" to "text/csv",
        "ppt" to "application/vnd.ms-powerpoint",
        "pptx" to "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "txt" to "text/plain", "log" to "text/plain", "md" to "text/plain",
        "json" to "text/plain", "xml" to "text/plain", "yml" to "text/plain",
        "yaml" to "text/plain", "toml" to "text/plain", "ini" to "text/plain",
        "html" to "text/plain", "css" to "text/plain", "js" to "text/plain",
        "ts" to "text/plain", "rs" to "text/plain", "py" to "text/plain",
        "sh" to "text/plain", "kt" to "text/plain", "java" to "text/plain",
    )

    /**
     * 预览容器四分(spec「四容器一分区」路由唯一依据):图片 = Compose
     * zoom 查看器;视频/音频 = Media3 ExoPlayer 原流直连(移动端原生解码
     * 不经转码,spec 裁定);pdf/office/md/文本 = WebView(previewer
     * bundle);其余 = 类型页 + 下载引导。
     */
    fun containerOf(name: String): PreviewContainer {
        val ext = extOf(name)
        return when {
            ext in IMAGE_EXTS -> PreviewContainer.IMAGE
            ext in VIDEO_EXTS -> PreviewContainer.VIDEO
            ext in AUDIO_EXTS -> PreviewContainer.AUDIO
            ext in DOC_EXTS -> PreviewContainer.DOC
            else -> PreviewContainer.UNSUPPORTED
        }
    }

    /** HEIC 源(jpeg 转码端点仅对 heic 装配——非 HEIC 恒 404,桌面同语义)。 */
    fun isHeic(name: String): Boolean = extOf(name) == "heic"

    /**
     * 图片查看器取数序(桌面 image.ts 渐进序镜像):thumb 恒先显(快速
     * 首帧),主图 heic = jpeg 端点优先 → raw 兜底;其余恒 raw。
     */
    fun imageFaces(name: String): List<String> =
        if (isHeic(name)) listOf("jpeg", "raw") else listOf("raw")

    /**
     * 原流回环 URL:`http://127.0.0.1:{port}/f/{handle}/raw?t={token}`
     * (`?t=` 查询令牌与 Bearer 双缝等价,T20 面;仅本机进程可达)。
     */
    fun rawUrl(port: Long, handle: String, token: String): String =
        "http://127.0.0.1:$port/f/$handle/raw?t=$token"

    /** HEIC 转码 JPEG 回环 URL(形同 raw;非 HEIC 恒 404)。 */
    fun jpegUrl(port: Long, handle: String, token: String): String =
        "http://127.0.0.1:$port/f/$handle/jpeg?t=$token"

    /** doc 容器的 previewer kind 词面(非 doc 容器/未知扩展 = null)。 */
    fun docKindOf(name: String): String? = DOC_KIND_BY_EXT[extOf(name)]

    /** open-as MIME(未知/无扩展名 → application/octet-stream 兜底)。 */
    fun mimeOf(name: String): String =
        MIME_BY_EXT[extOf(name)] ?: "application/octet-stream"

    /**
     * WebView 导航守卫(review fix round 1 Important #2;特权 WebView 锁面):
     * 仅 file(资产宿主页 + 渲染内部引用)放行;http(s) 外链点击交系统
     * 浏览器、其余 scheme(intent:/javascript:/data: 等)拒——file://
     * 全域访问特权面永不触外部内容。空串拒。
     */
    fun linkNavigable(url: String): Boolean =
        // r67 #5 根修:自家资产域放行(WebViewAssetLoader 迁移后宿主页是
        // https://appassets…,旧 file: 判把初始加载拦成「外链」= 恒白屏)
        url.startsWith("file:") || url.startsWith("https://appassets.androidplatform.net/")

    /** 取小写扩展名(与桌面 filetype.ts extOf 同口径):无点/点在首位(点
     *  文件)/点在末尾 → 空串。 */
    private fun extOf(name: String): String {
        val i = name.lastIndexOf('.')
        if (i <= 0 || i == name.length - 1) return ""
        return name.substring(i + 1).lowercase()
    }
}

/** 预览容器四分(T23 路由唯一依据,见 [PreviewRoute.containerOf])。 */
enum class PreviewContainer { IMAGE, VIDEO, AUDIO, DOC, UNSUPPORTED }
