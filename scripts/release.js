#!/usr/bin/env node

/**
 * 一键发版：统一版本 -> 提交 -> 创建 vX.Y.Z tag -> 推送 origin。
 * 推送 tag 后由 .github/workflows/release.yml 构建 App 与 Server 产物。
 *
 * 用法：
 *   pnpm release                使用当前版本
 *   pnpm release patch          升级补丁版本
 *   pnpm release minor          升级次版本
 *   pnpm release major          升级主版本
 *   pnpm release 0.2.0          使用指定版本
 */

const { execSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const readline = require("node:readline");

const rootDirectory = path.resolve(__dirname, "..");
const semverPattern = /^\d+\.\d+\.\d+$/;
const bumpTypes = new Set(["patch", "minor", "major"]);

function packageFiles() {
  return ["apps", "packages"].flatMap((directory) => {
    const absoluteDirectory = path.join(rootDirectory, directory);
    if (!fs.existsSync(absoluteDirectory)) return [];
    return fs.readdirSync(absoluteDirectory, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => `${directory}/${entry.name}/package.json`)
      .filter((file) => fs.existsSync(path.join(rootDirectory, file)));
  });
}

const versionFiles = ["package.json", ...packageFiles()];

function shell(command) {
  return execSync(command, {
    cwd: rootDirectory,
    stdio: ["pipe", "pipe", "inherit"],
  }).toString().trim();
}

function run(command) {
  console.log(`> ${command}`);
  execSync(command, { cwd: rootDirectory, stdio: "inherit" });
}

function succeeds(command) {
  try {
    execSync(command, { cwd: rootDirectory, stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(path.join(rootDirectory, file), "utf8"));
}

function setVersion(file, version) {
  const absolutePath = path.join(rootDirectory, file);
  const content = fs.readFileSync(absolutePath, "utf8");
  const updated = content.replace(
    /("version"\s*:\s*")[^"]*(")/,
    (_match, prefix, suffix) => `${prefix}${version}${suffix}`,
  );
  if (updated === content) {
    throw new Error(`${file} 没有可更新的 version 字段`);
  }
  fs.writeFileSync(absolutePath, updated);
}

function bumpVersion(version, type) {
  const [major, minor, patch] = version.split(".").map(Number);
  if (type === "major") return `${major + 1}.0.0`;
  if (type === "minor") return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

function changelogHasVersion(version) {
  const changelog = path.join(rootDirectory, "CHANGELOG.md");
  if (!fs.existsSync(changelog)) return false;
  return fs.readFileSync(changelog, "utf8")
    .split(/\r?\n/)
    .some((line) => line.trim() === `## ${version}`);
}

function localTagExists(tag) {
  return succeeds(`git rev-parse -q --verify refs/tags/${tag}`);
}

function remoteTagExists(tag) {
  try {
    execSync(`git ls-remote --exit-code origin refs/tags/${tag}`, {
      cwd: rootDirectory,
      stdio: "ignore",
    });
    return true;
  } catch (error) {
    // git-ls-remote uses status 2 when the remote is reachable but no ref
    // matches. Authentication, DNS and transport failures must stop release.
    if (error && typeof error === "object" && error.status === 2) return false;
    throw new Error(`无法检查 origin 上的 ${tag}，请确认网络和 Git 权限`);
  }
}

function ask(prompt) {
  const input = readline.createInterface({ input: process.stdin, output: process.stdout });
  return new Promise((resolve) => {
    let settled = false;
    const finish = (answer) => {
      if (settled) return;
      settled = true;
      resolve(answer);
    };
    input.once("close", () => finish(undefined));
    input.question(prompt, (answer) => {
      finish(answer);
      input.close();
    });
  });
}

function printHelp() {
  console.log(`Usage:
  pnpm release                使用当前版本创建 tag
  pnpm release patch          升级补丁版本
  pnpm release minor|major    升级次版本或主版本
  pnpm release 0.2.0          使用指定版本`);
}

async function main() {
  const versionArgument = process.argv[2];
  if (versionArgument === "-h" || versionArgument === "--help") {
    printHelp();
    return;
  }

  let status;
  try {
    status = shell("git status --porcelain");
  } catch {
    throw new Error("当前目录不是 Git 仓库");
  }

  const currentVersion = readJson("package.json").version;
  if (!semverPattern.test(currentVersion ?? "")) {
    throw new Error(`根 package.json 版本号无效：${currentVersion}`);
  }

  let nextVersion = currentVersion;
  if (versionArgument) {
    if (semverPattern.test(versionArgument)) {
      nextVersion = versionArgument;
    } else if (bumpTypes.has(versionArgument)) {
      nextVersion = bumpVersion(currentVersion, versionArgument);
    } else {
      throw new Error(`无法识别版本参数：${versionArgument}`);
    }
  }
  const versionChanged = nextVersion !== currentVersion;

  if (status) {
    throw new Error(`工作区有未提交改动，请先处理：\n${status}`);
  }
  if (!versionChanged) {
    const mismatches = versionFiles
      .filter((file) => readJson(file).version !== currentVersion)
      .map((file) => `${file}=${readJson(file).version}`);
    if (mismatches.length) {
      throw new Error(`以下版本与根版本不一致，请先运行 pnpm sync:versions：\n${mismatches.join("\n")}`);
    }
  }

  const branch = shell("git rev-parse --abbrev-ref HEAD");
  if (branch === "HEAD") throw new Error("当前处于 detached HEAD，请先切换到发布分支");
  const originUrl = shell("git remote get-url origin");
  const tag = `v${nextVersion}`;

  console.log("────────────────────────────────────────");
  console.log(`  当前版本:  v${currentVersion}`);
  console.log(`  发布版本:  ${tag}`);
  console.log(`  分支:      ${branch}`);
  console.log(`  远程:      ${originUrl}`);
  console.log("────────────────────────────────────────");
  if (!changelogHasVersion(nextVersion)) {
    console.warn(`⚠ CHANGELOG.md 没有 \"## ${nextVersion}\" 段落，Release notes 将使用默认文案`);
  }

  // 先检查并确认 tag 覆盖，再修改版本文件、创建提交或删除任何 tag。
  const existsLocally = localTagExists(tag);
  const existsRemotely = remoteTagExists(tag);
  const locations = [existsLocally ? "本地" : "", existsRemotely ? "origin" : ""]
    .filter(Boolean)
    .join("、");

  if (existsLocally || existsRemotely) {
    console.log(`  tag 状态:  已存在（${locations}）→ 将删除后重打`);
    const answer = await ask(`⚠ tag ${tag} 已存在（${locations}）。删除后重新打 tag 并推送？[y/N] `);
    if (answer?.trim().toLowerCase() !== "y") {
      throw new Error("已取消，未删除任何 tag，发版中止");
    }
  } else {
    console.log("  tag 状态:  新建");
    await ask("回车确认发版，Ctrl+C 取消...");
  }

  if (versionChanged) {
    for (const file of versionFiles) setVersion(file, nextVersion);
    run(`git add ${versionFiles.map((file) => `"${file}"`).join(" ")}`);
    run(`git commit -m "chore(release): ${tag}"`);
  }

  if (existsRemotely) run(`git push origin --delete "${tag}"`);
  if (existsLocally) run(`git tag -d "${tag}"`);
  run(`git tag -a "${tag}" -m "${tag}"`);
  run(`git push origin "${branch}"`);
  run(`git push origin "${tag}"`);

  console.log(`✓ 发版完成！tag ${tag} 已推送 — GitHub Actions 将构建 App 与 Server 产物`);
  console.log("  https://github.com/while-coder/ClipRoam/actions");
}

main().catch((error) => {
  console.error(`✗ ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
