#!/usr/bin/env node

const { execFileSync, execSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const rootDirectory = path.resolve(__dirname, "..");
const { version } = JSON.parse(fs.readFileSync(path.join(rootDirectory, "package.json"), "utf8"));
const imageName = "qingfeng346/cliproam";
const versionTag = `${imageName}:${version}`;
const latestTag = `${imageName}:latest`;

if (process.argv[2] === "publish") {
  for (const tag of [versionTag, latestTag]) {
    execFileSync("docker", ["push", tag], { cwd: rootDirectory, stdio: "inherit" });
    console.log(`Pushed ${tag}`);
  }
  process.exit(0);
}

const dockerfile = path.join(rootDirectory, "apps", "server", "Dockerfile");
const context = path.join(rootDirectory, "apps", "server", "dist");
const tarPath = path.join(rootDirectory, "cliproam-server.tar");

execSync("pnpm build:server", { cwd: rootDirectory, stdio: "inherit" });
execFileSync("docker", [
  "build",
  "--tag", versionTag,
  "--tag", latestTag,
  "--file", dockerfile,
  context,
], { cwd: rootDirectory, stdio: "inherit" });
execFileSync("docker", ["save", "--output", tarPath, versionTag], {
  cwd: rootDirectory,
  stdio: "inherit",
});

console.log(`Built ${versionTag} and ${latestTag}`);
console.log(`Saved ${tarPath} with tag ${versionTag}`);
