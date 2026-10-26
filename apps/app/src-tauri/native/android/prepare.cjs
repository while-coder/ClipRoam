#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { syncIcons } = require("./sync-icons.cjs");

// 分发来源标记：写入 AndroidManifest meta-data，便于识别 APK 面向哪个商店/渠道。
// CI 打包时用 CLIPROAM_DISTRIBUTION_SOURCE 覆盖（如 googleplay、coolapk）。
const DISTRIBUTION_SOURCE_META = "io.cliproam.SOURCE";

function patchAndroidManifest(android) {
  const manifestPath = path.join(android, "app", "src", "main", "AndroidManifest.xml");
  let manifest = fs.readFileSync(manifestPath, "utf8");

  const applicationTag = manifest.match(/<application\b[^>]*>/);
  if (!applicationTag) throw new Error("AndroidManifest.xml 缺少 <application> 节点");
  if (!manifest.includes(`android:name="${DISTRIBUTION_SOURCE_META}"`)) {
    const source = process.env.CLIPROAM_DISTRIBUTION_SOURCE || "github";
    manifest = manifest.replace(
      applicationTag[0],
      `${applicationTag[0]}\n    <meta-data android:name="${DISTRIBUTION_SOURCE_META}" android:value="${source}" />`,
    );
  }
  // 应用内更新：APK 安装需要「安装未知应用」权限，下载目录通过 FileProvider 共享给系统安装器。
  if (!manifest.includes("android.permission.REQUEST_INSTALL_PACKAGES")) {
    manifest = manifest.replace(
      "</manifest>",
      '    <uses-permission android:name="android.permission.REQUEST_INSTALL_PACKAGES" />\n</manifest>',
    );
  }
  // CI 显式指定来源时强制覆盖，便于同一份工程打出不同渠道的 APK。
  if (process.env.CLIPROAM_DISTRIBUTION_SOURCE) {
    const source = process.env.CLIPROAM_DISTRIBUTION_SOURCE;
    const existing = manifest.match(
      new RegExp(`(android:name="${DISTRIBUTION_SOURCE_META}" android:value=")([^"]*)(")`),
    );
    if (existing && existing[2] !== source) {
      manifest = manifest.replace(existing[0], `${existing[1]}${source}${existing[3]}`);
    }
  }
  if (!manifest.includes("androidx.core.content.FileProvider")) {
    manifest = manifest.replace(
      "</application>",
      `    <provider
      android:name="androidx.core.content.FileProvider"
      android:authorities="\${applicationId}.fileprovider"
      android:exported="false"
      android:grantUriPermissions="true">
      <meta-data
        android:name="android.support.FILE_PROVIDER_PATHS"
        android:resource="@xml/file_paths" />
    </provider>
</application>`,
    );
  }
  fs.writeFileSync(manifestPath, manifest);
}

function prepareAndroid(android = path.resolve(__dirname, "../../gen/android")) {
  if (!fs.existsSync(path.join(android, "settings.gradle"))) {
    throw new Error("找不到 Android 工程，请先运行 pnpm android:init");
  }
  fs.cpSync(path.join(__dirname, "app"), path.join(android, "app"), { recursive: true });
  patchAndroidManifest(android);
  syncIcons(android);
  console.log("已配置 Android 签名、HTTP、返回键、本地化、启动图标、更新权限与分发来源标记");
}

module.exports = { prepareAndroid };
if (require.main === module) prepareAndroid();
