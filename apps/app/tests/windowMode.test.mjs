import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";

const require = createRequire(import.meta.url);
const ts = require("typescript");
const source = readFileSync(new URL("../src/features/quick-paste/pasteWindow.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;

// Exercise the production mode controller against an asynchronous native-window
// substitute. OS focus delivery still needs a desktop runtime check.
function harness({ signedIn = true, maximized = false, fullscreen = false, nativeQuick = false, platform = "Win32" } = {}) {
  const mode = { value: false };
  const state = {
    size: { width: 1020, height: 740 }, position: { x: 110, y: 180 },
    maximized, fullscreen, decorations: true, top: false, skipTaskbar: false,
    visible: true, nativeQuick,
  };
  let failDecorations = false;
  let captures = 0;
  class Size {
    constructor(width, height) { this.width = width; this.height = height; }
  }
  class Position {
    constructor(x, y) { this.x = x; this.y = y; }
  }
  const window = {
    hide: async () => { state.visible = false; },
    show: async () => { state.visible = true; },
    unminimize: async () => {},
    setFocus: async () => {},
    isMaximized: async () => state.maximized,
    isFullscreen: async () => state.fullscreen,
    unmaximize: async () => { state.maximized = false; },
    maximize: async () => { state.maximized = true; },
    setFullscreen: async (value) => { state.fullscreen = value; },
    innerSize: async () => new Size(state.size.width, state.size.height),
    outerSize: async () => state.size,
    outerPosition: async () => new Position(state.position.x, state.position.y),
    setMinSize: async () => {},
    setDecorations: async (value) => {
      if (failDecorations && !value) { failDecorations = false; throw new Error("decoration failed"); }
      state.decorations = value;
    },
    setAlwaysOnTop: async (value) => { state.top = value; },
    setSkipTaskbar: async (value) => {
      assert.notEqual(platform, "MacIntel", "macOS does not support taskbar visibility");
      state.skipTaskbar = value;
    },
    setTitle: async () => {},
    setSize: async (value) => { state.size = { width: value.width, height: value.height }; },
    setPosition: async (value) => { state.position = { x: value.x, y: value.y }; },
    center: async () => { state.position = { x: 400, y: 200 }; },
  };
  const mocks = {
    "../../platform/runtime": { isNative: true },
    vue: { nextTick: async () => {} },
    "@tauri-apps/api/core": { invoke: async (command, args) => {
      if (command === "native_capture_paste_target") { captures++; return; }
      assert.equal(command, "native_set_quick_paste_mode");
      const previous = state.nativeQuick;
      state.nativeQuick = args.enabled;
      return previous;
    } },
    "@tauri-apps/api/window": {
      getCurrentWindow: () => window, LogicalSize: Size, PhysicalPosition: Position,
      cursorPosition: async () => ({ x: 1900, y: 1000 }),
      monitorFromPoint: async () => ({ workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 } } }),
    },
    "../../composables/usePlatform": { isQuickPaste: mode, isMobile: { value: false }, isToastWindow: false },
    "../sync/accountSession": { getAccountSession: () => signedIn ? { signal: new AbortController().signal } : undefined },
    "../history/useHistorySync": { focusSearch: () => {}, focusSearchInput: () => {} },
  };
  const module = { exports: {} };
  new Function("require", "module", "exports", "document", "navigator", compiled)(
    (id) => { assert.ok(id in mocks, id); return mocks[id]; },
    module, module.exports, { documentElement: { classList: { toggle: () => {} } } }, { platform },
  );
  return { api: module.exports, state, mode, failNextSwitch: () => { failDecorations = true; }, captures: () => captures };
}

test("quick mode changes appearance and tray return restores the original geometry", async () => {
  const { api, state, mode } = harness();
  await api.showPasteWindow();
  assert.equal(mode.value, true);
  assert.equal(state.decorations, false);
  assert.equal(state.top, true);
  assert.equal(state.skipTaskbar, true);
  assert.equal(state.nativeQuick, true);
  assert.ok(state.position.x + state.size.width <= 1920);
  assert.ok(state.position.y + state.size.height <= 1080);
  await api.hideWindow();
  assert.equal(state.visible, false);
  await api.showMainWindow();
  assert.equal(mode.value, false);
  assert.equal(state.decorations, true);
  assert.equal(state.top, false);
  assert.equal(state.skipTaskbar, false);
  assert.equal(state.nativeQuick, false);
  assert.deepEqual(state.size, { width: 1020, height: 740 });
  assert.deepEqual(state.position, { x: 110, y: 180 });
});

test("rapid shortcut and tray requests preserve the main snapshot", async () => {
  const { api, state, captures } = harness({ maximized: true, fullscreen: true });
  await Promise.all([api.showPasteWindow(), api.showPasteWindow(), api.showMainWindow()]);
  assert.equal(captures(), 2);
  assert.equal(state.nativeQuick, false);
  assert.equal(state.maximized, true);
  assert.equal(state.fullscreen, true);
  assert.deepEqual(state.size, { width: 1020, height: 740 });
});

test("failed switching restores the main window and does not poison later requests", async () => {
  const { api, state, mode, failNextSwitch } = harness();
  failNextSwitch();
  await assert.rejects(api.showPasteWindow(), /decoration failed/);
  assert.equal(state.visible, true);
  assert.equal(state.decorations, true);
  assert.equal(mode.value, false);
  assert.deepEqual(state.size, { width: 1020, height: 740 });
  await api.showPasteWindow();
  assert.equal(mode.value, true);
});

test("signed-out shortcuts leave the current window alone", async () => {
  const { api, state, captures } = harness({ signedIn: false });
  await api.showPasteWindow();
  assert.equal(captures(), 0);
  assert.equal(state.decorations, true);
  assert.deepEqual(state.size, { width: 1020, height: 740 });
});

test("webview reload in quick mode restores a normal login window", async () => {
  const { api, state } = harness({ nativeQuick: true });
  await api.initializeWindowMode();
  assert.equal(state.nativeQuick, false);
  assert.equal(state.decorations, true);
  assert.equal(state.visible, true);
  assert.deepEqual(state.size, { width: 960, height: 700 });
});

test("macOS switches modes without the unsupported taskbar API", async () => {
  const { api, state } = harness({ platform: "MacIntel" });
  await api.showPasteWindow();
  assert.equal(state.decorations, false);
  await api.showMainWindow();
  assert.equal(state.decorations, true);
});
