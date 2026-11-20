/// 浏览缩略图纯函数面(V4-1 T24;钦定 mockup mobile-tiles-v2/mobile-browse
/// .html iOS 列):缩略图资格矩阵 / 回环 URL 组装 / 时长角标格式化。零
/// SwiftUI/Foundation-UI 依赖,宿主单测直驱(BrowseThumbsTests)。安卓
/// ui/browse/BrowseThumbs.kt 同构(词表/格式化/URL 单源漂移防护)。

import Foundation

public enum BrowseThumbs {

    /// 缩略图请求宽(brief 钉死 `?w=256`;daemon 侧 loopback /thumb 的
    /// THUMB_DEFAULT_WIDTH 同值锚,两处不得漂移)。
    public static let thumbWidth = 256

    /// 平铺列数(钦定 mockup:393pt 屏宽一行 4 枚磁贴)。
    public static let gridColumns = 4

    /// 图片扩展名词表(与桌面 ui/src/lib/filetype.ts img 类同源,小写存取)。
    static let imageExts: Set<String> = [
        "jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif",
    ]

    /// 视频扩展名词表(与桌面 filetype.ts vid 类同源)。
    static let videoExts: Set<String> = [
        "mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp",
    ]

    /// 该条目是否出缩略图(图片/视频文件;目录/其余 = 类型图标占位)。
    public static func thumbEligible(name: String, isDir: Bool) -> Bool {
        guard !isDir else { return false }
        let ext = extOf(name)
        return imageExts.contains(ext) || videoExts.contains(ext)
    }

    /// 该条目是否视频(时长角标只落视频,图片有 secs 也不显,mockup 同)。
    public static func isVideoName(_ name: String) -> Bool {
        videoExts.contains(extOf(name))
    }

    /**
     * 时长角标串(桌面 ui/src/lib/duration.ts fmtDuration 逐字镜像):
     * `m:ss`(不满 1 小时,分位不补零)/ `h:mm:ss`(满 1 小时,时分位补零);
     * 小数向下取整;nil/非正/非有限数 → 空串(有则显无则不显,角标不落垃圾)。
     */
    public static func durationBadge(_ secs: Double?) -> String {
        guard let secs, secs.isFinite, secs > 0 else { return "" }
        let total = Int64(secs) // 正数 = floor
        let ss = String(format: "%02d", Int(total % 60))
        let mins = total / 60 % 60
        let hours = total / 3600
        if hours == 0 { return "\(mins):\(ss)" } // 分位不补零
        let mm = String(format: "%02d", Int(mins))
        return "\(hours):\(mm):\(ss)"
    }

    /// 条目时长角标(视频名 + secs 在场才显;其余空串)。
    public static func durationBadgeFor(name: String, secs: Double?) -> String {
        isVideoName(name) ? durationBadge(secs) : ""
    }

    /**
     * 缩略图回环 URL:`http://127.0.0.1:{port}/f/{handle}/thumb?w={w}&t={token}`
     * (?t= 查询令牌与 Bearer 双缝等价,T20 面;URLSession/直连不带头的消费面
     * 靠它,与桌面 previewer bundle 同一凭据缝)。
     */
    public static func thumbUrl(port: Int64, handle: String, token: String, w: Int = thumbWidth) -> String {
        "http://127.0.0.1:\(port)/f/\(handle)/thumb?w=\(w)&t=\(token)"
    }

    /// 取小写扩展名(与桌面 filetype.ts extOf 同口径):无点/点在首位(点
    /// 文件)/点在末尾 → 空串(视为无扩展名)。
    static func extOf(_ name: String) -> String {
        guard let i = name.lastIndex(of: "."), i != name.startIndex, i != name.index(before: name.endIndex) else {
            return ""
        }
        return String(name[name.index(after: i)...]).lowercased()
    }
}
