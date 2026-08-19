#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");

const rootDirectory = path.resolve(__dirname, "..");
const rootPackage = readJson(path.join(rootDirectory, "package.json"));
const rootVersion = rootPackage.version;

if (!rootVersion) {
  console.error("根 package.json 没有 version 字段");
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

let changed = 0;
for (const file of packageFiles) {
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
  fs.writeFileSync(absolutePath, updated);
  changed++;
  console.log(`${file}: ${currentVersion} -> ${rootVersion}`);
}

if (changed === 0) {
  console.log(`${packageFiles.length} 个子项目版本已与根版本一致 (${rootVersion})`);
} else {
  console.log(`已同步 ${changed}/${packageFiles.length} 个子项目到 ${rootVersion}`);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}
