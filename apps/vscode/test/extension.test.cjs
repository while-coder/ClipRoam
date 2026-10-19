const { test } = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");

function setup(state = new Map()) {
  const commands = new Map();
  const sent = [], errors = [], notices = [];
  const dialogs = { saveUri: undefined, folderUri: undefined, saves: [], folders: [] };
  const editor = {
    document: { isClosed: false, getText: () => "selected code" },
    selections: [{ start: 0, end: 3 }, { start: 8, end: 10 }],
    viewColumn: 1,
  };
  let receive, dispose, activeChanged;
  let clipboard = "clipboard text";
  const panel = {
    webview: {
      cspSource: "https://resources.example",
      asWebviewUri: (uri) => `https://resources.example/${path.basename(uri.fsPath)}`,
      onDidReceiveMessage: (handler) => { receive = handler; return { dispose() {} }; },
      postMessage: async (message) => { sent.push(message); return true; },
      set html(value) { panel.html = value; queueMicrotask(() => receive({ type: "ready" })); },
    },
    viewColumn: 2,
    reveal() {},
    onDidDispose: (handler) => { dispose = handler; },
    dispose: () => dispose(),
  };
  const vscode = {
    window: {
      activeTextEditor: editor,
      onDidChangeActiveTextEditor: (handler) => { activeChanged = handler; return { dispose() {} }; },
      createWebviewPanel: () => panel,
      showErrorMessage: (text) => errors.push(text),
      showInformationMessage: (text) => notices.push(text),
      showSaveDialog: async (options) => { dialogs.saves.push(options); return dialogs.saveUri; },
      showOpenDialog: async (options) => { dialogs.folders.push(options); return dialogs.folderUri ? [dialogs.folderUri] : undefined; },
    },
    commands: { registerCommand: (name, handler) => { commands.set(name, handler); return { dispose() {} }; } },
    ViewColumn: { Beside: 2 },
    Uri: {
      joinPath: (uri, ...parts) => ({ scheme: uri.scheme ?? "file", fsPath: path.join(uri.fsPath, ...parts) }),
      file: (filePath) => ({ scheme: "file", fsPath: filePath }),
      parse: (text) => ({ scheme: text.split(":")[0] }),
    },
    env: {
      clipboard: { readText: async () => clipboard, writeText: async (text) => { clipboard = text; } },
      openExternal: async () => true,
    },
    workspace: { fs: { createDirectory: (uri) => fs.promises.mkdir(uri.fsPath, { recursive: true }) } },
  };
  const exports = {};
  const module = { exports };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../dist/extension.js"), "utf8"), {
    exports, module, require: (name) => name === "vscode" ? vscode : require(name), setTimeout, clearTimeout, Buffer,
  });
  const globalState = {
    get: (key, fallback) => structuredClone(state.get(key) ?? fallback),
    update: async (key, value) => { state.set(key, structuredClone(value)); },
  };
  module.exports.activate({ extensionUri: { fsPath: path.join(__dirname, "..") }, subscriptions: [], globalState });
  let requestId = 100;
  async function request(operation, value) {
    const id = ++requestId;
    await receive({ type: "request", id, operation, value });
    const response = sent.find((message) => message.type === "response" && message.id === id);
    if (response.error) throw new Error(response.error);
    return response.value;
  }
  return { commands, panel, sent, errors, notices, dialogs, editor, state, globalState, request, receive: (...args) => receive(...args), blur: () => activeChanged(undefined) };
}

