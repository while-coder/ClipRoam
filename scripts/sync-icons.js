#!/usr/bin/env node

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const rootDirectory = path.resolve(__dirname, "..");
const appDirectory = path.join(rootDirectory, "apps", "app");
const sourceIcon = path.join(rootDirectory, "apps", "icon", "icon.png");
const iconManifest = path.join(rootDirectory, "apps", "icon", "icons.json");
const androidIcon = path.join(rootDirectory, "apps", "icon", "icon-android.png");
const tauriIconsDirectory = path.join(appDirectory, "src-tauri", "icons");
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

for (const input of [sourceIcon, iconManifest, androidIcon]) {
  if (!fs.existsSync(input)) {
    console.error(`图标输入不存在：${input}`);
    process.exit(1);
  }
}

const tauriCli = require.resolve("@tauri-apps/cli/tauri.js", {
  paths: [appDirectory],
});
execFileSync(
  process.execPath,
  [
    tauriCli,
    "icon",
    iconManifest,
    "--output",
    tauriIconsDirectory,
    "--ios-color",
    "#0f172a",
  ],
  { cwd: rootDirectory, stdio: "inherit" },
);

fs.copyFileSync(path.join(tauriIconsDirectory, "128x128.png"), favicon);
fs.mkdirSync(path.dirname(sharedIcon), { recursive: true });
fs.copyFileSync(path.join(tauriIconsDirectory, "128x128.png"), sharedIcon);
// iOS uses full-bleed artwork; system masking supplies the rounded corners.
require("../apps/app/src-tauri/native/ios/sync-icons.cjs").syncIosIcons();
console.log(`已更新 Web 图标：${path.relative(rootDirectory, favicon)}`);

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
  const sizes = platform === "vscode" ? [] : platform === "pwa" ? [192, 512] : [16, 32, 48, 128];
  for (const size of sizes) {
    fs.copyFileSync(path.join(browserIconsDirectory, `${size}x${size}.png`), path.join(iconsDirectory, `icon-${size}.png`));
  }
  console.log(`已更新 ${platform} 图标：${path.relative(rootDirectory, publicDirectory)}`);
}

// Reuse the full-bleed Android artwork so launchers can apply their own mask
// without adding a white background around the transparent desktop icon.
execFileSync(
  process.execPath,
  [tauriCli, "icon", androidIcon, "--output", maskableIconsDirectory,
    ...[192, 512].flatMap((size) => ["--png", String(size)])],
  { cwd: rootDirectory, stdio: "inherit" },
);
for (const size of [192, 512]) {
  fs.copyFileSync(
    path.join(maskableIconsDirectory, `${size}x${size}.png`),
    path.join(rootDirectory, "apps", "pwa", "public", "icons", `icon-maskable-${size}.png`),
  );
}
console.log("已更新 PWA 自适应图标");

if (fs.existsSync(androidResourcesDirectory)) {
  // The Android source fills the adaptive layer and keeps its artwork inside
  // the central viewport. Let the launcher apply its own mask without insets.
  console.log(
    `Tauri 已更新 Android 图标：${path.relative(rootDirectory, androidResourcesDirectory)}`,
  );
}

if (process.platform === "darwin") {
  execFileSync("swift", [path.join(__dirname, "mac-icon.swift")], {
    cwd: rootDirectory,
    stdio: "inherit",
  });
}

console.log(`已从 ${path.relative(rootDirectory, sourceIcon)} 刷新全部图标`);
