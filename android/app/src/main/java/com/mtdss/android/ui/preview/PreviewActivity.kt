package com.mtdss.android.ui.preview

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import kotlin.math.abs
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.foundation.border
import coil.request.CachePolicy
import coil.request.ImageRequest
import coil.compose.AsyncImage
import com.mtdss.android.ui.browse.BrowseThumbs
import com.mtdss.android.ui.browse.PreviewHandlePool
import androidx.compose.runtime.rememberCoroutineScope
import androidx.core.view.WindowInsetsControllerCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowCompat
import androidx.compose.ui.platform.LocalView
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Image
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.activity.ComponentActivity
import androidx.core.content.FileProvider
import com.mtdss.android.ui.common.AppIcons
import com.mtdss.android.ui.common.VectorGlyph
import com.mtdss.android.Graph
import com.mtdss.android.data.MtdssRepository
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.PreviewOpenResult
import com.mtdss.android.ui.browse.BrowseViewModel
import com.mtdss.android.ui.common.ActionGlyph
import com.mtdss.android.ui.common.ActionGlyphKind
import com.mtdss.android.ui.common.DevicePickerSheet
import com.mtdss.android.ui.common.Format
import com.mtdss.android.ui.common.MaterialGlyphs
import com.mtdss.android.ui.common.MockGlyph
import com.mtdss.android.ui.theme.SynshuTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/**
 * 全屏预览屏(V4-1 T23;钦定 mockup mobile-preview.html Android 列;spec
 * 「四容器一分区」):浏览行/磁贴文件点入推入(独立 Activity,非导出——
 * 仅 App 内部调起),黑底沉浸:
 * - 图片 = Compose 自研 zoom 查看器(双指缩放/拖拽/‹›切换;thumb 先显 →
 *   raw 渐进,HEIC = jpeg 端点优先)+ 底部操作条 下载·转发·全屏·信息 +
 *   页点(mockup);
 * - 视频/音频 = Media3 ExoPlayer 原流直连(raw?t=;移动端原生解码不经
 *   转码——spec 裁定,标准控制器);
 * - 文档(pdf/office/md/文本)= WebView 宿主页 + previewer bundle(boot
 *   经 JS 注入,[PreviewBoot] 单点);
 * - 不支持格式 = 类型页 + 下载引导(previewer unsupported 同词面)。
 *
 * 「···」菜单(全容器):下载 / 转发 / 用其他应用打开(在场句柄 raw 拷
 * 缓存 + FileProvider 系统打开器,[UpdateInstaller] 先例)/ 查看信息
 * (名字·大小·路径·设备)。
 *
 * 降级面:源离线 =「源设备离线,无法预览」(mockup 确认点⑤);T10
 * `icloud_not_local` 词透出 =「原件在 iCloud,需先下载」(远端 iPhone
 * 源场景);转发/下载 = 既有流入口(DevicePickerSheet / downloadToInbox
 * 收件箱落点),发起即 toast,进度归传输列表。
 *
 * 句柄生命周期:条目切换 = 旧句柄 close(NonCancellable)+ 新条目 open
 * (在途取消不入账,[PreviewHandlePool] 同款竞态闭合);屏面卸载(返回/
 * finish/旋转重建)全量收口——组合销毁序先关句柄后撤 scope(daemon 60s
 * 空闲回收终极兜底);旋转后重开一枚新句柄,预览态复位到点入条目。
 */
class PreviewActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val graph = com.mtdss.android.AppGraph.get()
        val deviceId = intent.getStringExtra(EXTRA_DEVICE_ID) ?: ""
        val isRemote = intent.getBooleanExtra(EXTRA_IS_REMOTE, true)
        val deviceLabel = intent.getStringExtra(EXTRA_DEVICE_LABEL) ?: deviceId
        val entry = intent.getStringExtra(EXTRA_ENTRY)?.let(::decodeEntry) ?: run {
            finish()
            return
        }
        val siblings = intent.getStringArrayListExtra(EXTRA_SIBLINGS)
            ?.mapNotNull(::decodeEntry)
            ?: emptyList()
        setContent {
            SynshuTheme {
                PreviewScreen(
                    graph = graph,
                    deviceId = deviceId,
                    isRemote = isRemote,
                    deviceLabel = deviceLabel,
                    initialEntry = entry,
                    siblings = siblings,
                )
            }
        }
    }

    // ── 调起面(浏览行/磁贴/树行/磁贴入口共用)─────────────────────────────

    companion object {

        /** WebView 宿主页资产路径(previewer bundle 同目录)。 */
        const val HOST_PAGE_URL = "file:///android_asset/previewer/preview.html"

        private const val EXTRA_DEVICE_ID = "device_id"
        private const val EXTRA_IS_REMOTE = "is_remote"
        private const val EXTRA_DEVICE_LABEL = "device_label"
        private const val EXTRA_ENTRY = "entry"
        private const val EXTRA_SIBLINGS = "siblings"

        /** 条目内字段分隔(␁;路径不含控制字节,FavPickRequestSaver 同象防歧义)。 */
        internal const val FIELD_SEP = "\u0001"

        /**
         * 推入全屏预览(context = Activity 即可,无需 NEW_TASK)。
         * siblings = 同屏文件兄弟集(展示序;图库 ‹› 切换集取其图片类,
         * [PreviewGallery])。
         */
        /** 兄弟集入 Intent 帽(binder transaction 界;超大目录图库截 500,
         *  截断只影响 ‹› 切换集,预览本体不受影响)。 */
        internal const val MAX_SIBLINGS = 500

        fun start(
            context: Context,
            deviceId: String,
            isRemote: Boolean,
            deviceLabel: String,
            entry: DirEntry,
            siblings: List<DirEntry>,
        ) {
            context.startActivity(
                Intent(context, PreviewActivity::class.java).apply {
                    putExtra(EXTRA_DEVICE_ID, deviceId)
                    putExtra(EXTRA_IS_REMOTE, isRemote)
                    putExtra(EXTRA_DEVICE_LABEL, deviceLabel)
                    putExtra(EXTRA_ENTRY, encodeEntry(entry))
                    putStringArrayListExtra(
                        EXTRA_SIBLINGS,
                        ArrayList(siblings.take(MAX_SIBLINGS).map(::encodeEntry)),
                    )
                },
            )
            // r63 #8:底部滑入转场(iOS sheet 手感;Activity 上下文才生效)
            (context as? android.app.Activity)?.overridePendingTransition(
                android.R.anim.fade_in, android.R.anim.fade_in,
            )
        }

        /** 条目紧凑编码(name␁path␁size␁mtime;文件行快照四元)。 */
        internal fun encodeEntry(e: DirEntry): String =
            listOf(e.name, e.path, e.size.toString(), e.mtimeMs.toString()).joinToString(FIELD_SEP)

        /** 条目解码(段缺 = null 不猜;size/mtime 缺省 0 兜底)。 */
        internal fun decodeEntry(s: String): DirEntry? {
            val f = s.split(FIELD_SEP)
            if (f.size < 4) return null
            return DirEntry(
                name = f[0],
                path = f[1],
                size = f[2].toLongOrNull() ?: 0L,
                mtimeMs = f[3].toLongOrNull() ?: 0L,
                isDir = false,
            )
        }
    }
}

// ── 预览屏壳(顶栏/内容分发/底部操作条/菜单/弹层群)────────────────────────

/** 句柄 UI 态(null = 不开启——unsupported 容器零 preview_open)。 */
private sealed interface HandleUi {
    data object Loading : HandleUi
    data class Open(val result: PreviewOpenResult) : HandleUi
    data class Failed(val face: String) : HandleUi
}

/** 副标题文案(「3.3 MB · MIX 3」mockup 形;尺寸缺席 = 单段)。 */
internal fun previewSubtitle(size: Long, deviceLabel: String): String =
    if (size > 0) "${Format.size(size, isDir = false)} · $deviceLabel" else deviceLabel

