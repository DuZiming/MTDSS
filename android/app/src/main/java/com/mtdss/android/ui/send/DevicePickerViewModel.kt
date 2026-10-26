package com.mtdss.android.ui.send

import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirectoryDevice
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 目标设备选择弹层的 VM(devices op 全量拉取 + event.devices 快照替换;
 * 语义镜像 DevicesViewModel——弹层生命周期独立,不复用屏面 VM)。
 */
class DevicePickerViewModel(
    private val repo: MtdssRepository,
    hub: MtdssEventHub,
    private val scope: CoroutineScope,
) {
    data class UiState(
        val devices: List<DirectoryDevice> = emptyList(),
        val loading: Boolean = true,
        val error: String? = null,
    )

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    init {
        scope.launch {
            hub.awaitSubscribed()
            hub.devices.collect { devs ->
                _state.value = _state.value.copy(devices = devs, loading = false)
            }
        }
        refresh()
    }

    private fun refresh() {
        scope.launch {
            repo.devices().fold(
                onSuccess = { r -> _state.value = _state.value.copy(devices = r.devices, loading = false) },
                onFailure = { e ->
                    _state.value = _state.value.copy(loading = false, error = e.message)
                },
            )
        }
    }
}
