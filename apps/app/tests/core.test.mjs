import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { IDBFactory } from "fake-indexeddb";
import { sourceLoader } from "./loadSource.mjs";

function setup(options) {
  globalThis.indexedDB = new IDBFactory();
  const values = new Map();
  globalThis.localStorage = {getItem:(key) => values.get(key) ?? null,setItem:(key,value) => values.set(key,String(value)),removeItem:(key) => values.delete(key)};
  return sourceLoader(options);
}
const entry = (id="e",version=1) => ({ id,version,kind:"text",content:"hello",sourceDeviceId:"d",createdAt:"2026-10-13T00:00:00Z" });
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");

test("clipboard events retain consecutive snapshots and ignore other or closed accounts", async () => {
  let notify;
  const calls = [];
  const load = setup({ native: true,
    call: async (command, args) => { calls.push({command,args}); },
    listen: async (event, handler) => { assert.equal(event,"cliproam://clipboard-changed"); notify = handler; return () => {}; },
  });
  const { AccountFiles } = load("platform/files"), { HistoryCache } = load("core/historyCache");
  const { CaptureService } = load("core/captureService"), { profileName } = load("core/hash");
  const files = new AccountFiles("a"), cache = new HistoryCache(files), abort = new AbortController();
  const session = { accountKey:"a",files,cache,database:files.database,signal:abort.signal,
    preferences:{watchClipboard:true},state:{localClipboardRevision:0},ownAsync:async (value) => value,
    interval:() => { throw new Error("Clipboard polling must stay in Rust"); },
  };
  let count = 0, complete;
  const done = new Promise((resolve) => { complete = resolve; });
  const errors = [];
  const service = new CaptureService(session,() => { if (++count === 2) complete(); },(error) => errors.push(error));
  await service.start();
  const event = (scope,id,text) => ({scope,capture:{id,signature:id,data:{kind:"text",text}}});
  notify(event("other-account","ignored","wrong"));
  notify(event(profileName("a"),"first","A"));
  notify(event(profileName("a"),"second","B"));
  await done;
  assert.deepEqual((await cache.pendingRows()).map((row) => row.entry.content),["A","B"]);
  abort.abort(); notify(event(profileName("a"),"closed","C"));
  assert.equal((await cache.pendingRows()).length,2);
  assert.deepEqual(calls,[{command:"native_watch_clipboard",args:{scope:profileName("a"),enabled:true}}]);
  assert.deepEqual(errors,[]);
});

test("incremental SHA-256 agrees with native crypto across padding/chunk boundaries", () => {
  const { Sha256 } = setup()("core/hash");
  for (const length of [0,1,55,56,63,64,65,127,128,129,65536,131073]) {
    const bytes = Uint8Array.from({length},(_,i) => i % 251);
    const hash = new Sha256(); for (let i=0;i<length;i+=31) hash.update(bytes.subarray(i,i+31));
    assert.equal(hash.digest(),digest(bytes),String(length));
  }
});

test("IndexedDB keeps identical entry IDs isolated by server/account", async () => {
  const { AccountDatabase } = setup()("core/database");
  const a = new AccountDatabase("account:server:alice"), b = new AccountDatabase("account:server:bob");
  await a.put("entries","same",entry("same")); await b.put("entries","same",{...entry("same"),content:"bob"});
  assert.equal((await a.get("entries","same")).content,"hello");
  assert.equal((await b.get("entries","same")).content,"bob");
  await a.delete("entries","same"); assert.equal((await b.list("entries")).length,1);
});

test("concurrent sequence allocation and updates commit without lost writes", async () => {
  const { AccountDatabase } = setup()("core/database"); const db = new AccountDatabase("a");
  const numbers = await Promise.all(Array.from({length:20},() => db.nextSequence()));
  assert.equal(new Set(numbers).size,20);
  await db.put("meta","count",0);
  await Promise.all(Array.from({length:20},() => db.update("meta","count",(count) => count+1)));
  assert.equal(await db.get("meta","count"),20);
});

test("late manifest responses cannot overwrite newer cached revisions", async () => {
  const load = setup(); const { AccountFiles } = load("platform/files"), { HistoryCache } = load("core/historyCache");
  const cache = new HistoryCache(new AccountFiles("a"));
  await Promise.all([cache.upsert([entry("e",4)]),cache.upsert([entry("e",2)])]);
  assert.equal((await cache.get("e")).version,4);
  assert.deepEqual(await cache.stale([{id:"e",version:4},{id:"missing",version:1}]),["missing"]);
});

test("durable captures replay only once and retry does not alter other pending rows", async () => {
  const load = setup(); const { AccountFiles } = load("platform/files"), { HistoryCache } = load("core/historyCache");
  const cache = new HistoryCache(new AccountFiles("a"));
  const first = await cache.enqueue(entry(),"capture-1"), second = await cache.enqueue({...entry(),content:"other"},"capture-2");
  assert.equal(await cache.enqueue(entry(),"capture-1"),first);
  await cache.setPendingStatus(first,"failed"); await cache.setPendingStatus(second,"failed");
  await cache.setPendingStatus(first,"wait");
  const rows = await cache.pendingRows(); assert.equal(rows.length,2);
  assert.equal(rows[0].status,"wait"); assert.equal(rows[1].status,"failed");
  await cache.database.delete("pending",String(first));
  assert.equal(await cache.enqueue(entry(),"capture-1"),first);
  assert.equal((await cache.pendingRows()).length,1);
});

