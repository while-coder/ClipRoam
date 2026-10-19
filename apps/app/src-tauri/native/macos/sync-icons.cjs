#!/usr/bin/env node
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons() {
  if (process.platform !== "darwin") {
    throw new Error("macOS Dock 和托盘图标需要在 macOS 上生成");
  }
  const swift = path.join(__dirname, "mac-icon.swift");
  return generateNativeIcons("macos", {
    inputs: [__filename, swift],
    generate({ generated, output, root }) {
      fs.copyFileSync(path.join(generated, "32x32.png"), path.join(output, "32x32.png"));
      execFileSync("swift", ["-module-cache-path", path.join(root, ".tmp/swift-module-cache"), swift],
        { cwd: root, stdio: "inherit" });
    },
  });
}

module.exports = { syncIcons };
if (require.main === module) syncIcons();
