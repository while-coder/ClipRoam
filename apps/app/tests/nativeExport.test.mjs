import assert from "node:assert/strict";
import { test } from "node:test";
import { sourceLoader } from "./loadSource.mjs";

const fileId = "a".repeat(64);
const entry = {
  id: "entry", kind: "files", content: "folder",
  fileInfo: { folder: { "a.txt": { f: fileId, s: 3 }, empty: {} } },
};

function setup(call) {
  const { AccountFiles } = sourceLoader({ native: true, call })("platform/files");
  const files = new AccountFiles("account");
  files.resolve = async () => "/cache/content";
  return files;
}

test("export walks nested and empty directories using returned opaque locations", async () => {
  for (const kind of ["path", "documentTree"]) {
    const root = kind === "path" ? "/selected" : "content://provider/tree/root";
    const calls = [];
    const locations = kind === "path" ? ["/selected/folder", "/selected/folder/a.txt", "/selected/folder/empty"]
      : ["content://provider/tree/root/document/opaque-1", "content://provider/tree/root/document/opaque-2", "content://provider/tree/root/document/opaque-3"];
    const files = setup(async (command, args) => { calls.push([command, args]); return locations[calls.length - 1]; });
    assert.equal(await files.export(entry, { kind, value: root }, new AbortController().signal), 1);
    assert.deepEqual(calls, [
      ["native_fs_mkdir", { path: root, name: "folder" }],
      ["native_fs_copy", { source: "/cache/content", destination: locations[0], name: "a.txt" }],
      ["native_fs_mkdir", { path: locations[0], name: "empty" }],
    ]);
  }
});

test("Android export failure removes only the newly created root document", async () => {
  const calls = [], root = "content://provider/tree/root", created = `${root}/document/new-folder`;
  const files = setup(async (command, args) => {
    calls.push([command, args]);
    if (command === "native_fs_mkdir") return created;
    if (command === "native_fs_copy") throw new Error("provider refused write");
  });
  await assert.rejects(files.export(entry, { kind: "documentTree", value: root }, new AbortController().signal), /refused/);
  assert.deepEqual(calls.at(-1), ["native_fs_remove", { path: created }]);
  assert.equal(calls.length, 3);
});

test("cancellation during an Android file copy cleans created items and stops traversal", async () => {
  const controller = new AbortController(), calls = [], root = "content://provider/tree/root";
  const files = setup(async (command, args) => {
    calls.push([command, args]);
    if (command === "native_fs_mkdir") return `${root}/document/folder`;
    if (command === "native_fs_copy") { controller.abort(new Error("cancelled")); return `${root}/document/file`; }
  });
  await assert.rejects(files.export(entry, { kind: "documentTree", value: root }, controller.signal), /cancelled/);
  assert.deepEqual(calls.map(([command]) => command), ["native_fs_mkdir", "native_fs_copy", "native_fs_remove"]);
  assert.deepEqual(calls.at(-1)[1], { path: `${root}/document/folder` });
});

test("single desktop files keep the selected full path; Android images use parent and name", async () => {
  const calls = [], root = "content://provider/tree/root";
  const files = setup(async (command, args) => { calls.push([command, args]); return `${root}/document/image`; });
  const image = { id: "image", kind: "image", content: "photo", imageInfo: { fileId, size: 3 } };
  assert.equal(await files.export(image, { kind: "documentTree", value: root }, new AbortController().signal), 1);
  assert.deepEqual(calls.pop(), ["native_fs_copy", { source: "/cache/content", destination: root, name: "photo.webp" }]);
  assert.equal(await files.export(image, { kind: "path", value: "/selected/custom.webp" }, new AbortController().signal), 1);
  assert.deepEqual(calls.pop(), ["native_fs_copy", { source: "/cache/content", destination: "/selected/custom.webp" }]);
});
