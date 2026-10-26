package com.mtdss.android.update

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.core.content.FileProvider
import java.io.File

/**
 * 拉起系统安装器(spec §6:更新只到「拉起」,装不装用户定;无静默自装)。
 *
 * - API 29 兼容形态:ACTION_VIEW + package-archive MIME + GRANT_READ_URI_
 *   PERMISSION(ACTION_INSTALL_PACKAGE 自 API 29 起废弃)。APK 从 app 私有
 *   `{filesDir}/updates/` 经 FileProvider(res/xml/file_paths.xml,authority
 *   = 包名 + ".fileprovider")以 content:// 授权安装器读——私有目录直路径
 *   安装器读不到,FileProvider 是唯一口;
 * - App 上下文 startActivity 须 FLAG_ACTIVITY_NEW_TASK;本方法可任意线程
 *   调用(binder 调用,无 UI 面前置);
 * - 安装器缺席(极少数精简 ROM)抛 IllegalStateException,由
 *   UpdateController 转 Failed(重试 = 重拉);
 * - 「未知来源安装」单应用授权由系统安装器自带引导(MIUI 双层弹窗见
 *   docs/smoke-p3.md F6),App 侧不申请 REQUEST_INSTALL_PACKAGES(该权限
 *   属 PackageInstaller 静默装面,本设计不用)。
 */
object UpdateInstaller {

    private const val APK_MIME = "application/vnd.android.package-archive"
    private const val AUTHORITY_SUFFIX = ".fileprovider"

    fun launch(context: Context, apk: File) {
        val ctx = context.applicationContext
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + AUTHORITY_SUFFIX, apk)
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, APK_MIME)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        try {
            ctx.startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            throw IllegalStateException("系统未提供应用安装器", e)
        }
    }
}
