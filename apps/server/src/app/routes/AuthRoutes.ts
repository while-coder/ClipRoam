import type { FastifyInstance } from "fastify";
import type { AuthService } from "../../services/AuthService.js";

export type AuthRouteDeps = {
  auth: Pick<AuthService, "register" | "login" | "changePassword" | "authenticateSession">;
  // Changing the password invalidates every live session, so the socket for
  // each of the user's devices is closed and the clients re-login.
  onPasswordChanged: (userId: string) => void;
};

export function registerAuthRoutes(app: FastifyInstance, deps: AuthRouteDeps): void {
  const { auth, onPasswordChanged } = deps;

  app.post("/auth/register", async (request, reply) => {
    const result = await auth.register(request.body);
    return reply.code(result.statusCode).send(result.payload);
  });

  app.post("/auth/login", async (request, reply) => {
    const result = await auth.login(request.ip, request.body);
    return reply.code(result.statusCode).send(result.payload);
  });

  app.post("/auth/password", async (request, reply) => {
    const token = readBearerToken(request.headers.authorization);
    const user = token ? auth.authenticateSession(token) : undefined;
    const result = await auth.changePassword(request.ip, token, request.body);
    if (result.statusCode === 204 && user) onPasswordChanged(user.id);
    return reply.code(result.statusCode).send(result.payload);
  });
}

// Shared by every place that authenticates a Bearer header, including the
// server-wide `onRequest` hook that fills `request.sessionUser`.
export function readBearerToken(header: string | undefined): string | undefined {
  const match = header?.match(/^Bearer\s+(.+)$/i);
  return match?.[1]?.trim() || undefined;
}
