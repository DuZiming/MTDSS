package com.mtdss.android.ui.login

import com.mtdss.android.BuildConfig
import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirectoryDevice
import com.mtdss.android.ipc.LoginStep
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File

/**
 * 登录屏 VM(V3-1 四相位状态机,流程镜像桌面 GuideView):
 * - **Form**:服务器 + 邮箱 →「获取验证码」(dev 服务器回显码直接预填,同款
 *   便利)→ 验证码(+设备名,屏面预填 Build.MODEL)→「登录」;
 * - **Approval**:verify 落审批挂起 → 等待受信终端确认(弹窗或 6 位审批码
 *   直输),轮询 login.status 收口(ok → Done;denied/expired 终态回 Form
 *   文案;limit 转场踢选);
 * - **Limit**:verify(或轮询)落配额超额 → 踢选屏(移出所选设备腾位,
 *   本机即收口登录);
 * - **Done**:ok 收口(会话已落盘,daemon 广播 event.login → 会话对账),
 *   导航翻转以 SessionViewModel 的 status 为权威,本 VM 的 loggedIn 只是
 *   屏内反馈信号。
 *
 * 相位守卫:步进分派只自预期相位接受(verify 自 Form、submit/轮询自
 * Approval、evict 自 Limit)——取消后的迟到步进不拖回旧相位。扫码相位
 * (qr)不进 Android(扫码器随 V3-4 重构,邮箱路径完整可用)。
 */
