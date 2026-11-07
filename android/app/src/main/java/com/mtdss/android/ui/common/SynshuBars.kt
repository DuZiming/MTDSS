package com.mtdss.android.ui.common

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mtdss.android.ui.theme.SynshuColors
import com.mtdss.android.ui.theme.SynshuColorsDark
import com.mtdss.android.ui.theme.SynshuSemantic

/**
 * Synshu 顶栏/底栏组件库(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §2.2 规格值;iOS BrowseNav.swift / SelectionBar.swift 解剖逐项镜像;
 * 字形 = 视觉稿 06-detail-screens.html 同款 SVG path,零第三方图标依赖)。
 *
 * 供壳与各屏幕任务消费(计划 T4 名面钉死):`CenterTopBar`(浏览顶栏)、
 * `SelectionTopBar`(选择顶栏)、`SelectionActionBar`(选择底栏动作条)、
 * `CapsuleNavBar`(毛玻璃胶囊底栏)+ `CapsuleTabSpec`/`CapsuleTabGlyph`、
 * `SearchField`(搜索框)。
 *
 * 毛玻璃纪律(spec §2.2):胶囊/动作条玻璃面走 RenderEffect,**API 31+
 * 才半透明+blur;API <31(含 MIX 3/Android 10)退化不透明底**,描边/投影/
 * 布局不变。blur+saturate 的 saturate 半 RenderEffect 无对应,观感票记录。
 */

// ── 字形 path(视觉稿逐字面,24 视口;fill/stroke 见各用点)──────────────────

/** 返回箭头(视觉稿 xback:stroke 2.8 圆头折线;前进为其镜像 chevron)。 */
private const val BACK_PATH = "M14.5 5l-7 7 7 7"
/** 放大镜(视觉稿 xmag:圆 + 手柄,stroke 2)。 */
private const val MAG_PATH = "M4 10.5a6.5 6.5 0 1 0 13 0a6.5 6.5 0 1 0 -13 0M15.5 15.5L21 21"
/** 显示器/设备(视觉稿 xpc;T6 设备行同形复用,故 internal)。 */
internal const val MONITOR_PATH = "M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h7l-2 3v1h8v-1l-2-3h7c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12z"
/** 传输列表双向箭头(视觉稿 xswap)。 */
private const val SWAP_PATH = "M7 4L3 8l4 4v-3h8V7H7V4zm10 8l4 4-4 4v-3H9v-2h8v-3z"
/** 我的(视觉稿 xperson)。 */
private const val PERSON_PATH = "M12 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z"
/** 下载(视觉稿 xdl,实心)。 */
private const val DOWNLOAD_PATH = "M12 3v10.2l3.6-3.6L17 11l-5 5-5-5 1.4-1.4L12 13.2V3zM5 19h14v2H5z"
/** 转发(视觉稿 actbar 同款,实心)。 */
private const val FORWARD_PATH = "M11 4L9.6 5.4l5.6 5.6H3v2h12.2l-5.6 5.6L11 20l8-8zM19 3h2v18h-2z"
/** 确认白勾(视觉稿 xcheck:stroke 3 白;internal = T7 「···」排序勾同形复用)。 */
internal const val CHECK_BOLD_PATH = "M5 12.5l4.5 4.5L19 7"
/** 分享(square.and.arrow.up 同象;视觉稿无此 symbol,同风格手作)。 */
private const val SHARE_UP_PATH = "M12 3v11M8.5 6.5L12 3l3.5 3.5M6.5 9H5.2C4.5 9 4 9.6 4 10.3v9.4C4 20.4 4.6 21 5.3 21h13.4c.7 0 1.3-.6 1.3-1.3v-9.4C20 9.6 19.5 9 18.8 9H17.5"
/** 删除(trash 同象;视觉稿无此 symbol,同风格手作)。 */
private const val TRASH_PATH = "M4 7h16M6.5 7v12.2C6.5 20.2 7.3 21 8.3 21h7.4c1 0 1.8-.8 1.8-1.8V7M9.5 7V5.2C9.5 4.5 10 4 10.7 4h2.6c.7 0 1.2.5 1.2 1.2V7"
/** 快捷 badge 加号(folder.badge.plus 的徽记半边)。 */
private const val FAVORITE_PLUS_PATH = "M17.5 13.5v6M14.5 16.5h6"

