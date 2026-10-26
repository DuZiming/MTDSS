package com.mtdss.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import com.mtdss.android.ui.MtdssApp

/**
 * 唯一 Activity:T7 的临时 hello 调试屏已被真实导航(登录 → 设备 → 浏览)
 * 取代;调试观测面收编进设备页「设置 → 诊断」(DiagnosticsSheet)。
 * 引擎拉起/IPC 装配在 MtdssApplication(AppGraph),与 Activity 生命周期
 * 解耦(旋转/重建不重置连接与订阅)。
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                MtdssApp(AppGraph.get())
            }
        }
    }
}
