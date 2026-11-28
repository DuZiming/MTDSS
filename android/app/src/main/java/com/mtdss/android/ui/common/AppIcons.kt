package com.mtdss.android.ui.common

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.ChevronLeft
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.DeleteSweep
import androidx.compose.material.icons.rounded.Description
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.CreateNewFolder
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Fullscreen
import androidx.compose.material.icons.rounded.Inventory2
import androidx.compose.material.icons.rounded.Monitor
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material.icons.rounded.PhotoCamera
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Smartphone
import androidx.compose.material.icons.rounded.Star
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material.icons.rounded.Mail
import androidx.compose.material.icons.rounded.Redeem
import androidx.compose.material.icons.rounded.Check
import androidx.compose.runtime.Stable
import androidx.compose.ui.graphics.vector.ImageVector

/**
 * 中心图标表(r60 #2:官方 Material Icons Rounded 族全量换装——撤手搓
 * MockGlyph 路径面)。选型原则:
 * - **同族 Rounded**:统一圆角笔触,视觉一致(替代描边/填充两态跳变);
 * - **选中态 = 同字形变色 + 胶囊底**(iOS tab 范式),不再切 fill/stroke;
 * - 语义命名集中于此,后续换族只动本表。
 */
object AppIcons {
    /** tab:快捷目录(文件夹)。 */
    val tabFavorites: ImageVector get() = Icons.Rounded.Folder
    /** tab:设备(显示器)。 */
    val tabDevices: ImageVector get() = Icons.Rounded.Monitor
    /** tab:传输(左右互换)。 */
    val tabTransfers: ImageVector get() = Icons.Rounded.SwapHoriz
    /** tab:我的(人像)。 */
    val tabProfile: ImageVector get() = Icons.Rounded.Person

    /** 文件类型:目录。 */
    val fileDir: ImageVector get() = Icons.Rounded.Folder
    /** 文件类型:通用文档(行首/类型页)。 */
    val fileDoc: ImageVector get() = Icons.Rounded.Description

    /** 动作:下载。 */
    val actionDownload: ImageVector get() = Icons.Rounded.Download
    /** 动作:清除已完成(扫帚)。 */
    val actionDeleteSweep: ImageVector get() = Icons.Rounded.DeleteSweep
    /** 动作:新建/加入快捷(带 + 的文件夹)。 */
    val actionFolderNew: ImageVector get() = Icons.Rounded.CreateNewFolder
    /** 动作:加号。 */
    val actionPlus: ImageVector get() = Icons.Rounded.Add
    /** 动作:刷新。 */
    val actionRefresh: ImageVector get() = Icons.Rounded.Refresh
    /** 动作:搜索。 */
    val actionSearch: ImageVector get() = Icons.Rounded.Search
    /** 动作:关闭/取消。 */
    val actionClose: ImageVector get() = Icons.Rounded.Close
    /** 确认勾(选择圈/全选)。 */
    val check: ImageVector get() = Icons.Rounded.Check

    /** 导航:左/右尖角。 */
    val chevronLeft: ImageVector get() = Icons.Rounded.ChevronLeft
    val chevronRight: ImageVector get() = Icons.Rounded.ChevronRight

    /** 我的页:扫码。 */
    val scanQr: ImageVector get() = Icons.Rounded.QrCodeScanner
    /** 我的页:设置齿轮。 */
    val settings: ImageVector get() = Icons.Rounded.Settings
    /** 我的页占位:库存/钱包/帮助等占位行。 */
    val placeholderBox: ImageVector get() = Icons.Rounded.Inventory2
    /** 扫码 sheet:相机。 */
    val camera: ImageVector get() = Icons.Rounded.PhotoCamera

    /** 预览:全屏。 */
    val previewFullscreen: ImageVector get() = Icons.Rounded.Fullscreen

    /** 设备选择:手机。 */
    val devicePhone: ImageVector get() = Icons.Rounded.Smartphone

    /** 我的占位行:邮件/评分/权益。 */
    val placeholderMail: ImageVector get() = Icons.Rounded.Mail
    val placeholderStar: ImageVector get() = Icons.Rounded.Star
    val placeholderGift: ImageVector get() = Icons.Rounded.Redeem
}
