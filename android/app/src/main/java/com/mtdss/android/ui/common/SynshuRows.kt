package com.mtdss.android.ui.common

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.EaseInOut
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import java.util.Calendar
import kotlin.math.roundToInt

/**
 * 文件 App 式浏览行组件库(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §2.2 规格值;iOS app/MTDSS/Browse/BrowseView.swift BrowseEntryRow 参数面
 * 与解剖镜像;字形 = 视觉稿 06-detail-screens.html 同款 SVG path)。
 *
 * 供浏览/快捷目录/设备等列表任务消费(计划 T4 名面钉死):
 * `entrySubtitle`(行副文格式器,iOS BrowseFormat 逐字镜像)、`SynshuRow`(行)、
 * `ExpandChevron`(展开钮)、`FileTypeIcon`(行图标)、`SelectionCircle`(行首选圈);
 * `SwipeRevealRow`(行左滑自绘皮肤,T7 浏览行/T8 传输行共用);
 * `Hairline`(发丝分割线,T16 台账 T11 起 Profile/Settings 共用件)。
 */

// ── 行副文格式器(iOS MTDSSKit BrowseFormat.subtitle 逐字镜像)───────────────

/** 副文段分隔符(iOS 逐字面:`parts.joined(separator: " - ")`)。 */
private const val SUBTITLE_SEPARATOR = " - "

/**
 * 行副文(iOS MTDSSKit BrowseFormat.subtitle 结构与措辞逐字镜像;spec §2.3
 * 「中文文案与 iOS 端逐字一致」为准,V3-4 视觉稿日期词为示意渲染非裁定):
 *
 * - 日期段:`yyyy-MM-dd`(本地时区,零填充);mtime 0 = daemon 无 mtime
 *   语义 → 段剔除(iOS BrowseFormat.date 同口径);
 * - 目录行([isDir] = true):children 已知 →「N 项」段;children 未知
 *   (契约 null,目录 stat 未及)→ 退化纯日期(iOS childCount nil 同构);
 * - 文件行([isDir] = false):大小段**恒附加**(0 字节 = "0 B",不退化),
 *   人类可读化复用 [Format.size](1024 进制一位小数不剪尾,与 iOS
 *   BrowseFormat.size / 桌面 ui/src/lib/browse.ts 双端口径同源),不重复实现;
 * - 全空 = 空串(行渲染层隐藏副文,iOS `!sub.isEmpty` 同构)。
 *
 * [isDir] 为 T4 评审 fix round 1 增补面:计划钉死的 (mtimeMs,children,size)
 * 三参无法表达「文件 size 恒附加」与「目录 children 未知退化」并存,消费端
 * (T7/T10)自 DirEntry.isDir 直传。
 */
fun entrySubtitle(mtimeMs: Long, children: Int?, size: Long, isDir: Boolean): String {
    val parts = ArrayList<String>(2)
    dateText(mtimeMs)?.let { parts.add(it) }
    if (isDir) {
        if (children != null) {
            parts.add("$children 项")
        }
    } else {
        parts.add(Format.size(size, isDir = false))
    }
    return parts.joinToString(SUBTITLE_SEPARATOR)
}

/** mtime → `yyyy-MM-dd`(本地时区,零填充);0 → null(daemon 无 mtime 语义)。 */
private fun dateText(mtimeMs: Long): String? {
    if (mtimeMs == 0L) return null
    val cal = Calendar.getInstance().apply { timeInMillis = mtimeMs }
    return String.format(
        java.util.Locale.ROOT,
        "%04d-%02d-%02d",
        cal.get(Calendar.YEAR),
        cal.get(Calendar.MONTH) + 1,
        cal.get(Calendar.DAY_OF_MONTH),
    )
}

// ── 行规格常量(spec §2.2 逐值)──────────────────────────────────────────────

