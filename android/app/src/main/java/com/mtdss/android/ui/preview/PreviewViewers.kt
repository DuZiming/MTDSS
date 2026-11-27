package com.mtdss.android.ui.preview

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import kotlin.math.abs
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import coil.compose.AsyncImage
import coil.request.CachePolicy
import coil.request.ImageRequest
import com.mtdss.android.ipc.DirEntry
import com.mtdss.android.ipc.PreviewOpenResult
import com.mtdss.android.ui.browse.BrowseThumbs
import com.mtdss.android.ui.common.FileTypeIcon
import com.mtdss.android.ui.common.MockGlyph

// ── 四容器(V4-1 T23「四容器一分区」;钦定 mockup mobile-preview.html)──────
// image = Compose 自研 zoom 查看器(thumb 先显 → raw 渐进,HEIC = jpeg 端点
// 优先);video/audio = Media3 ExoPlayer 原流直连(raw?t=,移动端原生解码
// 不经转码——spec 裁定,标准控制器不另出图);doc(pdf/office/md/文本)=
// WebView 宿主页 + previewer bundle(boot 经 JS 注入);unsupported = 类型
// 页 + 下载引导。真机解码/WebView 真渲染留冒烟。

/** 预览黑底(mockup mobile-preview Android 列画面区 #111114)。 */
internal val PreviewBlack = Color(0xFF111114)

/** 图片查看器双指缩放上限(mockup 确认点③只钉交互,上限取常规 8x)。 */
internal const val IMAGE_MAX_ZOOM = 8f

/** 全屏字形(canonical Material fullscreen 24dp filled;core 集无此件,
 *  底栏动作条「⤢ 全屏」字形槽用)。 */
internal const val FULLSCREEN_PATH =
    "M7 14H5v5h5v-2H7v-3zm-2-4h2V7h3V5H5v5zm12 7h-3v2h5v-5h-2v3zM14 5v2h3v3h2V5h-5z"

/**
 * 图片查看器(Compose 自研 zoom;mockup Android 列):黑底居中等比,
 * 双指缩放(1x–8x)+ 放大拖拽(1x 复位);thumb 先显(回环缩略图端点,
 * 快速首帧)→ 主图渐进(jpeg/raw 端点,[PreviewRoute.imageFaces] 序:
 * HEIC = jpeg 优先,raw 兜底);全面失败 = 字面 + 重试。
 */