@Composable
internal fun PreviewScreen(
    graph: Graph,
    deviceId: String,
    isRemote: Boolean,
    deviceLabel: String,
    initialEntry: DirEntry,
    siblings: List<DirEntry>,
) {
    val context = LocalContext.current
    // 屏面协程域(组合级;先注册 = 后销毁,收口面在其前跑,BrowseScreen 同序)
    val scope = remember { CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate) }
    DisposableEffect(Unit) {
        onDispose { scope.cancel() }
    }

    // ── 图库(‹›切换集 = 兄弟集图片类;mockup 确认点③;切集含当前条目兜底)
    val images = remember(initialEntry, siblings) {
        val sib = PreviewGallery.imageSiblings(siblings)
        if (sib.any { it.path == initialEntry.path }) sib else listOf(initialEntry) + sib
    }
    var current by remember { mutableStateOf(initialEntry) }
    var fullscreen by remember { mutableStateOf(false) }
    // r62 #5:沉浸态 = 藏顶栏/底栏/胶片条/系统栏(WindowInsetsController;
    // iOS statusBarHidden 同象;全容器通用,视频点画面区也可进)
    val activity = LocalContext.current as? ComponentActivity
    LaunchedEffect(fullscreen, activity) {
        val act = activity ?: return@LaunchedEffect
        val wc = WindowCompat.getInsetsController(act.window, act.window.decorView)
        wc.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        if (fullscreen) wc.hide(WindowInsetsCompat.Type.systemBars())
        else wc.show(WindowInsetsCompat.Type.systemBars())
    }
    var retryNonce by remember { mutableIntStateOf(0) }
    val container = PreviewRoute.containerOf(current.name)
    val needsHandle = container != PreviewContainer.UNSUPPORTED
    val imageIndex = images.indexOfFirst { it.path == current.path }

    // r60 #3:视频/音频滑切集(图片集既有);兄弟类切换与 iOS 同象
    val videos = remember(initialEntry, siblings) {
        val sib = PreviewGallery.videoSiblings(siblings)
        if (sib.any { it.path == initialEntry.path }) sib else listOf(initialEntry) + sib
    }
    val videoIndex = videos.indexOfFirst { it.path == current.path }
    val audios = remember(initialEntry, siblings) {
        val sib = PreviewGallery.audioSiblings(siblings)
        if (sib.any { it.path == initialEntry.path }) sib else listOf(initialEntry) + sib
    }
    val audioIndex = audios.indexOfFirst { it.path == current.path }

    fun stepImage(delta: Int) {
        if (images.isEmpty()) return
        val base = if (imageIndex < 0) 0 else imageIndex
        current = images[PreviewGallery.wrapIndex(base + delta, images.size)]
    }

    fun stepVideo(delta: Int) {
        if (videos.isEmpty()) return
        val base = if (videoIndex < 0) 0 else videoIndex
        current = videos[PreviewGallery.wrapIndex(base + delta, videos.size)]
    }

    fun stepAudio(delta: Int) {
        if (audios.isEmpty()) return
        val base = if (audioIndex < 0) 0 else audioIndex
        current = audios[PreviewGallery.wrapIndex(base + delta, audios.size)]
    }

    // ── 弹层/反馈面
    var handleUi by remember { mutableStateOf<HandleUi?>(HandleUi.Loading) }
    var menuOpen by remember { mutableStateOf(false) }
    var showInfo by remember { mutableStateOf(false) }
    var showForward by remember { mutableStateOf(false) }
    var toast by remember { mutableStateOf<String?>(null) }
    var actionError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(toast) {
        toast?.let {
            delay(2200)
            toast = null
        }
    }

    // ── 动作(下载/转发/用其他应用打开;既有流入口,brief 钉)──────────────

    fun doDownload() {
        scope.launch {
            graph.repo.downloadToInbox(deviceId, listOf(current.path)).fold(
                onSuccess = { toast = "已开始 1 项 · 进度见传输列表" },
                onFailure = { actionError = BrowseViewModel.userErrorText(it) },
            )
        }
    }

    fun doForward(target: com.mtdss.android.ipc.DirectoryDevice) {
        scope.launch {
            // 分档 = 浏览档(远端 = 转发 remote_copy;本机 = 路径形态 upload,
            // BrowseViewModel.sendItems 同口径;dir = 收件箱哨兵)
            val r = if (isRemote) {
                graph.repo.forward(from = deviceId, paths = listOf(current.path), to = target.deviceId)
            } else {
                graph.repo.uploadFiles(files = listOf(current.path), to = target.deviceId, dir = MtdssRepository.INBOX_DIR)
            }
            r.fold(
                onSuccess = { toast = "已开始 1 项 · 进度见传输列表" },
                onFailure = { actionError = BrowseViewModel.userErrorText(it) },
            )
        }
    }

    fun doOpenAs() {
        // r64 #7:全类型常开(iOS r54 同象)——句柄自足:在场句柄直用;
        // 不支持类型现开临时句柄,取毕即关
        scope.launch {
            val present = (handleUi as? HandleUi.Open)?.result
            val tempResult = if (present == null) {
                graph.repo.previewOpen(
                    deviceId = if (isRemote) deviceId else null,
                    path = current.path,
                    mtimeMs = current.mtimeMs,
                    size = current.size,
                ).getOrNull()
            } else null
            val handle = present ?: tempResult ?: run {
                actionError = "打开失败:文件打开失败"
                return@launch
            }
            if (tempResult != null) {
                // 临时句柄取毕即关(detached 不随生命周期取消)
                kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) {
                    scope.launch(kotlinx.coroutines.NonCancellable) {
                        graph.repo.previewClose(tempResult.handle)
                    }
                }
            }
            val fetched = withContext(Dispatchers.IO) {
                runCatching { fetchToOpenAsFile(current, handle, context.cacheDir) }
            }
            fetched.fold(
                onSuccess = { file ->
                    val uri: Uri = FileProvider.getUriForFile(
                        context,
                        context.packageName + ".fileprovider",
                        file,
                    )
                    // r63 #7:系统 App 选择器(iOS ActivitySheet 同象)——
                    // ACTION_SEND + chooser:选微信 = 微信发送、选豆包 =
                    // 发给豆包、选 WPS = 打开;一个入口全通
                    val send = Intent(Intent.ACTION_SEND).apply {
                        type = PreviewRoute.mimeOf(current.name)
                        putExtra(Intent.EXTRA_STREAM, uri)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                    val chooser = Intent.createChooser(send, "分享/打开「${current.name}」").apply {
                        putExtra(Intent.EXTRA_TITLE, "选择应用")
                    }
                    try {
                        context.startActivity(chooser)
                    } catch (e: ActivityNotFoundException) {
                        actionError = "未找到可处理此文件的应用"
                    }
                },
                onFailure = { actionError = "打开失败:" + BrowseViewModel.userErrorText(it) },
            )
        }
    }

    // ── 句柄生命周期(条目切换旧 close 新 open;屏面卸载全量收口)────────────

    DisposableEffect(Unit) {
        onDispose {
            // 收口面(先于 scope.cancel 跑,序见上):在场句柄 close
            val cur = handleUi
            if (cur is HandleUi.Open) {
                scope.launch(NonCancellable) {
                    runCatching { graph.repo.previewClose(cur.result.handle) }
                }
            }
        }
    }
    DisposableEffect(current.path, container, retryNonce) {
        if (!needsHandle) {
            handleUi = null
            onDispose { }
        } else {
            handleUi = HandleUi.Loading
            val job = scope.launch {
                val r = graph.repo.previewOpen(
                    deviceId = if (isRemote) deviceId else null,
                    path = current.path,
                    mtimeMs = current.mtimeMs,
                    size = current.size,
                )
                // 条目已切走/屏面已卸 = 迟到应答不入账(在途 IPC 应答即弃)
                coroutineContext.ensureActive()
                handleUi = r.fold(
                    onSuccess = { HandleUi.Open(it) },
                    onFailure = { HandleUi.Failed(PreviewErrors.userFace(it.message)) },
                )
            }
            onDispose {
                job.cancel()
                val prior = handleUi
                if (prior is HandleUi.Open) {
                    scope.launch(NonCancellable) {
                        runCatching { graph.repo.previewClose(prior.result.handle) }
                    }
                }
            }
        }
    }

    // 系统返回:全屏中先回带操作条;否则默认 finish(Activity 返回栈)
    BackHandler(enabled = fullscreen) { fullscreen = false }

    // ── 面(mockup Android 列:黑底沉浸 + 顶栏 + 画面 + 底部操作条)─────────

    androidx.compose.material3.Surface(Modifier.fillMaxSize(), color = PreviewBlack) {
        Column(Modifier.fillMaxSize()) {
            if (!fullscreen) {
                PreviewTopBar(
                    name = current.name,
                    subtitle = previewSubtitle(current.size, deviceLabel),
                    onBack = {
                    (context as? ComponentActivity)?.finish()
                    // r63 #8:滑出收场(与滑入对称)
                    (context as? ComponentActivity)?.overridePendingTransition(
                        android.R.anim.fade_in, android.R.anim.fade_out,
                    )
                },
                    menuOpen = menuOpen,
                    onMenuToggle = { menuOpen = !menuOpen },
                    onMenuDismiss = { menuOpen = false },
                    openAsEnabled = true, // r64 #7:全类型常开
                    onDownload = { menuOpen = false; doDownload() },
                    onForward = { menuOpen = false; showForward = true },
                    onOpenAs = { menuOpen = false; doOpenAs() },
                    onInfo = { menuOpen = false; showInfo = true },
                )
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when (container) {
                    PreviewContainer.IMAGE -> {
                        val h = handleUi
                        when {
                            h is HandleUi.Open -> PreviewImageViewer(current, h.result, Modifier.fillMaxSize(), onSwipe = ::stepImage, onTap = { fullscreen = !fullscreen })
                            h is HandleUi.Failed -> PreviewErrorFace(h.face, { retryNonce += 1 }, Modifier.fillMaxSize())
                            else -> PreviewLoadingFace(Modifier.fillMaxSize())
                        }
                        if (fullscreen) {
                            // 沉浸:任意点按回 chrome(系统返回同效)
                            Box(
                                Modifier
                                    .matchParentSize()
                                    .pointerInput(Unit) {
                                        detectTapGestures(onTap = { fullscreen = !fullscreen })
                                    },
                            )
                        } else if (images.size > 1) {
                            // r62 #5:胶片条(iOS 相册同象;页点随撤)——
                            // 兄弟缩略图,当前 accent 描边,点选跳图
                            PreviewFilmstrip(
                                repo = graph.repo,
                                deviceId = deviceId,
                                isRemote = isRemote,
                                images = images,
                                currentPath = current.path,
                                modifier = Modifier
                                    .align(Alignment.BottomCenter)
                                    .padding(bottom = 8.dp),
                                onSelect = { entry -> if (entry.path != current.path) current = entry },
                            )
                        }
                    }

                    PreviewContainer.VIDEO, PreviewContainer.AUDIO -> {
                        val h = handleUi
                        // r63 #2(iOS r49 同象):进去不自动播——海报面(视频 =
                        // 缩略图 + 中央播放钮;音频 = 播放钮卡),点播放才起
                        // ExoPlayer;r60 横滑切保留(切换回海报面)
                        var playing by remember(current.path) { mutableStateOf(false) }
                        var swipeAccum by remember { mutableFloatStateOf(0f) }
                        val onSwipe: (Int) -> Unit = { delta ->
                            playing = false
                            if (container == PreviewContainer.VIDEO) stepVideo(delta) else stepAudio(delta)
                        }
                        Box(
                            Modifier
                                .fillMaxSize()
                                // r64 #3 根修:海报面 clickable/ExoPlayer
                                // PlayerView 会消费 down → detectHorizontal
                                // DragGestures(要求未消费)死 = 只能滑一次。
                                // 自研容错手势:不要求未消费;水平超阈即切。
                                .pointerInput(container) {
                                    awaitEachGesture {
                                        awaitFirstDown(requireUnconsumed = false)
                                        var accumX = 0f
                                        var accumY = 0f
                                        while (true) {
                                            val event = awaitPointerEvent()
                                            if (event.changes.none { it.pressed }) break
                                            val change = event.changes.first()
                                            accumX += change.positionChange().x
                                            accumY += change.positionChange().y
                                            if (abs(accumX) > 40f && abs(accumX) > abs(accumY)) {
                                                change.consume()
                                            }
                                        }
                                        if (abs(accumX) > 72f && abs(accumX) > abs(accumY)) {
                                            onSwipe(if (accumX < 0f) 1 else -1)
                                        }
                                    }
                                },
                        ) {
                        when {
                            h is HandleUi.Open -> if (playing) {
                                PreviewMediaViewer(
                                    PreviewRoute.rawUrl(h.result.port, h.result.handle, h.result.token),
                                    Modifier.fillMaxSize(),
                                )
                            } else {
                                MediaPosterFace(
                                    name = current.name,
                                    isVideo = container == PreviewContainer.VIDEO,
                                    thumbUrl = if (container == PreviewContainer.VIDEO) {
                                        BrowseThumbs.thumbUrl(h.result.port, h.result.handle, h.result.token)
                                    } else null,
                                    modifier = Modifier.fillMaxSize(),
                                    onPlay = { playing = true },
                                )
                            }
                            h is HandleUi.Failed -> PreviewErrorFace(h.face, { retryNonce += 1 }, Modifier.fillMaxSize())
                            else -> PreviewLoadingFace(Modifier.fillMaxSize())
                        }
                        }
                    }

                    PreviewContainer.DOC -> {
                        val h = handleUi
                        when {
                            h is HandleUi.Open -> {
                                val kind = PreviewRoute.docKindOf(current.name) ?: "text"
                                val js = PreviewBoot.injectionJs(
                                    PreviewBoot.bootJson(
                                        PreviewBoot.requestJson(
                                            kind = kind,
                                            rawUrl = PreviewRoute.rawUrl(h.result.port, h.result.handle, h.result.token),
                                            name = current.name,
                                            size = current.size,
                                        ),
                                        h.result.handle,
                                    ),
                                )
                                key(js) { PreviewDocViewer(js, Modifier.fillMaxSize()) }
                            }
                            h is HandleUi.Failed -> PreviewErrorFace(h.face, { retryNonce += 1 }, Modifier.fillMaxSize())
                            else -> PreviewLoadingFace(Modifier.fillMaxSize())
                        }
                    }

                    PreviewContainer.UNSUPPORTED ->
                        PreviewUnsupportedFace(current, onDownload = ::doDownload, Modifier.fillMaxSize())
                }
            }
            // r62 #4:底部动作条全容器统一(iOS r49 同象;图片沉浸态藏);
            // 四钮 iOS 口径(下载/转发/用其他应用打开/查看信息——全屏钮
            // 由图片点按沉浸接管)
            if (!fullscreen) {
                PreviewBottomBar(
                    onDownload = ::doDownload,
                    onForward = { showForward = true },
                    onOpenAs = { menuOpen = false; doOpenAs() },
                    onInfo = { showInfo = true },
                )
            }
        }

        // 动作发起反馈胶囊(底部悬浮;iOS toast 同构)
        toast?.let { text ->
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
                Text(
                    text,
                    Modifier
                        .padding(bottom = 96.dp)
                        .background(Color.White.copy(alpha = 0.15f), RoundedCornerShape(percent = 50))
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                    color = Color.White,
                    fontSize = 13.sp,
                    maxLines = 1,
                )
            }
        }
    }

    // ── 弹层群(信息/转发/动作失败)────────────────────────────────────────

    if (showInfo) {
        // r62 #6:实底 surface(黑底 Activity 上缺省容器色透明 = 可读性差
        // 根因)+ 内容补全(iOS PreviewInfoSheet 口径:格式/三时间;
        // ctime/atime 引擎 list 已发,0 = 不显)
        val sheetBg = MaterialTheme.colorScheme.surface
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { showInfo = false },
            containerColor = sheetBg,
            titleContentColor = MaterialTheme.colorScheme.onSurface,
            textContentColor = MaterialTheme.colorScheme.onSurface,
            title = { Text("文件信息") },
            text = {
                Column {
                    InfoRow("名称", current.name)
                    InfoRow("大小", Format.size(current.size, isDir = false))
                    InfoRow("格式", infoTypeLine(current.name))
                    InfoRow("路径", current.path)
                    InfoRow("设备", deviceLabel)
                    InfoRow("修改时间", Format.mtime(current.mtimeMs))
                    if (current.ctimeMs > 0) InfoRow("创建时间", Format.mtime(current.ctimeMs))
                    if (current.atimeMs > 0) InfoRow("访问时间", Format.mtime(current.atimeMs))
                }
            },
            confirmButton = {
                TextButton(onClick = { showInfo = false }) { Text("完成") }
            },
        )
    }

    if (showForward) {
        DevicePickerSheet(
            graph = graph,
            title = if (isRemote) "转发 1 项到…" else "发送 1 项到…",
            excluding = deviceId, // 来源设备前置滤(daemon 拒 from==to,BrowseScreen 同口径)
            onPick = { target ->
                showForward = false
                doForward(target)
            },
        )
    }

    actionError?.let { errorText ->
        AlertDialog(
            onDismissRequest = { actionError = null },
            title = { Text("操作失败") },
            text = { Text(errorText) },
            confirmButton = {
                TextButton(onClick = { actionError = null }) { Text("好") }
            },
        )
    }
}

