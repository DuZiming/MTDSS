/// 图库切换几何纯函数(V4-1 T25;钦定 mockup mobile-preview ‹› 切换 +
/// 底部页点,安卓 ui/preview/PreviewGallery.kt 同构):宿主单测直驱
/// (PreviewGalleryTests)。

import Foundation

public enum PreviewGallery {

    /// 图库切换集 = 文件行中的图片类(目录/非图片不入;展示序不变)——
    /// ‹› 只在图片容器出(Android 列 mockup:图片查看器页点;视频/文档
    /// 单件预览无切换面)。
    public static func imageSiblings(_ files: [DirEntry]) -> [DirEntry] {
        files.filter { !$0.isDir && PreviewRoute.containerOf(name: $0.name) == .image }
    }

    /// 视频切换集(r49 #2:iOS 视频相册化——中央滑切面取视频类,展示序
    /// 不变,目录/非视频不入)。
    public static func videoSiblings(_ files: [DirEntry]) -> [DirEntry] {
        files.filter { !$0.isDir && PreviewRoute.containerOf(name: $0.name) == .video }
    }

    /// 音频切换集(r52 #4:iOS 音频滑切——兄弟集音频类,展示序不变)。
    public static func audioSiblings(_ files: [DirEntry]) -> [DirEntry] {
        files.filter { !$0.isDir && PreviewRoute.containerOf(name: $0.name) == .audio }
    }

    /// 兄弟集 = 展示序同深度连续切片(展开子树不串层):浏览投影行树中,
    /// index 行所在目录的连续段(左右两侧扫过深度 ≥ 本行的行,遇更浅深度
    /// 收口);index 越界退单条。行树结构:父行(depth d)后随子行(d+1),
    /// 切片即所在目录列表。
    public static func siblingFiles(_ rows: [BrowseRow], index: Int) -> [DirEntry] {
        if !(rows.indices.contains(index)) {
            return rows.first.map { [$0.entry] } ?? []
        }
        let depth = rows[index].depth
        var lo = index
        var hi = index
        while lo > 0, rows[lo - 1].depth >= depth { lo -= 1 }
        while hi + 1 < rows.count, rows[hi + 1].depth >= depth { hi += 1 }
        return rows[lo...hi].map(\.entry)
    }

    /// 环绕索引(‹› 首尾相接;size 0 守卫 = 恒 0)。
    public static func wrapIndex(_ index: Int, _ size: Int) -> Int {
        guard size > 0 else { return 0 }
        return ((index % size) + size) % size
    }
}
