#!/usr/bin/env node

const { execFileSync, execSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const rootDirectory = path.resolve(__dirname, "..");
const { version } = JSON.parse(fs.readFileSync(path.join(rootDirectory, "package.json"), "utf8"));
const imageName = "cliproam-server";
const versionTag = `${imageName}:${version}`;
const latestTag = `${imageName}:latest`;
const dockerfile = path.join(rootDirectory, "apps", "server", "Dockerfile");
const context = path.join(rootDirectory, "apps", "server", "dist");
const tarPath = path.join(rootDirectory, `${imageName}.tar`);

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