/** 打开失败裸条目取数(用其他应用打开前无在场句柄时的兜底;恒 false 面)。 */
private suspend fun fetchToOpenAsFile(
    entry: DirEntry,
    handle: PreviewOpenResult,
    cacheDir: File,
): File {
    val dir = File(cacheDir, "preview-open").apply { mkdirs() }
    // 目录自净:打开器只消费当次拷贝,旧件不留(cache-path FileProvider 面)
    dir.listFiles()?.forEach { it.delete() }
    val safe = entry.name.replace(Regex("[\\\\/:*?\"<>|]"), "_").takeLast(120)
    val file = File(dir, "${System.currentTimeMillis()}-$safe")
    val conn = URL(PreviewRoute.rawUrl(handle.port, handle.handle, handle.token)).openConnection() as HttpURLConnection
    try {
        conn.connectTimeout = 10_000
        conn.readTimeout = 30_000
        if (conn.responseCode >= 400) throw IOException("HTTP ${conn.responseCode}")
        conn.inputStream.use { input ->
            file.outputStream().use { output -> input.copyTo(output, 64 * 1024) }
        }
    } finally {
        conn.disconnect()
    }
    return file
}

// ── 顶栏/底部操作条/行件(mockup Android 列度量)───────────────────────────

/** 顶栏(← / 名字+副文 / ··· 菜单;mockup:白名字 14sp + 灰副文 11sp)。 */
@Composable
private fun PreviewTopBar(
    name: String,
    subtitle: String,
    onBack: () -> Unit,
    menuOpen: Boolean,
    onMenuToggle: () -> Unit,
    onMenuDismiss: () -> Unit,
    openAsEnabled: Boolean,
    onDownload: () -> Unit,
    onForward: () -> Unit,
    onOpenAs: () -> Unit,
    onInfo: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(start = 4.dp, end = 8.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(40.dp)
                .clickable(onClickLabel = "返回", onClick = onBack),
            contentAlignment = Alignment.Center,
        ) {
            VectorGlyph(AppIcons.chevronLeft, 22.dp, Color.White)
        }
        Column(Modifier.weight(1f)) {
            Text(
                name,
                color = Color.White,
                fontSize = 14.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                subtitle,
                Modifier.padding(top = 1.dp),
                color = Color(0xFF9A9AA5),
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Box {
            Text(
                "···",
                Modifier
                    .clickable(onClickLabel = "更多操作", onClick = onMenuToggle)
                    .padding(horizontal = 10.dp, vertical = 6.dp),
                color = Color.White,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
            )
            DropdownMenu(
                expanded = menuOpen,
                onDismissRequest = onMenuDismiss,
                // r62 #7:实底 surface(黑底 Activity 上缺省透明 = 可读性差)
                containerColor = MaterialTheme.colorScheme.surface,
            ) {
                DropdownMenuItem(text = { Text("下载") }, onClick = onDownload)
                DropdownMenuItem(text = { Text("转发") }, onClick = onForward)
                DropdownMenuItem(
                    text = { Text("用其他应用打开") },
                    onClick = onOpenAs,
                )
                DropdownMenuItem(text = { Text("查看信息") }, onClick = onInfo)
            }
        }
    }
}

/**
 * 底部操作条(mockup:下载·转发·全屏·信息 均布,灰白 10.5sp 字面 + 字形;
 * 字形 = 单源表直取,无 core 件的走 canonical path)。
 */
@Composable
private fun PreviewBottomBar(
    onDownload: () -> Unit,
    onForward: () -> Unit,
    onOpenAs: () -> Unit,
    onInfo: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(top = 12.dp, bottom = 18.dp),
        horizontalArrangement = Arrangement.SpaceEvenly,
    ) {
        PreviewBarAction(glyph = { ActionGlyph(ActionGlyphKind.DOWNLOAD, Color.White, 20.dp) }, label = "下载", onClick = onDownload)
        PreviewBarAction(glyph = { ActionGlyph(ActionGlyphKind.FORWARD, Color.White, 20.dp) }, label = "转发", onClick = onForward)
        PreviewBarAction(
            glyph = { Icon(Icons.Filled.Share, contentDescription = null, tint = Color.White, modifier = Modifier.size(20.dp)) },
            label = "用其他应用打开",
            onClick = onOpenAs,
        )
        PreviewBarAction(
            glyph = { Icon(Icons.Filled.Info, contentDescription = null, tint = Color.White, modifier = Modifier.size(20.dp)) },
            label = "信息",
            onClick = onInfo,
        )
    }
}

