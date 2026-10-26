package com.whilecode.cliproam

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import java.io.File

/**
 * 应用内更新的原生辅助：安装权限检测/申请，以及下载完成后唤起系统安装器。
 * 由 Rust 侧通过 JNI 静态调用（platforms/mobile/android/updater.rs）。
 */
object Updater {
  @JvmStatic
  fun canInstallPackages(context: Context): Boolean =
    // 「安装未知应用」权限自 Android 8.0（API 26）引入，低版本无需检测。
    Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
      context.packageManager.canRequestPackageInstalls()

  @JvmStatic
  fun requestInstallPermission(context: Context) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val intent = Intent(
      Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
      Uri.parse("package:${context.packageName}"),
    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    context.startActivity(intent)
  }

  @JvmStatic
  fun installApk(context: Context, path: String) {
    val apkUri = FileProvider.getUriForFile(
      context,
      "${context.packageName}.fileprovider",
      File(path),
    )
    val intent = Intent(Intent.ACTION_INSTALL_PACKAGE).apply {
      data = apkUri
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    context.startActivity(intent)
  }
}
