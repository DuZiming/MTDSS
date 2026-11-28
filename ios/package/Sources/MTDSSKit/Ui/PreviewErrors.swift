/// 预览降级面文案纯函数(V4-1 T25,宿主单测直驱 PreviewErrorsTests;安卓
/// ui/preview/PreviewErrors.kt 同构):preview_open 失败的 RPC 错误消息
/// (daemon ctrl 帧 message 直透,面向用户)→ 用户面映射:
/// - `icloud_not_local` 稳定词在消息中 = T10 词透出——iPhone 未卸载 iCloud
///   优化件拒开 seek fd 的错误词(远端 iPhone 源场景 + **自机**照片库预览
///   遇未卸载项同词,fdpass.rs KIND_SEEK_FD 注;不静默全量拉);面 =
///   「原件在 iCloud,需先下载」;
/// - 离线族(远端 PeerOffline「设备离线: …」/ 本机 source_offline「源打开
///   失败: …」/ 稳定词 peer_offline)→ mockup 确认点⑤ 词面「源设备离线,
///   无法预览」;
/// - 其余消息直透(daemon 消息本就面向用户);空/nil 归通用面。

import Foundation

public enum PreviewErrors {

    /// 源设备离线面(mockup 确认点⑤逐字)。
    public static let offlineFace = "源设备离线,无法预览"

    /// iCloud 原件未下载面(T10 `icloud_not_local` 词透出)。
    public static let icloudFace = "原件在 iCloud,需先下载"

    /// 通用失败面(空/无名错误兜底)。
    public static let genericFace = "无法预览该文件"

    /// 离线族稳定词/消息词(小词表直查,保守归一)。
    private static let offlineMarkers = ["设备离线", "peer_offline", "source_offline", "源打开失败"]

    public static func userFace(_ message: String?) -> String {
        // 空白归通用面(安卓 isBlank 同口径);其余先词后透传(原样返回,
        // daemon 消息本就面向用户,不做二次裁写)
        guard let message, !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return genericFace
        }
        if message.contains("icloud_not_local") { return icloudFace }
        if offlineMarkers.contains(where: { message.contains($0) }) { return offlineFace }
        return message
    }
}