test("browser content writer verifies size/hash before committing a Blob", async () => {
  const { AccountFiles } = setup()("platform/files"); const files = new AccountFiles("a");
  const bytes = new TextEncoder().encode("hello"), id = digest(bytes);
  const writer = await files.writer(id,bytes.length); await writer.append(bytes.subarray(0,2)); await writer.append(bytes.subarray(2)); await writer.commit();
  assert.deepEqual(await files.read(id,0,5),bytes);
  const badId = "a".repeat(64), bad = await files.writer(badId,bytes.length); await bad.append(bytes);
  await assert.rejects(bad.commit(),/校验失败/); assert.equal(await files.resolve(badId),undefined);
  files.dispose();
});

test("content summary counts nested/duplicate files without mutating the full tree", async () => {
  const { summarize } = setup()("core/content"); const id="a".repeat(64);
  const full = {...entry(),kind:"files",fileInfo:{folder:{a:{f:id,s:3},b:{f:id,s:3},empty:{}}}};
  const result = await summarize(full,new Set([id]),async () => "local",true);
  assert.equal(result.summary.fileCount,2); assert.equal(result.summary.contentCount,1);
  assert.equal(result.summary.totalSize,3); assert.equal(result.summary.readyCount,1);
  assert.equal(result.fileInfo,undefined); assert.ok(full.fileInfo);
});

test("name filtering and tree validation preserve wildcard behavior and reject traversal", () => {
  const { nameMatches,leaves } = setup()("core/content");
  assert.equal(nameMatches("BUILD",["build*"]),true); assert.equal(nameMatches("a.log",["*.log"]),true);
  assert.equal(nameMatches("a.txt",["*.log"]),false);
  assert.throws(() => leaves({"../outside":{f:"a",s:1}}),/不合法/);
});

test("browser ZIP retains UTF-8 names and empty directories", async () => {
  const { zipFiles } = setup()("core/zip");
  const blob = await zipFiles([{name:"目录/",blob:new Blob()},{name:"目录/a.txt",blob:new Blob(["hello"])}],new AbortController().signal);
  const bytes = new Uint8Array(await blob.arrayBuffer()), view=new DataView(bytes.buffer);
  assert.equal(view.getUint32(0,true),0x04034b50);
  assert.equal(view.getUint32(bytes.length-22,true),0x06054b50);
  assert.equal(view.getUint16(bytes.length-12,true),2);
  assert.ok(new TextDecoder().decode(bytes).includes("目录/a.txt"));
});

test("login configuration is encrypted locally and round-trips without native services", async () => {
  const storage=setup()("platform/loginStorage");
  const config={serverAddress:"host:4810",serverProtocol:"https",username:"alice",sessionToken:"secret-session-token"};
  await storage.writeLoginConfig(config);
  const first=localStorage.getItem("cliproam.connection.v1");
  assert.ok(!first.includes(config.sessionToken)); assert.ok(!first.includes(config.username));
  assert.deepEqual(await storage.readLoginConfig(),config);
  await storage.writeLoginConfig(config); assert.notEqual(localStorage.getItem("cliproam.connection.v1"),first);
  await storage.writeLoginConfig(null); assert.equal(await storage.readLoginConfig(),null);
});

test("tampered ciphertext is rejected and a fresh login replaces it", async () => {
  const storage=setup()("platform/loginStorage"),config={serverAddress:"host:4810",serverProtocol:"http",username:"u",sessionToken:"t"};
  await storage.writeLoginConfig(config);
  const stored=JSON.parse(localStorage.getItem("cliproam.connection.v1"));
  stored.ciphertext="AAAA"; localStorage.setItem("cliproam.connection.v1",JSON.stringify(stored));
  await assert.rejects(storage.readLoginConfig(),/无法解密/);
  await storage.writeLoginConfig(config); assert.deepEqual(await storage.readLoginConfig(),config);
});

test("a fresh login recovers a missing or damaged local encryption key", async () => {
  const storage=setup()("platform/loginStorage"),config={serverAddress:"host:4810",serverProtocol:"http",username:"u",sessionToken:"t"};
  for (const damaged of [null,"invalid-key"]) {
    await storage.writeLoginConfig(config);
    if (damaged === null) localStorage.removeItem("cliproam.storage-key.v1");
    else localStorage.setItem("cliproam.storage-key.v1",damaged);
    await assert.rejects(storage.readLoginConfig(),/无法解密/);
    await storage.writeLoginConfig(config);
    assert.deepEqual(await storage.readLoginConfig(),config);
  }
});