function loadTs(filename, imports) {
  const ts = require("typescript");
  const exports = {};
  const code = ts.transpileModule(fs.readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  vm.runInNewContext(code, { exports, Buffer, ReadableStream, require: (id) => {
    if (id in imports) return imports[id];
    throw new Error(`Unexpected import: ${id}`);
  } });
  return exports;
}

function exportClient(app, blobs) {
  const { NativeVscode } = loadTs(path.join(__dirname, "../src/webview/native.ts"), {
    "@cliproam/ui/native-web": { NativeWeb: class { async getCapabilities() { return {}; } } },
    "@cliproam/ui/bytes": { bytesToBase64: (bytes) => Buffer.from(bytes).toString("base64") },
    "./bridge": { request: app.request }, "../../package.json": {},
  });
  const native = new NativeVscode();
  const { AccountFiles } = loadTs(path.join(__dirname, "../../../packages/ui/src/platform/files.ts"), {
    "@cliproam/protocol": { isFileNode: (node) => typeof node.f === "string" },
    "./database": { AccountDatabase: class { async get(_store, id) { return blobs.get(id); } } },
    "../utils": { safeName: (name) => name.replace(/[<>:"/\\|?*\x00-\x1f]/g, "_") },
    "./runtime": { isNative: false }, "./native": { native },
  });
  return { native, files: new AccountFiles("test-account") };
}

function tempDirectory(t) {
  const parent = require("node:os").tmpdir();
  const directory = fs.mkdtempSync(path.join(parent, "cliproam-export-"));
  t.after(() => {
    assert.equal(path.dirname(directory), parent);
    assert.ok(path.basename(directory).startsWith("cliproam-export-"));
    fs.rmSync(directory, { recursive: true, force: true });
  });
  return directory;
}

test("loads packaged assets with CSP and allows only the supported message operations", async () => {
  const app = setup();
  await app.commands.get("cliproam.open")();
  assert.match(app.panel.html, /Content-Security-Policy/);
  assert.match(app.panel.html, /src="https:\/\/resources.example\/index-/);
  assert.doesNotMatch(app.panel.html, /src="\.\//);
  await app.receive({ type: "request", id: 1, operation: "clipboard.write", value: "code" });
  await app.receive({ type: "request", id: 2, operation: "clipboard.read" });
  assert.equal(app.sent.at(-1).value, "code");
  await app.receive({ type: "request", id: 3, operation: "external.open", value: "command:workbench.action.quit" });
  assert.match(app.sent.at(-1).error, /HTTP\/HTTPS/);
  await app.receive({ type: "request", id: 4, operation: "arbitrary.command" });
  assert.match(app.sent.at(-1).error, /不支持/);
  app.panel.dispose();
});

test("queues selected text without login and removes it only after import acknowledgement", async () => {
  const app = setup();
  await app.commands.get("cliproam.open")();
  app.blur();
  await app.commands.get("cliproam.captureSelection")();
  assert.equal(app.errors.length, 0);
  assert.match(app.notices[0], /暂存/);
  assert.ok(app.sent.some((message) => message.type === "sharesChanged"));
  await app.receive({ type: "request", id: 1, operation: "shares.pending", value: "account-a" });
  const shares = app.sent.at(-1).value;
  assert.equal(shares.length, 1);
  assert.equal(shares[0].text, "selected code\nselected code");
  await app.receive({ type: "request", id: 2, operation: "shares.pending", value: "account-a" });
  assert.equal(app.sent.at(-1).value[0].id, shares[0].id, "failed imports retain their deduplication ID");
  await app.receive({ type: "request", id: 3, operation: "shares.acknowledge", value: shares[0].id });
  await app.receive({ type: "request", id: 4, operation: "shares.pending", value: "account-a" });
  assert.equal(app.sent.at(-1).value.length, 0);
  app.editor.document.isClosed = true;
  await app.commands.get("cliproam.captureSelection")();
  assert.match(app.errors.at(-1), /打开/);
  app.panel.dispose();
});

test("preserves queued text across panel close and extension reload without importing into another account", async () => {
  const app = setup();
  await app.commands.get("cliproam.captureSelection")();
  app.panel.dispose();
  const restored = setup(app.state);
  await restored.commands.get("cliproam.open")();
  await restored.receive({ type: "request", id: 1, operation: "shares.pending", value: "account-a" });
  const id = restored.sent.at(-1).value[0].id;
  await restored.receive({ type: "request", id: 2, operation: "shares.pending", value: "account-b" });
  assert.equal(restored.sent.at(-1).value.length, 0);
  await restored.receive({ type: "request", id: 3, operation: "shares.pending", value: "account-a" });
  assert.equal(restored.sent.at(-1).value[0].id, id);
  restored.panel.dispose();
});

test("serializes simultaneous selections and acknowledgements without losing queued text", async () => {
  const app = setup();
  await app.commands.get("cliproam.open")();
  await Promise.all([app.commands.get("cliproam.captureSelection")(), app.commands.get("cliproam.captureSelection")()]);
  await app.receive({ type: "request", id: 1, operation: "shares.pending", value: "account-a" });
  const shares = app.sent.at(-1).value;
  assert.equal(shares.length, 2);
  assert.notEqual(shares[0].id, shares[1].id);
  await Promise.all([
    app.receive({ type: "request", id: 2, operation: "shares.acknowledge", value: shares[0].id }),
    app.commands.get("cliproam.captureSelection")(),
  ]);
  await app.receive({ type: "request", id: 3, operation: "shares.pending", value: "account-a" });
  const remaining = app.sent.at(-1).value;
  assert.equal(remaining.length, 2);
  assert.ok(remaining.some((item) => item.id === shares[1].id));
  assert.ok(!remaining.some((item) => item.id === shares[0].id));
  app.panel.dispose();
});

test("reports storage failures without claiming that text was queued", async () => {
  const app = setup();
  const update = app.globalState.update;
  app.globalState.update = async () => { throw new Error("storage failed"); };
  await app.commands.get("cliproam.captureSelection")();
  assert.match(app.errors[0], /storage failed/);
  assert.equal(app.notices.length, 0);
  app.globalState.update = update;
  await app.commands.get("cliproam.captureSelection")();
  assert.equal(app.notices.length, 1, "later saves recover after a failed storage update");
  app.panel.dispose();
});

test("exports an image through the native save dialog to the selected file", async (t) => {
  const app = setup();
  const directory = tempDirectory(t);
  await app.commands.get("cliproam.open")();
  app.dialogs.saveUri = { scheme: "file", fsPath: path.join(directory, "chosen.webp") };
  const bytes = Buffer.alloc(200000, 123);
  const { native, files } = exportClient(app, new Map([["image", new Blob([bytes])]]));
  assert.equal((await native.getCapabilities()).nativeFileExport, true);
  const target = await native.selectSaveTarget(true, "screenshot.webp");
  assert.equal(path.basename(app.dialogs.saves[0].defaultUri.fsPath), "screenshot.webp");
  const count = await files.export({ kind: "image", content: "screenshot", imageInfo: { fileId: "image" } }, target, new AbortController().signal);
  assert.equal(count, 1);
  assert.deepEqual(fs.readFileSync(app.dialogs.saveUri.fsPath), bytes);
  await assert.rejects(app.request("files.write", { target: target.value, path: ["screenshot.webp"], data: "", append: false }), /保存位置/);
  app.panel.dispose();
});

test("exports folders and empty directories only within the selected directory", async (t) => {
  const app = setup();
  const directory = tempDirectory(t);
  await app.commands.get("cliproam.open")();
  app.dialogs.folderUri = { scheme: "file", fsPath: directory };
  const { native, files } = exportClient(app, new Map([["text", new Blob(["hello world"])]]));
  const target = await native.selectSaveTarget(false, "folder");
  assert.equal(app.dialogs.folders[0].canSelectFolders, true);
  await assert.rejects(app.request("files.write", { target: target.value, path: ["..", "outside.txt"], data: "", append: false }), /文件路径/);
  const count = await files.export({ kind: "files", fileInfo: { folder: { "note.txt": { f: "text", s: 11 }, empty: {} } } }, target, new AbortController().signal);
  assert.equal(count, 1);
  assert.equal(fs.readFileSync(path.join(directory, "folder", "note.txt"), "utf8"), "hello world");
  assert.ok(fs.statSync(path.join(directory, "folder", "empty")).isDirectory());
  app.panel.dispose();
});

test("cancelled dialogs and aborted exports do not write a file", async (t) => {
  const app = setup();
  const directory = tempDirectory(t);
  await app.commands.get("cliproam.open")();
  const { native, files } = exportClient(app, new Map([["image", new Blob(["image"])]]));
  assert.equal(await native.selectSaveTarget(true, "screenshot.webp"), null);
  await assert.rejects(app.request("files.write", { target: path.join(directory, "unselected.webp"), path: ["image.webp"], data: "", append: false }), /保存位置/);
  app.dialogs.saveUri = { scheme: "file", fsPath: path.join(directory, "cancelled.webp") };
  const target = await native.selectSaveTarget(true, "screenshot.webp");
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(files.export({ kind: "image", content: "screenshot", imageInfo: { fileId: "image" } }, target, controller.signal));
  assert.equal(fs.existsSync(app.dialogs.saveUri.fsPath), false);
  app.panel.dispose();
});
