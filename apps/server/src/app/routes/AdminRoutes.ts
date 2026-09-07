import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import type { FastifyInstance } from "fastify";
import type { AdminService } from "../../services/AdminService.js";
import type { TlsCertificateService, TlsOptions } from "../../services/TlsCertificateService.js";
import { getTransferSettings, updateTransferSettings, type ServerConfig } from "../ServerConfig.js";

const adminSessionCookie = "cliproam_admin";
const adminSessionMaxAgeSeconds = 8 * 60 * 60;
// The admin app ships inside the server package once built; the workspace
// fallback serves it straight from the monorepo during development.
const bundledAdminDirectory = fileURLToPath(new URL("../../admin", import.meta.url));
const workspaceAdminDirectory = fileURLToPath(new URL("../../../admin", import.meta.url));

export type AdminRouteDeps = {
  admin: AdminService;
  tls: TlsCertificateService;
  config: ServerConfig;
  // The running HTTP(S) server, so a new TLS certificate can be applied
  // without a restart when the runtime supports it.
  liveServer: { setSecureContext?: (context: TlsOptions) => void };
};

export function registerAdminRoutes(app: FastifyInstance, deps: AdminRouteDeps): void {
  const { admin, tls, config, liveServer } = deps;

  const requireAdmin = (
    request: { headers: { cookie?: string } },
    reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown } },
  ): boolean => {
    if (admin.authenticate(readCookie(request.headers.cookie, adminSessionCookie))) return true;
    reply.code(401).send({ code: "ADMIN_AUTH_REQUIRED", message: "请先登录管理后台。" });
    return false;
  };

  const adminCookie = (value: string, maxAge: number): string => {
    const parts = [
      `${adminSessionCookie}=${value}`,
      "HttpOnly",
      "Path=/admin",
      "SameSite=Strict",
      `Max-Age=${maxAge}`,
    ];
    if (tls.status.enabled) parts.push("Secure");
    return parts.join("; ");
  };

  app.post("/admin/api/login", async (request, reply) => {
    const password = request.body && typeof request.body === "object" && "password" in request.body
      ? (request.body as { password?: unknown }).password
      : undefined;
    const result = admin.login(request.ip, password);
    if ("error" in result) {
      const responses = {
        NOT_CONFIGURED: [503, "管理员密码未配置。请设置 CLIPROAM_ADMIN_PASSWORD 后重启服务。"],
        INVALID_CREDENTIALS: [401, "管理员密码错误。"],
        TOO_MANY_ATTEMPTS: [429, "登录尝试过多，请稍后再试。"],
      } as const;
      const [statusCode, message] = responses[result.error];
      return reply.code(statusCode).send({ code: result.error, message });
    }
    reply.header("Set-Cookie", adminCookie(result.token, adminSessionMaxAgeSeconds));
    return { ok: true };
  });

  app.post("/admin/api/logout", async (request, reply) => {
    admin.logout(readCookie(request.headers.cookie, adminSessionCookie));
    reply.header("Set-Cookie", adminCookie("", 0));
    return { ok: true };
  });

  app.get("/admin/api/status", async (request, reply) => {
    if (!requireAdmin(request, reply)) return;
    return { tls: tls.status, transfer: getTransferSettings(config) };
  });

  app.put("/admin/api/transfer-settings", async (request, reply) => {
    if (!requireAdmin(request, reply)) return;
    try {
      return { transfer: updateTransferSettings(config, request.body) };
    } catch (error) {
      return reply.code(400).send({
        code: "INVALID_TRANSFER_SETTINGS",
        message: error instanceof Error ? error.message : "传输配置无效。",
      });
    }
  });

  app.put("/admin/api/tls", async (request, reply) => {
    if (!requireAdmin(request, reply)) return;
    const body = request.body as { cert?: unknown; key?: unknown } | undefined;
    try {
      const options = tls.replace(body?.cert, body?.key);
      if (liveServer.setSecureContext) {
        liveServer.setSecureContext(options);
        return { tls: tls.status, restartRequired: false };
      }
      return { tls: tls.status, restartRequired: true };
    } catch (error) {
      return reply.code(400).send({
        code: "INVALID_TLS_CONFIGURATION",
        message: error instanceof Error ? error.message : "证书配置无效。",
      });
    }
  });

  app.delete("/admin/api/tls", async (request, reply) => {
    if (!requireAdmin(request, reply)) return;
    try {
      tls.remove();
      return { tls: tls.status, restartRequired: true };
    } catch (error) {
      return reply.code(400).send({
        code: "INVALID_TLS_CONFIGURATION",
        message: error instanceof Error ? error.message : "证书删除失败。",
      });
    }
  });

  app.get("/admin", async (_request, reply) => serveAdminAsset("", reply));
  app.get("/admin/*", async (request, reply) => {
    const path = (request.params as { "*"?: string })["*"] ?? "";
    return serveAdminAsset(path, reply);
  });
}

function readCookie(header: string | undefined, name: string): string | undefined {
  if (!header) return undefined;
  const prefix = `${name}=`;
  return header.split(";").map((part) => part.trim()).find((part) => part.startsWith(prefix))?.slice(prefix.length);
}

async function serveAdminAsset(requestPath: string, reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown }; type: (contentType: string) => { send: (payload: unknown) => unknown } }): Promise<unknown> {
  const directory = existsSync(bundledAdminDirectory) ? bundledAdminDirectory : workspaceAdminDirectory;
  if (!existsSync(directory)) {
    return reply.code(503).send({ message: "管理后台资源未构建。请先执行 pnpm --filter @cliproam/admin build。" });
  }

  const relativePath = requestPath || "index.html";
  const assetPath = resolve(directory, relativePath);
  if (!assetPath.startsWith(`${directory}${sep}`) && assetPath !== directory) {
    return reply.code(404).send({ message: "Not found" });
  }
  try {
    if (!(await stat(assetPath)).isFile()) throw new Error("Not a file");
    return reply.type(contentTypeFor(assetPath)).send(await readFile(assetPath));
  } catch {
    if (extname(relativePath)) return reply.code(404).send({ message: "Not found" });
    return reply.type("text/html; charset=utf-8").send(await readFile(join(directory, "index.html")));
  }
}

function contentTypeFor(path: string): string {
  switch (extname(path)) {
    case ".html": return "text/html; charset=utf-8";
    case ".css": return "text/css; charset=utf-8";
    case ".js": return "text/javascript; charset=utf-8";
    case ".svg": return "image/svg+xml";
    case ".json": return "application/json; charset=utf-8";
    case ".ico": return "image/x-icon";
    case ".png": return "image/png";
    default: return "application/octet-stream";
  }
}
