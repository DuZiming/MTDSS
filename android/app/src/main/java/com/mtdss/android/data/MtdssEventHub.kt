package com.mtdss.android.data

import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.EventFrame
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.MtdssEvent
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.onSubscription
import kotlinx.coroutines.launch

/**
 * 事件分接器(app 级单订阅):ipc.events(no-replay SharedFlow)→ 类型化
 * 状态/流。**先订阅后动作**的协议闸在此收口——hub 构造即订阅,
 * [awaitSubscribed] 让 VM 的动作调用方等待订阅落位(verify 触发
 * event.login、devices 拉取伴随 event.devices 等,均不得早于订阅)。
 *
 * 各面的呈现口径(镜像桌面 App.tsx 的 listen* 处理):
 * - login/selfOnline:粘滞单字段([StateFlow],初值 null = 未知);
 * - devices:最近一次快照(replay=1:晚到的订阅方也能拿到最近列表);
 * - notify:toast 事件(短缓冲,丢新不阻塞);
 * - transfer(T11 消费面)与 fs.publish / fs.query_dest(InboxProvider 直订)
 *   不经本 hub——各自生命周期与闸门不同。
 */
class MtdssEventHub(ipc: IpcClient, scope: CoroutineScope) {

    private val gate = CompletableDeferred<Unit>()

    private val _login = MutableStateFlow<Boolean?>(null)

    /** 登录态事件(event.login;仅事件面,权威态以 status 拉取为准)。 */
    val login: StateFlow<Boolean?> = _login.asStateFlow()

    private val _selfOnline = MutableStateFlow<Boolean?>(null)

    /** 本机信令在线(event.online;桌面同款「只翻单字段」)。 */
    val selfOnline: StateFlow<Boolean?> = _selfOnline.asStateFlow()

    private val _devices = MutableSharedFlow<List<DirectoryDevice>>(replay = 1, extraBufferCapacity = 16)

    /** 设备目录快照事件(event.devices;replay=1 保最近一次)。 */
    val devices: SharedFlow<List<DirectoryDevice>> = _devices

    private val _notify = MutableSharedFlow<MtdssEvent.Notify>(extraBufferCapacity = 16)

    /** 接收完成等 toast 通知(event.notify)。 */
    val notify: SharedFlow<MtdssEvent.Notify> = _notify

    init {
        scope.launch {
            // onSubscription 块在订阅落位后、首值发射前执行:gate 完成即
            // 证明本订阅者已注册(SharedFlow 契约),此后动作触发的发射不丢
            ipc.events
                .onSubscription { gate.complete(Unit) }
                .collect { frame -> route(frame) }
        }
    }

    /** 订阅落位闸(幂等;动作前 await,防 no-replay 事件流丢帧)。 */
    suspend fun awaitSubscribed() = gate.await()

    private fun route(frame: EventFrame) {
        when (val ev = MtdssEvent.parse(frame)) {
            is MtdssEvent.Login -> _login.value = ev.loggedIn
            is MtdssEvent.Online -> _selfOnline.value = ev.online
            is MtdssEvent.Devices -> _devices.tryEmit(ev.devices)
            is MtdssEvent.Notify -> _notify.tryEmit(ev)
            is MtdssEvent.Transfer -> Unit // T11 任务面消费
            null -> Unit // 未知事件:跳过保持前向兼容(fs.publish/fs.query_dest 由 InboxProvider 直订)
        }
    }
}
