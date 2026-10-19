#!/usr/bin/env node
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { syncIosIcons } = require("./sync-ios-icons");

// Run after `tauri ios init`, which recreates the untracked Xcode project.
function prepareIos(apple = path.resolve(__dirname, "../apps/app/src-tauri/gen/apple"), generate = true) {
  const root = path.resolve(__dirname, "..");
  const project = fs.readFileSync(path.join(apple, "project.yml"), "utf8");
  const target = project.match(/^  ([\w-]+_iOS):\s*$/m)?.[1];
  if (!target) throw new Error("找不到 Tauri iOS 应用 target");
  const version = require(path.join(root, "package.json")).version;
  const buildVersion = project.match(/^\s+CFBundleVersion:\s*["']?([^\s"']+)/m)?.[1] ?? version;
  const appGroup = { "com.apple.security.application-groups": ["group.com.whilecode.cliproam"] };
  const extension = "ClipRoamShare";
  const source = path.join(apple, extension);
  fs.mkdirSync(source, { recursive: true });
  fs.copyFileSync(path.join(root, "apps/app/src-tauri/ios/ShareExtension/ShareViewController.swift"),
    path.join(source, "ShareViewController.swift"));
  fs.copyFileSync(path.join(root, "apps/app/src-tauri/plugins/share-receiver/ios/Sources/ShareInbox.swift"),
    path.join(source, "ShareInbox.swift"));
  const spec = {
    include: ["project.yml"],
    targets: {
      [target]: {
        settings: { base: { ASSETCATALOG_COMPILER_APPICON_NAME: "AppIcon" } },
        entitlements: { path: `${target}/${target}.entitlements`, properties: appGroup },
        dependencies: [{ target: extension, embed: true }],
      },
      [extension]: {
        type: "app-extension", platform: "iOS", deploymentTarget: "14.0",
        sources: [{ path: extension, excludes: ["*.plist", "*.entitlements"] }],
        settings: { groups: ["app"], base: {
          PRODUCT_NAME: extension, PRODUCT_BUNDLE_IDENTIFIER: "com.whilecode.cliproam.share",
          SWIFT_VERSION: "5.0", APPLICATION_EXTENSION_API_ONLY: true,
          SKIP_INSTALL: true, TARGETED_DEVICE_FAMILY: "1,2",
          GENERATE_INFOPLIST_FILE: false,
        } },
        info: { path: `${extension}/Info.plist`, properties: {
          CFBundleDisplayName: "ClipRoam", CFBundleShortVersionString: version, CFBundleVersion: buildVersion,
          NSExtension: {
            NSExtensionPointIdentifier: "com.apple.share-services",
            NSExtensionPrincipalClass: "$(PRODUCT_MODULE_NAME).ShareViewController",
            NSExtensionAttributes: { NSExtensionActivationRule: {
              NSExtensionActivationSupportsText: true,
              NSExtensionActivationSupportsWebURLWithMaxCount: 10,
              NSExtensionActivationSupportsImageWithMaxCount: 100,
              NSExtensionActivationSupportsFileWithMaxCount: 100,
              NSExtensionActivationSupportsMovieWithMaxCount: 10,
            } },
          },
        } },
        entitlements: { path: `${extension}/${extension}.entitlements`, properties: appGroup },
      },
    },
  };
  const specPath = path.join(apple, "project.cliproam.json");
  fs.writeFileSync(specPath, JSON.stringify(spec, null, 2) + "\n");
  if (generate) {
    syncIosIcons();
    execFileSync("xcodegen", ["generate", "--spec", specPath], { cwd: apple, stdio: "inherit" });
  }
  console.log("已配置 iOS Share Extension、App Group 和应用图标");
  return spec;
}

module.exports = { prepareIos };
if (require.main === module) prepareIos();