@Composable
fun PreviewImageViewer(
    entry: DirEntry,
    handle: PreviewOpenResult,
    modifier: Modifier = Modifier,
    /** r60 #3:未放大态横向滑切(delta = ±1;iOS 相册同象——放大态拖拽
     * 归平移,不触发)。 */
    onSwipe: ((Int) -> Unit)? = null,
    /** r62 #5:点按切沉浸(静止点按;null = 无面)。 */
    onTap: (() -> Unit)? = null,
) {
    val context = LocalContext.current
    val thumbUrl = BrowseThumbs.thumbUrl(handle.port, handle.handle, handle.token)
    val faces = remember(entry.name) { PreviewRoute.imageFaces(entry.name) }
    var faceIndex by remember(entry.path, handle.handle) { mutableIntStateOf(0) }
    var retryNonce by remember(entry.path, handle.handle) { mutableIntStateOf(0) }
    val faceExhausted = faceIndex >= faces.size
    val faceUrl = when (faces.getOrNull(faceIndex)) {
        "jpeg" -> PreviewRoute.jpegUrl(handle.port, handle.handle, handle.token)
        "raw" -> PreviewRoute.rawUrl(handle.port, handle.handle, handle.token)
        else -> null
    }
    // 缩放/拖拽态(切图/重试即复位;mockup 确认点③双指缩放+拖拽)
    var scale by remember(faceUrl, retryNonce) { mutableFloatStateOf(1f) }
    var offset by remember(faceUrl, retryNonce) { mutableStateOf(Offset.Zero) }

    Box(
        modifier
            .fillMaxSize()
            .background(PreviewBlack)
            .clipToBounds()
            .pointerInput(faceUrl, retryNonce, onSwipe) {
                // r60 #3:统一手势面——单指 ∧ 未放大 = 横滑切图(消费阻断
                // 变换);双指或放大态 = 缩放/平移(detectTransformGestures
                // 语义);单指 ∧ 未放大 ∧ 纵向分量主导 = 不消费(外层
                // 垂直滚动手势可用)。
                var swipeAccum = 0f
                awaitEachGesture {
                    var zooming = false
                    var panning = false
                    val down = awaitFirstDown(requireUnconsumed = false)
                    val downPos = down.position
                    var lastPos = downPos
                    while (true) {
                        val event = awaitPointerEvent()
                        event.changes.firstOrNull()?.let { lastPos = it.position }
                        val pressedCount = event.changes.count { it.pressed }
                        if (pressedCount == 0) break
                        if (pressedCount >= 2) zooming = true
                        if (zooming || scale > 1f) {
                            panning = true
                            val zoomChange = event.calculateZoom()
                            val panChange = event.calculatePan()
                            scale = (scale * zoomChange).coerceIn(1f, IMAGE_MAX_ZOOM)
                            offset = if (scale > 1f) offset + panChange else Offset.Zero
                            event.changes.forEach { it.consume() }
                        } else if (pressedCount == 1) {
                            val change = event.changes.first()
                            val dx = change.positionChange().x
                            if (abs(swipeAccum) > 8f || abs(dx) > abs(change.positionChange().y)) {
                                swipeAccum += dx
                                change.consume()
                            }
                        }
                    }
                    if (!zooming && !panning && scale <= 1f && onSwipe != null && abs(swipeAccum) > 72f) {
                        onSwipe(if (swipeAccum < 0f) 1 else -1)
                    }
                    swipeAccum = 0f
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        // 变换层:thumb 底 + 主图顶同动(thumb 恒在 = 快速首帧;主图成功后
        // 渐进覆盖,加载中不空屏)
        Box(
            Modifier
                .fillMaxSize()
                .graphicsLayer {
                    scaleX = scale
                    scaleY = scale
                    translationX = offset.x
                    translationY = offset.y
                },
            contentAlignment = Alignment.Center,
        ) {
            AsyncImage(
                model = ImageRequest.Builder(context)
                    .data(thumbUrl)
                    // handle URL 短命(离场即 close),不落盘(T22 ThumbCore 同口径)
                    .diskCachePolicy(CachePolicy.DISABLED)
                    .build(),
                contentDescription = null,
                modifier = Modifier.fillMaxSize(),
                contentScale = ContentScale.Fit,
            )
            if (faceUrl != null) {
                AsyncImage(
                    model = ImageRequest.Builder(context)
                        .data(faceUrl)
                        .diskCachePolicy(CachePolicy.DISABLED)
                        // 内存缓存键翻新 = 重试面(同 URL 重拉,Coil 失败不缓存)
                        .setParameter("retry", retryNonce)
                        .build(),
                    contentDescription = null,
                    modifier = Modifier.fillMaxSize(),
                    contentScale = ContentScale.Fit,
                    onError = {
                        // 逐面降级:jpeg 失败落 raw;全面耗尽 = 失败字面(faceIndex
                        // 越 faces.size → faceExhausted)
                        faceIndex += 1
                    },
                )
            }
        }
        // 失败面(全面耗尽):字面 + 重试(重试 = nonce 翻新全链重拉)
        if (faceExhausted) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text(
                    "图片加载失败",
                    color = Color.White.copy(alpha = 0.8f),
                    fontSize = 14.sp,
                    textAlign = TextAlign.Center,
                )
                TextButton(onClick = {
                    faceIndex = 0
                    retryNonce += 1
                }) { Text("重试") }
            }
        }
    }
}

/**
 * 媒体查看器(视频/音频;Media3 ExoPlayer 原流直连 raw?t=,移动端原生
 * 解码不经转码——spec 裁定;mockup:系统级全屏播放器标准控制器,不另出
 * 图)。错误 = 失败字面 + 下载引导 + 重试(整链重建)。
 */
