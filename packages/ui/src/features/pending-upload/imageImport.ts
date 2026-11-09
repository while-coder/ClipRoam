import { bytesToBase64 } from "../../utils";
import type { ClipboardData } from "../../platform/clipboard";

// Match the raster formats supported by the native image decoder.
export function isImageFile(name: string): boolean {
  return /\.(png|jpe?g|webp|bmp|gif)$/i.test(name);
}

/** Encode browser and iOS file-picker images using the existing WebP contract. */
export async function imageFromFile(file: File): Promise<Extract<ClipboardData, { kind: "image" }>> {
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error("图片读取失败"));
      image.src = url;
    });
    const canvas = document.createElement("canvas");
    canvas.width = image.naturalWidth; canvas.height = image.naturalHeight;
    const context = canvas.getContext("2d");
    if (!context || !canvas.width || !canvas.height) throw new Error("图片读取失败");
    context.drawImage(image, 0, 0);
    const blob = await new Promise<Blob>((resolve, reject) => {
      canvas.toBlob((value) => value?.type === "image/webp" ? resolve(value) : reject(new Error("不支持 WebP 编码")), "image/webp");
    });
    const scale = Math.min(1, 64 / Math.max(canvas.width, canvas.height));
    const width = canvas.width, height = canvas.height;
    canvas.width = Math.max(1, Math.round(width * scale));
    canvas.height = Math.max(1, Math.round(height * scale));
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    const thumbnail = canvas.toDataURL("image/webp");
    if (!thumbnail.startsWith("data:image/webp;base64,")) throw new Error("不支持 WebP 编码");
    return { kind: "image", data: bytesToBase64(new Uint8Array(await blob.arrayBuffer())), width, height,
      thumbnail: thumbnail.slice("data:image/webp;base64,".length) };
  } finally { URL.revokeObjectURL(url); }
}