class LoginViewModel(
    private val repo: MtdssRepository,
    private val hub: MtdssEventHub,
    private val scope: CoroutineScope,
    /**
     * 备案前演练钩子(spec §7 自签 dev CA;域名落地后此钩子与文件一并移除;
     * 文件由 adb 经 run-as 放入)。dev-ca.pem 候选路径({filesDir}/mtdss/
     * dev-ca.pem,由装配侧注入):仅 BuildConfig.DEBUG 且文件存在时作为
     * login.request_code 的 ca 传给 daemon(daemon 与 App 同 UID,读得见同
     * 路径);release 构建恒不注入,零行为变化。
     */
    private val devCaPath: String? = null,
    /** 审批轮询节拍(产线 3s;测试注入小值)。 */
    private val pollDelayMs: Long = 3_000,
) {
    /** 登录面相位(Form=邮箱表单;Approval=等待审批;Limit=踢选;Done=收口)。 */
    enum class LoginPhase { Form, Approval, Limit, Done }

    data class UiState(
        val phase: LoginPhase = LoginPhase.Form,
        // Form 面
        val server: String = "",
        val email: String = "",
        val name: String = "",
        val code: String = "",
        /** dev 服务器回显的验证码(已预填进 code;正式服务器为 null)。 */
        val devCode: String? = null,
        // Approval 面(等待受信终端确认;6 位审批码直输)
        val sessionId: String? = null,
        /** 审批会话到期秒(服务器 TTL;倒计时呈现用,不驱动终态)。 */
        val expiresAt: Long? = null,
        val approvalCode: String = "",
        // Limit 面(踢选凭据 + 服务器设备清单)
        val enrollToken: String? = null,
        val evictDevices: List<DirectoryDevice> = emptyList(),
        val busy: Boolean = false,
        /** 屏内错误(动作失败;成功路径清空)。 */
        val error: String? = null,
        /** 轮询终态文案(denied/expired;Form 顶部警示条,重发起时清)。 */
        val terminal: String? = null,
    ) {
        /** 收口 ok 的一次性屏内信号(导航权威态在 SessionViewModel)。 */
        val loggedIn: Boolean get() = phase == LoginPhase.Done
    }

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    /** 审批轮询协程(入 Approval 相位起拍,离相位即停)。 */
    private var pollJob: Job? = null

    fun setServer(v: String) = _state.setValue { it.copy(server = v) }

    fun setEmail(v: String) = _state.setValue { it.copy(email = v) }

    fun setName(v: String) = _state.setValue { it.copy(name = v) }

    fun setCode(v: String) = _state.setValue { it.copy(code = v) }

    fun setApprovalCode(v: String) = _state.setValue { it.copy(approvalCode = v) }

    /** login.request_code:空字段即本地校验失败(不触 RPC)。 */
    fun requestCode() {
        val s = _state.value
        if (s.server.isBlank() || s.email.isBlank()) {
            _state.value = s.copy(error = "请填写服务器地址与邮箱")
            return
        }
        scope.launch {
            _state.value = _state.value.copy(busy = true, error = null)
            // 演练钩子(见 devCaPath 注释):debug 构建且文件在,才注入 dev CA
            val ca = if (BuildConfig.DEBUG) devCaPath?.takeIf { File(it).isFile } else null
            repo.requestCode(s.server.trim(), s.email.trim(), ca).fold(
                onSuccess = { r ->
                    // dev 服务器回显验证码:预填(桌面同款便利);正式服务器
                    // 返回的是空码或回执,code 留给用户手填
                    _state.value = _state.value.copy(busy = false, devCode = r.code, code = r.code)
                },
                onFailure = { e ->
                    _state.value = _state.value.copy(busy = false, error = "请求验证码失败:${e.message}")
                },
            )
        }
    }

    /** login.verify(event.login 由本动作触发:先订阅后动作)→ 步进分派。 */
    fun verify() {
        val s = _state.value
        if (s.phase != LoginPhase.Form) return
        if (s.code.isBlank()) {
            _state.value = s.copy(error = "请填写验证码")
            return
        }
        scope.launch {
            _state.value = _state.value.copy(busy = true, error = null, terminal = null)
            hub.awaitSubscribed()
            repo.verify(s.code.trim(), s.email.trim(), s.name.trim()).fold(
                onSuccess = { step -> applyStep(step, LoginPhase.Form) },
                onFailure = { e ->
                    _state.value = _state.value.copy(busy = false, error = "验证失败:${e.message}")
                },
            )
        }
    }

    /** login.submit_code(受信终端弹窗上的 6 位审批码直输)。 */
    fun submitApprovalCode() {
        val s = _state.value
        if (s.phase != LoginPhase.Approval) return
        val sessionId = s.sessionId ?: return
        if (s.approvalCode.isBlank()) {
            _state.value = s.copy(error = "请填写审批码")
            return
        }
        scope.launch {
            // 取消竞态守卫:launch 排队期间相位已离 Approval 则整段丢弃
            if (_state.value.phase != LoginPhase.Approval) return@launch
            _state.value = _state.value.copy(busy = true, error = null)
            repo.submitCode(sessionId, s.approvalCode.trim()).fold(
                onSuccess = { step -> applyStep(step, LoginPhase.Approval) },
                onFailure = { e ->
                    if (_state.value.phase == LoginPhase.Approval) {
                        _state.value = _state.value.copy(busy = false, error = "提交审批码失败:${e.message}")
                    }
                },
            )
        }
    }

    /** login.evict(踢选屏动作:移出所选设备腾位,本机即收口登录)。 */
    fun evict(deviceId: String) {
        val s = _state.value
        if (s.phase != LoginPhase.Limit) return
        val token = s.enrollToken ?: return
        scope.launch {
            if (_state.value.phase != LoginPhase.Limit) return@launch // 同上:取消竞态守卫
            _state.value = _state.value.copy(busy = true, error = null)
            repo.evict(token, deviceId).fold(
                onSuccess = { step -> applyStep(step, LoginPhase.Limit) },
                onFailure = { e ->
                    if (_state.value.phase == LoginPhase.Limit) {
                        _state.value = _state.value.copy(busy = false, error = "移出失败:${e.message}")
                    }
                },
            )
        }
    }

    /**
     * 取消(任一中间态 → Form):本地先回退(相位清场),login.cancel 随后
     * 作废 daemon 续接槽;服务端会话无取消端点,TTL 自收口,失败不阻回退。
     */
    fun cancel() {
        if (_state.value.phase == LoginPhase.Form) return
        stopPolling()
        _state.value = _state.value.copy(
            phase = LoginPhase.Form,
            sessionId = null,
            expiresAt = null,
            approvalCode = "",
            enrollToken = null,
            evictDevices = emptyList(),
            busy = false,
            error = null,
            terminal = null,
        )
        scope.launch { repo.cancelLogin() }
    }

    /**
     * 步进分派收口(ok → Done;approval → 等待面+轮询;limit → 踢选面;
     * denied/expired → 终态回 Form 文案)。仅自 [expected] 相位接受——
     * 取消后的迟到步进(轮询在飞)原样丢弃。
     */
    private fun applyStep(step: LoginStep, expected: LoginPhase) {
        if (_state.value.phase != expected) return
        when (step) {
            is LoginStep.Ok -> {
                stopPolling()
                _state.value = _state.value.copy(
                    busy = false,
                    phase = LoginPhase.Done,
                    error = null,
                    sessionId = null,
                    approvalCode = "",
                    enrollToken = null,
                    evictDevices = emptyList(),
                )
            }
            is LoginStep.Approval -> {
                _state.value = _state.value.copy(
                    busy = false,
                    phase = LoginPhase.Approval,
                    sessionId = step.sessionId,
                    expiresAt = step.expiresAt,
                    error = null,
                )
                startPolling(step.sessionId)
            }
            is LoginStep.Limit -> {
                // verify 直达或轮询/审批转场都落此(evict 再撞 limit 同此换新)
                stopPolling()
                _state.value = _state.value.copy(
                    busy = false,
                    phase = LoginPhase.Limit,
                    enrollToken = step.enrollToken,
                    evictDevices = step.devices,
                    error = null,
                )
            }
            is LoginStep.Pending -> Unit // 审批面无 pending 形态(qr 专用,V3-4);保守不动相位
            LoginStep.Denied -> toTerminal("登录请求已被拒绝")
            LoginStep.Expired -> toTerminal("登录已超时,请重新发起")
        }
    }

    /** 终态(denied/expired):回 Form 顶部警示文案,中间态凭据清场。 */
    private fun toTerminal(message: String) {
        stopPolling()
        _state.value = _state.value.copy(
            busy = false,
            phase = LoginPhase.Form,
            terminal = message,
            sessionId = null,
            expiresAt = null,
            approvalCode = "",
            enrollToken = null,
            evictDevices = emptyList(),
        )
    }

    /**
     * 审批轮询(服务端 push 不达新终端,收口靠本面):每拍 [pollDelayMs]
     * 一次 login.status,步进自 Approval 相位分派;单拍失败(瞬态网络/引擎
     * 重启)不断轮询,终态由服务端 TTL 收口。相位离 Approval 即停。
     */
    private fun startPolling(sessionId: String) {
        stopPolling()
        pollJob = scope.launch {
            while (isActive) {
                delay(pollDelayMs)
                repo.loginStatus(sessionId).getOrNull()?.let { applyStep(it, LoginPhase.Approval) }
            }
        }
    }

    private fun stopPolling() {
        pollJob?.cancel()
        pollJob = null
    }

    private inline fun MutableStateFlow<UiState>.setValue(update: (UiState) -> UiState) {
        value = update(value)
    }
}
