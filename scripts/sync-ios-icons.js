const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

function syncIosIcons() {
  const root = path.resolve(__dirname, "..");
  const app = path.join(root, "apps/app");
  const cli = require.resolve("@tauri-apps/cli/tauri.js", { paths: [app] });
  // Generate outside src-tauri so Tauri cannot redirect output into gen/apple.
  const output = path.join(root, ".tmp/ios-icons/icons");
  execFileSync(process.execPath, [cli, "icon", path.join(root, "apps/icon/icon-android.png"),
    "--output", output, "--ios-color", "#0f172a"], { cwd: root, stdio: "inherit" });
  const icons = path.join(app, "src-tauri/icons/ios");
  fs.mkdirSync(icons, { recursive: true });
  fs.cpSync(path.join(output, "ios"), icons, { recursive: true });
  const catalog = path.join(app, "src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset");
  if (fs.existsSync(catalog)) fs.cpSync(icons, catalog, { recursive: true });
  console.log("已更新 iOS 全画布图标及 Xcode 资源");
}

module.exports = { syncIosIcons };
if (require.main === module) syncIosIcons();