/** 毛玻璃 API 门(胶囊/动作条共用;API 31+ RenderEffect,<31 退化不透明)。 */
private val GlassCapable: Boolean
    get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S

/**
 * 毛玻璃玻璃面层(API 31+):宿主容器背景保持透明,由本层以 RenderEffect
 * 模糊的半透明玻璃承担(API <31 不渲染,宿主背景转不透明退化)。
 */
@Composable
private fun GlassLayer(glass: Color, shape: Shape, modifier: Modifier = Modifier) {
    if (GlassCapable) {
        Box(modifier.fillMaxSize().blur(20.dp).background(glass, shape))
    }
}

// ── 浏览顶栏(spec §2.2「顶栏」)─────────────────────────────────────────────

/** 顶栏高 ~48dp(spec §2.2)。 */
private val TopBarHeight = 48.dp
/** 标题两侧净空(iOS .padding(.horizontal, 100):绝对居中防挤压)。 */
private val TitleSideInset = 100.dp
/** 「···」圆钮 34dp,三枚 4dp 蓝点(spec §2.2)。 */
private val DotsButtonSize = 34.dp

/**
 * 「···」菜单项自收口面(CenterTopBar 提供;消费面 = 菜单 [DropdownMenuItem]
 * 的 onClick 先收菜单再执行动作——iOS 同构:点项后菜单即合)。缺省空操作,
 * 供菜单项在 CenterTopBar 外预览/测试。
 */
val LocalTopBarMenuClose = compositionLocalOf { {} }
/** 箭头字形 16dp 粗体(iOS .font(16, .bold));触区 20×30(iOS 同构)。 */
private val ArrowGlyphSize = 16.dp
private val ArrowHitWidth = 20.dp
private val ArrowHitHeight = 30.dp

/**
 * 浏览顶栏模板(iOS BrowseNavBar 参数面/解剖镜像):左 返回/前进双箭头
 * (返回禁用 40% 透明蓝、前进不可用灰 `#C7C7CC`;两侧回调均缺省 = 根页
 * 不显箭头),标题绝对居中 17sp·600 长名**中截断**(头尾可见),右上
 * 「···」圆钮 34dp 弹出 [menuContent](null = 不渲染钮)。
 *
 * @param onBack  返回回调(null = 不显箭头组;iOS showsArrows 同语义。回调
 *   在场即恒可点——浏览根按返回 = 弹回标签根,由回调分流 iOS BrowseNavBar
 *   disabled(onBack == nil) 同构)
 * @param onForward 前进回调
 * @param canBack  返回可用(导航面分流用;箭头可见性/可点性由 onBack 定)
 * @param canForward 前进可用(不可用 = 灰 `#C7C7CC`,spec「不可用灰」)
 * @param menuContent 「···」菜单内容(选择/排序等;null = 隐藏圆钮)
 */
