package com.mtdss.android.ui.devices

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirectoryDevice
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 设备列表屏 VM:devices op 全量拉取 + event.devices 快照替换(桌面同款
 * listenDevices 全量替换;stale 标志只在 op 结果携带)。init 即订阅后拉取
 * ——hub 的订阅闸保证事件不先于订阅到达。
 */
class DevicesViewModel(
    private val repo: MtdssRepository,
    private val hub: MtdssEventHub,
    private val scope: CoroutineScope,
) {
    data class UiState(
        val devices: List<DirectoryDevice> = emptyList(),
        val stale: Boolean = false,
        val loading: Boolean = true,
        val error: String? = null,
    )

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    init {
        scope.launch {
            hub.awaitSubscribed()
            hub.devices.collect { devs -> _state.value = _state.value.copy(devices = devs) }
        }
        refresh()
    }

    /** 全量拉取 devices(手动刷新/重连对账入口)。 */
    fun refresh() {
        scope.launch {
            hub.awaitSubscribed()
            _state.value = _state.value.copy(loading = true, error = null)
            repo.devices().fold(
                onSuccess = { r ->
                    _state.value = _state.value.copy(devices = r.devices, stale = r.stale, loading = false)
                },
                onFailure = { e ->
                    _state.value = _state.value.copy(loading = false, error = e.message)
                },
            )
        }
    }
}
