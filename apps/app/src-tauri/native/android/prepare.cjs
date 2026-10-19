#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const { syncAndroidIcons } = require("./sync-icons.cjs");

function prepareAndroid(android = path.resolve(__dirname, "../../gen/android")) {
  if (!fs.existsSync(path.join(android, "settings.gradle"))) {
    throw new Error("找不到 Android 工程，请先运行 pnpm android:init");
  }
  fs.cpSync(path.join(__dirname, "app"), path.join(android, "app"), { recursive: true });
  syncAndroidIcons(android);
  console.log("已配置 Android 签名、HTTP、返回键、本地化和启动图标");
}

module.exports = { prepareAndroid };
if (require.main === module) prepareAndroid();
