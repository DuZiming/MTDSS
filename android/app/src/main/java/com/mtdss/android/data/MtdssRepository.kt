package com.mtdss.android.data

import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.ipc.ConnState
import com.mtdss.android.ipc.DevicesResult
import com.mtdss.android.ipc.HelloResult
import com.mtdss.android.ipc.IpcClient
import com.mtdss.android.ipc.ListResult
import com.mtdss.android.ipc.LoginRequestCodeResult
import com.mtdss.android.ipc.LoginVerifyResult
import com.mtdss.android.ipc.StatResult
import com.mtdss.android.ipc.StatusResult
import com.mtdss.android.ipc.TransferStartResult
import com.mtdss.android.send.BridgeLimits
import kotlinx.coroutines.flow.StateFlow

/**
 * 屏面数据边界(spec §4.3「UI 不持 IpcClient」):类型化调用 + 收件箱下载
 * 语义收口。VM/屏只经本面触 daemon——测试缝 = IpcClient 的 SocketFactory
 * (FakeDaemon),仓库本身零逻辑、零状态,不另设接口。
 */
class MtdssRepository(private val ipc: IpcClient) {

    /** IPC 连接态(IpcClient 自愈重连的可见面,屏面直读)。 */
    val connectionState: StateFlow<ConnState> get() = ipc.connectionState

    suspend fun connect(): Result<Unit> = ipc.connect()

    suspend fun hello(): Result<HelloResult> = ipc.hello()

    suspend fun status(): Result<StatusResult> = ipc.status()

    suspend fun devices(): Result<DevicesResult> = ipc.devices()

    suspend fun requestCode(server: String, email: String, ca: String? = null): Result<LoginRequestCodeResult> =
        ipc.loginRequestCode(server, email, ca)

    suspend fun verify(code: String, email: String, name: String): Result<LoginVerifyResult> =
        ipc.loginVerify(code, email, name)

    suspend fun logout(): Result<Unit> = ipc.loginLogout()

    suspend fun list(device: String, path: String): Result<ListResult> = ipc.list(device, path)

    suspend fun stat(device: String, path: String): Result<StatResult> = ipc.stat(device, path)

    /**
     * 下载到收件箱:dir 恒为哨兵 [INBOX_DIR](spec §4.2 评审裁定②:android
     * cfg 下 daemon 路由 BridgeDestIo);cut 恒 false(裁定④:fd 桥无删除
     * 原语,UI 不给选项)。skip/overwrite 语义与桌面 conflict 面同源
     * (顶层名或目录前缀命中)。
     */
    suspend fun downloadToInbox(
        device: String,
        paths: List<String>,
        skip: List<String> = emptyList(),
        overwrite: List<String> = emptyList(),
    ): Result<TransferStartResult> =
        ipc.transferStartDownload(device, paths, INBOX_DIR, skip, overwrite, cut = false)

    /**
     * 发送到对端(fd 桥上传,T10):items 元数据先行,daemon 驱动起步逐文件
     * fs.request_source 懒开 fd(SourceProvider 应答)。dir = **对端目标目录**
     * (PC 对端 = 绝对路径,UI 层给输入 + 记忆);cut 恒 false(裁定④)。
     * skip/overwrite:默认双空 = 跳过对端已收完 + 断点续传;overwrite=true
     * 时名单 = 全部条目 rel(整批覆盖重发)。
     *
     * 条目数护栏([BridgeLimits.MAX_BRIDGE_ITEMS])在此前置镜像:超限即
     * 友好错误(词面与 daemon 一致),不再发 RPC;daemon 侧同词面兜底。
     */
    suspend fun startUpload(
        items: List<BridgeItem>,
        to: String,
        dir: String,
        skip: List<String> = emptyList(),
        overwrite: List<String> = emptyList(),
    ): Result<TransferStartResult> {
        BridgeLimits.itemCountError(items.size)?.let { return Result.failure(IllegalStateException(it)) }
        return ipc.transferStartUploadBridge(items, to, dir, skip, overwrite, cut = false)
    }

    companion object {
        /** 收件箱哨兵(与 daemon 侧 intake::INBOX_SENTINEL 同值)。 */
        const val INBOX_DIR: String = "inbox"
    }
}
