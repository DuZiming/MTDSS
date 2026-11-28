/// 实机缩略图编码器(V4-1 T11;spec「AVAssetImageGenerator 视频抽帧 +
/// UIImage 降采样图片含 HEIC 系统解码,→JPEG 压缩」):显示名扩展名分型
/// ——视频 = `AVAssetImageGenerator` 抽帧(maximumSize 钳 ≤w,最近关键
/// 帧),其余 = `UIImage` 系统解码(HEIC 系统解码覆盖)经
/// `byPreparingThumbnail` 等比降采样到 ≤w(不放大),统一 JPEG 压缩回字
/// 节。Kit ``ThumbEncoder`` 的 App 目标实现(Photos/AVFoundation/UIKit 为
/// iOS-only 系统框架,Kit/宿主测试不触碰本文件,PHPhotoLibraryService 先
/// 案同款;解码正确性归实机冒烟,宿主单测走伪编码器)。
///
/// **fd → 路径**:AVAsset/ImageIO 均按路径开文件,源 fd 经 `fcntl(F_GETPATH)`
/// 取回沙箱路径(fd 只读不消费;fd 所有权归调用方——应答器编码后即关,
/// SeekResponder 交付即关同哲学)。**重活自派后台**:解码/压缩在
/// `Task.detached`,await 不占调用方隔离域(主 actor 应答环不卡帧)。

import AVFoundation
import MTDSSKit
import QuickLookThumbnailing
import UIKit

struct IOSThumbEncoder: ThumbEncoder {

    /// 视频扩展名分型表(AndroidThumbEncoder VIDEO_EXTS 同表;表外一律走
    /// 图片解码——ImageIO/UIImage 对非图源如实 nil,不误伤未知视频容器之
    /// 外的类型面)。
    private static let videoExts: Set<String> = [
        "mp4", "mov", "m4v", "mkv", "webm", "3gp", "avi", "ts",
    ]

    /// PDF 扩展名分型表(r30:QLThumbnailGenerator 文档缩略图——平台后端
    /// 无 PDF 面;QuickLookThumbnailing 为 iOS-only 系统框架,Ruling E 同款
    /// 绑定归 App)。
    private static let pdfExts: Set<String> = ["pdf"]

    /// JPEG 压缩质量(AndroidThumbEncoder compress 同值 85)。
    private static let jpegQuality: CGFloat = 0.85

    func jpeg(fd: Int32, displayName: String, w: Int) async -> Data? {
        // fd → 沙箱路径(F_GETPATH;失败 = 不可解面,拒答落回退)
        var buf = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        guard fcntl(fd, F_GETPATH, &buf) == 0 else { return nil }
        let path = String(cString: buf)
        let ext = (displayName as NSString).pathExtension.lowercased()
        return await Task.detached(priority: .userInitiated) { () -> Data? in
            if Self.videoExts.contains(ext) {
                guard let cg = await Self.videoFrame(path: path, w: w) else { return nil }
                return UIImage(cgImage: cg).jpegData(compressionQuality: Self.jpegQuality)
            }
            if Self.pdfExts.contains(ext) {
                return await Self.pdfThumbnail(path: path, w: w)
            }
            return await Self.imageThumbnail(path: path, w: w)?
                .jpegData(compressionQuality: Self.jpegQuality)
        }.value
    }

    /// PDF 文档缩略图(r30:QLThumbnailGenerator 首页;失败/不可得 = nil
    /// 拒答回退图标——与图片/视频臂同降级语义)。
    private static func pdfThumbnail(path: String, w: Int) async -> Data? {
        let request = QLThumbnailGenerator.Request(
            fileAt: URL(fileURLWithPath: path),
            size: CGSize(width: CGFloat(w), height: CGFloat(w)),
            scale: 1,
            representationTypes: .thumbnail)
        return await withCheckedContinuation { (cont: CheckedContinuation<Data?, Never>) in
            QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { rep, _ in
                cont.resume(returning: rep?.uiImage.jpegData(compressionQuality: Self.jpegQuality))
            }
        }
    }

    /// 视频抽帧(首帧最近关键帧;maximumSize 钳 ≤w 等比降采样——
    /// OPTION_CLOSEST_SYNC 同象:t=0 容忍向后跳最近关键帧)。
    private static func videoFrame(path: String, w: Int) async -> CGImage? {
        let asset = AVURLAsset(url: URL(fileURLWithPath: path))
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: CGFloat(w), height: CGFloat(w))
        generator.requestedTimeToleranceBefore = .zero
        generator.requestedTimeToleranceAfter = .positiveInfinity
        do {
            let (image, _) = try await generator.image(at: .zero)
            return image
        } catch {
            return nil
        }
    }

    /// 图片系统解码(UIImage;HEIC 系统解码覆盖)+ 等比降采样到 ≤w
    /// (`byPreparingThumbnail`,不放大——原尺寸已 ≤w 恒原样;解码失败/
    /// 非图源如实 nil)。
    private static func imageThumbnail(path: String, w: Int) async -> UIImage? {
        guard let image = UIImage(contentsOfFile: path) else { return nil }
        // contentsOfFile 载入 scale = 1 → size 即像素;等比钳最长边 ≤w
        // (Android 兜底等比缩同象,等比不裁切——查看端语义)
        let px = image.size
        let longest = max(px.width, px.height)
        let target: CGSize
        if longest > CGFloat(w) {
            let scale = CGFloat(w) / longest
            target = CGSize(
                width: max(1, (px.width * scale).rounded(.down)),
                height: max(1, (px.height * scale).rounded(.down)))
        } else {
            target = px
        }
        return await image.byPreparingThumbnail(ofSize: target)
    }
}
