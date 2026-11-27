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
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Info
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
        val handle = (handleUi as? HandleUi.Open)?.result ?: return
        scope.launch {
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
                    val intent = Intent(Intent.ACTION_VIEW).apply {
                        setDataAndType(uri, PreviewRoute.mimeOf(current.name))
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                    try {
                        context.startActivity(intent)
                    } catch (e: ActivityNotFoundException) {
                        actionError = "未找到可打开此文件的应用"
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
                    onBack = { (context as? ComponentActivity)?.finish() },
                    menuOpen = menuOpen,
                    onMenuToggle = { menuOpen = !menuOpen },
                    onMenuDismiss = { menuOpen = false },
                    openAsEnabled = handleUi is HandleUi.Open,
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
                            h is HandleUi.Open -> PreviewImageViewer(current, h.result, Modifier.fillMaxSize(), onSwipe = ::stepImage)
                            h is HandleUi.Failed -> PreviewErrorFace(h.face, { retryNonce += 1 }, Modifier.fillMaxSize())
                            else -> PreviewLoadingFace(Modifier.fillMaxSize())
                        }
                        if (fullscreen) {
                            // 全屏:任意点按回操作条(系统返回同效)
                            Box(
                                Modifier
                                    .matchParentSize()
                                    .pointerInput(Unit) {
                                        detectTapGestures(onTap = { fullscreen = false })
                                    },
                            )
                        } else if (images.size in 1..20) {
                            // 页点限 20 枚内(r60 #3:‹› 箭头随撤——横滑
                            // 切换,页点保留)
                            PreviewPageDots(
                                count = images.size,
                                active = if (imageIndex < 0) 0 else imageIndex,
                                modifier = Modifier
                                    .align(Alignment.BottomCenter)
                                    .padding(bottom = 14.dp),
                            )
                        }
                    }

                    PreviewContainer.VIDEO, PreviewContainer.AUDIO -> {
                        val h = handleUi
                        // r60 #3:中央横滑切上一个/下一个(iOS 同象;兄弟类
                        // 集环绕)——累计位移超阈即切,consume 防串控制器
                        var swipeAccum by remember { mutableFloatStateOf(0f) }
                        val onSwipe: (Int) -> Unit = { delta ->
                            if (container == PreviewContainer.VIDEO) stepVideo(delta) else stepAudio(delta)
                        }
                        Box(
                            Modifier
                                .fillMaxSize()
                                .pointerInput(container) {
                                    detectHorizontalDragGestures(
                                        onHorizontalDrag = { change, amount ->
                                            swipeAccum += amount
                                            change.consume()
                                        },
                                        onDragEnd = {
                                            if (abs(swipeAccum) > 72f) {
                                                onSwipe(if (swipeAccum < 0f) 1 else -1)
                                            }
                                            swipeAccum = 0f
                                        },
                                    )
                                },
                        ) {
                        when {
                            h is HandleUi.Open -> PreviewMediaViewer(
                                PreviewRoute.rawUrl(h.result.port, h.result.handle, h.result.token),
                                Modifier.fillMaxSize(),
                            )
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
            // 底部操作条(仅图片容器,mockup;全屏中藏)
            if (!fullscreen && container == PreviewContainer.IMAGE) {
                PreviewBottomBar(
                    onDownload = ::doDownload,
                    onForward = { showForward = true },
                    onFullscreen = { fullscreen = true },
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
        AlertDialog(
            onDismissRequest = { showInfo = false },
            title = { Text("信息") },
            text = {
                Column {
                    InfoRow("名字", current.name)
                    InfoRow("大小", Format.size(current.size, isDir = false))
                    InfoRow("路径", current.path)
                    InfoRow("设备", deviceLabel)
                }
            },
            confirmButton = {
                TextButton(onClick = { showInfo = false }) { Text("好") }
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
            ) {
                DropdownMenuItem(text = { Text("下载") }, onClick = onDownload)
                DropdownMenuItem(text = { Text("转发") }, onClick = onForward)
                DropdownMenuItem(
                    text = { Text("用其他应用打开") },
                    enabled = openAsEnabled,
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
    onFullscreen: () -> Unit,
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
            glyph = { VectorGlyph(AppIcons.previewFullscreen, 20.dp, Color.White) },
            label = "全屏",
            onClick = onFullscreen,
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
