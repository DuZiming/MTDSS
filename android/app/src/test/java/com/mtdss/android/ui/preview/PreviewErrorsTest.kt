package com.mtdss.android.ui.preview

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 预览降级面文案 JVM 单测(V4-1 T23):源离线 mockup 确认点⑤ 词面
 * 「源设备离线,无法预览」;T10 `icloud_not_local` 词透出 =「原件在
 * iCloud,需先下载」(安卓侧远端 iPhone 源场景);daemon RPC 错误消息
 * (ctrl 帧 message 直透) → 用户面映射纯函数。
 */
class PreviewErrorsTest {

    @Test
    fun `iCloud 未卸载词透出`() {
        assertEquals(
            "icloud_not_local 稳定词在消息中即透出 iCloud 面",
            "原件在 iCloud,需先下载",
            PreviewErrors.userFace("远端: icloud_not_local 原件未下载到本机"),
        )
        assertEquals(
            "原件在 iCloud,需先下载",
            PreviewErrors.userFace("icloud_not_local"),
        )
    }

    @Test
    fun `源离线词归一(mockup 确认点⑤词面)`() {
        // 远端设备离线(ctrl PeerOffline 词)
        assertEquals(
            "源设备离线,无法预览",
            PreviewErrors.userFace("设备离线: MIX 3"),
        )
        // 本机源打开失败(api.rs source_offline 词)
        assertEquals(
            "源设备离线,无法预览",
            PreviewErrors.userFace("源打开失败: Permission denied"),
        )
        // 稳定词形(peer_offline)
        assertEquals(
            "源设备离线,无法预览",
            PreviewErrors.userFace("peer_offline: 连接失败"),
        )
    }

    @Test
    fun `其他消息透传(daemon 消息本就面向用户)`() {
        assertEquals("文件已变更,请重试", PreviewErrors.userFace("文件已变更,请重试"))
    }

    @Test
    fun `空与 null 归通用面`() {
        assertEquals("无法预览该文件", PreviewErrors.userFace(null))
        assertEquals("无法预览该文件", PreviewErrors.userFace(""))
        assertEquals("无法预览该文件", PreviewErrors.userFace("   "))
    }

    @Test
    fun `词面常量锚`() {
        assertEquals("源设备离线,无法预览", PreviewErrors.OFFLINE_FACE)
        assertEquals("原件在 iCloud,需先下载", PreviewErrors.ICLOUD_FACE)
        assertEquals("无法预览该文件", PreviewErrors.GENERIC_FACE)
    }
}