/** 行高 58dp。 */
private val RowHeight = 58.dp
/** 行首分割线内缩 20dp(树级随缩进顺延)。 */
private val SeparatorLeadingInset = 20.dp
/** 发丝线 0.5dp。 */
private val SeparatorThickness = 0.5.dp
/** 缩进 18dp/级。 */
private val IndentPerLevel = 18.dp
/** 选圈列固定宽:非选择 20dp → 选择 56dp。 */
private val SelectColumnNarrow = 20.dp
private val SelectColumnWide = 56.dp
/** 选圈列行首内缩 20dp。 */
private val CircleLeadingInset = 20.dp
/** 行内展开钮布局占位 12dp;触区外扩 28×28dp。 */
private val ChevronPlaceholder = 12.dp
private val ChevronHitArea = 28.dp
/** 行内容竖列间距(iOS HStack spacing 12)。 */
private val RowInnerSpacing = 12.dp
/** 行尾内缩 20dp。 */
private val RowTrailingInset = 20.dp
/** 按下行内容变暗至 60%(spec §2.2;iOS PressableRowStyle 同值)。 */
private const val PressedAlpha = 0.6f
/** 展开旋转时长(0.2s easeInOut,spec §2.2)。 */
private const val ExpandRotationMs = 200

/** 行名 17sp / 副文 12.5sp(spec §2.2 字面值;不随环境 Typography 漂移)。 */
private val RowNameSize = 17.sp
private val RowSubSize = 12.5.sp

// ── 行组件 ──────────────────────────────────────────────────────────────────

/**
 * 文件 App 通栏浏览行(iOS BrowseEntryRow 参数面镜像):选圈列固定宽
 * (非选择 20 → 选择 56,圈钉行首固定缘不随缩进漂移)、展开钮在图标左侧
 * (› 展开旋转 90°;文件行占位不可点)、目录蓝实心夹/文件灰问号文档、
 * 名 17sp 副文 12.5sp 各一行截断、发丝分割线行首内缩 20dp+缩进、按下行
 * 内容变暗 60%、选中 = 行 `#F2F2F7`(深色白 6%)底。
 *
 * 长按 0.4s 进选择模式与左滑动作由列表宿主挂接(iOS 同构:外挂
 * simultaneousGesture/NativeSwipeActions,行本体保持哑件)。
 *
 * @param name    条目名(一行截断)
 * @param isDir   目录行(蓝夹 + 可展开)/文件行(灰问号文档 + 占位)
 * @param depth   缩进层级(行内展开;根级 0)
 * @param expanded 目录行展开态(› 旋转 ∨)
 * @param selecting 选择模式(选圈列展开)
 * @param selected  已选(蓝底白勾 + 行底高亮)
 * @param onOpen   行点按(iOS:选择态 = 翻选,非选择 = 目录推入/文件预览)
 * @param onToggleExpand 目录行的箭头点按 = 行内展开;null = 无展开
 *   (文件行占位;目录未挂展开时呈静态箭头,iOS 同构)
 * @param sub      行副文([entrySubtitle] 产物;null/空 = 不渲染副文行)
 */
