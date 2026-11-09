package com.mtdss.android.ui.profile

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.lifecycle.awaitInstance
import androidx.camera.view.PreviewView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.mtdss.android.ui.common.MockGlyph
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * 扫码登录取景页(V3-4 T12,spec §3-9/§4.9;iOS ScanSheet 同任):
 * - 相机 = CameraX 预览 + ZXing QR-only 解析(纯 core,无 GMS 依赖);
 * - 权限:进入页请求(notDetermined → requestPermission);拒绝 → 指引文案
 *   + 「前往设置」(spec §4.9 指路设置;照片权限同款 spirit);
 * - 命中链路:QR 原文交 [QrLoginCode] 纯解析——命中本家形态即回调 token
 *   (claim/approve 授权闭环由调用方 ProfileScreen 经 [QrApprovalModel]
 *   接线,本页不受影响);非本家码保持扫描,行内提示;
 * - 全屏深色底(#16171C 系)+ 居中取景框 246dp r12(框外 92% 压暗、框内
 *   全亮)+ 四角蓝角标(#4E9AFF)+ 提示词沉底。
 */

/** 扫码登录二维码内容的纯解析面(iOS Kit QrLoginCode 语义镜像;无 IO):
 * 二维码内容 = daemon 拼 `https://www.mtdss.top/qr?t=<token>`(login.qr_begin
 * 契约,服务端不参与)。扫得任意字符串交 [parse]:命中本家 URL 形态 → 取
 * t 参数为 qr token;其余(网页链接/文本/他家二维码)→ null,调用方给
 * 「非 Synshu 登录码」行内提示续扫。token 交 [QrApprovalModel] 走
 * claim/approve 授权闭环。 */
object QrLoginCode {

    /** 二维码 URL 固定前缀(daemon account.rs qr_url 同字面)。 */
    private const val PREFIX = "https://www.mtdss.top/qr?t="

    /** 从扫描原文提取 qr token:严格匹配前缀(大小写敏感,防钓鱼变体),
     * token = 其后首个查询参数值(URL 编码不解——daemon 生成的 token 为
     * URL 安全字母表,原样直用)。空白首尾容忍;空 token → null。 */
    fun parse(raw: String): String? {
        val text = raw.trim()
        if (!text.startsWith(PREFIX)) return null
        val rest = text.removePrefix(PREFIX)
        // t 为首个查询参数;若后面还拼了别的参数(&)只取本段
        val token = rest.split("&", limit = 2).first()
        return token.ifEmpty { null }
    }
}

/** 取景框边长(spec/计划钉值 246dp)。 */
private val FrameSide = 246.dp
/** 取景框圆角(r12)。 */
private val FrameCorner = 12.dp
/** 蓝角标臂长与线宽。 */
private val BracketArm = 26.dp
private val BracketStroke = 4.dp

/** 深色底(#16171C 系;框外压暗同色)。 */
private val ScanBackground = Color(0xFF16171C)
/** 框外压暗(iOS black.opacity(0.92) 同档)。 */
private val ScanScrim = Color(0xFF16171C).copy(alpha = 0.92f)
/** 蓝角标(深色档强调蓝)。 */
private val BracketBlue = Color(0xFF4E9AFF)
/** 取景页标题/提示白(深色档主文字)。 */
private val ScanTitle = Color(0xFFE5E6EB)
private val ScanHint = Color.White.copy(alpha = 0.75f)
/** 行内提示橙(非本家码;iOS .orange 同象)。 */
private val ScanWarn = Color(0xFFFF9F0A)

/** 待取景提示词(计划钉面,两行)。 */
private const val HINT_STANDBY = "对准电脑上 Synshu 显示的登录码\n授权后电脑将直接登录"
/** 非本家码行内提示(iOS ScanSheet 词面照抄)。 */
private const val HINT_FOREIGN = "非 Synshu 登录码,请对准 PC 端「扫码登录」二维码"

@Composable
internal fun ScanSheet(
    onClose: () -> Unit,
    onCode: (String) -> Unit,
) {
    val context = LocalContext.current
    // 相机授权态(null = 请求中;false = 拒绝/不再询问 → 指路设置)
    var permission by remember { mutableStateOf<Boolean?>(null) }
    var missHint by remember { mutableStateOf<String?>(null) }
    val handled = remember { AtomicBoolean(false) }

    val permissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted -> permission = granted }

    LaunchedEffect(Unit) {
        val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED
        if (granted) {
            permission = true
        } else {
            permissionLauncher.launch(Manifest.permission.CAMERA)
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(ScanBackground)
            .statusBarsPadding()
            .navigationBarsPadding(),
    ) {
        // 顶栏(‹ 关闭 + 标题绝对居中;iOS ProfileNavBar(title:"扫码登录") 同构)
        Box(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 18.dp)
                .padding(top = 10.dp, bottom = 8.dp)
                .height(34.dp),
        ) {
            Text(
                "扫码登录",
                Modifier.align(Alignment.Center),
                color = ScanTitle,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
            )
            Box(
                Modifier
                    .align(Alignment.CenterStart)
                    .size(width = 34.dp, height = 34.dp)
                    .clickable(onClickLabel = "关闭") { onClose() },
                contentAlignment = Alignment.Center,
            ) {
                MockGlyph(
                    pathData = "M14.5 5l-7 7 7 7",
                    size = 16.dp,
                    strokeWidth = 2.8f,
                    color = BracketBlue,
                )
            }
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (permission) {
                true -> {
                    QrCamera(
                        onPayload = { raw ->
                            // 命中本家码即回调离场(父层关 sheet 建模型 claim);
                            // 关闭窗内的重复帧由 handled 拦下
                            val token = QrLoginCode.parse(raw)
                            if (token != null && handled.compareAndSet(false, true)) {
                                onCode(token)
                            } else if (token == null) {
                                missHint = HINT_FOREIGN
                            }
                        },
                        modifier = Modifier.fillMaxSize(),
                    )
                    // 取景框 + 框外压暗 + 四角蓝角标(相机预览之上)
                    ViewfinderOverlay()
                }
                false -> PermissionDenied(onToSettings = { openAppSettings(context) })
                null -> RequestingPermission()
            }
        }

        // 提示行沉底(待取景词 / 非本家码橙色警示;iOS hintRow 同位)
        Text(
            missHint ?: HINT_STANDBY,
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp)
                .padding(vertical = 10.dp)
                .padding(bottom = 8.dp),
            color = if (missHint != null) ScanWarn else ScanHint,
            fontSize = 13.5.sp,
            lineHeight = 19.sp,
            textAlign = TextAlign.Center,
        )
    }
}

