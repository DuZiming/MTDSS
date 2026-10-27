package com.mtdss.android.ui.send

import android.content.Context
import android.content.SharedPreferences
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.BridgeItem
import com.mtdss.android.send.BridgeLimits
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * 发送流程控制器(各发送入口的统一编排:条目数护栏 → transfer.start
 * bridge 形态 → 反馈)。UI 侧一次动作的小状态机,不做轮询/订阅(任务
 * 进度是 T11 任务页的面)。
 */
class SendController(
    private val repo: MtdssRepository,
    private val scope: CoroutineScope,
    private val memory: DirMemory? = null,
) {
    data class State(
        val sending: Boolean = false,
        /** 发送已开始(job_id;一次性反馈,消费后清)。 */
        val sentJobId: String? = null,
        val error: String? = null,
    )

    private val _state = MutableStateFlow(State())

    val state: StateFlow<State> = _state.asStateFlow()

    /** 最近一次成功发送的对端目录("" = 无记忆)。 */
    val lastDir: String get() = memory?.last ?: ""

    /** 发送条目(护栏前置:超限错误直达 State.error,不发出 RPC)。 */
    fun send(items: List<BridgeItem>, to: String, dir: String, overwrite: Boolean) {
        if (_state.value.sending) return
        BridgeLimits.itemCountError(items.size)?.let {
            _state.value = State(error = it)
            return
        }
        _state.value = State(sending = true)
        scope.launch {
            repo.startUpload(
                items = items,
                to = to,
                dir = dir,
                skip = emptyList(),
                overwrite = if (overwrite) items.map { it.rel } else emptyList(),
            ).fold(
                onSuccess = { r ->
                    memory?.remember(dir)
                    _state.value = State(sentJobId = r.jobId)
                },
                onFailure = { e ->
                    _state.value = State(error = e.message ?: e.javaClass.simpleName)
                },
            )
        }
    }

    /** 清一次性反馈。 */
    fun clearFeedback() {
        _state.value = State()
    }

    /** dir 记忆(最近一次发送成功的对端目录;PC 对端的绝对路径输入免每次重敲)。 */
    interface DirMemory {
        val last: String
        fun remember(dir: String)
    }

    companion object {
        private const val PREFS_NAME = "mtdss_send"
        private const val KEY_LAST_DIR = "last_dir"

        /** 屏面入口:dir 记忆 = 应用级 SharedPreferences。 */
        fun create(repo: MtdssRepository, scope: CoroutineScope, context: Context): SendController =
            SendController(repo, scope, prefsMemory(context))

        private fun prefsMemory(context: Context): DirMemory {
            val prefs: SharedPreferences =
                context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            return object : DirMemory {
                override val last: String get() = prefs.getString(KEY_LAST_DIR, "") ?: ""
                override fun remember(dir: String) {
                    prefs.edit().putString(KEY_LAST_DIR, dir).apply()
                }
            }
        }
    }
}
