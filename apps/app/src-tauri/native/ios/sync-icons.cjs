const fs = require("node:fs");
const path = require("node:path");
const { syncNativeIcons } = require("../sync-icons.cjs");

function syncIosIcons() {
  const icons = syncNativeIcons("ios");
  const catalog = path.resolve(__dirname, "../../gen/apple/Assets.xcassets/AppIcon.appiconset");
  if (fs.existsSync(catalog)) {
    for (const entry of fs.readdirSync(icons)) {
      if (entry.startsWith("AppIcon-")) fs.copyFileSync(path.join(icons, entry), path.join(catalog, entry));
    }
  }
  console.log("已更新 iOS 全画布图标及 Xcode 资源");
}

module.exports = { syncIosIcons };
if (require.main === module) syncIosIcons();
