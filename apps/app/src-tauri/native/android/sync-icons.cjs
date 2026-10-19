const fs = require("node:fs");
const path = require("node:path");
const { syncNativeIcons } = require("../sync-icons.cjs");

function syncAndroidIcons(android = path.resolve(__dirname, "../../gen/android")) {
  const icons = syncNativeIcons("android");
  if (fs.existsSync(path.join(android, "settings.gradle"))) {
    fs.cpSync(path.join(icons, "res"), path.join(android, "app/src/main/res"), { recursive: true });
  }
}

module.exports = { syncAndroidIcons };
if (require.main === module) syncAndroidIcons();
