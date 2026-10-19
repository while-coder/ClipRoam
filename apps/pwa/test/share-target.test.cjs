const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { resolve } = require("node:path");
const { createHash } = require("node:crypto");
const vm = require("node:vm");
const ts = require("typescript");

function load(relative, dependencies = {}, globals = {}) {
  const exports = {};
  const source = readFileSync(resolve(__dirname, relative), "utf8");
  const code = ts.transpileModule(source, {
    compilerOptions: {target:ts.ScriptTarget.ES2022, module:ts.ModuleKind.CommonJS},
  }).outputText;
  vm.runInNewContext(code, {
    exports, crypto, FormData, File, Response, URL, ...globals,
    require(id) { assert.ok(id in dependencies, `Unexpected import: ${id}`); return dependencies[id]; },
  });
  return exports;
}

const { shareFromForm } = load("../src/shareInbox.ts");

function receiver(scope, saveShare) {
  const events = {};
  load("../src/service-worker.ts", {"./shareInbox":{saveShare, shareFromForm}}, {
    self: {registration:{scope}, addEventListener(name, handler) { events[name] = handler; }},
  });
  return (request) => {
    let response;
    events.fetch({request, respondWith(value) { response = value; }});
    return response;
  };
}

test("share form retains title, Android text URLs, multiple files and empty files", async () => {
  const form = new FormData();
  form.set("title", "标题");
  form.set("text", "https://example.com/article");
  form.set("url", "https://example.com/article");
  form.append("files", new File([new Uint8Array([0, 255, 4])], "照片.png", {type:"image/png"}));
  form.append("files", new File([], "empty.txt", {type:"text/plain"}));
  const share = shareFromForm(form);
  assert.equal(share.text, "标题\nhttps://example.com/article");
  assert.equal(share.files.length, 2);
  assert.equal(share.files[0].name, "照片.png");
  assert.deepEqual([...new Uint8Array(await share.files[0].arrayBuffer())], [0, 255, 4]);
  assert.equal(share.files[1].size, 0);
});

test("empty share is rejected without dropping legitimate empty files", () => {
  assert.throws(() => shareFromForm(new FormData()), /分享内容为空/);
  const form = new FormData();
  form.append("files", new File([], ""));
  assert.throws(() => shareFromForm(form), /分享内容为空/);
});

for (const scope of ["https://example.com/", "https://example.com/ClipRoam/"]) {
  test(`share POST commits before redirect at ${scope}`, async () => {
    let commit;
    let saved;
    const receive = receiver(scope, async (databaseScope, share) => {
      saved = {databaseScope, share};
      await new Promise((resolve) => { commit = resolve; });
    });
    const form = new FormData();
    form.set("text", "分享文字");
    form.append("files", new File(["original"], "manifest.json"));
    let completed = false;
    const response = receive(new Request(new URL("share-target", scope), {method:"POST", body:form}));
    void response.then(() => { completed = true; });
    while (!commit) await new Promise((resolve) => setImmediate(resolve));
    assert.equal(completed, false);
    assert.equal(saved.databaseScope, scope);
    assert.equal(await saved.share.files[0].text(), "original");
    commit();
    const result = await response;
    assert.equal(result.status, 303);
    assert.equal(result.headers.get("Location"), scope);
  });
}

test("storage failure returns an error; unrelated requests pass through", async () => {
  const receive = receiver("https://example.com/ClipRoam/", async () => { throw new Error("quota"); });
  assert.equal(receive(new Request("https://example.com/ClipRoam/share-target")), undefined);
  assert.equal(receive(new Request("https://other.example/ClipRoam/share-target", {method:"POST"})), undefined);
  assert.equal(receive(new Request("https://example.com/entries", {method:"POST"})), undefined);
  const form = new FormData(); form.set("text", "text");
  const result = await receive(new Request("https://example.com/ClipRoam/share-target", {method:"POST", body:form}));
  assert.equal(result.status, 500);
  assert.equal(result.headers.get("Location"), null);
});

function captureSession(shares) {
  const captures = new Set(), entries = [], acknowledgements = [], errors = [], blobs = new Map();
  let failFiles = false;
  const { CaptureService } = load("../../../packages/ui/src/features/pending-upload/captureService.ts", {
    "../../utils": {
      safeName: (name) => name, profileName: () => "account", nameMatches: () => false,
      Sha256: class {
        hash = createHash("sha256");
        update(bytes) { this.hash.update(bytes); }
        digest() { return this.hash.digest("hex"); }
      },
    },
    "@cliproam/protocol": {isFileNode: (value) => "f" in value},
    "../../platform/clipboard": {clipboardPlatform:{
      shares: async () => shares,
      acknowledgeShare: async (id) => { acknowledgements.push(id); },
    }},
    "../../platform/database": {},
  });
  const session = {
    signal: new AbortController().signal, state:{localClipboardRevision:0},
    preferences:{excludePatterns:[], maxCaptureFileCount:10},
    database: {
      get: async (_store, id) => captures.has(id),
      batch: async (changes) => { for (const change of changes) blobs.set(change.id, change.value); },
    },
    pendingUploads: {enqueue: async (entry, captureId) => {
      if (failFiles && entry.kind === "files") throw new Error("文件保存失败");
      entries.push(entry); if (captureId) captures.add(captureId);
    }},
  };
  return {capture:new CaptureService(session, () => {}, (error) => errors.push(error)), entries, blobs, acknowledgements, errors,
    failFiles(value) { failFiles = value; }};
}

test("browser share enters pending queue with file bytes and deduplicates retries", async () => {
  const files = [new File(["one"], "same.txt"), new File(["two"], "same.txt")];
  const state = captureSession([{id:"share-1", text:"text", images:[], paths:[], files}]);
  const summary = await state.capture.consumeShares();
  assert.equal(summary.texts, 1);
  assert.equal(summary.files, 2);
  assert.equal(state.entries.length, 2);
  assert.deepEqual(Object.keys(state.entries[1].fileInfo), ["same.txt", "same.txt (2)"]);
  for (const file of files) {
    const hash = createHash("sha256").update(await file.text()).digest("hex");
    assert.equal(await state.blobs.get(hash).text(), await file.text());
  }
  await state.capture.consumeShares();
  assert.equal(state.entries.length, 2);
  assert.deepEqual(state.acknowledgements, ["share-1", "share-1"]);
});

test("failed file import retains share; retry does not duplicate imported text", async () => {
  const state = captureSession([{id:"share-2", text:"text", images:[], paths:[], files:[new File(["file"], "file.txt")]}]);
  state.failFiles(true);
  await state.capture.consumeShares();
  assert.equal(state.entries.length, 1);
  assert.equal(state.acknowledgements.length, 0);
  assert.equal(state.errors.length, 1);
  state.failFiles(false);
  await state.capture.consumeShares();
  assert.equal(state.entries.length, 2);
  assert.deepEqual(state.acknowledgements, ["share-2"]);
});