/** 取景覆盖件:居中 246dp 框(透明孔)+ 框外压暗 + 四角蓝角标。 */
@Composable
private fun ViewfinderOverlay() {
    // 框位置(相对相机区左上;Canvas 与框同根,相对量可直接用)
    var camRect by remember { mutableStateOf<Rect?>(null) }
    var frameRect by remember { mutableStateOf<Rect?>(null) }

    Column(
        Modifier
            .fillMaxSize()
            .onGloballyPositioned { camRect = it.boundsInRoot() },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.weight(1f))
        Box(
            Modifier
                .size(FrameSide)
                .onGloballyPositioned { frameRect = it.boundsInRoot() },
        )
        Spacer(Modifier.height(26.dp))
        Spacer(Modifier.weight(1.15f))
    }

    Canvas(Modifier.fillMaxSize()) {
        val cam = camRect ?: return@Canvas
        val frame = frameRect ?: return@Canvas
        val rect = Rect(
            offset = Offset(frame.left - cam.left, frame.top - cam.top),
            size = Size(frame.width, frame.height),
        )
        // 框外压暗(偶奇填充:整幅 - 圆角孔)
        val scrim = Path().apply {
            fillType = PathFillType.EvenOdd
            addRect(Rect(Offset.Zero, size))
            addRoundRect(RoundRect(rect, CornerRadius(FrameCorner.toPx(), FrameCorner.toPx())))
        }
        drawPath(scrim, ScanScrim)
        // 四角蓝角标(L 形,圆头)
        val arm = BracketArm.toPx()
        val corners = Path().apply {
            moveTo(rect.left, rect.top + arm)
            lineTo(rect.left, rect.top)
            lineTo(rect.left + arm, rect.top)
            moveTo(rect.right - arm, rect.top)
            lineTo(rect.right, rect.top)
            lineTo(rect.right, rect.top + arm)
            moveTo(rect.right, rect.bottom - arm)
            lineTo(rect.right, rect.bottom)
            lineTo(rect.right - arm, rect.bottom)
            moveTo(rect.left + arm, rect.bottom)
            lineTo(rect.left, rect.bottom)
            lineTo(rect.left, rect.bottom - arm)
        }
        drawPath(
            corners,
            BracketBlue,
            style = Stroke(width = BracketStroke.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round),
        )
    }
}

