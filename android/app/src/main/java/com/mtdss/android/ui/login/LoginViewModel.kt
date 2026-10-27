package com.mtdss.android.ui.login

import com.mtdss.android.BuildConfig
import com.mtdss.android.data.MtdssEventHub
import com.mtdss.android.data.MtdssRepository
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.io.File

/**
 * 登录屏 VM(spec §4.3;流程镜像桌面 GuideView):服务器 + 邮箱 →「获取
 * 验证码」(dev 服务器回显码直接预填,同款便利)→ 验证码(+设备名,屏面
 * 预填 Build.MODEL)→「登录」。verify 成功的导航翻转以 SessionViewModel
 * 的 status 为权威(event.login → 全量刷新);本 VM 的 loggedIn 只是屏内
 * 反馈信号。
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
) {
    data class UiState(
        val server: String = "",
        val email: String = "",
        val name: String = "",
        val code: String = "",
        /** dev 服务器回显的验证码(已预填进 code;正式服务器为 null)。 */
        val devCode: String? = null,
        val busy: Boolean = false,
        val error: String? = null,
        /** verify 成功的一次性屏内信号(导航权威态在 SessionViewModel)。 */
        val loggedIn: Boolean = false,
    )

    private val _state = MutableStateFlow(UiState())

    val state: StateFlow<UiState> = _state.asStateFlow()

    fun setServer(v: String) = _state.setValue { it.copy(server = v) }

    fun setEmail(v: String) = _state.setValue { it.copy(email = v) }

    fun setName(v: String) = _state.setValue { it.copy(name = v) }

    fun setCode(v: String) = _state.setValue { it.copy(code = v) }

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

    /** login.verify(event.login 由本动作触发:先订阅后动作)。 */
    fun verify() {
        val s = _state.value
        if (s.code.isBlank()) {
            _state.value = s.copy(error = "请填写验证码")
            return
        }
        scope.launch {
            _state.value = _state.value.copy(busy = true, error = null)
            hub.awaitSubscribed()
            repo.verify(s.code.trim(), s.email.trim(), s.name.trim()).fold(
                onSuccess = {
                    _state.value = _state.value.copy(busy = false, loggedIn = true)
                },
                onFailure = { e ->
                    _state.value = _state.value.copy(busy = false, error = "验证失败:${e.message}")
                },
            )
        }
    }

    private inline fun MutableStateFlow<UiState>.setValue(update: (UiState) -> UiState) {
        value = update(value)
    }
}