/** 底栏单动作(字形 + 10.5sp 字面;整格触面)。 */
@Composable
private fun PreviewBarAction(
    glyph: @Composable () -> Unit,
    label: String,
    onClick: () -> Unit,
) {
    Column(
        Modifier
            .clickable(onClickLabel = label, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 2.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) { glyph() }
        Text(
            label,
            Modifier.padding(top = 3.dp),
            color = Color(0xFFC9C9D2),
            fontSize = 10.5.sp,
        )
    }
}

/** r62 #6:格式行(iOS typeLine 同源:容器词面 · 大写扩展名)。 */
private fun infoTypeLine(name: String): String {
    val word = when (PreviewRoute.containerOf(name)) {
        PreviewContainer.IMAGE -> "图片"
        PreviewContainer.VIDEO -> "视频"
        PreviewContainer.AUDIO -> "音频"
        PreviewContainer.DOC -> "文档"
        PreviewContainer.UNSUPPORTED -> "文件"
    }
    val ext = name.substringAfterLast('.', "")
    return if (ext.isEmpty()) word else "$word · ${ext.uppercase()}"
}

/** 信息弹层行(名字·大小·路径·设备,brief 钉;标签灰 12sp,值白 13sp)。 */
@Composable
private fun InfoRow(label: String, value: String) {
    Row(Modifier.padding(vertical = 4.dp)) {
        Text(
            label,
            Modifier.width(44.dp),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = 12.sp,
        )
        Text(
            value,
            Modifier.weight(1f),
            fontSize = 13.sp,
            overflow = TextOverflow.Ellipsis,
            maxLines = 3,
        )
    }
}

