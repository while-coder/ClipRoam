#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons(apple = path.resolve(__dirname, "../../gen/apple")) {
  const icons = generateNativeIcons("ios", {
    inputs: [__filename, path.join(__dirname, "action-icon.svg")],
    // iOS uses full-bleed artwork; system masking supplies the rounded corners.
    input: "icon-android.png",
    generate({ generated, output, root }) {
      fs.copyFileSync(path.join(generated, "icon.png"), path.join(output, "icon.png"));
      fs.cpSync(path.join(generated, "ios"), output, { recursive: true });
      // Action extensions use an alpha template, without the app icon's background.
      const cli = require.resolve("@tauri-apps/cli/tauri.js", { paths: [path.join(root, "apps/app")] });
      const template = path.join(output, "action");
      const representations = [
        ["iphone", "60x60", "2x", 120], ["iphone", "60x60", "3x", 180],
        ["ipad", "76x76", "1x", 76], ["ipad", "76x76", "2x", 152],
        ["ipad", "83.5x83.5", "2x", 167],
      ];
      execFileSync(process.execPath, [cli, "icon", path.join(__dirname, "action-icon.svg"),
        "--output", template, ...representations.flatMap(([, , , pixels]) => ["--png", String(pixels)])],
        { cwd: root, stdio: "inherit" });
      const catalog = path.join(output, "ActionIcon.appiconset");
      fs.mkdirSync(catalog, { recursive: true });
      const images = representations.map(([idiom, size, scale, pixels]) => {
        const filename = `${pixels}x${pixels}.png`;
        fs.copyFileSync(path.join(template, filename), path.join(catalog, filename));
        return { idiom, size, scale, filename };
      });
      fs.writeFileSync(path.join(catalog, "Contents.json"), JSON.stringify({ images,
        info: { version: 1, author: "xcode" } }, null, 2) + "\n");
      fs.rmSync(template, { recursive: true, force: true });
    },
  });
  const catalog = path.join(apple, "Assets.xcassets/AppIcon.appiconset");
  if (fs.existsSync(catalog)) {
    for (const entry of fs.readdirSync(icons)) {
      if (entry.startsWith("AppIcon-")) fs.copyFileSync(path.join(icons, entry), path.join(catalog, entry));
    }
  }
  const action = path.join(apple, "ClipRoamAction");
  if (fs.existsSync(action)) {
    fs.cpSync(path.join(icons, "ActionIcon.appiconset"),
      path.join(action, "Assets.xcassets/ActionIcon.appiconset"), { recursive: true });
  }
  console.log("已更新 iOS 全画布图标及 Xcode 资源");
  return icons;
}

module.exports = { syncIcons };
if (require.main === module) syncIcons();