/** 请求中面(iOS「正在请求相机权限…」词面同构)。 */
@Composable
private fun RequestingPermission() {
    Column(
        Modifier.fillMaxSize(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        CircularProgressIndicator(color = BracketBlue, strokeWidth = 2.dp, modifier = Modifier.size(22.dp))
        Spacer(Modifier.height(12.dp))
        Text("正在请求相机权限…", color = ScanHint, fontSize = 14.sp)
    }
}

/** 权限拒指引(iOS ScanSheet permissionDenied 词面逐字 + 指路设置)。 */
@Composable
private fun PermissionDenied(onToSettings: () -> Unit) {
    Column(
        Modifier
            .fillMaxSize()
            .padding(horizontal = 30.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        MockGlyph(CAMERA_PATH, 44.dp, Color(0xFF8E8E93), strokeWidth = 1.6f)
        Spacer(Modifier.height(14.dp))
        Text(
            "未获得相机权限",
            color = ScanTitle,
            fontSize = 17.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Spacer(Modifier.height(10.dp))
        Text(
            "扫码登录需要使用相机。\n请在系统设置中允许 Synshu 访问相机后重试。",
            color = Color(0xFF8E8E93),
            fontSize = 13.5.sp,
            lineHeight = 19.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(18.dp))
        // 「前往设置」(蓝底白字胶囊;iOS 同形)
        Box(
            Modifier
                .clip(RoundedCornerShape(percent = 50))
                .background(BracketBlue)
                .clickable(onClickLabel = "前往设置", onClick = onToSettings)
                .padding(horizontal = 22.dp, vertical = 10.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text("前往设置", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

/** 系统设置·本应用详情页(spec §4.9 指路设置)。 */
private fun openAppSettings(context: Context) {
    context.startActivity(
        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
            .setData(Uri.fromParts("package", context.packageName, null)),
    )
}

// ── 相机核心(CameraX + ZXing;iOS ScannerCore 同任)─────────────────────────

/**
 * QR 相机面:PreviewView 全幅 + ImageAnalysis(QR-only,KEEP_ONLY_LATEST)
 * 单帧解码交 [onPayload](主线程投递)。绑定/解绑随组合生命周期(息屏/
 * 退出即 unbind,相机释放);命中后由调用方关 sheet,重复帧在 hit 旗后拦下。
 */
@Composable
private fun QrCamera(onPayload: (String) -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val previewView = remember {
        PreviewView(context).apply { scaleType = PreviewView.ScaleType.FILL_CENTER }
    }
    AndroidView(factory = { previewView }, modifier = modifier)

    DisposableEffect(lifecycleOwner) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        val executor = Executors.newSingleThreadExecutor()
        val hit = AtomicBoolean(false)
        val main = Handler(Looper.getMainLooper())
        var provider: ProcessCameraProvider? = null
        val job = scope.launch {
            val p = ProcessCameraProvider.awaitInstance(context)
            provider = p
            if (hit.get()) return@launch // 已离场:不再绑定
            val preview = Preview.Builder().build().also {
                it.setSurfaceProvider(previewView.surfaceProvider)
            }
            val analysis = ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build()
            analysis.setAnalyzer(executor) { proxy ->
                try {
                    val text = decodeQr(proxy)
                    if (text != null && hit.compareAndSet(false, true)) {
                        main.post { onPayload(text) }
                    }
                } finally {
                    proxy.close()
                }
            }
            p.unbindAll()
            p.bindToLifecycle(lifecycleOwner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
        }
        onDispose {
            hit.set(true)
            job.cancel()
            try {
                provider?.unbindAll()
            } catch (_: RuntimeException) {
                // 组合先于绑定完成的竞态:无绑定可解
            }
            executor.shutdown()
            scope.cancel()
        }
    }
}

/** QR 解码提示(只认 QR,契约面唯一形态;iOS metadataObjectTypes=[.qr] 同口径)。 */
private val QR_HINTS: Map<DecodeHintType, Any> = mapOf(
    DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
    DecodeHintType.TRY_HARDER to true,
)

/**
 * ZXing 单帧解码(亮度面直喂;四向旋转重试——传感器朝向不定,竖屏机典型
 * 90° 命中首试,其余向兜底)。无码 = null(分析器续扫)。
 */
private fun decodeQr(proxy: ImageProxy): String? {
    val plane = proxy.planes[0]
    val width = proxy.width
    val height = proxy.height
    val buffer = plane.buffer
    val y = ByteArray(width * height)
    if (plane.rowStride == width) {
        buffer.get(y)
    } else {
        var out = 0
        for (row in 0 until height) {
            buffer.position(row * plane.rowStride)
            buffer.get(y, out, width)
            out += width
        }
        buffer.rewind()
    }
    val rotation = proxy.imageInfo.rotationDegrees
    // 首试标称旋转,再补其余三向(全向兜底,横拍/倒拍不漏)
    val order = listOf(rotation, (rotation + 90) % 360, (rotation + 270) % 360, (rotation + 180) % 360)
    for (deg in order) {
        val (data, w, h) = rotateLuminance(y, width, height, deg)
        val bitmap = BinaryBitmap(
            HybridBinarizer(PlanarYUVLuminanceSource(data, w, h, 0, 0, w, h, false)),
        )
        val text = try {
            MultiFormatReader().decode(bitmap, QR_HINTS).text
        } catch (_: ReaderException) {
            // ReaderException 全族(NotFound/Checksum/Format)= 本向无码或
            // 帧损,续试;不外抛——分析器线程未捕获异常即进程崩溃
            null
        }
        if (text != null) return text
    }
    return null
}

/**
 * 亮度面旋转(deg ∈ {0,90,180,270};90 = 顺时针,270 = 逆时针):dst 宽×
 * 高互换(90/270)。正向映射 dst(x' = h-1-y_src, y' = x_src)〔CW〕/
 * dst(x' = y_src, y' = w-1-x_src)〔CCW〕,行主序重排——**行系数恒取自
 * src 高维**(行 = `* w` 的下标分量须来自 0 until h),否则 w>h 的横置
 * 传感器帧(典型 640×480)即 AIOOBE(修 round 1 回归源;纯 JVM 测试在
 * ScanSheetRotateTest 逐字节锁定)。
 */
internal fun rotateLuminance(src: ByteArray, w: Int, h: Int, deg: Int): Triple<ByteArray, Int, Int> =
    when (((deg % 360) + 360) % 360) {
        90 -> {
            val out = ByteArray(src.size)
            var i = 0
            for (yDst in 0 until w) {
                for (xDst in 0 until h) {
                    out[i++] = src[(h - 1 - xDst) * w + yDst]
                }
            }
            Triple(out, h, w)
        }
        180 -> {
            val out = ByteArray(src.size)
            for (i in src.indices) out[i] = src[src.size - 1 - i]
            Triple(out, w, h)
        }
        270 -> {
            val out = ByteArray(src.size)
            var i = 0
            for (yDst in 0 until w) {
                for (xDst in 0 until h) {
                    out[i++] = src[xDst * w + (w - 1 - yDst)]
                }
            }
            Triple(out, h, w)
        }
        else -> Triple(src, w, h)
    }

/** 相机字形(未授权指引;camera 同象手作,24 视口)。 */
private const val CAMERA_PATH =
    "M3.5 8.5C3.5 7.4 4.4 6.5 5.5 6.5H7.2L8.6 4.5H15.4L16.8 6.5H18.5C19.6 6.5 20.5 7.4 20.5 8.5V16.5C20.5 17.6 19.6 18.5 18.5 18.5H5.5C4.4 18.5 3.5 17.6 3.5 16.5V8.5ZM12 15.2C13.8 15.2 15.2 13.8 15.2 12C15.2 10.2 13.8 8.8 12 8.8C10.2 8.8 8.8 10.2 8.8 12C8.8 13.8 10.2 15.2 12 15.2Z"

/** 相机权限运行时态(ON_RESUME 刷新;ProfileScreen 扫码入口灰显依据,
 *  spec §4.9「拒绝 = 入口置灰」)。 */
@Composable
internal fun rememberCameraGranted(): Boolean {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                granted = ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                    PackageManager.PERMISSION_GRANTED
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    return granted
}
