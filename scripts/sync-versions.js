#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");

const rootDirectory = path.resolve(__dirname, "..");
const rootPackage = readJson(path.join(rootDirectory, "package.json"));
const rootVersion = rootPackage.version;

if (!/^\d+\.\d+\.\d+$/.test(rootVersion ?? "")) {
  console.error("根 package.json 的 version 必须为 major.minor.patch 格式");
  process.exit(1);
}

const packageFiles = ["apps", "packages"].flatMap((directory) => {
  const absoluteDirectory = path.join(rootDirectory, directory);
  if (!fs.existsSync(absoluteDirectory)) return [];
  return fs.readdirSync(absoluteDirectory, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => path.join(directory, entry.name, "package.json"))
    .filter((file) => fs.existsSync(path.join(rootDirectory, file)));
});

const versionFiles = [...packageFiles, "apps/chrome/public/manifest.json", "apps/firefox/public/manifest.json"];
const updates = [];
for (const file of versionFiles) {
  const absolutePath = path.join(rootDirectory, file);
  const content = fs.readFileSync(absolutePath, "utf8");
  const currentVersion = readJson(absolutePath).version;
  if (!currentVersion) {
    console.warn(`${file} 没有 version 字段，已跳过`);
    continue;
  }
  if (currentVersion === rootVersion) continue;
  const updated = content.replace(
    /("version"\s*:\s*")[^"]*(")/,
    (_match, prefix, suffix) => `${prefix}${rootVersion}${suffix}`,
  );
  updates.push({ file, absolutePath, updated, currentVersion });
}

// 只同步应用自身的 Rust 版本，不改插件和第三方依赖版本。
for (const [file, pattern] of [
  ["apps/app/src-tauri/Cargo.toml", /(\[package\][\s\S]*?^version\s*=\s*")([^"]+)(")/m],
  ["apps/app/src-tauri/Cargo.lock", /(\[\[package\]\]\r?\nname = "cliproam"\r?\nversion = ")([^"]+)(")/],
]) {
  const absolutePath = path.join(rootDirectory, file);
  const content = fs.readFileSync(absolutePath, "utf8");
  const match = content.match(pattern);
  if (!match) throw new Error(`${file} 没有应用版本字段`);
  const currentVersion = match[2];
  if (currentVersion === rootVersion) continue;
  const updated = content.replace(pattern, (_match, prefix, _version, suffix) => `${prefix}${rootVersion}${suffix}`);
  updates.push({ file, absolutePath, updated, currentVersion });
}

for (const { file, absolutePath, updated, currentVersion } of updates) {
  fs.writeFileSync(absolutePath, updated);
  console.log(`${file}: ${currentVersion} -> ${rootVersion}`);
}

if (updates.length === 0) {
  console.log(`所有客户端版本已与根版本一致 (${rootVersion})`);
} else {
  console.log(`已同步 ${updates.length} 个版本文件到 ${rootVersion}`);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}
