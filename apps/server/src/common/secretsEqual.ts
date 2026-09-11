import { timingSafeEqual } from "node:crypto";

// Constant-time comparison for tokens and password hashes; the length check
// comes first because timingSafeEqual throws on mismatched lengths.
export function secretsEqual(a: string | Buffer, b: string | Buffer): boolean {
  const left = Buffer.isBuffer(a) ? a : Buffer.from(a);
  const right = Buffer.isBuffer(b) ? b : Buffer.from(b);
  return left.length === right.length && timingSafeEqual(left, right);
}
