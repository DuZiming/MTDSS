package com.mtdss.android

import android.content.Context
import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.engine.MtdssEngine
import com.mtdss.android.fdpass.FdPassChannel
import com.mtdss.android.inbox.InboxProvider
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ui.SessionViewModel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.io.File

/**
 * 应用级装配(单例;MtdssApplication.onCreate 初始化):IpcClient + 事件
 * hub + 仓库 + fdpass 通道 + 收件箱 Provider + 会话 VM,一次构造、进程
 * 生命周期共享。
 *
 * **构造序即协议序**:hub 与 InboxProvider 在此订阅事件流(先于任何屏面
 * 动作——对端也可能不经本 App 界面直接 upload 到我们的收件箱,
 * fs.request_dest 的订阅面必须是 app 级);SessionViewModel 随后发起首连
 * (fast-fail,IpcClient 自愈覆盖引擎冷启动间隙)。
 */
object AppGraph {

    @Volatile
    private var instance: Graph? = null

    fun init(context: Context): Graph =
        instance ?: synchronized(this) {
            instance ?: Graph(context).also { instance = it }
        }

    fun get(): Graph = instance ?: error("AppGraph 未初始化(MtdssApplication.onCreate 应先调用 init)")
}

/** 装配产物:屏面只经本对象取依赖,不直接构造。 */
class Graph(context: Context) {

    /** app 级协程域(与进程对齐;屏面级 VM 用各自 remember 域,见屏面)。 */
    val appScope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val ipc: IpcClient = IpcClient(MtdssEngine.daemonSocketFile(context), appScope)

    val hub: MtdssEventHub = MtdssEventHub(ipc, appScope)

    val repo: MtdssRepository = MtdssRepository(ipc)

    /** fdpass 通道:fd 旁路,socket = {filesDir}/mtdss/fds.sock(serve_fdpass 监听位)。 */
    val fdpass: FdPassChannel = FdPassChannel(File(MtdssEngine.daemonHome(context), "fds.sock"), appScope)

    /** 收件箱应答编排(构造即订阅 fs.request_dest)。 */
    val inbox: InboxProvider = InboxProvider(context, ipc, fdpass, appScope)

    /** 会话状态(app 级 VM,AppNav 路由依据;init 发起首连)。 */
    val session: SessionViewModel = SessionViewModel(repo, hub, appScope)
}
