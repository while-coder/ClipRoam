#!/usr/bin/env node
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../../../..");
const app = path.join(root, "apps/app");
const platforms = ["windows", "macos", "linux", "android", "ios"];

function filesIn(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? filesIn(file) : [file];
  });
}

function syncNativeIcons(platform) {
  if (!platforms.includes(platform)) throw new Error(`不支持的图标平台：${platform}`);
  if (platform === "macos" && process.platform !== "darwin") {
    throw new Error("macOS Dock 和托盘图标需要在 macOS 上生成");
  }
  const cli = require.resolve("@tauri-apps/cli/tauri.js", { paths: [app] });
  const source = path.join(root, "apps/icon");
  const swift = path.join(__dirname, "macos/mac-icon.swift");
  const inputs = [__filename, path.join(source, "icon.png"), path.join(source, "icon-android.png"),
    path.join(source, "icons.json"), path.join(path.dirname(cli), "package.json")];
  if (platform === "macos") inputs.push(swift);
  const hash = createHash("sha256").update(platform);
  for (const file of inputs) hash.update(fs.readFileSync(file));
  const fingerprint = hash.digest("hex");
  const output = path.join(__dirname, platform, "icons");
  const temporary = path.join(root, ".tmp/native-icons", platform);
  const stamp = path.join(temporary, "generated.json");
  try {
    const saved = JSON.parse(fs.readFileSync(stamp, "utf8"));
    if (saved.fingerprint === fingerprint && saved.files.length &&
        saved.files.every((file) => fs.existsSync(path.join(output, file)))) return output;
  } catch { /* Generate on a fresh checkout or after the cache was removed. */ }

  fs.mkdirSync(temporary, { recursive: true });
  fs.rmSync(output, { recursive: true, force: true });
  fs.mkdirSync(output, { recursive: true });
  const generated = path.join(temporary, "icons");
  const input = platform === "ios" ? "icon-android.png" : "icons.json";
  execFileSync(process.execPath, [cli, "icon", path.join(source, input),
    "--output", generated, "--ios-color", "#0f172a"], { cwd: root, stdio: "inherit" });

  if (platform === "windows") {
    fs.copyFileSync(path.join(generated, "icon.ico"), path.join(output, "icon.ico"));
  } else if (platform === "linux") {
    for (const name of ["32x32.png", "128x128.png", "128x128@2x.png"]) {
      fs.copyFileSync(path.join(generated, name), path.join(output, name));
    }
  } else if (platform === "macos") {
    fs.copyFileSync(path.join(generated, "32x32.png"), path.join(output, "32x32.png"));
    execFileSync("swift", ["-module-cache-path", path.join(root, ".tmp/swift-module-cache"), swift],
      { cwd: root, stdio: "inherit" });
  } else if (platform === "android") {
    fs.copyFileSync(path.join(generated, "icon.png"), path.join(output, "icon.png"));
    fs.cpSync(path.join(generated, "android"), path.join(output, "res"), { recursive: true });
  } else {
    fs.copyFileSync(path.join(generated, "icon.png"), path.join(output, "icon.png"));
    fs.cpSync(path.join(generated, "ios"), output, { recursive: true });
  }
  fs.writeFileSync(stamp, JSON.stringify({ fingerprint,
    files: filesIn(output).map((file) => path.relative(output, file)),
  }) + "\n");
  console.log(`已更新 ${platform} 图标：${path.relative(root, output)}`);
  return output;
}

module.exports = { syncNativeIcons };
if (require.main === module) {
  const platform = process.argv[2] || process.env.TAURI_ENV_PLATFORM ||
    ({ win32: "windows", darwin: "macos", linux: "linux" })[process.platform];
  syncNativeIcons(platform);
}
