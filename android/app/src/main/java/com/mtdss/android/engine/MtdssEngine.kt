package com.mtdss.android.engine

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.io.File
import java.io.IOException
import kotlin.concurrent.thread

/**
 * MTDSS 引擎(mtdssd 子进程)生命周期管理器(设计 §4.1 进程模型)。
 *
 * 职责单一:按「App 起则起,App 亡则亡」拉起/看护引擎进程;IPC 客户端
 * (T8)与 UI 不在此层。关键决策与依据:
 *
 * ## 拉起必须在主线程(PDEATHSIG 父线程语义)
 * 引擎侧 android cfg 一启动即 `prctl(PR_SET_PDEATHSIG, SIGKILL)`:
 * 「创建子进程的那个父线程」消亡,内核即 SIGKILL 引擎(见
 * crates/daemon/src/main.rs 的 set_parent_death_signal)。因此每次 fork
 * (含重启)都必须发生在与 App 进程同寿命的线程 = 主线程。本类所有
 * spawn 走 [mainHandler] 提交回主线程执行;若误由工作线程 fork,该线程
 * 回收时引擎被误杀。Android 主线程在进程存活期间永不退出,语义恰好
 * 收敛为「进程亡 → 引擎亡」。(引擎 pid 不在此层取:logcat 每行自带
 * 引擎进程的 pid 列,且 java.lang.Process#pid() 系 API 34+ 方法,
 * minSdk 29 不可用。)
 *
 * ## 环境与路径
 * - 二进制:APK 以 jniLibs 约定打包 `libmtdssd.so`(app/build.gradle.kts
 *   的 cargoDaemon* 任务生成,useLegacyPackaging 安装期解包),运行时位于
 *   `applicationInfo.nativeLibraryDir/libmtdssd.so`——exec 需真实文件,
 *   这是必须解包而非 APK 内 mmap 的原因。
 * - `MTDSS_HOME={filesDir}/mtdss`(引擎侧 mtdss_ipc::mtdss_home 读该环境
 *   变量;daemon 无 CLI home 旗标,环境变量是唯一注入面)。
 * - daemon 套接字 = `{home}/daemon.sock`(mtdss_ipc::socket_path 的 unix
 *   分支:{home}/daemon.sock),见 [daemonSocketFile]。
 * - 不传任何 CLI 参数:android 下 --log-file 恒被覆写为 stderr(引擎侧
 *   cfg),recv_dir 走 settings.json/缺省值,均无需 Kotlin 干预。
 *
 * ## 日志面
 * 引擎在 android 下日志恒走 stderr;[redirectErrorStream] 合并
 * stdout/stderr 后逐行转 logcat(tag [ENGINE_LOG_TAG]),与 PC 侧
 * 「stderr→控制台」同源。
 *
 * ## 死亡与重启
 * App 存活期间引擎意外退出 → 指数退避重启(500ms 起、倍增、上限 15s;
 * 存活满 [STABLE_UPTIME_MS] 视为稳定,退出后计数归零从头退避)。重启后
 * 任务存储自 tasks.json 恢复,running 任务翻「中断」(P2 语义,引擎侧
 * 自愈,Kotlin 不参与)。引擎被 SIGKILL 后残留的 daemon.lock 由引擎启动
 * 侧陈旧锁观察窗(2s)自清除,重启无死角。
 *
 * ## stop() 的定位
 * 进程级死亡由 PDEATHSIG 兜底(App 亡引擎即亡,含被系统回收/用户划走)。
 * [stop] 是显式关停面(供后续设置页/调试用),并非 Activity 生命周期钩:
 * 不在 Activity.onDestroy 调用——旋转/配置变更也会走 onDestroy,那会把
 * 还活着的 App 的引擎误杀。正常退出路径 = 进程死亡 = PDEATHSIG。
 */
object MtdssEngine {

    private const val TAG = "MtdssEngine"

    /** 引擎日志转 logcat 的 tag(与进程名对齐,便于 logcat -s mtdssd)。 */
    const val ENGINE_LOG_TAG = "mtdssd"

    /** 引擎二进制在 nativeLibraryDir 下的文件名(jniLibs 约定)。 */
    private const val ENGINE_LIB_NAME = "libmtdssd.so"

    /** 退避基数与上限:500ms、1s、2s、4s、8s、15s(封顶)。 */
    private const val BACKOFF_BASE_MS = 500L
    private const val BACKOFF_CAP_MS = 15_000L

    /** 存活满此时长视为稳定,下次退出后退避计数归零。 */
    private const val STABLE_UPTIME_MS = 60_000L

    /** stop() 给引擎的体面收尾窗(SIGTERM 后等这么久,再 destroyForcibly)。 */
    private const val STOP_GRACE_MS = 2_000L

    /** 引擎阶段;[status] 携带累计重启计数与上次退出原因,调试屏直读。 */
    enum class Phase { RUNNING, BACKOFF, STOPPED }

    data class EngineStatus(
        val phase: Phase,
        val restartCount: Int,
        val lastExit: String?,
    )

    private val mainHandler = Handler(Looper.getMainLooper())

    @Volatile
    private var appContext: Context? = null

    @Volatile
    private var current: Process? = null

    @Volatile
    private var stopping = false

    @Volatile
    private var startedAt = 0L

    private val _status = MutableStateFlow(EngineStatus(Phase.STOPPED, 0, null))

    /** 引擎状态面(UI/调试观察用;只读)。 */
    val status: StateFlow<EngineStatus> = _status.asStateFlow()

