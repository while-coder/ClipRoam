/// <reference path="./utif.d.ts" />

// Consumers compile this shared source with their own tsconfig; include the UTIF types explicitly.
/** Decode only the first TIFF page and bound its RGBA allocation before decompression. */
export async function decodeTiff(bytes: Uint8Array): Promise<{ width: number; height: number; rgba: Uint8ClampedArray }> {
  const imported = await import("utif");
  const TIFF = ("default" in imported ? imported.default : imported) as typeof imported;
  const buffer = bytes.slice().buffer;
  const image = TIFF.decode(buffer)[0];
  const width = image?.t256?.[0] ?? 0;
  const height = image?.t257?.[0] ?? 0;
  if (!image || width <= 0 || height <= 0 || width * height > 16_000_000) throw new Error("TIFF 尺寸超过预览上限");
  TIFF.decodeImage(buffer, image);
  return { width: image.width, height: image.height, rgba: new Uint8ClampedArray(TIFF.toRGBA8(image)) };
}
