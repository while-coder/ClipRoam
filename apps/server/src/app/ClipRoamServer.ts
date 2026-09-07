import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import Fastify, { type FastifyInstance } from "fastify";
import websocket, { type WebSocket } from "@fastify/websocket";
import {
  ClientMessageSchema,
  FILE_CHUNK_SIZE,
  FileIdSchema,
  MAX_MESSAGE_BYTES,
  UploadBeginRequestSchema,
  type ClientMessage,
  type Device,
  type ServerMessage,
} from "@cliproam/protocol";
import { AuthService } from "../services/AuthService.js";
import { AdminService } from "../services/AdminService.js";
import { getLogger } from "./Logger.js";
import type { ClientConnection, ConnectionTarget } from "./Connection.js";
import { FileDownloadService } from "../files/FileDownloadService.js";
import { UploadHttpError, UploadService } from "../files/UploadService.js";
import { getTransferSettings, loadServerConfig, updateTransferSettings, type ServerConfig } from "./ServerConfig.js";
import { ClipRoamStore } from "../storage/ClipRoamStore.js";
import { TlsCertificateService, type TlsOptions } from "../services/TlsCertificateService.js";

const adminSessionCookie = "cliproam_admin";
const adminSessionMaxAgeSeconds = 8 * 60 * 60;
const garbageCollectionIntervalMs = 6 * 60 * 60 * 1_000;
const logger = getLogger("ClipRoamServer");
const bundledAdminDirectory = fileURLToPath(new URL("../admin", import.meta.url));
const workspaceAdminDirectory = fileURLToPath(new URL("../../admin", import.meta.url));

export class ClipRoamServer {
  readonly #tls = new TlsCertificateService();
  readonly #app = createApp(this.#tls.options);
  readonly #store = new ClipRoamStore();
  readonly #auth = new AuthService(this.#store);
  readonly #admin = new AdminService();
  readonly #clients = new Set<ClientConnection>();
  readonly #downloads: FileDownloadService;
  readonly #uploads: UploadService;
  #collectionTimer?: NodeJS.Timeout;

