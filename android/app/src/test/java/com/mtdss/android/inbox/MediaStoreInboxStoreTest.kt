package com.mtdss.android.inbox

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * F10 fd 尺寸对账单测:exists 改「真开 fd 为准」后唯一可 JVM 钉死的纯判据;
 * open/statSize 的 MediaStore 行为由实机验证(MediaStoreInboxStore 保持薄,
 * openFileDescriptor("r") + statSize 对账在设备上走 M4b 冒烟)。
 */
class MediaStoreInboxStoreTest {

    @Test
    fun `fd 尺寸吻合才在场`() {
        assertTrue(MediaStoreInboxStore.fdSizeMatches(statSize = 2025, expected = 2025))
        // 尺寸不符(同名不同内容/残留行陈旧)→ 不在场,引擎 FULL 重收
        assertFalse(MediaStoreInboxStore.fdSizeMatches(statSize = 2024, expected = 2025))
    }

    @Test
    fun `fd 尺寸不可知按不在场`() {
        // statSize=-1(ParcelFileDescriptor「尺寸不可知」)→ 不在场
        // (失败方向铁律:宁可重传,不可假完成)
        assertFalse(MediaStoreInboxStore.fdSizeMatches(statSize = -1, expected = 2025))
    }
}
