package com.mtdss.android.ui.settings

import com.mtdss.android.inbox.InboxGrantStore
import com.mtdss.android.update.UpdateController
import com.mtdss.android.ui.SessionViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 设置页 VM(spec §4.3:账号信息 / 收件箱 / 检查更新 / 关于):账号面是
 * app 级 SessionViewModel 的纯投影(登录/在线翻转全由会话状态机驱动,本 VM
 * 零自有业务);更新面整体委托 [update](T12 编排器,app 级——APK 下载不随
 * 设置页进出重置)。收件箱面 = SAF 目录授权状态 + 授权入口(P4a-r2,
 * spec §7.1:授权 Download/MTDSS 最优,或 Download 由 App 在树内自管
 * MTDSS 子目录;状态实时读 [InboxGrantStore] 账本)。
 */
class SettingsViewModel(
    sessionState: StateFlow<SessionViewModel.State>,
    appVersionName: String,
    appVersionCode: Int,
    val update: UpdateController,
    private val inboxGrant: InboxGrantStore,
    scope: CoroutineScope,
) {
    data class UiState(
        val loggedIn: Boolean? = null,
        val deviceId: String? = null,
        val deviceName: String? = null,
        val selfOnline: Boolean? = null,
        val appVersion: String = "",
        /** 收件箱 SAF 目录授权(未授权 = 首次收件发布将「保存失败」,引导先授权)。 */
        val inboxAuthorized: Boolean = false,
    )

    private val _state = MutableStateFlow(
        UiState(appVersion = "$appVersionName ($appVersionCode)", inboxAuthorized = inboxGrant.treeUri() != null),
    )

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

    /**
     * 登记 OPEN_DOCUMENT_TREE 授权结果(账本有效性甄别失败 → false,屏面
     * 提示重选;成功刷新状态)。发布前的收件授权闸即此面——无授权时发布
     * 词面「收件箱未授权:请在 App 内完成目录授权后重试」指向本入口。
     */
    fun grantInbox(treeUri: String): Boolean {
        val granted = inboxGrant.grant(treeUri) != null
        _state.value = _state.value.copy(inboxAuthorized = granted)
        return granted
    }
}
