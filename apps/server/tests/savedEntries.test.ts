import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { test, type TestContext } from "node:test";
import Database from "better-sqlite3";
import { EntryManifestQuerySchema } from "@cliproam/protocol";
import { UserDataStore } from "../src/clipboard/UserDataStore.js";
import type { FileStore } from "../src/files/FileStore.js";

const files = { register() {} } as FileStore;
const userId = randomUUID();

function fixture(t: TestContext, seed?: (database: Database.Database) => void) {
  const root = resolve(import.meta.dirname, "../../../.tmp");
  mkdirSync(root, { recursive: true });
  const directory = mkdtempSync(join(root, "saved-entries-"));
  const path = join(directory, "data.sqlite");
  if (seed) {
    const database = new Database(path);
    try { seed(database); } finally { database.close(); }
  }
  let store = new UserDataStore(userId, files, path);
  t.after(() => {
    store.close();
    assert.equal(dirname(directory), root);
    rmSync(directory, { recursive: true, force: true });
  });
  return {
    get store() { return store; },
    path,
    reopen() { store.close(); store = new UserDataStore(userId, files, path); return store; },
  };
}

function publish(store: UserDataStore, content = "saved text") {
  return store.upsert({ kind: "text", content, html: `<b>${content}</b>`, sourceDeviceId: "desktop" });
}

test("sticky management pages isolate notes, filter content and reflect batch deletion", (t) => {
  const { store } = fixture(t);
  const first = publish(store, "work: 100% complete");
  const second = publish(store, "personal reminder");
  publish(store, "history only");
  store.setFavorite([first.id], true);
  const notes = store.setSticky([first.id, second.id], true, 20);
  const query = EntryManifestQuerySchema.parse({ stickyOnly: "true" });
  assert.equal(store.listManifestPage(query, 100).total, 2);
  const page1 = store.listManifestPage({ ...query, page: 1 }, 1);
  const page2 = store.listManifestPage({ ...query, page: 2 }, 1);
  assert.equal(page1.total, 2);
  assert.equal(page2.total, 2);
  assert.notEqual(page1.manifest[0].id, page2.manifest[0].id);
  const matched = store.listManifestPage({ ...query, search: "100%", deviceIds: ["desktop"], kind: "text" }, 100);
  assert.equal(matched.total, 1);
  assert.equal(store.listByIds(matched.manifest.map(({ id }) => id))[0].content, first.content);
  assert.equal(store.listManifestPage({ ...query, deviceIds: ["other"] }, 100).total, 0);
  assert.equal(store.listManifestPage({ ...query, kind: "image" }, 100).total, 0);
  assert.equal(EntryManifestQuerySchema.safeParse({ stickyOnly: "true", favoriteOnly: "true" }).success, false);
  assert.equal(EntryManifestQuerySchema.safeParse({ stickyOnly: "true", favoriteFolderId: "unfiled" }).success, false);
  store.delete(notes.map(({ id }) => id));
  assert.deepEqual(store.listManifestPage(query, 100), { manifest: [], total: 0 });
  assert.deepEqual(store.listSticky(), []);
  assert.equal(store.listManifestPage({}, 100).total, 3);
  assert.equal(store.listManifestPage({ favoriteOnly: true }, 100).total, 1);
});

