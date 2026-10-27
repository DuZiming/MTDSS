package com.mtdss.android.send

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** 条目数护栏单测(词面与 daemon MAX_BRIDGE_ITEMS 拒绝语逐字一致)。 */
class BridgeLimitsTest {

    @Test
    fun `恰在上限通过`() {
        assertNull(BridgeLimits.itemCountError(BridgeLimits.MAX_BRIDGE_ITEMS))
        assertNull(BridgeLimits.itemCountError(0))
        assertNull(BridgeLimits.itemCountError(1))
    }

    @Test
    fun `超限即拒且词面对齐 daemon`() {
        assertEquals(
            "桥接条目过多(513 项,上限 512):请分批发送",
            BridgeLimits.itemCountError(513),
        )
        assertEquals(
            "桥接条目过多(600 项,上限 512):请分批发送",
            BridgeLimits.itemCountError(600),
        )
    }
}