@Composable
fun CenterTopBar(
    title: String,
    onBack: (() -> Unit)? = null,
    onForward: (() -> Unit)? = null,
    canBack: Boolean = false,
    canForward: Boolean = false,
    menuContent: (@Composable ColumnScope.() -> Unit)? = null,
) {
    Box(
        Modifier
            .fillMaxWidth()
            .height(TopBarHeight)
            .padding(horizontal = 18.dp),
    ) {
        // 标题绝对居中:两侧各留箭头+菜单净空(iOS 同构)
        Text(
            title,
            Modifier
                .align(Alignment.Center)
                .fillMaxWidth()
                .padding(horizontal = TitleSideInset),
            color = MaterialTheme.colorScheme.onSurface,
            fontSize = 17.sp,
            fontWeight = FontWeight.SemiBold,
            textAlign = TextAlign.Center,
            maxLines = 1,
            overflow = TextOverflow.MiddleEllipsis,
        )
        if (onBack != null || onForward != null) {
            Row(
                Modifier.align(Alignment.CenterStart),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                TopBarArrow(
                    pathData = BACK_PATH,
                    strokeWidth = 2.8f,
                    // 回调在场即可点(iOS BrowseNavBar disabled(onBack == nil)
                    // 同构;T7:浏览根按返回 = 弹回标签根,canBack 只定
                    // goBack-or-pop,由回调侧分流——40% 禁用形仅回调缺失,
                    // 而缺失时箭头组整组不渲染,本面恒全色可点)
                    enabled = true,
                    tint = MaterialTheme.colorScheme.primary,
                    onClick = onBack,
                    label = "返回",
                )
                TopBarArrow(
                    pathData = SynshuGlyphs.CHEVRON,
                    strokeWidth = 2.6f,
                    enabled = canForward,
                    tint = if (canForward) MaterialTheme.colorScheme.primary else SynshuColors.StrokeGray,
                    onClick = onForward,
                    label = "前进",
                )
            }
        }
        if (menuContent != null) {
            var menuOpen by remember { mutableStateOf(false) }
            Box(
                Modifier
                    .align(Alignment.CenterEnd)
                    .size(DotsButtonSize)
                    .clip(CircleShape)
                    .clickable(onClickLabel = "更多") { menuOpen = true },
                contentAlignment = Alignment.Center,
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(3.dp)) {
                    repeat(3) {
                        Box(Modifier.size(4.dp).background(MaterialTheme.colorScheme.primary, CircleShape))
                    }
                }
                CompositionLocalProvider(LocalTopBarMenuClose provides { menuOpen = false }) {
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }, content = menuContent)
                }
            }
        }
    }
}

/** 顶栏箭头钮(iOS:20×30 触区、16dp 粗圆头折线;不可用灰显不可点)。 */
@Composable
private fun TopBarArrow(
    pathData: String,
    strokeWidth: Float,
    enabled: Boolean,
    tint: Color,
    onClick: (() -> Unit)?,
    label: String,
) {
    Box(
        Modifier
            .size(ArrowHitWidth, ArrowHitHeight)
            .clickable(enabled = enabled, onClickLabel = label) { onClick?.invoke() },
        contentAlignment = Alignment.Center,
    ) {
        MockGlyph(pathData = pathData, size = ArrowGlyphSize, strokeWidth = strokeWidth, color = tint)
    }
}

// ── 选择顶栏(spec §2.2;iOS SelectionNavBar)────────────────────────────────

/** 确认钮 34dp 蓝圆白勾(粗),投影 accent 35%(spec §2.2)。 */
private val ConfirmButtonSize = 34.dp

/**
 * 选择模式顶栏(替换常规浏览顶栏):左「全选」(蓝 17sp)/ 居中「N 项」/
 * 右 34dp 蓝圆白勾确认(带 accent 35% 投影)。确认 = 退出选择模式
 * (无论选否,无副作用——语义收口在宿主;iOS Ruling O 同构)。
 *
 * @param count 已选计数(标题「N 项」)
 */
@Composable
fun SelectionTopBar(
    count: Int,
    onSelectAll: () -> Unit,
    onConfirm: () -> Unit,
) {
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = TopBarHeight)
            .padding(start = 18.dp, end = 18.dp, top = 10.dp, bottom = 8.dp),
    ) {
        Text(
            "$count 项",
            Modifier.align(Alignment.Center),
            color = MaterialTheme.colorScheme.onSurface,
            fontSize = 17.sp,
            maxLines = 1,
        )
        Text(
            "全选",
            Modifier
                .align(Alignment.CenterStart)
                .clickable(onClickLabel = "全选", onClick = onSelectAll),
            color = MaterialTheme.colorScheme.primary,
            fontSize = 17.sp,
        )
        Box(
            Modifier
                .align(Alignment.CenterEnd)
                .size(ConfirmButtonSize)
                .shadow(
                    6.dp,
                    CircleShape,
                    clip = false,
                    ambientColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.35f),
                    spotColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.35f),
                )
                .background(MaterialTheme.colorScheme.primary, CircleShape)
                .clickable(onClickLabel = "确认", onClick = onConfirm),
            contentAlignment = Alignment.Center,
        ) {
            MockGlyph(pathData = CHECK_BOLD_PATH, size = 15.dp, strokeWidth = 3f, color = Color.White)
        }
    }
}

// ── 选择底栏动作条(spec §2.2「选择底栏」;iOS SelectionActionBar)────────────

