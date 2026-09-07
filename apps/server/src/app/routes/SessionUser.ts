import type { FastifyReply, FastifyRequest } from "fastify";

// The authenticated caller of an HTTP route. The server's central `onRequest`
// hook resolves the Bearer token once and parks the result here.
export type SessionUser = { id: string };

declare module "fastify" {
  interface FastifyRequest {
    sessionUser?: SessionUser;
  }
}

// Shared guard for every authenticated route: sends the 401 itself, so a
// handler only has to bail out when it returns undefined.
export function requireSessionUser(request: FastifyRequest, reply: FastifyReply): SessionUser | undefined {
  const user = request.sessionUser;
  if (!user) void reply.code(401).send({ message: "登录已失效，请重新登录" });
  return user;
}
