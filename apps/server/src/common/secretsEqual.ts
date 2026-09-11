import { timingSafeEqual } from "node:crypto";

// Constant-time comparison for tokens and password hashes. Anything that is
// not a string compares unequal; the length check comes first because
// timingSafeEqual throws on mismatched lengths.
export function secretsEqual(a: unknown, b: unknown): boolean {
  if (typeof a !== "string" || typeof b !== "string") return false;
  const left = Buffer.from(a);
  const right = Buffer.from(b);
  return left.length === right.length && timingSafeEqual(left, right);
}
