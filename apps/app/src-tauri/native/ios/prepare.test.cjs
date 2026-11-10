const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { prepareIos } = require("./prepare.cjs");

test("iOS preparation embeds and signs WidgetKit alongside existing extensions", (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cliproam-widget-test-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  fs.mkdirSync(path.join(directory, "cliproam_iOS"));
  fs.writeFileSync(path.join(directory, "project.yml"), "targets:\n  cliproam_iOS:\n    info:\n      properties:\n        CFBundleVersion: 42\n");
  const first = prepareIos(directory, false);
  const spec = prepareIos(directory, false);
  assert.deepEqual(first, spec);
  const app = spec.targets.cliproam_iOS;
  assert.deepEqual(app.dependencies.map(({ target }) => target), ["ClipRoamShare", "ClipRoamAction", "ClipRoamWidget"]);
  const widget = spec.targets.ClipRoamWidget;
  assert.equal(widget.info.properties.NSExtension.NSExtensionPointIdentifier, "com.apple.widgetkit-extension");
  assert.equal(widget.info.properties.NSExtension.NSExtensionPrincipalClass, undefined);
  assert.deepEqual(widget.entitlements.properties, app.entitlements.properties);
  assert.equal(widget.info.properties.CFBundleVersion, "42");
  assert.equal(widget.info.properties.NSAppTransportSecurity.NSAllowsArbitraryLoads, true);
  for (const name of ["ClipRoamShare", "ClipRoamAction", "ClipRoamWidget"]) {
    assert.ok(app.postBuildScripts[0].script.includes(`$app/PlugIns/${name}.appex`));
  }
  for (const file of ["StickyWidget.swift", "StickyWidgetStore.swift", "en.lproj/Localizable.strings", "zh-Hans.lproj/Localizable.strings", "zh-Hant.lproj/Localizable.strings"]) {
    assert.ok(fs.existsSync(path.join(directory, "ClipRoamWidget", file)));
  }
  for (const target of ["cliproam_iOS", "ClipRoamShare", "ClipRoamAction", "ClipRoamWidget"]) {
    assert.ok(fs.existsSync(path.join(directory, target, "AppBrand.swift")));
    for (const locale of ["zh-Hans", "zh-Hant"]) {
      for (const file of ["Localizable.strings", "InfoPlist.strings"]) {
        const strings = fs.readFileSync(path.join(directory, target, `${locale}.lproj`, file), "utf8");
        const values = [...strings.matchAll(/"[^"\n]*"\s*=\s*"([^"\n]*)";/g)].map((match) => match[1]);
        assert.ok(values.some((value) => value.includes("随心贴")));
        assert.ok(values.every((value) => !value.includes("ClipRoam")));
      }
    }
  }
  assert.ok(fs.readFileSync(path.join(directory, "ClipRoamWidget/en.lproj/Localizable.strings"), "utf8")
    .includes('"ClipRoam 便签" = "ClipRoam Notes";'));
  // Both builds must use the same account/cache implementation.
  assert.equal(fs.readFileSync(path.join(directory, "ClipRoamWidget/StickyWidgetStore.swift"), "utf8"),
    fs.readFileSync(path.join(__dirname, "../../share/plugin/ios/Sources/StickyWidgetStore.swift"), "utf8"));
});