    /**
     * 拉起引擎(幂等:已在运行即空操作)。须在主线程调用——见类注释
     * 「PDEATHSIG 父线程语义」;Application.onCreate 天然满足。
     */
    fun start(context: Context) {
        check(Looper.myLooper() == Looper.getMainLooper()) {
            "MtdssEngine.start 须在主线程调用(PDEATHSIG 父线程语义,见类注释)"
        }
        if (current?.isAlive == true) return
        appContext = context.applicationContext
        stopping = false
        spawnAndWatch(consecutiveDeaths = 0)
    }

    /**
     * 显式关停引擎。kill(SIGTERM)→ 体面窗 → destroyForcibly。引擎被硬杀
     * 残留的 daemon.lock/daemon.sock 由下次启动自愈(陈旧锁观察窗 2s;
     * serve 先清残留 socket 文件)。注:不接 Activity.onDestroy,理由见
     * 类注释「stop() 的定位」。
     */
    fun stop() {
        stopping = true
        mainHandler.removeCallbacksAndMessages(null)
        val p = current
        current = null
        if (p != null) {
            p.destroy()
            if (!waitFor(p, STOP_GRACE_MS)) {
                p.destroyForcibly()
            }
        }
        _status.value = EngineStatus(Phase.STOPPED, 0, "stop()")
    }

    /** 引擎数据目录:{filesDir}/mtdss(MTDSS_HOME 注入值)。 */
    fun daemonHome(context: Context): File = File(context.filesDir, "mtdss")

    /**
     * daemon 套接字路径:{MTDSS_HOME}/daemon.sock。与引擎侧
     * mtdss_ipc::socket_path 的 unix 分支同口径(home/daemon.sock);
     * 客户端以 LocalSocket(Namespace.FILESYSTEM) 连接(T8 的 IpcClient
     * 沿用同一派生)。
     */
    fun daemonSocketFile(context: Context): File = File(daemonHome(context), "daemon.sock")

    // ── 内部:拉起 + 看护(以下均在主线程,除非另注) ─────────────────────

    /** 拉起一次并挂看护线程;[consecutiveDeaths] 为此前连续死亡次数(退避用)。 */
    private fun spawnAndWatch(consecutiveDeaths: Int) {
        val ctx = appContext ?: return
        val home = daemonHome(ctx)
        val bin = File(ctx.applicationInfo.nativeLibraryDir, ENGINE_LIB_NAME)
        val p: Process = try {
            home.mkdirs()
            ProcessBuilder(bin.absolutePath)
                .redirectErrorStream(true) // stdout/stderr 合流,统一转发面
                .apply { environment()["MTDSS_HOME"] = home.absolutePath }
                .start()
        } catch (e: IOException) {
            Log.e(TAG, "引擎拉起失败(退避重试): ${bin.absolutePath}", e)
            _status.value = EngineStatus(
                Phase.BACKOFF, consecutiveDeaths + 1, "spawn-failed: ${e.message}",
            )
            scheduleRespawn(consecutiveDeaths + 1)
            return
        }
        current = p
        startedAt = SystemClock.elapsedRealtime()
        Log.i(TAG, "引擎已拉起 home=${home.absolutePath} bin=${bin.absolutePath}")
        _status.value = EngineStatus(Phase.RUNNING, consecutiveDeaths, null)
        forwardEngineOutput(p)

        // 看护线程(每次化身一条,daemon 线程不阻进程退出):
        // 只做 waitFor + 计算退避,真正的重生 fork 仍交回主线程。
        thread(name = "mtdss-watch", isDaemon = true) {
            val exitCode = try {
                p.waitFor()
            } catch (_: InterruptedException) {
                return@thread
            }
            if (stopping) return@thread
            val uptime = SystemClock.elapsedRealtime() - startedAt
            val deaths = if (uptime >= STABLE_UPTIME_MS) 0 else consecutiveDeaths + 1
            val delay = backoffMs(deaths)
            Log.w(
                TAG,
                "引擎退出 code=$exitCode uptimeMs=$uptime → ${delay}ms 后重启(连续死亡 $deaths)",
            )
            _status.value = EngineStatus(Phase.BACKOFF, deaths, "exit=$exitCode")
            scheduleRespawn(deaths)
        }
    }

    /** 退避后回主线程重生(主线程 fork,见类注释 PDEATHSIG 段)。 */
    private fun scheduleRespawn(consecutiveDeaths: Int) {
        val delay = backoffMs(consecutiveDeaths)
        mainHandler.postDelayed(
            { if (!stopping) spawnAndWatch(consecutiveDeaths) },
            delay,
        )
    }

    /** 第 n 次连续死亡的退避:500ms * 2^(n-1),封顶 15s;n=0(稳定后首退)即基数。 */
    private fun backoffMs(consecutiveDeaths: Int): Long {
        require(consecutiveDeaths >= 0)
        val shift = (consecutiveDeaths - 1).coerceAtLeast(0)
        return (BACKOFF_BASE_MS shl shift).coerceAtMost(BACKOFF_CAP_MS)
    }

    /** 引擎输出(合并后的 stdout/stderr)逐行转 logcat,tag 恒 [ENGINE_LOG_TAG]。 */
    private fun forwardEngineOutput(p: Process) {
        thread(name = "mtdss-stderr", isDaemon = true) {
            try {
                p.inputStream.bufferedReader(Charsets.UTF_8).useLines { lines ->
                    for (line in lines) {
                        Log.i(ENGINE_LOG_TAG, line)
                    }
                }
            } catch (_: IOException) {
                // 进程被杀时读端 EOF/重置属正常收尾,不必刷日志
            }
        }
    }

    /** Process.waitFor(timeout) 的可空包装(true=已退出)。 */
    private fun waitFor(p: Process, timeoutMs: Long): Boolean =
        try {
            p.waitFor(timeoutMs, java.util.concurrent.TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            true
        }
}