@Composable
fun PreviewMediaViewer(
    url: String,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    var nonce by remember(url) { mutableIntStateOf(0) }
    var failed by remember(url, nonce) { mutableStateOf(false) }
    key(nonce) {
        val player = remember(url, nonce) { ExoPlayer.Builder(context).build() }
        val lifecycleOwner = LocalLifecycleOwner.current
        DisposableEffect(player, lifecycleOwner) {
            val listener = object : Player.Listener {
                override fun onPlayerError(error: PlaybackException) {
                    failed = true
                }
            }
            player.addListener(listener)
            // 生命周期接线(review fix round 1 Important #3):ON_PAUSE/ON_STOP
            // 即暂停(音频不随 Home 背景续播;pause 为必需面,回前台不自动续
            // ——用户手点播放;playWhenReady 翻 false 幂等)。
            val lifecycleObserver = LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_PAUSE, Lifecycle.Event.ON_STOP -> player.pause()
                    else -> Unit
                }
            }
            lifecycleOwner.lifecycle.addObserver(lifecycleObserver)
            onDispose {
                lifecycleOwner.lifecycle.removeObserver(lifecycleObserver)
                player.removeListener(listener)
                player.release()
            }
        }
        LaunchedEffect(player, url) {
            player.setMediaItem(MediaItem.fromUri(url))
            player.prepare()
            player.playWhenReady = true
        }
        if (failed) {
            Box(modifier.fillMaxSize().background(PreviewBlack), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(
                        "无法播放此格式",
                        color = Color.White.copy(alpha = 0.8f),
                        fontSize = 14.sp,
                        textAlign = TextAlign.Center,
                    )
                    Text(
                        "可下载到收件箱后用其他应用打开",
                        Modifier.padding(top = 6.dp),
                        color = Color(0xFF9A9AA5),
                        fontSize = 12.sp,
                    )
                    TextButton(onClick = { nonce += 1 }) { Text("重试") }
                }
            }
        } else {
            AndroidView(
                factory = { ctx ->
                    PlayerView(ctx).apply {
                        this.player = player
                        useController = true // 标准控制器(mockup:不另出图)
                    }
                },
                modifier = modifier.fillMaxSize().background(Color.Black),
                update = { view -> view.player = player },
            )
        }
    }
}

/**
 * 文档容器(WebView;pdf/office/md/文本 = previewer bundle 真渲染):宿主
 * 页 = assets/previewer/preview.html(模块脚本载同目录 previewer.js),
 * boot(`{req, handle}` 形,[PreviewBoot] 单点装配)于 onPageFinished 经
 * evaluateJavascript 注入;JS 开关 + file URL 全域访问(bundle 内 fetch
 * 直连回环口不带头,`?t=` 查询令牌凭据,T20 面;file:// 同源豁免 = 回环
 * fetch 唯一无 CORS 头通路)。真渲染留冒烟。
 */
