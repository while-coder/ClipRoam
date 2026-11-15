declare module "utif" {
  interface Image { width: number; height: number; t256?: number[]; t257?: number[]; }
  export function decode(buffer: ArrayBuffer): Image[];
  export function decodeImage(buffer: ArrayBuffer, image: Image): void;
  export function toRGBA8(image: Image): Uint8Array;
}