  constructor(private readonly config: ServerConfig = loadServerConfig()) {
    this.#downloads = new FileDownloadService(
      this.#store.files(),
      this.#store.canReadFile.bind(this.#store),
      this.#send.bind(this),
    );
    this.#uploads = new UploadService(
      this.#store.files(),
      config,
      this.#publishFileAvailability.bind(this),
    );
  }

  get port(): number { return this.config.port; }

  get adminUrl(): string {
    const protocol = this.#tls.status.enabled ? "https" : "http";
    return `${protocol}://localhost:${this.config.port}/admin`;
  }

  async start(): Promise<void> {
    await this.#app.register(websocket, { options: { maxPayload: MAX_MESSAGE_BYTES } });
    this.#registerRoutes();
    this.#collectionTimer = setInterval(() => {
      this.#collectGarbage();
    }, garbageCollectionIntervalMs);
    this.#collectionTimer.unref();
    await this.#app.listen({ port: this.config.port, host: "0.0.0.0" });
  }

  async stop(): Promise<void> {
    if (this.#collectionTimer) clearInterval(this.#collectionTimer);
    await this.#app.close();
  }

  #registerRoutes(): void {
    this.#app.addHook("onRequest", async (request, reply) => {
      reply
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Headers", "Content-Type, Authorization")
        .header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS");
      if (request.method === "OPTIONS") return reply.code(204).send();
    });
    // Upload chunks arrive as raw bytes rather than base64-in-JSON, which the
    // WebSocket path was forced into by its text framing.
    this.#app.addContentTypeParser(
      "application/octet-stream",
      { parseAs: "buffer" },
      (_request, body, done) => done(null, body),
    );
    this.#app.post("/upload/begin", async (request, reply) => {
      const user = this.#sessionUser(request);
      if (!user) return reply.code(401).send({ message: "登录已失效，请重新登录" });
      const parsed = UploadBeginRequestSchema.safeParse(request.body);
      if (!parsed.success) return reply.code(400).send({ message: "上传参数无效" });
      try {
        return this.#uploads.begin(parsed.data.fileId, parsed.data.size);
      } catch (error) {
        return this.#uploadError(reply, error);
      }
    });
    this.#app.put("/upload/:fileId", { bodyLimit: FILE_CHUNK_SIZE + 4096 }, async (request, reply) => {
      const user = this.#sessionUser(request);
      if (!user) return reply.code(401).send({ message: "登录已失效，请重新登录" });
      const { fileId } = request.params as { fileId: string };
      const index = Number((request.query as { index?: string }).index);
      const chunk = request.body;
      // A session id from a pre-ledger client can never be a content id, so it
      // is rejected here instead of reaching the store.
      if (!FileIdSchema.safeParse(fileId).success || !Number.isInteger(index) || index < 0 || !Buffer.isBuffer(chunk)) {
        return reply.code(400).send({ message: "上传参数无效" });
      }
      try {
        return this.#uploads.uploadPart(fileId, index, chunk);
      } catch (error) {
        return this.#uploadError(reply, error);
      }
    });

    this.#app.get("/health", async () => ({ status: "ok", service: "cliproam-server" }));
    this.#app.post("/auth/register", async (request, reply) => {
      const result = await this.#auth.register(request.body);
      return reply.code(result.statusCode).send(result.payload);
    });
    this.#app.post("/auth/login", async (request, reply) => {
      const result = await this.#auth.login(request.ip, request.body);
      return reply.code(result.statusCode).send(result.payload);
    });
    this.#app.post("/auth/password", async (request, reply) => {
      const token = readBearerToken(request.headers.authorization);
      const user = token ? this.#auth.authenticateSession(token) : undefined;
      const result = await this.#auth.changePassword(request.ip, token, request.body);
      if (result.statusCode === 204 && user) this.#disconnectUserClients(user.id);
      return reply.code(result.statusCode).send(result.payload);
    });
    this.#app.get("/ws", { websocket: true }, (socket) => this.#handleSocket(socket));
    this.#registerAdminRoutes();
    this.#app.addHook("onClose", async () => this.#store.close());
  }

  #registerAdminRoutes(): void {
    this.#app.post("/admin/api/login", async (request, reply) => {
      const password = request.body && typeof request.body === "object" && "password" in request.body
        ? (request.body as { password?: unknown }).password
        : undefined;
      const result = this.#admin.login(request.ip, password);
      if ("error" in result) {
        const responses = {
          NOT_CONFIGURED: [503, "管理员密码未配置。请设置 CLIPROAM_ADMIN_PASSWORD 后重启服务。"],
          INVALID_CREDENTIALS: [401, "管理员密码错误。"],
          TOO_MANY_ATTEMPTS: [429, "登录尝试过多，请稍后再试。"],
        } as const;
        const [statusCode, message] = responses[result.error];
        return reply.code(statusCode).send({ code: result.error, message });
      }
      reply.header("Set-Cookie", this.#adminCookie(result.token, adminSessionMaxAgeSeconds));
      return { ok: true };
    });

    this.#app.post("/admin/api/logout", async (request, reply) => {
      this.#admin.logout(readCookie(request.headers.cookie, adminSessionCookie));
      reply.header("Set-Cookie", this.#adminCookie("", 0));
      return { ok: true };
    });

    this.#app.get("/admin/api/status", async (request, reply) => {
      if (!this.#requireAdmin(request.headers.cookie, reply)) return;
      return { tls: this.#tls.status, transfer: getTransferSettings(this.config) };
    });

    this.#app.put("/admin/api/transfer-settings", async (request, reply) => {
      if (!this.#requireAdmin(request.headers.cookie, reply)) return;
      try {
        return { transfer: updateTransferSettings(this.config, request.body) };
      } catch (error) {
        return reply.code(400).send({
          code: "INVALID_TRANSFER_SETTINGS",
          message: error instanceof Error ? error.message : "传输配置无效。",
        });
      }
    });

    this.#app.put("/admin/api/tls", async (request, reply) => {
      if (!this.#requireAdmin(request.headers.cookie, reply)) return;
      const body = request.body as { cert?: unknown; key?: unknown } | undefined;
      try {
        const options = this.#tls.replace(body?.cert, body?.key);
        const liveServer = this.#app.server as unknown as { setSecureContext?: (context: TlsOptions) => void };
        if (liveServer.setSecureContext) {
          liveServer.setSecureContext(options);
          return { tls: this.#tls.status, restartRequired: false };
        }
        return { tls: this.#tls.status, restartRequired: true };
      } catch (error) {
        return reply.code(400).send({
          code: "INVALID_TLS_CONFIGURATION",
          message: error instanceof Error ? error.message : "证书配置无效。",
        });
      }
    });

    this.#app.delete("/admin/api/tls", async (request, reply) => {
      if (!this.#requireAdmin(request.headers.cookie, reply)) return;
      try {
        this.#tls.remove();
        return { tls: this.#tls.status, restartRequired: true };
      } catch (error) {
        return reply.code(400).send({
          code: "INVALID_TLS_CONFIGURATION",
          message: error instanceof Error ? error.message : "证书删除失败。",
        });
      }
    });

    this.#app.get("/admin", async (_request, reply) => this.#serveAdminAsset("", reply));
    this.#app.get("/admin/*", async (request, reply) => {
      const path = (request.params as { "*"?: string })["*"] ?? "";
      return this.#serveAdminAsset(path, reply);
    });
  }

  #requireAdmin(cookie: string | undefined, reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown } }): boolean {
    if (this.#admin.authenticate(readCookie(cookie, adminSessionCookie))) return true;
    reply.code(401).send({ code: "ADMIN_AUTH_REQUIRED", message: "请先登录管理后台。" });
    return false;
  }

  #adminCookie(value: string, maxAge: number): string {
    const parts = [
      `${adminSessionCookie}=${value}`,
      "HttpOnly",
      "Path=/admin",
      "SameSite=Strict",
      `Max-Age=${maxAge}`,
    ];
    if (this.#tls.status.enabled) parts.push("Secure");
    return parts.join("; ");
  }

  async #serveAdminAsset(requestPath: string, reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown }; type: (contentType: string) => { send: (payload: unknown) => unknown } }): Promise<unknown> {
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

  #handleSocket(socket: WebSocket): void {
    let client: ClientConnection | undefined;

    socket.on("message", (data: Buffer) => {
      try {
        const parsed = ClientMessageSchema.safeParse(JSON.parse(data.toString()));
        if (!parsed.success) {
          logger.warn(`Rejected invalid WebSocket message: ${parsed.error.issues[0]?.message ?? "unknown validation error"}`);
          this.#send({ socket }, {
            type: "error",
            code: "INVALID_MESSAGE",
            message: parsed.error.issues[0]?.message ?? "Invalid message",
          });
          return;
        }
        if (parsed.data.type === "auth") {
          if (client) {
            this.#send(client, { type: "error", code: "ALREADY_AUTHENTICATED", message: "Connection is already authenticated." });
            return;
          }
          client = this.#authenticateClient(socket, parsed.data.token, parsed.data.device);
          return;
        }
        if (!client) {
          this.#send({ socket }, {
            type: "error",
            code: "AUTH_REQUIRED",
            message: "Authenticate before sending messages.",
          });
          return;
        }
        const authenticatedClient = client;
        void this.#handleMessage(authenticatedClient, parsed.data).catch((error: unknown) => {
          logger.error(`Failed to handle ${parsed.data.type} from device ${authenticatedClient.device.id}:`, error);
          this.#send(authenticatedClient, { type: "error", code: "INTERNAL_ERROR", message: "服务器处理消息失败。" });
        });
      } catch (error) {
        logger.warn("Rejected invalid WebSocket JSON:", error);
        this.#send({ socket }, { type: "error", code: "INVALID_JSON", message: "Messages must be valid JSON." });
      }
    });
    socket.on("close", () => {
      if (client) this.#handleClientClose(client);
    });
  }

  async #handleMessage(client: ClientConnection, message: ClientMessage): Promise<void> {
    switch (message.type) {
      case "clipboard.publish":
        this.#publishClipboard(client, message.entry);
        return;
      case "clipboard.activate":
        this.#activateClipboard(client, message.entryId);
        return;
      case "clipboard.delete":
        this.#deleteClipboard(client, message.entryId);
        return;
      case "clipboard.fetch":
        this.#send(client, {
          type: "clipboard.entries",
          requestId: message.requestId,
          entries: this.#store.listByIds(client.userId, message.entryIds),
        });
        return;
      case "file.download": {
        await this.#downloads.download(client, message, this.#clients);
        return;
      }
      // Chunks and completions over the socket belong to device-to-server
      // download relays only; uploads live on the HTTP routes now.
      case "file.chunk":
        if (!this.#downloads.receiveChunk(client, message.transferId, message.data)) {
          this.#send(client, { type: "file.failed", transferId: message.transferId, message: "文件传输不存在或已过期" });
        }
        return;
      case "file.complete":
        if (!this.#downloads.complete(client, message.transferId)) {
          this.#send(client, { type: "file.failed", transferId: message.transferId, message: "文件传输不存在或已过期" });
        }
        return;
      case "file.abort":
        this.#downloads.fail(client, message.transferId, message.message);
        return;
      case "ping":
        this.#send(client, { type: "pong" });
        return;
    }
  }

  #authenticateClient(socket: WebSocket, token: string, device: Device): ClientConnection | undefined {
    const user = this.#auth.authenticateSession(token);
    if (!user) {
      logger.warn(`Rejected WebSocket authentication for device ${device.id}`);
      this.#send({ socket }, { type: "error", code: "AUTH_FAILED", message: "登录已失效，请重新登录" });
      socket.close(1008, "Authentication failed");
      return undefined;
    }
    const client: ClientConnection = { socket, userId: user.id, device };
    this.#clients.add(client);
    this.#store.upsertDevice(user.id, device);
    logger.info(`Device authenticated: user=${user.id} device=${device.id}`);
    this.#send(client, {
      type: "auth.ack",
      manifest: this.#store.listManifest(user.id),
      devices: this.#store.listDevices(user.id),
    });
    this.#broadcast(user.id, { type: "device.presence", device, online: true }, client);
    return client;
  }

  #publishClipboard(client: ClientConnection, entry: Extract<ClientMessage, { type: "clipboard.publish" }>["entry"]): void {
    const storedEntry = this.#store.upsert(client.userId, {
      ...entry,
      sourceDeviceId: client.device.id,
    });
    logger.info(`Clipboard entry stored: user=${client.userId} entry=${storedEntry.id} device=${client.device.id}`);
    this.#send(client, { type: "clipboard.created", entry: storedEntry });
    this.#broadcast(client.userId, { type: "clipboard.created", entry: storedEntry }, client);
  }

  #activateClipboard(client: ClientConnection, entryId: string): void {
    const [entry] = this.#store.listByIds(client.userId, [entryId]);
    if (!entry) {
      this.#send(client, { type: "error", code: "ENTRY_NOT_FOUND", message: "剪贴板记录不存在" });
      return;
    }
    // File-list clipboards are intentionally history-only. Broadcasting them
    // would make receivers materialize unused directory views and temporary
    // files before the user has chosen to paste anything.
    if (entry.kind === "files") return;
    logger.info(`Clipboard activated: user=${client.userId} entry=${entry.id} device=${client.device.id}`);
    this.#broadcast(client.userId, { type: "clipboard.activated", entry }, client);
  }

  #deleteClipboard(client: ClientConnection, entryId: string): void {
    this.#store.delete(client.userId, entryId);
    logger.info(`Clipboard entry deleted: user=${client.userId} entry=${entryId} device=${client.device.id}`);
    this.#send(client, { type: "clipboard.deleted", entryId });
    this.#broadcast(client.userId, { type: "clipboard.deleted", entryId }, client);
  }

  #publishFileAvailability(fileId: string): void {
    // The content pool is global, so availability is not scoped to the
    // uploader: any signed-in device that references the content wants this.
    for (const client of this.#clients) this.#send(client, { type: "file.available", fileId });
  }

  // Sweeping walks the whole content pool, so it is deferred off the message
  // handler rather than run inline with the delete that triggered it.
  #collectGarbage(): void {
    setTimeout(() => {
      try {
        const { removedFiles, removedBytes } = this.#store.collectGarbage(this.config.resumableUploadTtlMs);
        if (removedFiles > 0) {
          logger.info(`Reclaimed ${removedFiles} globally unreferenced files (${removedBytes} bytes)`);
        }
      } catch (error) {
        logger.error("Failed to collect globally unreferenced files:", error);
      }
    }, 0);
  }

  #handleClientClose(client: ClientConnection): void {
    this.#downloads.handleClientClose(client);
    this.#clients.delete(client);
    logger.info(`Device disconnected: user=${client.userId} device=${client.device.id}`);
    this.#broadcast(client.userId, { type: "device.presence", device: client.device, online: false });
  }

  #sessionUser(request: { headers: { authorization?: string } }): { id: string } | undefined {
    const token = readBearerToken(request.headers.authorization);
    return token ? this.#auth.authenticateSession(token) : undefined;
  }

  #uploadError(reply: { code: (statusCode: number) => { send: (payload: unknown) => unknown } }, error: unknown): unknown {
    if (error instanceof UploadHttpError) {
      return reply.code(error.status).send({ message: error.message });
    }
    throw error;
  }

  #send(client: ConnectionTarget, message: ServerMessage): void {
    if (client.socket.readyState === 1) client.socket.send(JSON.stringify(message));
  }

  #broadcast(userId: string, message: ServerMessage, except?: ClientConnection): void {
    for (const client of this.#clients) {
      if (client !== except && client.userId === userId) this.#send(client, message);
    }
  }

  #disconnectUserClients(userId: string): void {
    for (const client of this.#clients) {
      if (client.userId === userId) client.socket.close(1008, "Password changed");
    }
  }
}

function createApp(tls: TlsOptions | undefined): FastifyInstance {
  return (tls
    ? Fastify({ logger: false, https: tls })
    : Fastify({ logger: false })) as FastifyInstance;
}

function readCookie(header: string | undefined, name: string): string | undefined {
  if (!header) return undefined;
  const prefix = `${name}=`;
  return header.split(";").map((part) => part.trim()).find((part) => part.startsWith(prefix))?.slice(prefix.length);
}

function readBearerToken(header: string | undefined): string | undefined {
  const match = header?.match(/^Bearer\s+(.+)$/i);
  return match?.[1]?.trim() || undefined;
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
