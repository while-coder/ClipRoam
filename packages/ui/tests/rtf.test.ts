import assert from "node:assert/strict";
import test from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>");
Object.assign(globalThis, { window: dom.window, document: dom.window.document, self: dom.window });
const { prepareRtf, attachmentPath, renderRtf } = await import("../src/components/preview/rtf.ts");
const { rtfdDocument, filePreviewKind, FilePreviewKind } = await import("../src/features/history/filePreview.ts");
const { decodeTiff } = await import("../src/components/preview/tiff.ts");
const bytes = (text: string) => Uint8Array.from(text, char => char.charCodeAt(0));

test("recognizes RTF files and RTFD directory packages including nested images", () => {
  const tree = { "TXT.rtf": { f: "body", s: 100 }, Images: { "a.png": { f: "image", s: 12 } }, empty: {} };
  const document = rtfdDocument("文稿.RTFD", "folder/文稿.RTFD", tree)!;
  assert.equal(document.body.fileId, "body");
  assert.equal(document.files.find(file => file.fileId === "image")?.path, "Images/a.png");
  assert.equal(document.tree, tree);
  assert.equal(filePreviewKind("文稿.RTF"), FilePreviewKind.RichText);
  assert.equal(filePreviewKind("文稿.rtfd"), FilePreviewKind.RichText);
  assert.equal(rtfdDocument("folder", "folder", tree), undefined);
  assert.equal(rtfdDocument("broken.rtfd", "broken.rtfd", {}), undefined);
});

test("leaves ordinary RTF bytes and escaped braces intact", () => {
  const input = bytes(String.raw`{\rtf1\ansi Text \{escaped\} \\path \'e9}`);
  assert.deepEqual(prepareRtf(input).bytes, input);
});

test("extracts NeXTGraphic names with spaces, UTF-8 hex and escaped braces", () => {
  const input = String.raw`{\rtf1 Before {\NeXTGraphic \'e5\'9b\'be \'e7\'89\'87\{1\}.png \width100\height200} after}`;
  const prepared = prepareRtf(bytes(input));
  assert.equal(prepared.attachments[0]?.name, "图 片{1}.png");
  const output = new TextDecoder().decode(prepared.bytes);
  assert.ok(output.includes(prepared.attachments[0]!.marker));
  assert.ok(output.endsWith(" after}"));
  assert.ok(!output.includes("NeXTGraphic"));
});

test("ignores attachment-like sequences inside binary data and escaped groups", () => {
  const binary = String.raw`{\NeXTGraphic fake.png}`;
  const input = bytes(`{\\rtf1\\bin${binary.length} ${binary} text}`);
  assert.equal(prepareRtf(input).attachments.length, 0);
  assert.equal(prepareRtf(bytes(String.raw`{\rtf1 \{\NeXTGraphic fake.png\}}`)).attachments.length, 0);
});

test("rejects unsupported flat RTFD, oversized bodies and incomplete references", () => {
  assert.throws(() => prepareRtf(bytes("not rtf")), /RTF/);
  assert.throws(() => prepareRtf(new Uint8Array(5 * 1024 * 1024 + 1)), /5 MiB/);
  assert.throws(() => prepareRtf(bytes(String.raw`{\rtf1 {\NeXTGraphic a.png`)), /不完整/);
  assert.throws(() => prepareRtf(bytes(`{\\rtf1 ${String.raw`{\NeXTGraphic a.png}`.repeat(101)}}`)), /100/);
});

test("attachment paths stay in the package and never resolve external URLs", () => {
  for (const name of ["../x.png", "/x.png", "https://host/x.png", "file:x.png", "C:\\x.png", "a/../x.png", "a//x.png", "a\0.png"]) {
    assert.equal(attachmentPath(name), undefined, name);
  }
  assert.equal(attachmentPath("Images/a b.png"), "Images/a b.png");
});

test("renders Chinese text, paragraphs, bold, italic and color", async () => {
  const input = String.raw`{\rtf1\ansi\deff0{\fonttbl{\f0 Arial;}}{\colortbl;\red255\green0\blue0;}\f0\fs24 Hello \b bold\b0 \i italic\i0 \cf1 red\cf0\par \u20013?\u25991?}`;
  const rendered = await renderRtf(prepareRtf(bytes(input)).bytes);
  assert.match(rendered.textContent!, /Hello bolditalicred中文/);
  assert.ok(rendered.querySelector('[style*="font-weight: bold"]'));
  assert.ok(rendered.querySelector('[style*="font-style: italic"]'));
  assert.ok(rendered.querySelector('[style*="color: rgb(255, 0, 0)"]'));
});

test("attachment markers survive rendering at the intended text position", async () => {
  const prepared = prepareRtf(bytes(String.raw`{\rtf1 before {\NeXTGraphic photo.png \width100\height200} after}`));
  const rendered = await renderRtf(prepared.bytes);
  assert.ok(rendered.textContent!.includes(`before ${prepared.attachments[0]!.marker} after`));
});

test("document markup cannot introduce active HTML or navigable links", async () => {
  const rendered = await renderRtf(bytes(String.raw`{\rtf1 <script>alert(1)</script>{\field{\*\fldinst HYPERLINK "javascript:alert(1)"}{\fldrslt Click}}}`));
  assert.equal(rendered.querySelector("script,a,iframe,object,style,form"), null);
  assert.match(rendered.textContent!, /<script>alert\(1\)<\/script>/);
});

test("embedded RTF PNG images retain their data without remote requests", async () => {
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jC5sAAAAASUVORK5CYII=", "base64").toString("hex");
  const rendered = await renderRtf(bytes(`{\\rtf1 before {\\pict\\pngblip\\picw1\\pich1 ${png}} after}`));
  assert.match(rendered.querySelector("img")?.getAttribute("src") ?? "", /^data:image\/png;base64,/);
  assert.equal(rendered.querySelectorAll("img").length, 1);
});

test("external IMPORT fields never fetch a document resource", async () => {
  const rendered = await renderRtf(bytes(String.raw`{\rtf1 before {\field{\*\fldinst IMPORT "https://example.invalid/track.png"}{\fldrslt fallback}} after}`));
  assert.equal(rendered.querySelector("img[src^='http']"), null);
  assert.match(rendered.textContent!, /before/);
  assert.match(rendered.textContent!, /after/);
});

test("TIFF attachments decode into the expected RGBA pixels", async () => {
  const imported = await import("utif");
  const TIFF = imported.default ?? imported;
  const rgba = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255]);
  const encoded = new Uint8Array(TIFF.encodeImage(rgba.buffer, 2, 1));
  const result = await decodeTiff(encoded);
  assert.equal(result.width, 2);
  assert.equal(result.height, 1);
  assert.deepEqual(new Uint8Array(result.rgba), rgba);
});

test("oversized TIFF dimensions are rejected before image decompression", async () => {
  const imported = await import("utif");
  const TIFF = imported.default ?? imported;
  const encoded = new Uint8Array(TIFF.encodeImage(new Uint8Array([0, 0, 0, 255]).buffer, 1, 1));
  const view = new DataView(encoded.buffer);
  // UTIF's encoder writes a big-endian IFD; replace its width tag without allocating pixels.
  const ifd = view.getUint32(4);
  for (let index = 0; index < view.getUint16(ifd); index++) {
    const offset = ifd + 2 + index * 12;
    if (view.getUint16(offset) === 256) {
      view.setUint16(offset + 2, 4); // LONG
      view.setUint32(offset + 8, 20_000_000);
    }
  }
  await assert.rejects(decodeTiff(encoded), /TIFF/);
});
