package com.mtdss.android.update

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/**
 * 检查更新 UI 状态机(spec §6;评审裁定①:APK 下载不走 daemon 任务面,
 * App 自管):idle → checking → available/UpToDate → downloading(进度)→
 * installing(安装器已拉起);分支 failed。词面/失败面镜像 crates/daemon/
 * src/update.rs(同一信任链两端,失败模式一致);M4 的空闲门/自动档/标记
 * 序列均为 daemon 专属,安卓无(无常驻 ⇒ 无后台检查,仅设置页手动触发)。
 */
sealed interface UpdateUiState {
    /** 更新源未配置(基址/公钥缺一或非法)——fail-closed,镜像 M4「更新未配置」。 */
    data object Unconfigured : UpdateUiState

    data object Idle : UpdateUiState

    data object Checking : UpdateUiState

    /** 检查完成无新版(含源 404 的「无更新」口径)。 */
    data class UpToDate(val current: String) : UpdateUiState

    data class Available(val latest: String, val notes: String) : UpdateUiState

    data class Downloading(val latest: String, val received: Long, val total: Long) : UpdateUiState

    /** 下载复核通过,系统安装器已拉起(装不装用户定;spec §6 无静默自装)。 */
    data class Installing(val latest: String) : UpdateUiState

    data class Failed(val error: String) : UpdateUiState
}

/**
 * 检查更新编排器(app 级:APK 下载不随设置页进出重置)。链路:
 * check(拉 `{base}/android/manifest.json` → 解析 → 验签 → 比版)→
 * download(进度 → SHA-256/字节数复核 → rename 就位)→ 拉起系统安装器。
 *
 * 重试口径镜像 M4 的清单缓存:验签通过的清单保留([verifiedManifest]),
 * 下载/安装期失败后 retry 直接重下,不重查;检查期失败(无缓存)重查。
 * 下载失败即删临时件 `.part`(系统零改动)。404 = 源无更新(非失败)。
 * 版本资产查找在 download 期(check 只比版),镜像 M4 的 apply 面。
 */
