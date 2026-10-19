#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons() {
  return generateNativeIcons("windows", {
    inputs: [__filename],
    generate({ generated, output }) {
      fs.copyFileSync(path.join(generated, "icon.ico"), path.join(output, "icon.ico"));
    },
  });
}

module.exports = { syncIcons };
if (require.main === module) syncIcons();