// ── r62 #5:胶片条(iOS 相册同象)─────────────────────────────────────────

/** 胶片条:兄弟图横向缩略图带(52dp 圆角 6,当前 accent 描边;点选跳图;
 * 当前变更居中跟随)。缩略图 = 浏览同款 PreviewHandlePool + coil(LazyRow
 * 只装可视胞;NSCache 会话内复用)。 */
@Composable
private fun PreviewFilmstrip(
    repo: MtdssRepository,
    deviceId: String?,
    isRemote: Boolean,
    images: List<DirEntry>,
    currentPath: String,
    modifier: Modifier = Modifier,
    onSelect: (DirEntry) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val pool = remember(deviceId) {
        PreviewHandlePool(
            open = { entry ->
                repo.previewOpen(
                    deviceId = if (isRemote) deviceId else null,
                    path = entry.path,
                    mtimeMs = entry.mtimeMs,
                    size = entry.size,
                )
            },
            close = { handle -> repo.previewClose(handle) },
        )
    }
    DisposableEffect(Unit) {
        onDispose { scope.launch(NonCancellable) { pool.releaseAll() } }
    }
    val listState = rememberLazyListState()
    LaunchedEffect(currentPath) {
        val idx = images.indexOfFirst { it.path == currentPath }
        if (idx >= 0) listState.animateScrollToItem(idx)
    }
    LazyRow(
        state = listState,
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        itemsIndexed(images, key = { _, e -> e.path }) { _, entry ->
            FilmstripCell(entry, pool, scope, entry.path == currentPath) { onSelect(entry) }
        }
    }
}

