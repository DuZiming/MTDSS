package com.mtdss.android.ui.profile

import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.QrClaimResult
import com.mtdss.android.ipc.RpcException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withTimeout

/**
 * QrApprovalModel — 扫码授权闭环状态机(V3-4 T12;iOS Kit QrApprovalModel
 * 语义镜像,App 侧 ProfileView 接弹层、ScanSheet 只管取景出 token):
 * - **claim**:扫得 token → qr.claim(daemon 持会话 Bearer 代打)→ 落
 *   ``Stage/Confirming``(候选面拍平:candidate_name/candidate_platform +
 *   expires_at,确认弹层展示);二维码已失效(typed not_found)/他端已认领
 *   (contested)/其余错 → ``Stage/Failed``(daemon 消息本就面向用户);
 *   **失败留 failed,可再 claim**(重扫原入口即重试,相位不拦);
 * - **approve**:同意/拒绝 → qr.approve(allow)→ 结果行文案落
 *   ``Stage/Result``:ok=已同意扫码登录 / denied=已拒绝该设备登录 /
 *   expired=二维码已过期,请重新扫码 / limit=对方需选择移除设备后完成登录
 *   (limit 为契约全集防御面:配额超额收口在 PC 侧轮询,本 op 恒不投影);
 *   RPC 失败 → ``Stage/Failed``;
 * - 相位守卫与 iOS 同象:approve 仅自 Confirming 接受(取消/关闭后的迟到
 *   裁决不外发); ``dismiss`` 清场回 [Stage.Idle]。
 *
 * 所有 RPC 单次限时 [callTimeoutMs](15s:daemon 侧 REST 15s 超时同档;测试
 * 注入小值)。动作为挂起函数,调用方(UI 记协程域/测试直驱)launch 驱动。
 */
class QrApprovalModel(
    private val repo: MtdssRepository,
    private val callTimeoutMs: Long = 15_000,
) {

    /**
     * 相位态:[Idle] = 初始(扫码中)/[Claiming] = qr.claim 在途/
     * [Confirming] = 候选确认(同意/拒绝)/[Result] = 裁决结果行/
     * [Failed] = 错误文案。
     */
    sealed interface Stage {
        data object Idle : Stage
        data object Claiming : Stage
        data class Confirming(val candidate: QrClaimResult) : Stage
        data class Result(val line: String) : Stage
        data class Failed(val text: String) : Stage
    }

    /** 单次 RPC 限时哨兵(内部;用户文案见 [userErrorText])。 */
    private class CallTimeout : Exception()

    private val _stage = MutableStateFlow<Stage>(Stage.Idle)

    /** 相位态(UI collectAsState 弹层数据源)。 */
    val stage: StateFlow<Stage> = _stage.asStateFlow()

    private val _busy = MutableStateFlow(false)

    /** RPC 在途(iOS busy 同名面;弹层按钮置灰/断言用)。 */
    val busy: StateFlow<Boolean> = _busy.asStateFlow()

    private val _token = MutableStateFlow<String?>(null)

    /** 确认中的 token(approve 随路携带;claim 成功即上帧)。 */
    val token: StateFlow<String?> = _token.asStateFlow()

    // ── 动作 ───────────────────────────────────────────────────────────────

    /** qr.claim(扫得 token 后的认领):成功落 Confirming;失败落 Failed。 */
    suspend fun claim(token: String) {
        _busy.value = true
        try {
            val candidate = try {
                rpc { repo.qrClaim(token) }.getOrThrow()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _stage.value = Stage.Failed(userErrorText(e))
                return
            }
            _token.value = token
            _stage.value = Stage.Confirming(candidate)
        } finally {
            _busy.value = false
        }
    }

    /** qr.approve(同意/拒绝):仅自 Confirming 接受;结果行文案落 Result,RPC 失败落 Failed。 */
    suspend fun approve(allow: Boolean) {
        val t = _token.value
        if (_stage.value !is Stage.Confirming || t == null) return
        _busy.value = true
        try {
            val result = try {
                rpc { repo.qrApprove(t, allow) }.getOrThrow()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                _stage.value = Stage.Failed(userErrorText(e))
                return
            }
            _stage.value = Stage.Result(resultLine(result.status))
        } finally {
            _busy.value = false
        }
    }

    /**
     * 关闭(弹层 好/取消):清场回初始态(不外发任何 RPC;未裁决的二维码
     * 由服务端 TTL 自收口)。
     */
    fun dismiss() {
        _stage.value = Stage.Idle
        _token.value = null
        _busy.value = false
    }

    // ── 内部 ───────────────────────────────────────────────────────────────

    /** 单次 RPC 限时包装:超时转 [CallTimeout](用户文案「请求超时」)。 */
    private suspend fun <T> rpc(call: suspend () -> T): T = try {
        withTimeout(callTimeoutMs) { call() }
    } catch (e: TimeoutCancellationException) {
        throw CallTimeout()
    }

    companion object {

        /** 结果行文案(status → 中文;UI 弹层 message 直用)。 */
        fun resultLine(status: String): String = when (status) {
            "ok" -> "已同意扫码登录"
            "denied" -> "已拒绝该设备登录"
            "expired" -> "二维码已过期,请重新扫码"
            "limit" -> "对方需选择移除设备后完成登录"
            else -> "二维码状态异常"
        }

        /** 错误用户文案(daemon 消息本就面向用户;传输/超时给常识文案)。 */
        fun userErrorText(e: Throwable): String = when (e) {
            is RpcException -> e.message ?: "请求失败:连接中断"
            // CallTimeout = rpc 限时哨兵;TimeoutCancellationException = 直入
            // 的裸超时(两者同词面)
            is CallTimeout -> "请求超时"
            is TimeoutCancellationException -> "请求超时"
            else -> "请求失败:连接中断"
        }
    }
}
