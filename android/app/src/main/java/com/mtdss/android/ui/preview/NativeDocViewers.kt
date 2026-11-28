package com.mtdss.android.ui.preview

import android.graphics.Bitmap
import android.graphics.pdf.PdfRenderer
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.nio.charset.Charset

/**
 * r70i:安卓原生文档渲染器(WebView 逃生门)。
 *
 * 背景:MIX3 实机(Chromium 83 WebView + 老 Adreno + MIUI kernel 4.9)上
 * WebView 内容面**物理屏不上屏**(截图/录屏走软件重合成有内容、物理屏恒
 * 黑;顶底 Compose 栏正常),且会话内退化到连截图都无帧(渲染器/GPU 面
 * 渐坏);LAYER_TYPE_SOFTWARE/HARDWARE、activity 关硬件加速三开关全试无
 * 效——WebView 栈在这类设备上不可救药,文档预览改**原生渲染**:
 * - 文本族(text/md/ini/json/code…):HttpURLConnection Range 取数 →
 *   BOM 嗅探解码 → Compose 等宽文本 + 「加载更多」续窗(64KB/窗);
 * - pdf:全量下载到 cache → 系统 [PdfRenderer] 逐页位图(适配宽度,
 *   首批 3 页 + 「查看更多」续批)——零第三方依赖,任何设备必上屏;
 * - office(docx/xlsx/pptx):仍走 WebView(r70i 注:现代设备正常;老设备
 *   可用「用其他应用打开」兜底)。
 * iOS/桌面不受影响(各保持 web previewer 路径)。
 */

// ── 取数底座(loopback 直读; HttpURLConnection 零依赖) ─────────────────────

/** 全量下载 raw 到 cache 文件(进度 0..1;失败上抛词面)。 */
internal suspend fun downloadRawToFile(
    rawUrl: String,
    dest: File,
    onProgress: (Float) -> Unit,
): File = withContext(Dispatchers.IO) {
    val conn = URL(rawUrl).openConnection() as HttpURLConnection
    try {
        if (conn.responseCode !in 200..299) throw IllegalStateException("HTTP ${conn.responseCode}")
        val total = conn.contentLengthLong
        conn.inputStream.use { input ->
            FileOutputStream(dest).use { out ->
                val buf = ByteArray(64 * 1024)
                var done = 0L
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    done += n
                    if (total > 0) onProgress(done.toFloat() / total)
                }
            }
        }
        dest
    } finally {
        conn.disconnect()
    }
}

/** Range 取一窗字节(null = 读尽/失败词面由调用面处理)。 */
internal suspend fun fetchRange(rawUrl: String, begin: Long, end: Long): ByteArray? =
    withContext(Dispatchers.IO) {
        runCatching {
            val conn = URL(rawUrl).openConnection() as HttpURLConnection
            try {
                conn.setRequestProperty("Range", "bytes=$begin-$end")
                if (conn.responseCode !in 200..299) return@runCatching null
                conn.inputStream.readBytes()
            } finally {
                conn.disconnect()
            }
        }.getOrNull()
    }

/** BOM 嗅探解码(UTF-8/UTF-16LE/BE;无 BOM 按 UTF-8 宽容解码)。 */
internal fun decodeWithBom(bytes: ByteArray): String = when {
    bytes.size >= 3 && bytes[0] == 0xEF.toByte() && bytes[1] == 0xBB.toByte() && bytes[2] == 0xBF.toByte() ->
        String(bytes, 3, bytes.size - 3, Charsets.UTF_8)
    bytes.size >= 2 && bytes[0] == 0xFF.toByte() && bytes[1] == 0xFE.toByte() ->
        String(bytes, 2, bytes.size - 2, Charsets.UTF_16LE)
    bytes.size >= 2 && bytes[0] == 0xFE.toByte() && bytes[1] == 0xFF.toByte() ->
        String(bytes, 2, bytes.size - 2, Charsets.UTF_16BE)
    else -> String(bytes, Charsets.UTF_8)
}

// ── 文本查看器 ────────────────────────────────────────────────────────────

private const val TEXT_WINDOW_BYTES = 64 * 1024

