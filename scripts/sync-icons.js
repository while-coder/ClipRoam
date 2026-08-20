#!/usr/bin/env node

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const rootDirectory = path.resolve(__dirname, "..");
const appDirectory = path.join(rootDirectory, "apps", "app");
const sourceIcon = path.join(rootDirectory, "apps", "icon", "icon.png");
const tauriIconsDirectory = path.join(appDirectory, "src-tauri", "icons");
const favicon = path.join(appDirectory, "public", "cliproam-icon.png");
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

if (!fs.existsSync(sourceIcon)) {
  console.error(`源图标不存在：${sourceIcon}`);
  process.exit(1);
}

const tauriCli = require.resolve("@tauri-apps/cli/tauri.js", {
  paths: [appDirectory],
});
execFileSync(
  process.execPath,
  [
    tauriCli,
    "icon",
    sourceIcon,
    "--output",
    tauriIconsDirectory,
    "--ios-color",
    "#0f172a",
  ],
  { cwd: rootDirectory, stdio: "inherit" },
);

fs.copyFileSync(path.join(tauriIconsDirectory, "128x128.png"), favicon);
console.log(`已更新 Web 图标：${path.relative(rootDirectory, favicon)}`);

if (fs.existsSync(androidResourcesDirectory)) {
  console.log(
    `Tauri 已更新 Android 图标：${path.relative(rootDirectory, androidResourcesDirectory)}`,
  );
}

console.log(`已从 ${path.relative(rootDirectory, sourceIcon)} 刷新全部图标`);
