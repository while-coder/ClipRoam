const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

function syncAndroidIcons(android = path.resolve(__dirname, "../../gen/android")) {
  if (!fs.existsSync(path.join(android, "settings.gradle"))) return;
  const root = path.resolve(__dirname, "../../../../..");
  const app = path.join(root, "apps/app");
  const cli = require.resolve("@tauri-apps/cli/tauri.js", { paths: [app] });
  const output = path.join(root, ".tmp/android-icons/icons");
  // Generate outside src-tauri, then copy into the selected generated project.
  execFileSync(process.execPath, [cli, "icon", path.join(root, "apps/icon/icons.json"),
    "--output", output, "--ios-color", "#0f172a"], { cwd: root, stdio: "inherit" });
  fs.cpSync(path.join(output, "android"), path.join(android, "app/src/main/res"), { recursive: true });
}

module.exports = { syncAndroidIcons };
if (require.main === module) syncAndroidIcons();
