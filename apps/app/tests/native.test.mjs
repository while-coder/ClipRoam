import assert from "node:assert/strict";
import { test } from "node:test";
import { sourceLoader } from "./loadSource.mjs";

const load = sourceLoader();
const { NativeRust } = load("platform/native_rust"), { NativeWeb } = load("platform/native_web");

test("Rust adapter preserves binary file content and clipboard images across the bridge", async () => {
  const bytes = Uint8Array.from([0, 255, 128, 10, 13]);
  let stored, image;
  const native = new NativeRust(async (command, args) => {
    if (command === "native_fs_write") stored = Buffer.from(args.data, "base64");
    else if (command === "native_fs_read") return stored.subarray(args.offset, args.offset + args.length).toString("base64");
    else if (command === "native_write_clipboard") image = args;
    else throw new Error(command);
  });
  await native.writeFile("image", bytes, false);
  assert.deepEqual(await native.readFile("image", 1, 3), bytes.subarray(1, 4));
  await native.writeClipboard({kind:"image",bytes}, true);
  assert.equal(image.synthesize, true);
  assert.equal(image.payload.kind, "image");
  assert.deepEqual(new Uint8Array(Buffer.from(image.payload.data,"base64")), bytes);
});

test("Web adapter reads/writes clipboard text and keeps unsupported native operations explicit", async () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis,"navigator");
  let text = "  ", written;
  Object.defineProperty(globalThis,"navigator",{configurable:true,value:{
    userAgent:"test-browser",clipboard:{readText:async () => text,writeText:async (value) => { written = value; }},
  }});
  try {
    const native = new NativeWeb();
    assert.equal(await native.readClipboard(true), null);
    text = "hello";
    assert.deepEqual(await native.readClipboard(true),{kind:"text",text:"hello"});
    await native.writeClipboard({kind:"text",text:"world"},false);
    assert.equal(written,"world");
    assert.equal((await native.getDeviceInfo()).osVersion,"test-browser");
    assert.equal((await native.getCapabilities()).clipboardMonitoring,false);
    assert.deepEqual(await native.selectSaveTarget(true,"a.txt"),{kind:"browser",value:"a.txt"});
    assert.deepEqual(await native.pendingShares("account"),[]);
    assert.equal(await native.previewSource("blob:preview"),"blob:preview");
    await native.watchClipboard("account",true);
    await assert.rejects(native.readFile("local",0,3),/需要桌面或手机客户端/);
    await assert.rejects(native.writeClipboard({kind:"files",paths:["local"]},false),/不支持系统文件剪贴板/);
  } finally {
    if (descriptor) Object.defineProperty(globalThis,"navigator",descriptor);
    else delete globalThis.navigator;
  }
});