@Composable
fun PreviewDocViewer(
    injectionJs: String,
    modifier: Modifier = Modifier,
) {
    val lifecycleOwner = LocalLifecycleOwner.current
    var webRef by remember { mutableStateOf<android.webkit.WebView?>(null) }
    AndroidView(
        factory = { ctx ->
            android.webkit.WebView(ctx).apply {
                settings.javaScriptEnabled = true // JS 开关(brief 钉)
                settings.domStorageEnabled = true
                settings.allowFileAccess = true
                // file:// 页 fetch 回环口不触 CORS(bundle 不带头;宿主/资产
                // 全为本 App 自产内容,无不可信 web 面)
                settings.allowUniversalAccessFromFileURLs = true
                settings.allowFileAccessFromFileURLs = true
                webViewClient = object : android.webkit.WebViewClient() {
                    // 导航守卫(review fix round 1 Important #2;特权 WebView
                    // 锁面):渲染面外链点击不得把 file://+全域访问的特权
                    // WebView 导去外部内容——仅 file(资产)放行,http(s)
                    // 交系统浏览器,其余 scheme 拒([PreviewRoute.linkNavigable])。
                    override fun shouldOverrideUrlLoading(
                        view: android.webkit.WebView,
                        request: android.webkit.WebResourceRequest,
                    ): Boolean {
                        val url = request.url.toString()
                        if (PreviewRoute.linkNavigable(url)) return false
                        if (url.startsWith("http://") || url.startsWith("https://")) {
                            runCatching {
                                ctx.startActivity(
                                    android.content.Intent(
                                        android.content.Intent.ACTION_VIEW,
                                        android.net.Uri.parse(url),
                                    ),
                                )
                            }
                        }
                        return true
                    }

                    override fun onPageFinished(view: android.webkit.WebView, url: String?) {
                        // boot 注入单点([PreviewBoot.injectionJs])
                        view.evaluateJavascript(injectionJs, null)
                    }
                }
                loadUrl(PreviewActivity.HOST_PAGE_URL)
            }.also { webRef = it }
        },
        modifier = modifier.fillMaxSize().background(Color.Black),
        // 卸载即销毁(review fix round 1 Important #1):doc 切换/旋转不再
        // 泄漏 WebView(Activity context + Chromium 栈);stopLoading 先断
        // 在途加载再 destroy(销毁后实例不可复用,release 即终态)。
        onRelease = { it.stopLoading(); it.destroy() },
    )
    webRef?.let { web ->
        DisposableEffect(web, lifecycleOwner) {
            // 生命周期接线(与媒体容器同面):后台暂停计时器/渲染面,
            // 回前台恢复(ON_PAUSE 先于 ON_STOP,双向幂等)。
            val lifecycleObserver = LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_PAUSE -> web.onPause()
                    Lifecycle.Event.ON_RESUME -> web.onResume()
                    else -> Unit
                }
            }
            lifecycleOwner.lifecycle.addObserver(lifecycleObserver)
            onDispose { lifecycleOwner.lifecycle.removeObserver(lifecycleObserver) }
        }
    }
}

/**
 * 不支持格式类型页(previewer unsupported.ts 同词面:「该格式暂未支持」;
 * 下载引导 = 原生下载动作替代桌面下载链接)。
 */
@Composable
fun PreviewUnsupportedFace(
    entry: DirEntry,
    onDownload: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(modifier.fillMaxSize().background(PreviewBlack), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            FileTypeIcon(isDir = false)
            Text(
                entry.name,
                Modifier.padding(top = 12.dp, start = 24.dp, end = 24.dp),
                color = Color.White,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
            )
            Text(
                "该格式暂未支持",
                Modifier.padding(top = 6.dp),
                color = Color(0xFF9A9AA5),
                fontSize = 13.sp,
            )
            Text(
                "可下载到收件箱后查看",
                Modifier.padding(top = 2.dp),
                color = Color(0xFF9A9AA5),
                fontSize = 12.sp,
            )
            TextButton(onClick = onDownload) { Text("下载") }
        }
    }
}

/** 打开失败面(降级文案 [PreviewErrors.userFace] + 重试)。 */
@Composable
fun PreviewErrorFace(
    message: String,
    onRetry: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(modifier.fillMaxSize().background(PreviewBlack), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                message,
                Modifier.padding(start = 24.dp, end = 24.dp),
                color = Color.White.copy(alpha = 0.85f),
                fontSize = 14.sp,
                textAlign = TextAlign.Center,
            )
            TextButton(onClick = onRetry) { Text("重试") }
        }
    }
}

/** 加载面(句柄开启在途)。 */
@Composable
fun PreviewLoadingFace(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize().background(PreviewBlack), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(color = Color.White.copy(alpha = 0.7f))
    }
}

/**
 * 页点(mockup Android 列底部:当前 = 16×5 圆角白条,余 = 5dp 圆点白
 * 40%;全尺寸 mockup 值)。图库 > 20 枚不渲点(横向溢出面;‹› 箭头照常),
 * 调用侧收口。
 */
@Composable
fun PreviewPageDots(
    count: Int,
    active: Int,
    modifier: Modifier = Modifier,
) {
    if (count <= 0) return
    Row(
        modifier,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        repeat(count) { i ->
            if (i == active) {
                Box(
                    Modifier
                        .size(width = 16.dp, height = 5.dp)
                        .background(Color.White, RoundedCornerShape(3.dp)),
                )
            } else {
                Box(
                    Modifier
                        .size(5.dp)
                        .background(Color.White.copy(alpha = 0.4f), CircleShape),
                )
            }
        }
    }
}