test("serialized configuration updates cannot restore a token after logout", async () => {
  const config=setup()("features/sync/syncSession");
  await Promise.all([
    config.persistSyncConfig({serverAddress:"host:4810",serverProtocol:"http",username:"u",sessionToken:"old-token"}),
    config.persistSyncConfig(null),
  ]);
  assert.equal(await config.loadSyncConfig(),null);
});

function downloadHarness(load) {
  const { AccountDatabase } = load("core/database"), { AccountFiles } = load("platform/files"), { HistoryCache } = load("core/historyCache");
  const account = crypto.randomUUID(), controller = new AbortController(), files = new AccountFiles(account);
  const session = { accountKey:account,signal:controller.signal, database:new AccountDatabase(account), files, cache:new HistoryCache(files),
    config:{serverAddress:"host:4810",serverProtocol:"http",sessionToken:"token"},
    state:{ downloadTasks:{value:[]},downloadRecords:{value:[]},entryDownloadToasts:{value:[]} },
    schedule:(_key,work,delay) => { if (delay < 60000) queueMicrotask(work); }, start:(work) => work(controller.signal),
  };
  const manager=new (load("core/downloadManager").DownloadManager)(session);
  return {session,manager,controller};
}

test("concurrent batches share one file transfer and persist independent records", async () => {
  const load=setup(), {session,manager}=downloadHarness(load), bytes=new TextEncoder().encode("download"), fileId=digest(bytes);
  await session.cache.upsert([{...entry(),kind:"image",imageInfo:{fileId,size:bytes.length,thumbnail:""}}]);
  const original=globalThis.fetch; let calls=0;
  globalThis.fetch=async () => {calls++; return new Response(bytes);};
  try {
    const outcomes=await Promise.all([manager.batch("e","first"),manager.batch("e","second")]);
    assert.equal(calls,1); assert.ok(outcomes.every((result) => !result.cancelled && !result.failedCount));
    assert.equal((await session.database.list("downloads")).length,2);
    assert.deepEqual(await session.files.read(fileId,0,bytes.length),bytes);
  } finally {globalThis.fetch=original;session.files.dispose();}
});

test("cancelling a shared transfer settles all requesters without committing partial content", async () => {
  const load=setup(), {session,manager}=downloadHarness(load), bytes=new TextEncoder().encode("cancelled"), fileId=digest(bytes);
  await session.cache.upsert([{...entry(),kind:"image",imageInfo:{fileId,size:bytes.length,thumbnail:""}}]);
  const original=globalThis.fetch; let started;
  const ready=new Promise((resolve) => {started=resolve;});
  globalThis.fetch=async (_url,{signal}) => new Response(new ReadableStream({start(controller) {
    signal.addEventListener("abort",() => controller.error(signal.reason),{once:true}); started();
  }}));
  try {
    const first=manager.batch("e","first"), second=manager.batch("e","second");
    await ready;
    // Wait for both independently persisted callers to attach to the same task.
    for(let i=0;i<100 && manager.tasks()[0]?.requestIds.length!==2;i++) await new Promise((r) => setTimeout(r,2));
    manager.cancelRequest("first");
    const outcomes=await Promise.all([first,second]); assert.ok(outcomes.every((result) => result.cancelled));
    assert.equal(await session.files.resolve(fileId),undefined);
  } finally {globalThis.fetch=original;manager.stop();session.files.dispose();}
});

test("restart marks active downloads interrupted and retains only 500 completed records", async () => {
  const load=setup(), {session,manager}=downloadHarness(load);
  await session.database.batch(Array.from({length:502},(_,i) => ({name:"downloads",id:String(i),value:{
    id:String(i),entryId:"e",label:"x",size:1,fileCount:1,status:i===501?"downloading":"succeeded",
    createdMs:Date.now()+i,finishedMs:i===501?null:Date.now(),receivedBytes:0,destination:"",error:null,files:[],
  }})));
  await manager.restore(); const records=manager.records(); assert.equal(records.length,500);
  assert.equal(records.find((record) => record.id==="501").status,"interrupted"); assert.ok(records.every((record) => record.finishedMs!==null));
});

test("cancellation during local availability checks never starts a late download", async () => {
  const load=setup(),{session,manager}=downloadHarness(load),fileId="a".repeat(64);
  await session.cache.upsert([{...entry(),kind:"image",imageInfo:{fileId,size:1,thumbnail:""}}]);
  let release,checking;
  const ready=new Promise((resolve) => {checking=resolve;});
  session.files.resolve=() => new Promise((resolve) => {release=resolve;checking();});
  const operation=manager.batch("e","cancel-before-start"); await ready;
  await manager.cancelRecord("cancel-before-start"); release(undefined);
  assert.equal((await operation).cancelled,true); assert.equal(manager.tasks().length,0);
});

test("restart removes only partial Blob chunks and retains verified content", async () => {
  const load=setup(),{session,manager}=downloadHarness(load);
  const id="a".repeat(64);
  await session.database.put("blobs",id,new Blob(["complete"]));
  await session.database.put("blobs","partial:old:0",new Blob(["partial"]));
  await manager.restore(); assert.deepEqual(await session.database.ids("blobs"),[id]);
});