/** 单胞:缩略图 52×52(coil 流式;失败 = 图片图标打底)+ 当前描边。 */
@Composable
private fun FilmstripCell(
    entry: DirEntry,
    pool: PreviewHandlePool,
    scope: CoroutineScope,
    isCurrent: Boolean,
    onClick: () -> Unit,
) {
    var url by remember(entry.path) { mutableStateOf<String?>(null) }
    var loaded by remember(entry.path) { mutableStateOf(false) }
    DisposableEffect(entry.path) {
        val job = scope.launch { url = pool.urlFor(entry) }
        onDispose {
            job.cancel()
            scope.launch(NonCancellable) { pool.release(entry) }
        }
    }
    Box(
        Modifier
            .size(52.dp)
            .clip(RoundedCornerShape(6.dp))
            .background(Color(0xFF2A2A30))
            .clickable(onClickLabel = "查看", onClick = onClick),
    ) {
        url?.let { u ->
            AsyncImage(
                model = ImageRequest.Builder(LocalContext.current)
                    .data(u)
                    .diskCachePolicy(CachePolicy.DISABLED)
                    .build(),
                contentDescription = null,
                modifier = Modifier.matchParentSize(),
                contentScale = ContentScale.Crop,
                onSuccess = { loaded = true },
            )
        }
        if (!loaded) {
            Box(Modifier.matchParentSize(), contentAlignment = Alignment.Center) {
                Icon(Icons.Filled.Image, contentDescription = null, tint = Color(0xFF5A5A66), modifier = Modifier.size(20.dp))
            }
        }
        if (isCurrent) {
            Box(
                Modifier
                    .matchParentSize()
                    .border(2.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(6.dp)),
            )
        }
    }
}


