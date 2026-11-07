package com.mtdss.android.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/**
 * Synshu 主题 token 层(spec docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md
 * §2 全值钉死;视觉稿真源 06-detail-screens.html)。M3 仅作工程底座,
 * ColorScheme/Typography 全 token 覆写为 iOS 文件 App 范式规格值;浅深随
 * 系统(§2.1「App 内不设主题开关」),语义色恒定不随主题漂移。
 *
 * 供所有 UI 任务消费(名面在计划 T4 钉死):`SynshuColors`(浅色常量)、
 * `SynshuColorsDark`(深色档)、`SynshuSemantic`(语义色)、
 * `SynshuLightScheme`/`SynshuDarkScheme`(M3 ColorScheme)、
 * `SynshuTypography`(字阶)、`SynshuBackgroundBrush`(深色纵向渐变)、
 * `SynshuTheme { }`(浅深随系统入口;T5 壳接线)。
 */

/** 浅色 token(spec §2.1 浅色表;玻璃白 · iOS 蓝)。 */
object SynshuColors {
    /** 强调色(主蓝)/语义「接收」「进行中」。 */
    val Accent = Color(0xFF007AFF)
    /** 背景/表面(纯白,无染色)。 */
    val Background = Color(0xFFFFFFFF)
    /** 输入框/搜索框底(≈iOS systemGray6)。 */
    val InputField = Color(0xFFF0F0F3)
    /** 选中行底/分组卡底。 */
    val SelectedRow = Color(0xFFF2F2F7)
    /** 分组卡底(与选中行同源)。 */
    val GroupCard = Color(0xFFF2F2F7)
    /** 主文字。 */
    val TextPrimary = Color(0xFF1C1C1E)
    /** 副文字。 */
    val TextSecondary = Color(0xFF8E8E93)
    /** 发丝分割线 rgba(60,60,67,.2)。 */
    val Hairline = Color(0xFF3C3C43).copy(alpha = 0.2f)
    /** 离线/禁用文字。 */
    val Disabled = Color(0xFFB4B4B9)
    /** 破坏红(= 语义「失败」)。 */
    val DestructiveRed = Color(0xFFFF3B30)
    /** 在线点(= 语义「发送」「完成」绿)。 */
    val OnlineDot = Color(0xFF34A853)
    /** 描边灰(选圈描边 1.5dp / 前进箭头不可用;spec §2.2)。 */
    val StrokeGray = Color(0xFFC7C7CC)
    /** 胶囊毛玻璃底(白 72%,spec §2.2;API 31+ 半透明,<31 退化不透明)。 */
    val CapsuleGlass = Color(0xFFFFFFFF).copy(alpha = 0.72f)
    /** 胶囊退化不透明底(API <31,含 MIX 3/Android 10;描边/投影/布局不变)。 */
    val CapsuleOpaque = Color(0xFFFFFFFF)
}

/** 深色 token(spec §2.1 深色表;同源配套,随系统)。 */
object SynshuColorsDark {
    /** 背景纵向渐变起端(分层质感)。 */
    val BackgroundStart = Color(0xFF0D0F15)
    /** 背景纵向渐变末端。 */
    val BackgroundEnd = Color(0xFF12141C)
    /** 强调蓝。 */
    val Accent = Color(0xFF4E9AFF)
    /** 主文字。 */
    val TextPrimary = Color(0xFFE5E6EB)
    /** 副文字。 */
    val TextSecondary = Color(0xFF989AA6)
    /** 分割线 rgba(255,255,255,.09)。 */
    val Hairline = Color(0xFFFFFFFF).copy(alpha = 0.09f)
    /** 选中行底 rgba(255,255,255,.06)。 */
    val SelectedRow = Color(0xFFFFFFFF).copy(alpha = 0.06f)
    /** 分组卡底(spec 未单列,族内推导 = 选中行底)。 */
    val GroupCard = SelectedRow
    /** 输入框/搜索框底(spec 未单列,族内推导白 8%)。 */
    val InputField = Color(0xFFFFFFFF).copy(alpha = 0.08f)
    /** 胶囊毛玻璃底 rgba(28,30,40,.74)(spec §2.1/§2.2)。 */
    val CapsuleGlass = Color(0xFF1C1E28).copy(alpha = 0.74f)
    /** 胶囊退化不透明底(毛玻璃色对渐变底的合成近似,<31 用)。 */
    val CapsuleOpaque = Color(0xFF1A1B24)
    /** 禁用/离线文字(spec 深色表未单列,#B4B4B9 中灰双主题可读,恒定)。 */
    val Disabled = SynshuColors.Disabled

    /** 背景纵向渐变(深色分层质感;浅色不使用)。 */
    val BackgroundBrush: Brush = Brush.verticalGradient(listOf(BackgroundStart, BackgroundEnd))
}

/** 语义色恒定(spec §2.1:不随主题/品牌漂移,伞形 §5 口径)。 */
object SynshuSemantic {
    /** 接收(下载)= 蓝。 */
    val Receive = Color(0xFF007AFF)
    /** 发送(上传)= 绿。 */
    val Send = Color(0xFF34A853)
    /** 调度(remote_copy)= 紫灰。 */
    val Schedule = Color(0xFF8E8AA0)
    /** 进行中 = 蓝。 */
    val InProgress = Color(0xFF007AFF)
    /** 完成 = 绿。 */
    val Done = Color(0xFF34A853)
    /** 失败 = 红。 */
    val Fail = Color(0xFFFF3B30)
}

