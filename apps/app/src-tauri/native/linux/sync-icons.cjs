#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons() {
  return generateNativeIcons("linux", {
    inputs: [__filename],
    generate({ generated, output }) {
      for (const name of ["32x32.png", "128x128.png", "128x128@2x.png"]) {
        fs.copyFileSync(path.join(generated, name), path.join(output, name));
      }
    },
  });
}

module.exports = { syncIcons };
if (require.main === module) syncIcons();
