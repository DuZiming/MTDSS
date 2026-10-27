package com.mtdss.android.ui.send

import com.mtdss.android.send.BridgeLimits
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 发送键可用性单测(fix 波钉死):条目数超限**不得静默禁键**——超限词面
 * 由 SendController 前置拦截进 State.error(弹层渲染),弹层只看输入完备。
 * 回归引入「按条目数禁键」即此红。
 */
class SendDialogsTest {

    @Test
    fun `超限条目数不禁键(词面路径可达)`() {
        // 选择器多选 513+ / SEND_MULTIPLE 大分享是真实可达路径
        assertTrue(sendButtonEnabled("C:/recv", BridgeLimits.MAX_BRIDGE_ITEMS + 1))
        assertTrue(sendButtonEnabled("C:/recv", 600))
        // 恰在上限照常
        assertTrue(sendButtonEnabled("C:/recv", BridgeLimits.MAX_BRIDGE_ITEMS))
    }

    @Test
    fun `输入不完备才禁键`() {
        assertFalse(sendButtonEnabled("", 5)) // dir 空(对端要绝对路径)
        assertFalse(sendButtonEnabled("   ", 5)) // dir 纯空白
        assertFalse(sendButtonEnabled("C:/recv", 0)) // 无条目(空展开不进弹层,防御)
    }
}