/** 原生文本视图:头部一窗 + 「加载更多」续窗(Range 断点,64KB/窗)。 */
@Composable
internal fun NativeTextViewer(
    rawUrl: String,
    size: Long,
    modifier: Modifier = Modifier,
) {
    var content by remember(rawUrl) { mutableStateOf("") }
    var received by remember(rawUrl) { mutableStateOf(0L) }
    var loading by remember(rawUrl) { mutableStateOf(true) }
    var error by remember(rawUrl) { mutableStateOf<String?>(null) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()

    suspend fun loadWindow() {
        if (loading) return
        loading = true
        val begin = received
        val window = if (begin == 0L) TEXT_WINDOW_BYTES * 3L else TEXT_WINDOW_BYTES.toLong()
        val chunk = fetchRange(rawUrl, begin, begin + window - 1)
        if (chunk == null || chunk.isEmpty()) {
            if (begin == 0L) error = "文本加载失败"
        } else {
            content += decodeWithBom(chunk)
            received += chunk.size
        }
        loading = false
    }

    fun retry() {
        error = null
        scope.launch { loadWindow() }
    }

    LaunchedEffect(rawUrl) { loadWindow() }

    Column(modifier.fillMaxSize()) {
        if (error != null) {
            ErrorFace(error!!) { retry() }
        } else {
            Text(
                content,
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                fontFamily = FontFamily.Monospace,
                fontSize = 13.sp,
                lineHeight = 20.sp,
                color = MaterialTheme.colorScheme.onSurface,
            )
            val done = size > 0 && received >= size
            if (!done) {
                Box(Modifier.fillMaxWidth().padding(vertical = 10.dp), contentAlignment = Alignment.Center) {
                    Button(onClick = { scope.launch { loadWindow() } }, enabled = !loading) {
                        Text(if (loading) "加载中…" else "加载更多")
                    }
                }
            }
        }
    }
}

// ── PDF 查看器(系统 PdfRenderer) ────────────────────────────────────────

private const val PDF_FIRST_BATCH = 3

/** 原生 pdf 视图:下载到 cache → PdfRenderer 逐页位图(适配宽,3 页/批)。 */
@Composable
internal fun NativePdfViewer(
    rawUrl: String,
    cacheDir: File,
    modifier: Modifier = Modifier,
) {
    var file by remember(rawUrl) { mutableStateOf<File?>(null) }
    var progress by remember(rawUrl) { mutableStateOf(0f) }
    var dlError by remember(rawUrl) { mutableStateOf<String?>(null) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()

    fun startDownload() {
        dlError = null
        scope.launch {
            runCatching {
                val dest = File(cacheDir, "preview-${rawUrl.hashCode()}.pdf")
                downloadRawToFile(rawUrl, dest) { p -> progress = p }
                file = dest
            }.onFailure { dlError = it.message ?: "下载失败" }
        }
    }

    LaunchedEffect(rawUrl) { startDownload() }

    when {
        dlError != null -> ErrorFace(dlError!!) { startDownload() }
        file == null -> Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                CircularProgressIndicator()
                Text(
                    "下载中 ${(progress * 100).toInt()}%",
                    color = androidx.compose.ui.graphics.Color.White.copy(alpha = 0.7f),
                    fontSize = 12.sp,
                )
            }
        }
        else -> PdfPages(file!!, modifier)
    }
}

@Composable
private fun PdfPages(file: File, modifier: Modifier = Modifier) {
    var pageCount by remember(file) { mutableIntStateOf(0) }
    var openError by remember(file) { mutableStateOf<String?>(null) }
    var rendered by remember(file) { mutableIntStateOf(0) }

    // 渲染器单例(整文件一枚;PdfRenderer 与其 fd 同生命周期)
    var renderer by remember(file) { mutableStateOf<PdfRenderer?>(null) }
    DisposableEffect(file) {
        val fd = android.os.ParcelFileDescriptor.open(
            file,
            android.os.ParcelFileDescriptor.MODE_READ_ONLY,
        )
        val r = PdfRenderer(fd)
        renderer = r
        pageCount = r.pageCount
        rendered = minOf(PDF_FIRST_BATCH, r.pageCount)
        onDispose {
            runCatching { r.close() }
        }
    }

    if (openError != null) {
        ErrorFace(openError!!) { openError = null }
        return
    }

    val density = LocalDensity.current
    LazyColumn(
        modifier.fillMaxSize(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items((0 until rendered).toList()) { index ->
            PageBitmap(renderer, index, density.density) { errMsg -> openError = errMsg }
        }
        if (rendered < pageCount) {
            item(key = "more") {
                Box(Modifier.fillMaxWidth().padding(vertical = 10.dp), contentAlignment = Alignment.Center) {
                    Button(onClick = { rendered = minOf(rendered + PDF_FIRST_BATCH, pageCount) }) {
                        Text("查看更多(${rendered}/$pageCount 页)")
                    }
                }
            }
        }
    }
}

@Composable
private fun PageBitmap(
    renderer: PdfRenderer?,
    index: Int,
    density: Float,
    onError: (String) -> Unit,
) {
    // 目标宽 = 屏宽(像素);PdfRenderer 按页原始宽高等比到目标宽渲染
    val screenWidthPx = with(LocalDensity.current) {
        androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp.dp.toPx()
    }
    var bitmap by remember(index) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(renderer, index) {
        val r = renderer ?: return@LaunchedEffect
        bitmap = withContext(Dispatchers.IO) {
            runCatching {
                r.openPage(index).use { page ->
                    val scale = screenWidthPx / page.width
                    val w = (page.width * scale).toInt().coerceAtLeast(1)
                    val h = (page.height * scale).toInt().coerceAtLeast(1)
                    val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
                    bmp.eraseColor(android.graphics.Color.WHITE)
                    page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                    bmp
                }
            }.getOrElse { null }
        }
        if (bitmap == null) onError("第 ${index + 1} 页渲染失败")
    }
    val bmp = bitmap
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        if (bmp == null) {
            CircularProgressIndicator(Modifier.padding(24.dp))
        } else {
            androidx.compose.foundation.Image(
                bitmap = bmp.asImageBitmap(),
                contentDescription = "第 ${index + 1} 页",
                modifier = Modifier.fillMaxWidth(),
                contentScale = ContentScale.FillWidth,
            )
        }
    }
}

@Composable
private fun ErrorFace(message: String, onRetry: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(message, color = androidx.compose.ui.graphics.Color.White, fontSize = 14.sp)
        Button(onClick = onRetry, modifier = Modifier.padding(top = 12.dp)) {
            Text("重试")
        }
    }
}
