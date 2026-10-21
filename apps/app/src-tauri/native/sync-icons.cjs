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
  platform = ({ win32: "windows", darwin: "macos" })[platform] || platform;
  if (!platforms.includes(platform)) throw new Error(`不支持的图标平台：${platform}`);
  return require(`./${platform}/sync-icons.cjs`).syncIcons();
}

function generateNativeIcons(platform, { inputs = [], input = "icons.json", generate }) {
  const cli = require.resolve("@tauri-apps/cli/tauri.js", { paths: [app] });
  const source = path.join(root, "apps/icon");
  const hash = createHash("sha256").update(platform);
  for (const file of [__filename, path.join(source, "icon.png"), path.join(source, "icon-fullbleed.png"),
    path.join(source, "icons.json"), path.join(path.dirname(cli), "package.json"), ...inputs]) {
    hash.update(fs.readFileSync(file));
  }
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
  execFileSync(process.execPath, [cli, "icon", path.join(source, input),
    "--output", generated, "--ios-color", "#0f172a"], { cwd: root, stdio: "inherit" });

  generate({ generated, output, root });
  fs.writeFileSync(stamp, JSON.stringify({ fingerprint,
    files: filesIn(output).map((file) => path.relative(output, file)),
  }) + "\n");
  console.log(`已更新 ${platform} 图标：${path.relative(root, output)}`);
  return output;
}

module.exports = { syncNativeIcons, generateNativeIcons };
if (require.main === module) {
  const platform = process.argv[2] || process.env.TAURI_ENV_PLATFORM || process.platform;
  syncNativeIcons(platform);
}