/** 选择底栏动作集(iOS SelectionBar.ActionKind 同序;出集门控在宿主/Kit)。 */
enum class SynshuSelectionAction(val label: String) {
    /** 分享(本机)。 */
    Share("分享"),
    /** 删除(红)。 */
    Delete("删除"),
    /** 下载(远端)。 */
    Download("下载"),
    /** 转发(远端)。 */
    Forward("转发"),
    /** 快捷(纯目录多选,V3-3)。 */
    Favorite("快捷"),
}

/** 左动作图标 22dp,钮触区 34dp,间距 26dp(iOS 同构);右圆点钮 38dp。 */
private val ActionGlyphSize = 22.dp
private val ActionHitSize = 34.dp
private val ActionSpacing = 26.dp
private val MoreCircleSize = 38.dp
/** 选择底栏玻璃底(视觉稿 .actbar:白 .85;深色族内推导)。 */
private val ActBarGlassLight = Color(0xFFFFFFFF).copy(alpha = 0.85f)
private val ActBarGlassDark = Color(0xFF1C1E28).copy(alpha = 0.85f)

/**
 * 选择模式底部动作条:毛玻璃 + 顶部发丝线;左功能图标组 22dp(间距 26dp,
 * 删除红/其余蓝)+ 右「圆点更多」38dp 圆(`#F2F2F7` 底,无菜单时灰显禁点)。
 *
 * @param actions 动作出集(顺序即自左向右;Kit/宿主按浏览档与删除门出集)
 * @param onAction 动作回调
 * @param moreMenu 「圆点更多」菜单内容(null = 无菜单,灰显禁点)
 */
