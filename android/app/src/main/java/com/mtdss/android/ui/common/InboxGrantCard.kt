package com.mtdss.android.ui.common

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.inbox.InboxGrantStore

/**
 * 收件箱授权引导卡(r62 #1:首次启动无授权 → 快捷目录根部引导;设置页
 * 「收件箱」行同件复用)。iOS 首次引导同象:
 * - 授权 = ACTION_OPEN_DOCUMENT_TREE(系统目录选择器预开 Download/MTDSS,
 *   用户确认即持久授权[读写双旗,重启存活]);
 * - 跳过 = 收件功能降级(收不到文件/收件箱不可见),设置页可随时再授;
 * - 已授权 = 不渲染(静默)。
 */
@Composable
fun InboxGrantCard(
    grant: InboxGrantStore,
    modifier: Modifier = Modifier,
    onGranted: (() -> Unit)? = null,
) {
    var hasGrant by remember { mutableStateOf(grant.treeUri() != null) }
    if (hasGrant) return
    val launcher = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree()
    ) { uri ->
        if (uri != null && grant.grant(uri.toString()) != null) {
            hasGrant = true
            onGranted?.invoke()
        }
    }
    Surface(
        modifier = modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.surfaceVariant,
        tonalElevation = 1.dp,
    ) {
        Column(Modifier.padding(16.dp)) {
            Text("启用收件箱", color = MaterialTheme.colorScheme.onSurface, fontSize = 16.sp)
            Spacer(Modifier.height(6.dp))
            Text(
                "授权后,其他设备发来的文件会自动保存到「下载/MTDSS」文件夹;" +
                    "选择「下载(Download)」目录即可,无需手动创建。可随时在 设置 中开启。",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                fontSize = 13.sp,
                lineHeight = 18.sp,
            )
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                Button(onClick = { launcher.launch(grant.inboxInitialUri()) }) {
                    Text("立即授权")
                }
                TextButton(onClick = { hasGrant = true /* 本次会话不再打扰;设置页可再授 */ }) {
                    Text("暂不")
                }
                Spacer(Modifier.width(4.dp))
            }
        }
    }
}
