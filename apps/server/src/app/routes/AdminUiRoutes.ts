import { existsSync } from "node:fs";
import { basename, extname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import fastifyStatic from "@fastify/static";
import type { FastifyInstance } from "fastify";

// Packaged builds keep UI files separate from the compiled backend ./admin.
// During development, serve the admin app's Vite output from the workspace.
const BUNDLED_ADMIN_DIRECTORY = fileURLToPath(new URL("../../admin-ui", import.meta.url));
const WORKSPACE_ADMIN_DIRECTORY = fileURLToPath(new URL("../../../admin", import.meta.url));

export function registerAdminUiRoutes(app: FastifyInstance): void {
  app.get("/", async (_request, reply) => reply.redirect("/admin/"));
  app.get("/admin", async (_request, reply) => reply.redirect("/admin/"));

  app.register(async (adminApp) => {
    const directory = existsSync(join(BUNDLED_ADMIN_DIRECTORY, "index.html"))
      ? BUNDLED_ADMIN_DIRECTORY
      : WORKSPACE_ADMIN_DIRECTORY;

    adminApp.addHook("onRequest", async (_request, reply) => {
      if (!existsSync(join(directory, "index.html"))) {
        return reply.code(503).send({ message: "管理后台资源未构建。请先执行 pnpm --filter @cliproam/admin build。" });
      }
    });

    await adminApp.register(fastifyStatic, {
      root: directory,
      dotfiles: "deny",
      cacheControl: false,
      setHeaders(response, assetPath) {
        // Only Vite assets bearing a content hash can be cached indefinitely.
        const hashedAsset = assetPath.startsWith(`${join(directory, "assets")}${sep}`)
          && /-[\w-]{8,}\.[^.]+$/.test(basename(assetPath));
        response.setHeader("Cache-Control", hashedAsset
          ? "public, max-age=31536000, immutable"
          : "no-cache");
      },
    });

    // Keep SPA history fallback inside /admin/, so API 404s remain API 404s.
    adminApp.setNotFoundHandler(async (request, reply) => {
      const pathname = new URL(request.url, "http://localhost").pathname;
      if ((request.method !== "GET" && request.method !== "HEAD")
        || pathname.startsWith("/admin/assets/") || extname(pathname)) {
        return reply.code(404).send({ message: "Not found" });
      }
      return reply.code(200).sendFile("index.html");
    });
  }, { prefix: "/admin" });
}