@Composable
fun SelectionActionBar(
    actions: List<SynshuSelectionAction>,
    onAction: (SynshuSelectionAction) -> Unit,
    moreMenu: (@Composable ColumnScope.() -> Unit)? = null,
) {
    val dark = isSystemInDarkTheme()
    val glass = if (dark) ActBarGlassDark else ActBarGlassLight
    val opaque = if (dark) SynshuColorsDark.CapsuleOpaque else SynshuColors.CapsuleOpaque
    Box(
        Modifier
            .fillMaxWidth()
            .background(if (GlassCapable) Color.Transparent else opaque, RectangleShape),
    ) {
        GlassLayer(glass, RectangleShape)
        // 顶部发丝线(spec「毛玻璃 + 顶部发丝线」;视觉稿 border-top 同值)
        Box(
            Modifier
                .fillMaxWidth()
                .height(0.5.dp)
                .background(MaterialTheme.colorScheme.outlineVariant),
        )
        Row(
            Modifier
                .fillMaxWidth()
                .padding(start = 24.dp, end = 24.dp, top = 6.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(ActionSpacing),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                actions.forEach { action ->
                    Box(
                        Modifier
                            .size(ActionHitSize)
                            .clickable(onClickLabel = action.label) { onAction(action) },
                        contentAlignment = Alignment.Center,
                    ) {
                        ActionGlyph(action, tint = actionTint(action))
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            MoreCircle(enabled = moreMenu != null, content = moreMenu)
        }
    }
}

/** 删除 = 红色(语义「失败」恒定值);其余蓝(iOS tint 同构)。 */
@Composable
private fun actionTint(action: SynshuSelectionAction): Color =
    if (action == SynshuSelectionAction.Delete) SynshuSemantic.Fail else MaterialTheme.colorScheme.primary

/** 动作字形(下载/转发 = 视觉稿 xdl/xfwd 直用;分享/删除/快捷同风格;
 * internal = 浏览行左滑动作簇同形复用,T7——iOS 左滑/底栏同 SF symbol 同构)。 */
@Composable
internal fun ActionGlyph(action: SynshuSelectionAction, tint: Color) {
    when (action) {
        SynshuSelectionAction.Share -> MockGlyph(SHARE_UP_PATH, ActionGlyphSize, tint, strokeWidth = 1.8f)
        SynshuSelectionAction.Delete -> MockGlyph(TRASH_PATH, ActionGlyphSize, tint, strokeWidth = 1.8f)
        SynshuSelectionAction.Download -> MockGlyph(DOWNLOAD_PATH, ActionGlyphSize, tint, fill = true)
        SynshuSelectionAction.Forward -> MockGlyph(FORWARD_PATH, ActionGlyphSize, tint, fill = true)
        SynshuSelectionAction.Favorite -> Box {
            MockGlyph(SynshuGlyphs.FOLDER, ActionGlyphSize, tint, strokeWidth = 1.8f)
            MockGlyph(
                FAVORITE_PLUS_PATH,
                12.dp,
                tint,
                Modifier.align(Alignment.BottomEnd).offset(x = 2.dp, y = (-2).dp),
                strokeWidth = 1.8f,
            )
        }
    }
}

/** 右「圆点更多」钮(iOS moreCircle:38dp 圆底,三枚 4.5dp 点;无菜单灰显禁点)。 */
@Composable
private fun MoreCircle(enabled: Boolean, content: (@Composable ColumnScope.() -> Unit)?) {
    var open by remember { mutableStateOf(false) }
    Box(
        Modifier
            .size(MoreCircleSize)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.surfaceContainer, CircleShape)
            .clickable(enabled = enabled, onClickLabel = "更多") { open = true },
        contentAlignment = Alignment.Center,
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(3.dp)) {
            repeat(3) {
                Box(
                    Modifier.size(4.5.dp).background(
                        if (enabled) MaterialTheme.colorScheme.primary else SynshuColors.StrokeGray,
                        CircleShape,
                    ),
                )
            }
        }
        if (content != null) {
            DropdownMenu(expanded = open, onDismissRequest = { open = false }, content = content)
        }
    }
}

// ── 胶囊底栏(spec §2.2「胶囊底栏」)─────────────────────────────────────────

/** 胶囊页签图标形(spec §3-1 四标签;字形 = 视觉稿 01/06 同款)。 */
enum class CapsuleTabGlyph { FAVORITES, DEVICES, TRANSFERS, PROFILE }

/** 胶囊页签 spec(词面 + 图标形;T5 AppTab 映射面,枚举序 = 四标签序)。 */
data class CapsuleTabSpec(val key: String, val label: String, val glyph: CapsuleTabGlyph)

/** 胶囊悬浮边距 16dp(左/右/下)、圆角 30(spec §2.2)。 */
private val CapsuleMargin = 16.dp
private val CapsuleRadius = 30.dp
/** 页签胶囊底 64×34dp(圆角 17)、图标 24dp、标签 11sp(spec §2.2)。 */
private val TabPillWidth = 64.dp
private val TabPillHeight = 34.dp
private val TabPillRadius = 17.dp
private val TabIconSize = 24.dp
private val TabLabelSize = 11.sp
private val TabRowGap = 3.dp

/**
 * 毛玻璃胶囊底栏:悬浮(左右/下 16dp)、圆角 30、毛玻璃(白 72%/深 74%,
 * API 31+ blur;<31 退化不透明底,描边/投影/布局不变)、1dp 描边、投影;
 * 页签 = 图标 24dp(胶囊底 64×34dp)+ 标签 11sp,选中 = 蓝胶囊底
 * (视觉稿 .capsule/.ci:accent 13% 胶囊底 + 蓝字 600,未选中灰)。
 *
 * 自带悬浮边距(宿主直接对齐屏幕底部放置;选择模式/扫码 sheet 期间由壳藏栏)。
 *
 * @param tabs 页签 spec 集(顺序 = 底栏自左向右)
 * @param active 选中页签下标
 * @param onTab 页签点按回调(参数 = 下标)
 */
@Composable
fun CapsuleNavBar(
    tabs: List<CapsuleTabSpec>,
    active: Int,
    onTab: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val dark = isSystemInDarkTheme()
    val glass = if (dark) SynshuColorsDark.CapsuleGlass else SynshuColors.CapsuleGlass
    val opaque = if (dark) SynshuColorsDark.CapsuleOpaque else SynshuColors.CapsuleOpaque
    // 视觉稿 .capsule:border rgba(0,0,0,.08)/shadow 0 10px 32px rgba(18,22,40,.14)
    val border = if (dark) SynshuColorsDark.Hairline else Color(0xFF000000).copy(alpha = 0.08f)
    val capsuleShadow = Color(0xFF121628).copy(alpha = 0.14f)
    val shape = RoundedCornerShape(CapsuleRadius)
    Box(modifier.fillMaxWidth().padding(start = CapsuleMargin, end = CapsuleMargin, bottom = CapsuleMargin)) {
        Box(
            Modifier
                .fillMaxWidth()
                .shadow(10.dp, shape, clip = false, ambientColor = capsuleShadow, spotColor = capsuleShadow)
                .clip(shape)
                .background(if (GlassCapable) Color.Transparent else opaque, shape)
                .border(1.dp, border, shape),
        ) {
            GlassLayer(glass, shape)
            Row(
                Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 6.dp, vertical = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                tabs.forEachIndexed { i, tab ->
                    CapsuleTab(
                        spec = tab,
                        selected = i == active,
                        onClick = { onTab(i) },
                        modifier = Modifier.weight(1f),
                    )
                }
            }
        }
    }
}

/** 单页签:图标 24dp(胶囊底 64×34)+ 标签 11sp;选中 = accent 13% 胶囊底。 */
@Composable
private fun CapsuleTab(spec: CapsuleTabSpec, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier.clickable(onClickLabel = spec.label, onClick = onClick),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(TabRowGap),
    ) {
        Box(
            Modifier
                .size(TabPillWidth, TabPillHeight)
                .clip(RoundedCornerShape(TabPillRadius))
                .background(
                    if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.13f) else Color.Transparent,
                    RoundedCornerShape(TabPillRadius),
                ),
            contentAlignment = Alignment.Center,
        ) {
            val tint = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant
            when (spec.glyph) {
                CapsuleTabGlyph.FAVORITES -> MockGlyph(SynshuGlyphs.FOLDER, TabIconSize, tint, fill = selected)
                CapsuleTabGlyph.DEVICES -> MockGlyph(MONITOR_PATH, TabIconSize, tint, fill = selected)
                CapsuleTabGlyph.TRANSFERS -> MockGlyph(SWAP_PATH, TabIconSize, tint, fill = selected)
                CapsuleTabGlyph.PROFILE -> MockGlyph(PERSON_PATH, TabIconSize, tint, fill = selected)
            }
        }
        Text(
            spec.label,
            color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
            fontSize = TabLabelSize,
            fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
            maxLines = 1,
        )
    }
}

