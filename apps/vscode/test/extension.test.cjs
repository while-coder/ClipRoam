const { test } = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");

function setup() {
  const commands = new Map();
  const sent = [], errors = [], edits = [];
  const editor = {
    document: { isClosed: false, getText: () => "selected code" },
    selections: [{ start: 0, end: 3 }, { start: 8, end: 10 }],
    viewColumn: 1,
    edit: async (callback) => { callback({ replace: (...args) => edits.push(args) }); return true; },
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
      showTextDocument: async () => editor,
      showErrorMessage: (text) => errors.push(text),
      showInformationMessage() {},
    },
    commands: { registerCommand: (name, handler) => { commands.set(name, handler); return { dispose() {} }; } },
    ViewColumn: { Beside: 2 },
    Uri: {
      joinPath: (uri, ...parts) => ({ fsPath: path.join(uri.fsPath, ...parts) }),
      parse: (text) => ({ scheme: text.split(":")[0] }),
    },
    env: {
      clipboard: { readText: async () => clipboard, writeText: async (text) => { clipboard = text; } },
      openExternal: async () => true,
    },
  };
  const exports = {};
  const module = { exports };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../dist/extension.js"), "utf8"), {
    exports, module, require: (name) => name === "vscode" ? vscode : require(name), setTimeout, clearTimeout,
  });
  module.exports.activate({ extensionUri: { fsPath: path.join(__dirname, "..") }, subscriptions: [] });
  return { commands, panel, sent, errors, edits, editor, receive: (...args) => receive(...args), blur: () => activeChanged(undefined) };
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

test("inserts literal text at every editor selection after the webview takes focus", async () => {
  const app = setup();
  await app.commands.get("cliproam.insertHistory")();
  app.blur();
  await app.receive({ type: "request", id: 1, operation: "editor.insert", value: "${literal}" });
  assert.equal(app.edits.length, 2);
  assert.equal(app.edits[0][1], "${literal}");
  app.editor.document.isClosed = true;
  await app.receive({ type: "request", id: 2, operation: "editor.insert", value: "code" });
  assert.match(app.sent.at(-1).error, /打开/);
  app.panel.dispose();
});

test("captures selected text and cleans pending capture requests on panel close", async () => {
  const app = setup();
  await app.commands.get("cliproam.open")();
  const work = app.commands.get("cliproam.captureSelection")();
  await new Promise((resolve) => setImmediate(resolve));
  const capture = app.sent.find((message) => message.type === "captureSelection");
  assert.equal(capture.text, "selected code\nselected code");
  await app.receive({ type: "captured", id: capture.id, error: "请先登录" });
  await work;
  assert.equal(app.errors[0], "请先登录");
  const second = app.commands.get("cliproam.captureSelection")();
  await new Promise((resolve) => setImmediate(resolve));
  app.panel.dispose();
  await second;
  assert.match(app.errors.at(-1), /已关闭/);
});
