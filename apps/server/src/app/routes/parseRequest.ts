import type { FastifyReply } from "fastify";

// Any zod schema satisfies this structurally; importing zod directly would
// make it the server's dependency when it is protocol's.
type Parsable<T> = {
  safeParse(value: unknown): { success: true; data: T } | { success: false };
};

// Shared safeParse + 400 pattern for route bodies and query strings: sends the
// 400 itself, so a handler only has to bail out when it returns undefined.
export function parseOr400<T>(reply: FastifyReply, schema: Parsable<T>, value: unknown, message: string): T | undefined {
  const parsed = schema.safeParse(value);
  if (!parsed.success) void reply.code(400).send({ message });
  return parsed.success ? parsed.data : undefined;
}