class UpdateController(
    private val dlBase: String,
    private val pubkeyHex: String,
    private val currentVersion: String,
    private val dir: File,
    private val http: HttpStack,
    private val installer: (File) -> Unit,
    private val scope: CoroutineScope,
    private val progressMinGapMs: Long = PROGRESS_MIN_GAP_MS,
) {
    private val _state = MutableStateFlow(initialState())

    val state: StateFlow<UpdateUiState> = _state.asStateFlow()

    /** 验签通过的清单缓存(M4 同款:失败重试免重查;仅主流程写)。 */
    @Volatile
    private var verifiedManifest: UpdateManifest? = null

    /** 已就位 APK(重拉安装器用;rename 成功后写,直到新版本覆盖)。 */
    @Volatile
    private var staged: Pair<UpdateManifest, File>? = null

    private val configured: Boolean =
        dlBase.isNotBlank() && UpdateCheck.pubkeyBytes(pubkeyHex) != null

    /** `{base}/android/manifest.json`(直发渠道安卓最新位,布局见 scripts/android/README.md)。 */
    private val manifestUrl: String
        get() = dlBase.trimEnd('/') + "/android/manifest.json"

    // ── 入口(设置页按钮面)────────────────────────────────────────────────────

    fun check() {
        if (busy() || !configured) return
        // 同步翻转 Checking(在途防抖:第二击在 launch 前即见 Checking 被拒,
        // 不依赖协程调度时序)
        _state.value = UpdateUiState.Checking
        scope.launch {
            http.fetch(manifestUrl).fold(
                onSuccess = { bytes -> onManifest(bytes) },
                onFailure = { e ->
                    if (e is HttpStack.HttpError && e.code == 404) {
                        // 源无清单 = 无更新(M4 404 口径),非失败
                        _state.value = UpdateUiState.UpToDate(currentVersion)
                    } else {
                        fail("拉取更新清单失败: ${e.message ?: e.javaClass.simpleName}")
                    }
                },
            )
        }
    }

    fun download() {
        if (busy()) return
        val m = verifiedManifest ?: return
        val asset = m.platforms[ANDROID_PLATFORM_KEY]
        if (asset == null) {
            // 资产查找在下载期(check 只比版,镜像 M4 的 apply 面)
            fail("清单缺少 $ANDROID_PLATFORM_KEY 资产")
            return
        }
        // 同步翻转 Downloading(在途防抖,同 check)
        _state.value = UpdateUiState.Downloading(m.version, 0, asset.size)
        scope.launch { downloadLoop(m, asset) }
    }

    /**
     * 重试:Installing = 重拉安装器(用户在系统弹窗取消后再试);Failed =
     * 有就位件先重拉,次选清单缓存重下,再退重查(镜像 M4 apply(None));
     * 其余态 = 重查。
     */
    fun retry() {
        when (_state.value) {
            is UpdateUiState.Installing -> {
                val (m, apk) = staged ?: return
                fireInstaller(m, apk)
            }
            is UpdateUiState.Failed -> when {
                staged != null -> {
                    val (m, apk) = staged!!
                    fireInstaller(m, apk)
                }
                verifiedManifest != null -> download()
                else -> check()
            }
            else -> check()
        }
    }

    // ── 检查臂 ────────────────────────────────────────────────────────────────

    private fun onManifest(bytes: ByteArray) {
        val m = try {
            UpdateCheck.parseManifest(String(bytes, Charsets.UTF_8))
        } catch (e: IllegalArgumentException) {
            fail("解析更新清单失败: ${e.message}")
            return
        }
        if (!UpdateCheck.verifySignature(m, pubkeyHex)) {
            fail("清单验签失败")
            return
        }
        val cmp = UpdateCheck.versionCompare(m.version, currentVersion)
        if (cmp == null) {
            fail("清单版本非法(须为严格 X.Y.Z)")
            return
        }
        if (cmp > 0) {
            verifiedManifest = m
            _state.value = UpdateUiState.Available(m.version, m.notes)
        } else {
            _state.value = UpdateUiState.UpToDate(currentVersion)
        }
    }

    // ── 下载臂 ────────────────────────────────────────────────────────────────

    private suspend fun downloadLoop(m: UpdateManifest, asset: PlatformAsset) {
        val tmp = File(dir, "mtdss-${m.version}.apk.part")
        val finalFile = File(dir, "mtdss-${m.version}.apk")
        val throttle = ProgressThrottle(progressMinGapMs)
        val outcome = withContext(Dispatchers.IO) {
            runCatching {
                if (!dir.exists() && !dir.mkdirs()) {
                    throw IllegalStateException("创建下载目录失败: $dir")
                }
                val (received, sha) = http.download(asset.url, tmp, asset.size) { received ->
                    if (throttle.shouldEmit(force = received >= asset.size)) {
                        _state.value = UpdateUiState.Downloading(m.version, received, asset.size)
                    }
                }.getOrThrow()
                if (!sha.equals(asset.sha256, ignoreCase = true)) {
                    throw IllegalStateException("下载复核失败: SHA-256 不符(得 $sha)")
                }
                if (received != asset.size) {
                    throw IllegalStateException("下载复核失败: 字节数不符($received != ${asset.size})")
                }
                if (!tmp.renameTo(finalFile)) {
                    throw IllegalStateException("下载就位失败(rename): $finalFile")
                }
                finalFile
            }
        }
        outcome.fold(
            onSuccess = { apk -> fireInstaller(m, apk) },
            onFailure = { e ->
                tmp.delete() // 失败即删临时件:系统零改动
                fail(downloadFailureMessage(e))
            },
        )
    }

    /** 下载失败词面:复核/就位类已终形(镜像 M4 即抛即用);网络/HTTP 类加「下载失败」前缀。 */
    private fun downloadFailureMessage(e: Throwable): String = when (e) {
        is HttpStack.HttpError -> "下载失败: ${e.message}"
        is HttpStack.ContentLengthMismatch, is IllegalStateException -> e.message ?: "下载失败"
        else -> "下载失败: ${e.message ?: e.javaClass.simpleName}"
    }

    private fun fireInstaller(m: UpdateManifest, apk: File) {
        // staged 先记(下载已就位;安装器拉起失败时重试 = 重拉而非重下)
        staged = m to apk
        try {
            installer(apk)
        } catch (e: Exception) {
            fail("无法拉起安装器: ${e.message ?: e.javaClass.simpleName}")
            return
        }
        _state.value = UpdateUiState.Installing(m.version)
    }

    private fun fail(msg: String) {
        _state.value = UpdateUiState.Failed(msg)
    }

    private fun busy(): Boolean = _state.value.let {
        it is UpdateUiState.Checking || it is UpdateUiState.Downloading || it is UpdateUiState.Installing
    }

    private fun initialState(): UpdateUiState =
        if (dlBase.isNotBlank() && UpdateCheck.pubkeyBytes(pubkeyHex) != null) UpdateUiState.Idle
        else UpdateUiState.Unconfigured

    private companion object {
        /** 进度上报最小间隔(镜像 daemon 的 PROGRESS_EVENT_MIN_GAP;首条与终条恒发)。 */
        const val PROGRESS_MIN_GAP_MS = 400L
    }
}

/**
 * 进度节流(纯函数,可测):首条即发,其后距上次 ≥ [minGapMs] 才发;终条
 * ([force])恒发(进度条必须到 100%,这是对 M4 节流口径的一点有意加严)。
 */
internal class ProgressThrottle(
    private val minGapMs: Long,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private var lastEmit = NOT_YET

    fun shouldEmit(force: Boolean): Boolean {
        val now = clock()
        if (force || lastEmit == NOT_YET || now - lastEmit >= minGapMs) {
            lastEmit = now
            return true
        }
        return false
    }

    private companion object {
        const val NOT_YET = Long.MIN_VALUE
    }
}
