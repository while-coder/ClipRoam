#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");

function readAppInfo() {
  const root = path.resolve(__dirname, "../../../../..");
  const readJson = (file) => JSON.parse(fs.readFileSync(path.join(root, file), "utf8"));
  const config = readJson("apps/app/src-tauri/tauri.conf.json");
  const ios = readJson("apps/app/src-tauri/tauri.ios.conf.json");
  const version = readJson("package.json").version;
  const plist = fs.readFileSync(path.join(__dirname, "Info.plist"), "utf8")
    .replace(/<!--[\s\S]*?-->/g, "");
  const decodeXml = (value) => value.replace(/&(amp|lt|gt|quot|apos);/g,
    (_, entity) => ({ amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" })[entity]);
  const privacy = Object.fromEntries(
    [...plist.matchAll(/<key>(NS[^<]*UsageDescription[^<]*)<\/key>\s*<string>([\s\S]*?)<\/string>/g)]
      .map(([, key, value]) => [key, decodeXml(value)]),
  );
  const repository = process.env.RELEASE_REPOSITORY || "while-coder/ClipRoam";
  const tag = process.env.TAG || "main";
  return {
    name: config.productName,
    bundleIdentifier: ios.identifier ?? config.identifier,
    developerName: "while-coder",
    localizedDescription: "保存剪贴板历史，在自己的多台设备之间同步文字、图片和文件。",
    iconURL: `https://raw.githubusercontent.com/${repository}/${encodeURIComponent(tag)}/apps/icon/icon.png`,
    version,
    // Tauri defaults CFBundleVersion to the app version; CI uses no --build-number.
    buildVersion: ios.bundle?.iOS?.bundleVersion ?? config.bundle?.iOS?.bundleVersion ?? version,
    minOSVersion: ios.bundle.iOS.minimumSystemVersion,
    appPermissions: {
      // Main app and extensions share this entitlement in prepare.cjs.
      entitlements: ["com.apple.security.application-groups"],
      privacy,
    },
  };
}

module.exports = { readAppInfo };
if (require.main === module) console.log(JSON.stringify(readAppInfo(), null, 2));