test("history deletion, cancellation and re-saving keep all three collections independent", (t) => {
  const { store } = fixture(t);
  const source = publish(store);
  const [favorite] = store.setFavorite([source.id], true);
  const [sticky] = store.setSticky([favorite.id], true, 20);
  assert.match(favorite.id, /^f-/);
  assert.match(sticky.id, /^s-/);
  assert.equal(new Set([source.id, favorite.id, sticky.id]).size, 3);
  assert.equal(favorite.html, source.html);
  assert.equal(sticky.html, source.html);
  assert.deepEqual(store.listByIds([source.id])[0], source);
  assert.equal(store.listManifestPage({}, 100).total, 1);
  assert.equal(store.listManifestPage({ favoriteOnly: true }, 100).total, 1);
  assert.equal(store.setFavorite([source.id, sticky.id], true)[0].id, favorite.id);
  assert.equal(store.setSticky([source.id, favorite.id], true, 20)[0].id, sticky.id);

  store.delete([source.id]);
  assert.equal(store.listManifestPage({}, 100).total, 0);
  assert.equal(store.listByIds([favorite.id, sticky.id]).length, 2);
  assert.deepEqual(store.setFavorite([sticky.id], false), []);
  assert.deepEqual(store.setSticky([favorite.id], false, 20), []);
  assert.equal(store.setFavorite([favorite.id], false)[0].id, favorite.id);
  assert.equal(store.listByIds([favorite.id]).length, 0);
  assert.equal(store.listSticky()[0].id, sticky.id);

  const [newFavorite] = store.setFavorite([sticky.id], true);
  assert.notEqual(newFavorite.id, favorite.id);
  store.setSticky([sticky.id], false, 20);
  assert.deepEqual(store.listSticky(), []);
  assert.equal(store.listByIds([newFavorite.id]).length, 1);
  const historyAgain = publish(store);
  assert.notEqual(historyAgain.id, source.id);
  assert.equal(historyAgain.isFavorite, false);
  store.delete([newFavorite.id]);
  assert.equal(store.listByIds([historyAgain.id]).length, 1);
});

test("history pruning and device deletion preserve saved files and access references", (t) => {
  const { store } = fixture(t);
  const fileId = "a".repeat(64);
  store.upsertDevice({ id: "desktop", deviceInfo: "test" });
  const source = store.upsert({ kind: "files", content: "folder", fileInfo: { folder: { "a.txt": { f: fileId, s: 10 } } }, sourceDeviceId: "desktop" });
  const [favorite] = store.setFavorite([source.id], true);
  const [sticky] = store.setSticky([source.id], true, 20);
  publish(store, "new history");
  assert.deepEqual(store.prune(1), [source.id]);
  assert.equal(store.hasFileReference(source.id, fileId), false);
  assert.equal(store.hasFileReference(favorite.id, fileId), true);
  assert.equal(store.hasFileReference(sticky.id, fileId), true);
  store.deleteDevice("desktop");
  assert.equal(store.listManifestPage({}, 100).total, 0);
  assert.equal(store.listByIds([favorite.id, sticky.id]).length, 2);
  store.setFavorite([favorite.id], false);
  assert.equal(store.hasFileReference(favorite.id, fileId), false);
  assert.equal(store.hasFileReference(sticky.id, fileId), true);
});

test("sticky quota counts destination copies, deduplicates and rolls back failed batches", (t) => {
  const { store } = fixture(t);
  const first = publish(store, "first");
  const second = publish(store, "second");
  const third = publish(store, "third");
  const [favorite] = store.setFavorite([first.id], true);
  const [sticky] = store.setSticky([first.id, favorite.id], true, 1);
  assert.equal(store.listSticky().length, 1);
  assert.equal(store.setSticky([sticky.id, favorite.id], true, 0)[0].id, sticky.id);
  assert.throws(() => store.setSticky([second.id, third.id], true, 2), /上限/);
  assert.deepEqual(store.listSticky().map(({ id }) => id), [sticky.id]);
  assert.throws(() => store.setSticky([second.id, "missing"], true, 20), /不存在/);
  store.setSticky([sticky.id], false, 0);
  assert.equal(store.setSticky([second.id], true, 1).length, 1);
});

test("favorite folders operate on saved copies and preserve subtree filtering", (t) => {
  const { store } = fixture(t);
  const parent = randomUUID();
  const child = randomUUID();
  store.saveFavoriteFolder({ name: "parent", parentId: null }, parent);
  store.saveFavoriteFolder({ name: "child", parentId: parent }, child);
  const source = publish(store, "100% match");
  const [favorite] = store.setFavorite([source.id], true, child);
  assert.equal(store.listFavoriteFolders().find(({ id }) => id === child)?.entryCount, 1);
  assert.equal(store.listManifestPage({ favoriteFolderId: parent, search: "100%" }, 100).total, 1);
  assert.equal(store.listManifestPage({ favoriteFolderId: "unfiled" }, 100).total, 0);
  assert.equal(store.setFavorite([source.id], true)[0].favoriteFolderId, child);
  store.deleteFavoriteFolder(parent);
  assert.equal(store.listManifestPage({ favoriteFolderId: "unfiled" }, 100).total, 1);
  assert.equal(store.listByIds([favorite.id])[0].version, favorite.version + 1);
  assert.equal(store.listByIds([source.id])[0].version, source.version);
});

