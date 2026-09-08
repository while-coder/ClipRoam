import Fastify, { type FastifyInstance } from "fastify";
import websocket from "@fastify/websocket";
import { MAX_MESSAGE_BYTES } from "@cliproam/protocol";
import { AuthService } from "../account/AuthService.js";
import { AdminService } from "../admin/AdminService.js";
import { getLogger } from "./Logger.js";
import { SocketHub } from "./SocketHub.js";
import { registerAuthRoutes } from "./routes/AuthRoutes.js";
import { registerDeviceRoutes } from "./routes/DeviceRoutes.js";
import { registerEntryRoutes } from "./routes/EntryRoutes.js";
import { registerFileRoutes } from "./routes/FileRoutes.js";
import { registerAdminRoutes } from "./routes/AdminRoutes.js";
import { readBearerToken } from "./routes/AuthRoutes.js";
import { FileRelayService } from "../files/FileRelayService.js";
import { UploadService } from "../files/UploadService.js";
import { loadServerConfig, type ServerConfig } from "./ServerConfig.js";
import { ClipRoamStore } from "../account/ClipRoamStore.js";
import { TlsCertificateService, type TlsOptions } from "../tls/TlsCertificateService.js";

const garbageCollectionIntervalMs = 6 * 60 * 60 * 1_000;
const logger = getLogger("ClipRoamServer");

export class ClipRoamServer {
  readonly #tls = new TlsCertificateService();
  readonly #app = createApp(this.#tls.options);
  readonly #store = new ClipRoamStore();
  readonly #auth = new AuthService(this.#store);
  readonly #admin = new AdminService();
  readonly #sockets: SocketHub;
  readonly #relays: FileRelayService;
  readonly #uploads: UploadService;
  #collectionTimer?: NodeJS.Timeout;

  constructor(private readonly config: ServerConfig = loadServerConfig()) {
    this.#sockets = new SocketHub({
      authenticateSession: (token) => this.#auth.authenticateSession(token),
      registerDevice: (userId, device) => this.#store.upsertDevice(userId, device),
    });
    this.#relays = new FileRelayService();
    this.#uploads = new UploadService(
      this.#store.files(),
      config,
      this.#publishFileAvailability.bind(this),
    );
  }

  get port(): number { return this.config.port; }

  get adminPassword(): string { return this.#admin.password; }

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

  // The server itself stays a thin shell: wiring, lifecycle and the push
  // hooks other components call back into. Route registration lives with the
  // feature it serves, under ./routes.
  #registerRoutes(): void {
    this.#app.addHook("onRequest", async (request, reply) => {
      reply
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Headers", "Content-Type, Authorization")
        .header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS");
      if (request.method === "OPTIONS") return reply.code(204).send();
      // One place resolves the Bearer token for every route; handlers opt in
      // through requireSessionUser().
      const token = readBearerToken(request.headers.authorization);
      request.sessionUser = token ? this.#auth.authenticateSession(token) : undefined;
    });
    // Upload chunks arrive as raw bytes rather than base64-in-JSON, which the
    // WebSocket path was forced into by its text framing.
    this.#app.addContentTypeParser(
      "application/octet-stream",
      { parseAs: "buffer" },
      (_request, body, done) => done(null, body),
    );
    registerFileRoutes(this.#app, {
      uploads: this.#uploads,
      relays: this.#relays,
      broadcast: this.#sockets.broadcast.bind(this.#sockets),
      store: this.#store,
    });
    registerEntryRoutes(this.#app, {
      store: this.#store,
      broadcast: this.#sockets.broadcast.bind(this.#sockets),
    });
    registerDeviceRoutes(this.#app, {
      store: this.#store,
    });
    registerAuthRoutes(this.#app, {
      auth: this.#auth,
      onPasswordChanged: (userId) => this.#sockets.disconnectUser(userId, "Password changed"),
    });
    this.#app.get("/health", async () => ({ status: "ok", service: "cliproam-server" }));
    this.#app.get("/ws", { websocket: true }, (socket) => this.#sockets.handleSocket(socket));
    registerAdminRoutes(this.#app, {
      admin: this.#admin,
      tls: this.#tls,
      config: this.config,
      liveServer: this.#app.server as unknown as { setSecureContext?: (context: TlsOptions) => void },
    });
    this.#app.addHook("onClose", async () => this.#store.close());
  }

  #publishFileAvailability(fileId: string): void {
    // The content pool is global, so availability is not scoped to the
    // uploader: any signed-in device that references the content wants this.
    this.#sockets.broadcastAll({ type: "file.available", fileId });
  }

  // Sweeping walks the whole content pool, so it is deferred off the caller
  // rather than run inline with the upload that triggered it.
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
}

function createApp(tls: TlsOptions | undefined): FastifyInstance {
  return (tls
    ? Fastify({ logger: false, https: tls })
    : Fastify({ logger: false })) as FastifyInstance;
}
