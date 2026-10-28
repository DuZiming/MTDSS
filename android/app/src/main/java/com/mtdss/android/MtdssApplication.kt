package com.mtdss.android

import android.app.Application
import com.mtdss.android.engine.MtdssEngine

/**
 * 应用入口。引擎生命周期与进程对齐(设计 §4.1「App 起则起,App 亡则
 * 亡」):onCreate 在主线程拉起 mtdssd——主线程是 PDEATHSIG 语义要求的
 * 唯一合法 fork 线程(MtdssEngine 类注释详述),Application.onCreate
 * 天然满足。App 退出(进程死亡)由引擎侧 PR_SET_PDEATHSIG 连带走杀,
 * 此处不设 onDestroy 关停。
 *
 * AppGraph 同点装配:IpcClient/事件 hub/收件箱发布器/会话 VM——
 * fs.publish / fs.query_dest 的订阅面必须是 app 级(对端收件任务的发布与
 * 查询请求不经本 App 界面到达),且订阅先于任何屏面动作(先订阅后动作的
 * 协议闸,见 MtdssEventHub)。
 */
class MtdssApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        MtdssEngine.start(this)
        AppGraph.init(this)
    }
}