test("legacy flags migrate once to independent copies and history revisions refresh", (t) => {
  const folderId = randomUUID();
  const setup = fixture(t, (database) => {
    database.exec(`CREATE TABLE entries (
      id INTEGER PRIMARY KEY, version INTEGER NOT NULL DEFAULT 1,
      is_favorite INTEGER NOT NULL DEFAULT 0, is_sticky INTEGER NOT NULL DEFAULT 0,
      favorite_folder_id TEXT, hash TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
      content TEXT NOT NULL, extra TEXT NOT NULL DEFAULT '{}', source_device_id TEXT NOT NULL, created_at TEXT NOT NULL
    ); CREATE TABLE favorite_folders (id TEXT PRIMARY KEY, parent_id TEXT, name TEXT NOT NULL);
    PRAGMA user_version = 1;`);
    database.prepare("INSERT INTO favorite_folders VALUES (?, NULL, 'old folder')").run(folderId);
    database.prepare("INSERT INTO entries VALUES (7, 5, 1, 1, ?, ?, 'text', 'old text', ?, 'desktop', '2026-01-01T00:00:00.000Z')")
      .run(folderId, createHash("sha256").update("text\0old text").digest("hex"), JSON.stringify({ html: "<b>old text</b>" }));
  });
  const original = setup.store.listByIds(["7"])[0];
  assert.equal(original.version, 6);
  assert.equal(original.isFavorite, false);
  assert.equal(original.isSticky, false);
  const [favoriteManifest] = setup.store.listManifestPage({ favoriteOnly: true }, 100).manifest;
  const [favorite] = setup.store.listByIds([favoriteManifest.id]);
  const [sticky] = setup.store.listSticky();
  assert.equal(favorite.favoriteFolderId, folderId);
  assert.equal(favorite.html, "<b>old text</b>");
  assert.equal(sticky.html, favorite.html);
  const reopened = setup.reopen();
  assert.equal(reopened.listSticky()[0].id, sticky.id);
  assert.equal(reopened.listManifestPage({ favoriteOnly: true }, 100).total, 1);
  assert.equal(reopened.setFavorite(["7"], true)[0].id, favorite.id);
  assert.equal(reopened.setSticky(["7"], true, 0)[0].id, sticky.id);
  const database = new Database(setup.path);
  try {
    const columns = database.pragma("table_info(entries)") as { name: string }[];
    assert.equal(columns.some(({ name }) => name === "is_favorite" || name === "is_sticky" || name === "favorite_folder_id"), false);
  } finally { database.close(); }
  reopened.delete(["7"]);
  assert.equal(reopened.listByIds([favorite.id, sticky.id]).length, 2);
});

test("image copies survive cancellation independently of history and the other collection", (t) => {
  const { store } = fixture(t);
  const source = store.upsert({
    kind: "image", content: "image", sourceDeviceId: "desktop",
    imageInfo: { fileId: "b".repeat(64), size: 42, thumbnail: "thumbnail" },
  });
  const [favorite] = store.setFavorite([source.id], true);
  const [sticky] = store.setSticky([favorite.id], true, 20);
  assert.deepEqual(sticky.imageInfo, source.imageInfo);
  store.setSticky([sticky.id], false, 20);
  assert.equal(store.listByIds([source.id, favorite.id]).length, 2);
  store.setFavorite([favorite.id], false);
  assert.deepEqual(store.listByIds([source.id]), [source]);
  assert.equal(store.hasFileReference(source.id, source.imageInfo!.fileId), true);
});