@Composable
fun SynshuRow(
    name: String,
    isDir: Boolean,
    depth: Int = 0,
    expanded: Boolean = false,
    selecting: Boolean = false,
    selected: Boolean = false,
    onOpen: () -> Unit,
    onToggleExpand: (() -> Unit)? = null,
    sub: String? = null,
) {
    val indent = IndentPerLevel * depth
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .background(if (selected) MaterialTheme.colorScheme.surfaceContainer else Color.Transparent)
            .clickable(interactionSource = interaction, indication = null, onClick = onOpen)
            .semantics { contentDescription = name + if (selected) ",已选" else "" },
    ) {
        // 按下行内容变暗(iOS PressableRowStyle:0.6,背景与分割线不变暗)
        Row(
            Modifier
                .fillMaxSize()
                .graphicsLayer { alpha = if (pressed) PressedAlpha else 1f },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // 选圈列(固定宽;圈钉行首固定缘,不随树缩进漂移)
            Box(
                Modifier.width(if (selecting) SelectColumnWide else SelectColumnNarrow),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (selecting) {
                    SelectionCircle(selected, Modifier.padding(start = CircleLeadingInset))
                }
            }
            Row(
                Modifier.weight(1f),
                horizontalArrangement = Arrangement.spacedBy(RowInnerSpacing),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ExpandChevronSlot(
                    isDir = isDir,
                    expanded = expanded,
                    onToggleExpand = onToggleExpand,
                    modifier = Modifier.padding(start = indent),
                )
                FileTypeIcon(isDir = isDir)
                // 名/副文行距 2dp(iOS BrowseEntryRow VStack spacing 2 同值,G2 度量:
                // 文本块 22+2+16=40,行内上下留白均匀不挤压)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(
                        name,
                        color = MaterialTheme.colorScheme.onSurface,
                        fontSize = RowNameSize,
                        lineHeight = 22.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (!sub.isNullOrEmpty()) {
                        Text(
                            sub,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            fontSize = RowSubSize,
                            lineHeight = 16.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
            // 行尾内缩(iOS .padding(.trailing, 20))
            Spacer(Modifier.width(RowTrailingInset))
        }
        // 发丝分割线:0.5dp,行首内缩 20dp + 树缩进,通栏到右缘(iOS FullBleedRow)
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .padding(start = SeparatorLeadingInset + indent)
                .fillMaxWidth()
                .height(SeparatorThickness)
                .background(MaterialTheme.colorScheme.outlineVariant),
        )
    }
}

/**
 * 展开钮槽位(iOS chevron 同构):目录挂展开回调 = 28dp 触区(负偏移还原
 * 12dp 布局占位);目录无回调 = 静态箭头;文件 = 12dp 透明占位对齐名称列。
 */
@Composable
private fun ExpandChevronSlot(
    isDir: Boolean,
    expanded: Boolean,
    onToggleExpand: (() -> Unit)?,
    modifier: Modifier = Modifier,
) {
    when {
        isDir && onToggleExpand != null -> {
            // 布局占位 12dp;触区外扩 28×28(负偏移还原,iOS padding(8)+(-8) 同构)
            Box(modifier.width(ChevronPlaceholder)) {
                Box(
                    Modifier
                        .offset(x = -(ChevronHitArea - ChevronPlaceholder) / 2)
                        .size(ChevronHitArea)
                        .clip(CircleShape)
                        .clickable(onClickLabel = if (expanded) "收起" else "展开", onClick = onToggleExpand),
                    contentAlignment = Alignment.Center,
                ) {
                    ExpandChevron(expanded)
                }
            }
        }
        isDir -> ExpandChevron(expanded, modifier)
        else -> Box(modifier.width(ChevronPlaceholder)) // 文件行:透明占位(iOS opacity 0)
    }
}

/**
 * 展开钮字形:› 14sp semibold 蓝,展开旋转 90° 成 ∨(0.2s easeInOut,
 * spec §2.2;视觉稿 xchev 同款 24 视口 stroke 2.6 圆头折线)。
 */
@Composable
fun ExpandChevron(expanded: Boolean, modifier: Modifier = Modifier) {
    val rotation by animateFloatAsState(
        targetValue = if (expanded) 90f else 0f,
        animationSpec = tween(durationMillis = ExpandRotationMs, easing = EaseInOut),
        label = "expandChevron",
    )
    MockGlyph(
        pathData = SynshuGlyphs.CHEVRON,
        size = 14.dp,
        strokeWidth = 2.6f,
        color = MaterialTheme.colorScheme.primary,
        modifier = modifier.graphicsLayer { rotationZ = rotation },
    )
}

/**
 * 行图标(spec §2.2/iOS FileTypeIcon:统一两枚,不按类型区分——用户钦定):
 * 目录 = 蓝实心文件夹 23dp(30dp 槽);文件 = 灰线框文档+问号 21dp。
 */
@Composable
fun FileTypeIcon(isDir: Boolean, modifier: Modifier = Modifier) {
    Box(modifier.width(30.dp), contentAlignment = Alignment.Center) {
        if (isDir) {
            MockGlyph(
                pathData = SynshuGlyphs.FOLDER,
                size = 23.dp,
                fill = true,
                color = MaterialTheme.colorScheme.primary,
            )
        } else {
            DocGlyph(tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** 问号文档(iOS FileTypeIcon 文件形:doc 线框 + 小问号 offset y 3)。 */
@Composable
private fun DocGlyph(tint: Color, modifier: Modifier = Modifier) {
    Box(modifier.size(21.dp), contentAlignment = Alignment.Center) {
        MockGlyph(pathData = DOC_BODY_PATH, size = 21.dp, strokeWidth = 1.6f, color = tint)
        MockGlyph(pathData = DOC_FOLD_PATH, size = 21.dp, strokeWidth = 1.6f, color = tint)
        Text("?", color = tint, fontSize = 9.sp, fontWeight = FontWeight.Bold, modifier = Modifier.offset(y = 3.dp))
    }
}

/**
 * 行首选圈(spec §2.2/iOS selectionCircle:24dp,1.5dp #C7C7CC 描边;
 * 选中 = 蓝底白勾 13dp,白勾 xck 同款 stroke 2.6 折线;未选中白底,iOS
 * `.background(Color.white, in: Circle())` 逐字镜像)。
 */
@Composable
fun SelectionCircle(selected: Boolean, modifier: Modifier = Modifier) {
    Box(modifier.size(24.dp), contentAlignment = Alignment.Center) {
        if (selected) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(MaterialTheme.colorScheme.primary, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                MockGlyph(pathData = CHECK_PATH, size = 13.dp, strokeWidth = 2.6f, color = Color.White)
            }
        } else {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color.White, CircleShape)
                    .border(1.5.dp, MaterialTheme.colorScheme.outline, CircleShape),
            )
        }
    }
}

// ── 行左滑自绘皮肤(spec §2.2「左滑动作」;T7 浏览行/T8 传输行共用)──────────

/** 单钮宽 68dp(iOS 原生 swipeActions 钮宽同量级);白图标 18dp(spec §2.2)。 */
private val SwipeButtonWidth = 68.dp

/**
 * 左滑动作钮规格(iOS SwipeActionSpec 同名同构:字形 + 底色 + 词面;词面
 * 挂 a11y onClickLabel)。字形 = 调用侧 [@Composable] 槽(白图标由调用侧
 * 传入底色——浏览行经 ActionGlyph 复用底栏字形,传输行自携 xmark/回环)。
 */
data class SwipeActionSpec(
    val label: String,
    val tint: Color,
    val glyph: @Composable () -> Unit,
)

/**
 * 左滑揭示行(自绘皮肤,零新增依赖;T7 浏览页私件提取共用,T8 传输列表
 * 复用同皮肤):水平拖拽揭示尾缘动作簇,**无全滑**(松手落最近锚点 =
 * 揭示/回弹,永不触发动作——逐钮确认);拖拽与列表纵向滚动天然分轴
 * (横向 drag LazyColumn 不消费,iOS 的 pan 争用问题在 Compose 分轴判定
 * 下不成立);揭示态点内容先合左滑(iOS 同:首点只合不复触行)。
 * 动作集空 = 直通(不挂手势,iOS 空集无滑面同构)。动作簇贴尾缘,
 * **簇内末位贴缘**(spec §2.2「破坏钮贴缘」;iOS reversed 声明的有效
 * 视觉 = 数组末位最贴右缘,同构)。**内容层自带不透明底**(页面 surface,
 * 随浅深)+ 满铺宽——消费面内容保持透明底也不会透出动作簇(R1 F2)。
 *
 * @param actions 动作规格簇(顺序自左向右,末位贴尾缘)
 * @param onAction 动作回调(参数 = [actions] 下标)
 */
@Composable
fun SwipeRevealRow(
    actions: List<SwipeActionSpec>,
    onAction: (Int) -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    if (actions.isEmpty()) {
        Box(modifier) { content() }
        return
    }
    val density = LocalDensity.current
    val revealWidthPx = with(density) { (SwipeButtonWidth * actions.size).toPx() }
    val offsetX = remember(actions.size) { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val settled = offsetX.value != 0f

    Box(modifier.fillMaxWidth()) {
        // 背景:动作簇贴尾缘(破坏面钮 = 簇内末位贴缘,spec §2.2;iOS reversed
        // 声明的有效视觉 = 数组末位最贴右缘,同构)
        Row(Modifier.matchParentSize()) {
            Spacer(Modifier.weight(1f))
            actions.forEachIndexed { i, spec ->
                Box(
                    Modifier
                        .width(SwipeButtonWidth)
                        .fillMaxHeight()
                        .background(spec.tint)
                        .clickable(onClickLabel = spec.label) {
                            onAction(i)
                            scope.launch { offsetX.animateTo(0f, tween(200)) }
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    spec.glyph()
                }
            }
        }
        // 内容行(偏移揭示;横向拖拽自绘,纵向滚动让列表;拖拽只触揭示锚点,
        // 不持动作闭包——键 = 揭示宽,动作簇数变化才重挂)。**内容层不透明
        // 满铺**(R1 F2:底色 = 页面 surface 随浅深,宽度恒满——否则未滑时
        // 尾缘动作簇自底下透出/窄内容尾缘外露;background 在 offset 之后 =
        // 随内容一起滑,揭示位才露钮)
        Box(
            Modifier
                .offset { IntOffset(offsetX.value.roundToInt(), 0) }
                .fillMaxWidth()
                .background(MaterialTheme.colorScheme.surface)
                .pointerInput(revealWidthPx) {
                    detectHorizontalDragGestures(
                        onHorizontalDrag = { change, dragAmount ->
                            change.consume()
                            scope.launch {
                                offsetX.snapTo((offsetX.value + dragAmount).coerceIn(-revealWidthPx, 0f))
                            }
                        },
                        onDragEnd = {
                            // 无全滑:松手落最近锚点(揭示/回弹),不触发动作
                            val target = if (offsetX.value < -revealWidthPx / 2f) -revealWidthPx else 0f
                            scope.launch { offsetX.animateTo(target, tween(200)) }
                        },
                        onDragCancel = {
                            scope.launch { offsetX.animateTo(0f, tween(200)) }
                        },
                    )
                },
        ) {
            content()
            // 揭示态透明拦截层:点内容先合左滑(iOS 同构;不复触行点按)
            if (settled) {
                Box(
                    Modifier
                        .matchParentSize()
                        .pointerInput(Unit) {
                            detectTapGestures {
                                scope.launch { offsetX.animateTo(0f, tween(200)) }
                            }
                        },
                )
            }
        }
    }
}

// ── 字形渲染件(视觉稿 06-detail-screens SVG path 直用)────────────────────

/** 行/栏共用的字形 path(视觉稿 06-detail-screens.html 逐字面,24 视口)。 */
internal object SynshuGlyphs {
    /** 展开箭头 ›(xchev:stroke 2.6 圆头折线;旋转 90° = 展开态)。 */
    const val CHEVRON = "M9.5 5l7 7-7 7"
    /** 蓝实心文件夹(xfolder,fill)。 */
    const val FOLDER = "M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8l-2-2z"
}

/** 问号文档(doc 线框 + 折角;「?」文字层见 [DocGlyph])。 */
private const val DOC_BODY_PATH = "M6 2.5h7l5 5V20a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 5 20V4a1.5 1.5 0 0 1 1-1.5z"
private const val DOC_FOLD_PATH = "M13 2.5l5 5h-5z"
/** 选圈白勾(xck:stroke 2.6 白)。 */
private const val CHECK_PATH = "M6 12.5l4 4 8-9"

/**
 * 视觉稿 SVG path 渲染件(24 视口缩放至 [size];[fill] = 填充形,
 * 否则 [strokeWidth] 为视口单位的圆头描边)。字形零第三方依赖
 * (material-icons-core 无 Folder/SF 系,icons-extended 禁新增)。
 */
@Composable
internal fun MockGlyph(
    pathData: String,
    size: Dp,
    color: Color,
    modifier: Modifier = Modifier,
    fill: Boolean = false,
    strokeWidth: Float = 1.8f,
) {
    val path = remember(pathData) { PathParser().parsePathString(pathData).toPath() }
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension / 24f
        scale(s, s, pivot = Offset.Zero) {
            if (fill) {
                drawPath(path, color)
            } else {
                drawPath(
                    path,
                    color,
                    style = Stroke(width = strokeWidth * s, cap = StrokeCap.Round, join = StrokeJoin.Round),
                )
            }
        }
    }
}

// ── 发丝分割线(T16 台账 T11:Profile/Settings 两套私件合一)────────────────

/**
 * 发丝分割线(0.5dp 发丝;leading 内缩,通栏到右缘;iOS FullBleedRow
 * 分隔线同构)。T16 台账 T11 起 = Profile/Settings 共用件(原两文件
 * 各持一套私件,渲染同形,合一收口)。
 *
 * @param leadingInset 左端内缩(分组行首对齐;iOS separator inset 同语义)
 */
@Composable
internal fun Hairline(leadingInset: Dp) {
    HorizontalDivider(
        modifier = Modifier.padding(start = leadingInset),
        thickness = 0.5.dp,
        color = MaterialTheme.colorScheme.outlineVariant,
    )
}
