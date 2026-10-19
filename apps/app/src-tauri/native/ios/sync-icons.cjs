#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons(apple = path.resolve(__dirname, "../../gen/apple")) {
  const icons = generateNativeIcons("ios", {
    inputs: [__filename],
    // iOS uses full-bleed artwork; system masking supplies the rounded corners.
    input: "icon-android.png",
    generate({ generated, output }) {
      fs.copyFileSync(path.join(generated, "icon.png"), path.join(output, "icon.png"));
      fs.cpSync(path.join(generated, "ios"), output, { recursive: true });
    },
  });
  const catalog = path.join(apple, "Assets.xcassets/AppIcon.appiconset");
  if (fs.existsSync(catalog)) {
    for (const entry of fs.readdirSync(icons)) {
      if (entry.startsWith("AppIcon-")) fs.copyFileSync(path.join(icons, entry), path.join(catalog, entry));
    }
  }
  console.log("已更新 iOS 全画布图标及 Xcode 资源");
  return icons;
}

module.exports = { syncIcons };
if (require.main === module) syncIcons();
