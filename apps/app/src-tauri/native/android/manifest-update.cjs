#!/usr/bin/env node
// 发版时把 Android APK 写入 updater manifest（latest.json）的 android 字段。
// 客户端自检只读这份静态清单，不请求 GitHub API。需在 tauri-updater-manifest
// 工作流（重新生成 latest.json）之后运行。环境变量：TAG、VERSION、GH_TOKEN。
// Android 只保存 APK URL，版本与更新说明复用清单顶层字段。
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const repo = process.env.GITHUB_REPOSITORY || "while-coder/ClipRoam";
const tag = requiredEnv("TAG");
const version = requiredEnv("VERSION");

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) throw new Error(`缺少环境变量 ${name}`);
  return value;
}

function gh(args) {
  return execFileSync("gh", args, { encoding: "utf8" }).trim();
}

// 与客户端版本比较逻辑配套：清单只放一个 URL，优先 universal，其次 aarch64/arm64。
function pickApkAsset() {
  const names = gh([
    "api",
    `repos/${repo}/releases/tags/${tag}`,
    "--jq",
    '.assets[] | select(.name | ascii_downcase | endswith(".apk")) | .name',
  ])
    .split("\n")
    .filter(Boolean);
  if (names.length === 0) throw new Error(`Release ${tag} 上没有 APK 资产`);
  const rank = (name) => {
    const lower = name.toLowerCase();
    if (lower.includes("universal")) return 0;
    if (lower.includes("aarch64") || lower.includes("arm64")) return 1;
    return 2;
  };
  return names.sort((a, b) => rank(a) - rank(b))[0];
}

const apkName = pickApkAsset();
const url = `https://github.com/${repo}/releases/download/${tag}/${apkName}`;

const workDir = fs.mkdtempSync(path.join(os.tmpdir(), "cliproam-manifest-"));
try {
  gh(["release", "download", "updater", "--repo", repo, "--pattern", "latest.json", "--dir", workDir]);
  const manifestPath = path.join(workDir, "latest.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (typeof manifest.version !== "string" || manifest.version.replace(/^v/, "") !== version.replace(/^v/, "")) {
    throw new Error("清单顶层 version 与当前 Android 发布版本不一致，拒绝写入 APK URL");
  }
  manifest.android = { url };
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  gh(["release", "upload", "updater", manifestPath, "--repo", repo, "--clobber"]);
  console.log(`已写入 android 更新项：${version} -> ${url}`);
} finally {
  fs.rmSync(workDir, { recursive: true, force: true });
}