// ── 搜索框(spec §2.2「搜索框」)─────────────────────────────────────────────

/** 搜索框高 40dp、圆角 11、放大镜 14dp + 15.5sp 文本(spec §2.2)。 */
private val SearchFieldHeight = 40.dp
private val SearchFieldRadius = 11.dp
private val SearchGlyphSize = 14.dp
private val SearchTextSize = 15.5.sp

/**
 * 搜索框:高 40dp、圆角 11、`#F0F0F3` 底(深色白 8%)、放大镜 14dp +
 * 15.5sp 文本;占位词「搜索」(视觉稿逐字面)。水平边距 20dp 内置
 * (视觉稿 .search margin;上下留宿主组排)。
 */
@Composable
fun SearchField(
    text: String,
    onTextChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "搜索",
) {
    Row(
        modifier
            .fillMaxWidth()
            .padding(horizontal = 20.dp)
            .height(SearchFieldHeight)
            .clip(RoundedCornerShape(SearchFieldRadius))
            .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(SearchFieldRadius))
            .padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        MockGlyph(pathData = MAG_PATH, size = SearchGlyphSize, strokeWidth = 2f, color = MaterialTheme.colorScheme.onSurfaceVariant)
        // 占位词覆盖在文本槽内文字起点(紧随放大镜 + 7dp 间距;iOS/视觉稿同构。
        // 评审 fix:占位若作 Row 兄弟节点会被 weight(1f) 文本槽推至行尾)
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
            BasicTextField(
                value = text,
                onValueChange = onTextChange,
                singleLine = true,
                textStyle = TextStyle(
                    color = MaterialTheme.colorScheme.onSurface,
                    fontSize = SearchTextSize,
                ),
                modifier = Modifier.fillMaxWidth(),
            )
            if (text.isEmpty()) {
                Text(
                    placeholder,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = SearchTextSize,
                )
            }
        }
    }
}
