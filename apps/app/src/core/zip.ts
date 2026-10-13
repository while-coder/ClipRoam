/** Stored ZIP archives retain directory structure without a platform API. */
const crcTable = new Uint32Array(256);
for (let i = 0; i < 256; i++) { let n = i; for (let j = 0; j < 8; j++) n = n & 1 ? 0xedb88320 ^ (n >>> 1) : n >>> 1; crcTable[i] = n >>> 0; }

export async function zipFiles(files: { name: string; blob: Blob }[], signal: AbortSignal): Promise<Blob> {
  const parts: BlobPart[] = [], directory: Uint8Array[] = [];
  let offset = 0, directorySize = 0;
  for (const file of files) {
    signal.throwIfAborted();
    if (file.blob.size > 0xffffffff || offset > 0xffffffff || files.length > 65535) throw new Error("浏览器 ZIP 导出超过 4 GB 或文件数量上限");
    let crc = 0xffffffff;
    const reader = file.blob.stream().getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read(); if (done) break;
        signal.throwIfAborted(); for (const byte of value) crc = crcTable[(crc ^ byte) & 255]! ^ (crc >>> 8);
      }
    } finally { await reader.cancel().catch(() => undefined); reader.releaseLock(); }
    crc = (crc ^ 0xffffffff) >>> 0;
    const name = new TextEncoder().encode(file.name);
    if (name.length > 65535) throw new Error("文件名过长");
    const local = new Uint8Array(30 + name.length), lv = new DataView(local.buffer);
    lv.setUint32(0,0x04034b50,true); lv.setUint16(4,20,true); lv.setUint16(6,0x800,true);
    lv.setUint32(14,crc,true); lv.setUint32(18,file.blob.size,true); lv.setUint32(22,file.blob.size,true); lv.setUint16(26,name.length,true); local.set(name,30);
    const central = new Uint8Array(46 + name.length), cv = new DataView(central.buffer);
    cv.setUint32(0,0x02014b50,true); cv.setUint16(4,20,true); cv.setUint16(6,20,true); cv.setUint16(8,0x800,true);
    cv.setUint32(16,crc,true); cv.setUint32(20,file.blob.size,true); cv.setUint32(24,file.blob.size,true); cv.setUint16(28,name.length,true);
    cv.setUint32(38,file.name.endsWith("/") ? 0x10 : 0,true); cv.setUint32(42,offset,true); central.set(name,46);
    parts.push(local, file.blob); directory.push(central); directorySize += central.length; offset += local.length + file.blob.size;
  }
  if (offset + directorySize > 0xffffffff) throw new Error("浏览器 ZIP 导出超过 4 GB 上限");
  const end = new Uint8Array(22), ev = new DataView(end.buffer);
  ev.setUint32(0,0x06054b50,true); ev.setUint16(8,files.length,true); ev.setUint16(10,files.length,true);
  ev.setUint32(12,directorySize,true); ev.setUint32(16,offset,true);
  return new Blob([...parts,...directory,end], { type: "application/zip" });
}
