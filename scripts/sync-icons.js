#!/usr/bin/env node

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const webOnly = process.argv.includes("--web");
const rootDirectory = path.resolve(__dirname, "..");
const appDirectory = path.join(rootDirectory, "apps", "app");
const sourceIcon = path.join(rootDirectory, "apps", "icon", "icon.png");
const iconManifest = path.join(rootDirectory, "apps", "icon", "icons.json");
const fullbleedIcon = path.join(rootDirectory, "apps", "icon", "icon-fullbleed.png");
const generatedIconsDirectory = path.join(rootDirectory, ".tmp", "app-icons");
const { syncNativeIcons } = require("../apps/app/src-tauri/native/sync-icons.cjs");
const favicon = path.join(appDirectory, "public", "cliproam-icon.png");
const sharedIcon = path.join(rootDirectory, "packages", "ui", "src", "assets", "cliproam-icon.png");
const browserIconsDirectory = path.join(rootDirectory, ".tmp", "browser-icons");
const maskableIconsDirectory = path.join(rootDirectory, ".tmp", "maskable-icons");
const browserIconSizes = [16, 32, 48, 128, 192, 512];
const androidResourcesDirectory = path.join(
  appDirectory,
  "src-tauri",
  "gen",
  "android",
  "app",
  "src",
  "main",
  "res",
);

for (const input of [sourceIcon, iconManifest, fullbleedIcon]) {
  if (!fs.existsSync(input)) {
    console.error(`图标输入不存在：${input}`);
    process.exit(1);
  }
}

const tauriCli = require.resolve("@tauri-apps/cli/tauri.js", {
  paths: [appDirectory],
});
if (!webOnly) {
  execFileSync(
    process.execPath,
    [
      tauriCli,
      "icon",
      iconManifest,
      "--output",
      generatedIconsDirectory,
      "--ios-color",
      "#0f172a",
    ],
    { cwd: rootDirectory, stdio: "inherit" },
  );

  fs.copyFileSync(path.join(generatedIconsDirectory, "128x128.png"), favicon);
  fs.mkdirSync(path.dirname(sharedIcon), { recursive: true });
  fs.copyFileSync(path.join(generatedIconsDirectory, "128x128.png"), sharedIcon);
  // Generate each native platform into its own directory.
  for (const platform of ["windows", "linux", "ios", "android"]) syncNativeIcons(platform);
  if (process.platform === "darwin") syncNativeIcons("macos");
  console.log(`已更新 Web 图标：${path.relative(rootDirectory, favicon)}`);
}

execFileSync(
  process.execPath,
  [tauriCli, "icon", sourceIcon, "--output", browserIconsDirectory,
    ...browserIconSizes.flatMap((size) => ["--png", String(size)])],
  { cwd: rootDirectory, stdio: "inherit" },
);
for (const platform of ["pwa", "chrome", "firefox", "vscode"]) {
  const publicDirectory = path.join(rootDirectory, "apps", platform, "public");
  const iconsDirectory = path.join(publicDirectory, "icons");
  fs.mkdirSync(iconsDirectory, { recursive: true });
  fs.copyFileSync(path.join(browserIconsDirectory, "128x128.png"), path.join(publicDirectory, "cliproam-icon.png"));
  // PWA installation icons are generated below from opaque full-bleed artwork.
  const sizes = ["vscode", "pwa"].includes(platform) ? [] : [16, 32, 48, 128];
  for (const size of sizes) {
    fs.copyFileSync(path.join(browserIconsDirectory, `${size}x${size}.png`), path.join(iconsDirectory, `icon-${size}.png`));
  }
  console.log(`已更新 ${platform} 图标：${path.relative(rootDirectory, publicDirectory)}`);
}

// Reuse the full-bleed artwork so launchers can apply their own mask
// without adding a white background around the transparent desktop icon.
execFileSync(
  process.execPath,
  [tauriCli, "icon", fullbleedIcon, "--output", maskableIconsDirectory,
    ...[180, 192, 512].flatMap((size) => ["--png", String(size)])],
  { cwd: rootDirectory, stdio: "inherit" },
);
for (const size of [192, 512]) {
  fs.copyFileSync(
    path.join(maskableIconsDirectory, `${size}x${size}.png`),
    path.join(rootDirectory, "apps", "pwa", "public", "icons", `icon-fullbleed-${size}.png`),
  );
}
console.log("已更新 PWA 自适应图标");

// Use a new URL to avoid cached transparent icons. Keep the conventional name
// too, for browsers that discover the touch icon without reading the link tag.
for (const name of ["apple-touch-icon-fullbleed.png", "apple-touch-icon.png"]) {
  fs.copyFileSync(
    path.join(maskableIconsDirectory, "180x180.png"),
    path.join(rootDirectory, "apps", "pwa", "public", name),
  );
}
console.log("已更新 PWA iOS 主屏幕图标");

if (!webOnly && fs.existsSync(androidResourcesDirectory)) {
  // The full-bleed source fills the adaptive layer and keeps its artwork inside
  // the central viewport. Let the launcher apply its own mask without insets.
  console.log(
    `Tauri 已更新 Android 图标：${path.relative(rootDirectory, androidResourcesDirectory)}`,
  );
}

console.log(`已从 ${path.relative(rootDirectory, sourceIcon)} 刷新${webOnly ? " Web 图标" : "全部图标"}`);
