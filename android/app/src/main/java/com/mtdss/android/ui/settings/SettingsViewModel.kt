package com.mtdss.android.ui.settings

import com.mtdss.android.update.UpdateController
import com.mtdss.android.ui.SessionViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 设置页 VM(spec §4.3:账号信息 / 收件箱位置 / 检查更新 / 关于):账号面是
 * app 级 SessionViewModel 的纯投影(登录/在线翻转全由会话状态机驱动,本 VM
 * 零自有业务);更新面整体委托 [update](T12 编排器,app 级——APK 下载不随
 * 设置页进出重置)。收件箱位置为固定展示(安卓收件箱固定 Downloads/MTDSS,
 * spec §4.2 评审裁定②),不占状态。
 */
class SettingsViewModel(
    sessionState: StateFlow<SessionViewModel.State>,
    appVersionName: String,
    appVersionCode: Int,
    val update: UpdateController,
    scope: CoroutineScope,
) {
    data class UiState(
        val loggedIn: Boolean? = null,
        val deviceId: String? = null,
        val deviceName: String? = null,
        val selfOnline: Boolean? = null,
        val appVersion: String = "",
    )

    private val _state = MutableStateFlow(UiState(appVersion = "$appVersionName ($appVersionCode)"))

    val state: StateFlow<UiState> = _state.asStateFlow()

    init {
        scope.launch {
            sessionState.collect { s ->
                _state.value = _state.value.copy(
                    loggedIn = s.loggedIn,
                    deviceId = s.deviceId,
                    deviceName = s.deviceName,
                    selfOnline = s.selfOnline,
                )
            }
        }
    }
}
