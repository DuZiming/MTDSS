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
 *
 * 行序 = **在线在前稳定投影**([onlineFirst],R12 ① iOS DeviceListOrder.
 * onlineFirst 镜像;T7 收口 T6 deferred minor ②):在线设备在前、组内保序
 * (daemon 目录投影序,客户端不二次排序免翻)、离线沉底——iOS 选设备弹层
 * onlineDevices + offlineDevices 同语义。设备页(DevicesViewModel)不受此面
 * ——安卓 spec §3-3 未列在线沉底,设备页保持 daemon created_at 定序既有。
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
                _state.value = _state.value.copy(devices = onlineFirst(devs), loading = false)
            }
        }
        refresh()
    }

    private fun refresh() {
        scope.launch {
            repo.devices().fold(
                onSuccess = { r -> _state.value = _state.value.copy(devices = onlineFirst(r.devices), loading = false) },
                onFailure = { e ->
                    _state.value = _state.value.copy(loading = false, error = e.message)
                },
            )
        }
    }
}

/**
 * 设备列表在线在前稳定投影(iOS MTDSSKit DeviceListOrder.onlineFirst 逐字
 * 镜像):在线设备在前,**组内保序**(Kotlin sortedWith 稳定排序保证;
 * daemon 目录投影即 name/platform 语义顺序),离线沉底(`online`
 * Bool:false < true)。消费面 = 选设备弹层(DevicePickerSheet 设备行)。
 */
internal fun onlineFirst(devices: List<DirectoryDevice>): List<DirectoryDevice> =
    devices.sortedBy { !it.online }