/** 浅色 M3 ColorScheme(全 token 覆写;M3 仅工程底座,规格值全为 spec §2)。 */
val SynshuLightScheme: ColorScheme = lightColorScheme(
    primary = SynshuColors.Accent,
    onPrimary = Color.White,
    primaryContainer = SynshuColors.SelectedRow,
    onPrimaryContainer = SynshuColors.TextPrimary,
    inversePrimary = SynshuColorsDark.Accent,
    secondary = SynshuColors.OnlineDot,
    onSecondary = Color.White,
    secondaryContainer = SynshuColors.SelectedRow,
    onSecondaryContainer = SynshuColors.TextPrimary,
    tertiary = SynshuSemantic.Schedule,
    onTertiary = Color.White,
    tertiaryContainer = SynshuColors.SelectedRow,
    onTertiaryContainer = SynshuColors.TextPrimary,
    background = SynshuColors.Background,
    onBackground = SynshuColors.TextPrimary,
    surface = SynshuColors.Background,
    onSurface = SynshuColors.TextPrimary,
    surfaceVariant = SynshuColors.InputField,
    onSurfaceVariant = SynshuColors.TextSecondary,
    surfaceTint = SynshuColors.Accent,
    inverseSurface = SynshuColors.TextPrimary,
    inverseOnSurface = SynshuColors.Background,
    error = SynshuColors.DestructiveRed,
    onError = Color.White,
    errorContainer = SynshuColors.DestructiveRed.copy(alpha = 0.12f),
    onErrorContainer = SynshuColors.DestructiveRed,
    outline = SynshuColors.StrokeGray,
    outlineVariant = SynshuColors.Hairline,
    scrim = Color.Black,
    surfaceBright = SynshuColors.Background,
    surfaceDim = SynshuColors.Background,
    surfaceContainerLowest = SynshuColors.Background,
    surfaceContainerLow = SynshuColors.Background,
    surfaceContainer = SynshuColors.SelectedRow,
    surfaceContainerHigh = SynshuColors.InputField,
    surfaceContainerHighest = SynshuColors.InputField,
)

/** 深色 M3 ColorScheme(全 token 覆写;背景槽位取渐变末端,屏面刷渐变笔刷)。 */
val SynshuDarkScheme: ColorScheme = darkColorScheme(
    primary = SynshuColorsDark.Accent,
    onPrimary = Color.White,
    primaryContainer = SynshuColorsDark.Accent.copy(alpha = 0.16f),
    onPrimaryContainer = SynshuColorsDark.TextPrimary,
    inversePrimary = SynshuColors.Accent,
    secondary = SynshuSemantic.Send,
    onSecondary = Color.White,
    secondaryContainer = SynshuColorsDark.SelectedRow,
    onSecondaryContainer = SynshuColorsDark.TextPrimary,
    tertiary = SynshuSemantic.Schedule,
    onTertiary = Color.White,
    tertiaryContainer = SynshuSemantic.Schedule.copy(alpha = 0.18f),
    onTertiaryContainer = SynshuColorsDark.TextPrimary,
    background = SynshuColorsDark.BackgroundEnd,
    onBackground = SynshuColorsDark.TextPrimary,
    surface = SynshuColorsDark.BackgroundEnd,
    onSurface = SynshuColorsDark.TextPrimary,
    surfaceVariant = SynshuColorsDark.InputField,
    onSurfaceVariant = SynshuColorsDark.TextSecondary,
    surfaceTint = SynshuColorsDark.Accent,
    inverseSurface = SynshuColorsDark.TextPrimary,
    inverseOnSurface = SynshuColorsDark.BackgroundEnd,
    error = SynshuSemantic.Fail,
    onError = Color.White,
    errorContainer = SynshuSemantic.Fail.copy(alpha = 0.16f),
    onErrorContainer = SynshuColorsDark.TextPrimary,
    outline = SynshuColors.StrokeGray,
    outlineVariant = SynshuColorsDark.Hairline,
    scrim = Color.Black,
    surfaceBright = SynshuColorsDark.CapsuleOpaque,
    surfaceDim = SynshuColorsDark.BackgroundStart,
    surfaceContainerLowest = SynshuColorsDark.BackgroundStart,
    surfaceContainerLow = SynshuColorsDark.BackgroundEnd,
    surfaceContainer = SynshuColorsDark.SelectedRow,
    surfaceContainerHigh = SynshuColorsDark.InputField,
    surfaceContainerHighest = Color(0xFFFFFFFF).copy(alpha = 0.12f),
)

/**
 * 字阶(spec §2.2 规格值):行名 17sp/副文 12.5sp/顶栏标题 17sp·600/
 * 尾注·弹窗正文 13sp/胶囊标签 11sp/搜索框 15.5sp/弹窗标题 16.5sp·600。
 * 未规格槽位沿 M3 缺省形态。
 */
val SynshuTypography: Typography = Typography(
    bodyLarge = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.Normal, fontSize = 17.sp, lineHeight = 22.sp),
    bodyMedium = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.Normal, fontSize = 13.sp, lineHeight = 18.sp),
    bodySmall = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.Normal, fontSize = 12.5.sp, lineHeight = 16.sp),
    titleLarge = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 16.5.sp, lineHeight = 21.sp),
    titleMedium = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, lineHeight = 22.sp),
    labelLarge = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.Normal, fontSize = 15.5.sp, lineHeight = 20.sp),
    labelSmall = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 14.sp),
)

/** Synshu 主题入口:浅深随系统(spec §2.1;App 内不设主题开关)。 */
@Composable
fun SynshuTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    MaterialTheme(
        colorScheme = if (dark) SynshuDarkScheme else SynshuLightScheme,
        typography = SynshuTypography,
        content = content,
    )
}
