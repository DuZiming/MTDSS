package com.mtdss.android.ui.profile

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * rotateLuminance 旋转矩阵纯 JVM 单测(fix round 1 回归锁定):3×2(w≠h)
 * 夹具四向逐字节断言 + 往返互逆 + 640×480(w>h,横置传感器典型)越界探针
 * ——90/270 行系数曾误取 src 宽维分量,w>h 帧首试即 AIOOBE(分析器线程
 * 未捕获异常 = 进程崩溃),此处防止回潮。
 */
class ScanSheetRotateTest {

    /**
     * 3×2(w=3,h=2)夹具,src[row*3+col]:
     * ```
     * A B C
     * D E F
     * ```
     * A=1 B=2 C=3 D=4 E=5 F=6。
     */
    private val src = byteArrayOf(1, 2, 3, 4, 5, 6)

    @Test
    fun `0 度原样返回宽高不变`() {
        val (data, w, h) = rotateLuminance(src, 3, 2, 0)
        assertEquals(src.toList(), data.toList())
        assertEquals(3, w)
        assertEquals(2, h)
    }

    @Test
    fun `90 度顺时针换轴逐字节`() {
        // CW:右列变首行(读序倒置)——dst 2 宽×3 高:
        // ```
        // D A
        // E B
        // F C
        // ```
        // 行主序 = [4,1, 5,2, 6,3]。
        val (data, w, h) = rotateLuminance(src, 3, 2, 90)
        assertEquals(byteArrayOf(4, 1, 5, 2, 6, 3).toList(), data.toList())
        assertEquals(2, w)
        assertEquals(3, h)
    }

    @Test
    fun `180 度全逆逐字节`() {
        val (data, w, h) = rotateLuminance(src, 3, 2, 180)
        assertEquals(byteArrayOf(6, 5, 4, 3, 2, 1).toList(), data.toList())
        assertEquals(3, w)
        assertEquals(2, h)
    }

    @Test
    fun `270 度逆时针换轴逐字节`() {
        // CCW:顶行变左列——dst 2 宽×3 高:
        // ```
        // C F
        // B E
        // A D
        // ```
        // 行主序 = [3,6, 2,5, 1,4];宽×高互换(同 90°)→ 返回 (w=2, h=3)。
        val (data, w, h) = rotateLuminance(src, 3, 2, 270)
        assertEquals(byteArrayOf(3, 6, 2, 5, 1, 4).toList(), data.toList())
        assertEquals(2, w)
        assertEquals(3, h)
    }

    @Test
    fun `360 度归零原样`() {
        val (data, w, h) = rotateLuminance(src, 3, 2, 360)
        assertEquals(src.toList(), data.toList())
        assertEquals(3, w)
        assertEquals(2, h)
    }

    @Test
    fun `90 与 270 互逆往返还原`() {
        val (r90, w90, h90) = rotateLuminance(src, 3, 2, 90)
        val (back, w, h) = rotateLuminance(r90, w90, h90, 270)
        assertEquals(src.toList(), back.toList())
        assertEquals(3, w)
        assertEquals(2, h)
    }

    @Test
    fun `横置传感器 640x480 四向全旋转不越界`() {
        // w>h 探针(旧 90° 分支 max index 409,439 ≫ 307,199):四向全跑,
        // 只要不 AIOOBE 即过(尺寸不变量 + 输出长度断言)。
        val w = 640
        val h = 480
        val big = ByteArray(w * h) { (it % 251).toByte() }
        for (deg in intArrayOf(0, 90, 180, 270)) {
            val (data, ow, oh) = rotateLuminance(big, w, h, deg)
            assertEquals(big.size, data.size)
            assertEquals(if (deg % 180 == 0) w else h, ow)
            assertEquals(if (deg % 180 == 0) h else w, oh)
        }
    }
}