// ── r63 #2:媒体海报面(iOS r49 同象)──────────────────────────────────────

/** 海报面:视频 = 缩略图铺底 + 中央播放钮;音频 = 播放钮卡(黑底)。点播放
 * 钮/画面任意处起播(onPlay)。 */
@Composable
private fun MediaPosterFace(
    name: String,
    isVideo: Boolean,
    thumbUrl: String?,
    modifier: Modifier = Modifier,
    onPlay: () -> Unit,
) {
    Box(
        modifier
            .background(Color(0xFF111114))
            .clickable(onClickLabel = "播放", onClick = onPlay),
        contentAlignment = Alignment.Center,
    ) {
        if (isVideo && thumbUrl != null) {
            coil.compose.AsyncImage(
                model = coil.request.ImageRequest.Builder(LocalContext.current)
                    .data(thumbUrl)
                    .diskCachePolicy(coil.request.CachePolicy.DISABLED)
                    .build(),
                contentDescription = null,
                modifier = Modifier.matchParentSize(),
                contentScale = androidx.compose.ui.layout.ContentScale.Fit,
            )
        }
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Box(
                Modifier
                    .size(68.dp)
                    .clip(androidx.compose.foundation.shape.CircleShape)
                    .background(Color.Black.copy(alpha = 0.45f)),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    androidx.compose.material.icons.Icons.Filled.PlayArrow,
                    contentDescription = "播放",
                    tint = Color.White,
                    modifier = Modifier.size(40.dp),
                )
            }
            if (!isVideo) {
                Spacer(Modifier.height(10.dp))
                Text(
                    name,
                    color = Color.White.copy(alpha = 0.85f),
                    fontSize = 14.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(horizontal = 24.dp),
                )
                Text("点按播放", color = Color.White.copy(alpha = 0.5f), fontSize = 11.sp)
            }
        }
    }
}
