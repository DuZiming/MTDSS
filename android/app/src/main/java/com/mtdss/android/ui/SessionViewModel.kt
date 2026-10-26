package com.mtdss.android.ui

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.ConnState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 会话状态 VM(app 级,AppNav 的路由依据):连接态 + 登录态 + 本机在线 +
 * 本机设备身份。数据面(镜像桌面 App.tsx):
 * - **首连发起**:init 即 connect(快速失败语义,IpcClient 后台自愈覆盖
 *   引擎冷启动「拉起 → socket 可连」的间隙);
 * - 连接翻转:Connected 即全量拉取 status(冷启动/引擎重启自愈同径);
 * - event.login 翻转 → 全量刷新(托盘/CLI 侧登录登出同样到达);
 * - event.online 只翻 selfOnline 单字段(不触发重拉)。
 *
 * loggedIn = null 表示尚无权威数据(引擎冷启动窗口),AppNav 以 splash 呈现。
 */
class SessionViewModel(
    private val repo: MtdssRepository,
    hub: MtdssEventHub,
    private val scope: CoroutineScope,
) {
    data class State(
        val conn: ConnState = ConnState.Disconnected,
        val loggedIn: Boolean? = null,
        val selfOnline: Boolean? = null,
        val deviceId: String? = null,
        val deviceName: String? = null,
    )

    private val _state = MutableStateFlow(State())

    val state: StateFlow<State> = _state.asStateFlow()

    init {
        scope.launch { repo.connect() } // 首连发起(fast-fail;自愈接管)
        scope.launch {
            repo.connectionState.collect { st ->
                _state.value = _state.value.copy(conn = st)
                if (st == ConnState.Connected) refresh()
            }
        }
        scope.launch {
            hub.login.collect { if (it != null) refresh() }
        }
        scope.launch {
            hub.selfOnline.collect { v -> if (v != null) _state.value = _state.value.copy(selfOnline = v) }
        }
    }

    /** 全量拉取 status(权威登录/在线/设备身份)。 */
    fun refresh() {
        scope.launch {
            repo.status().fold(
                onSuccess = { s ->
                    _state.value = _state.value.copy(
                        loggedIn = s.loggedIn,
                        selfOnline = s.online,
                        deviceId = s.device?.deviceId,
                        deviceName = s.device?.name,
                    )
                },
                onFailure = { /* 拉取失败保留前值;连接态事件会再触发 */ },
            )
        }
    }
}
