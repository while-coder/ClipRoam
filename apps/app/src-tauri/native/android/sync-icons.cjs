#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { generateNativeIcons } = require("../sync-icons.cjs");

function syncIcons(android = path.resolve(__dirname, "../../gen/android")) {
  const icons = generateNativeIcons("android", {
    inputs: [__filename],
    generate({ generated, output }) {
      fs.copyFileSync(path.join(generated, "icon.png"), path.join(output, "icon.png"));
      fs.cpSync(path.join(generated, "android"), path.join(output, "res"), { recursive: true });
    },
  });
  if (fs.existsSync(path.join(android, "settings.gradle"))) {
    fs.cpSync(path.join(icons, "res"), path.join(android, "app/src/main/res"), { recursive: true });
  }
  return icons;
}

module.exports = { syncIcons };
if (require.main === module) syncIcons();
