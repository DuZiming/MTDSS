package com.mtdss.android.ui.preview

/**
 * 预览降级面文案纯函数(V4-1 T23,宿主 JVM 单测直驱 [PreviewErrorsTest]):
 * preview_open 失败的 RPC 错误消息(daemon ctrl 帧 message 直透,面向
 * 用户)→ 用户面映射:
 * - `icloud_not_local` 稳定词在消息中 = T10 词透出——远端 iPhone 未卸载
 *   iCloud 优化件拒开 seek fd 的错误词,安卓侧远端 iPhone 源场景同词
 *   (fdpass.rs KIND_SEEK_FD 注);面 =「原件在 iCloud,需先下载」;
 * - 离线族(远端 PeerOffline「设备离线: …」/ 本机 source_offline
 *   「源打开失败: …」/ 稳定词 peer_offline)→ mockup 确认点⑤ 词面
 *   「源设备离线,无法预览」;
 * - 其余消息直透(daemon 消息本就面向用户);空/null 归通用面。
 */
object PreviewErrors {

    /** 源设备离线面(mockup 确认点⑤逐字)。 */
    const val OFFLINE_FACE = "源设备离线,无法预览"

    /** iCloud 原件未下载面(T10 `icloud_not_local` 词透出)。 */
    const val ICLOUD_FACE = "原件在 iCloud,需先下载"

    /** 通用失败面(空/无名错误兜底)。 */
    const val GENERIC_FACE = "无法预览该文件"

    /** 离线族稳定词/消息词(小词表直查,保守归一)。 */
    private val OFFLINE_MARKERS = listOf("设备离线", "peer_offline", "source_offline", "源打开失败")

    fun userFace(message: String?): String = when {
        message == null || message.isBlank() -> GENERIC_FACE
        message.contains("icloud_not_local") -> ICLOUD_FACE
        OFFLINE_MARKERS.any { message.contains(it) } -> OFFLINE_FACE
        else -> message
    }
}
