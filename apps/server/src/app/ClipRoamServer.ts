import Fastify, { type FastifyInstance } from "fastify";
import websocket from "@fastify/websocket";
import { type ServerSettings } from "@cliproam/protocol";
import { AuthService } from "../account/AuthService.js";
import { AdminService } from "../admin/AdminService.js";
import { getLogger } from "./Logger.js";
import { SocketHub } from "./SocketHub.js";
import { registerAuthRoutes } from "./routes/AuthRoutes.js";
import { registerDeviceRoutes } from "./routes/DeviceRoutes.js";
import { registerEntryRoutes } from "./routes/EntryRoutes.js";
import { registerFileRoutes } from "./routes/FileRoutes.js";
import { registerAdminRoutes } from "./routes/AdminRoutes.js";
import { registerAdminUiRoutes } from "./routes/AdminUiRoutes.js";
import { readBearerToken } from "./routes/AuthRoutes.js";
import { FileRelayService } from "../files/FileRelayService.js";
import { UploadService } from "../files/UploadService.js";
import { loadServerConfig, SERVER_PORT, SMALL_JSON_BODY_LIMIT, type ServerConfig } from "./ServerConfig.js";
import { ClipRoamStore } from "../account/ClipRoamStore.js";
import { TlsCertificateService, type TlsOptions } from "../tls/TlsCertificateService.js";

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

  constructor(private readonly config: ServerConfig = loadServerConfig()) {
    this.#sockets = new SocketHub({
      authenticateSession: (token) => this.#auth.authenticateSession(token),
      getDevice: (userId, deviceId) => this.#store.getDevice(userId, deviceId),
      recordDeviceOnline: (userId, deviceId) => this.#store.recordDeviceOnline(userId, deviceId),
      serverSettings: (userId) => serverSettings(this.#configForUser(userId)),
    });
    this.#relays = new FileRelayService();
    this.#uploads = new UploadService(
      this.#store.files(),
      (userId) => this.#configForUser(userId),
      this.#publishFileAvailability.bind(this),
    );
  }

  get port(): number { return SERVER_PORT; }

  get adminConfigured(): boolean { return this.#admin.isConfigured; }

  get adminUrl(): string {
    const protocol = this.#tls.status.enabled ? "https" : "http";
    return `${protocol}://localhost:${SERVER_PORT}/admin`;
  }

  async start(): Promise<void> {
    // Inbound socket messages are only `auth`/`ping` — a small explicit cap,
    // not the publish-body limit that used to double as this value.
    await this.#app.register(websocket, { options: { maxPayload: SMALL_JSON_BODY_LIMIT } });
    this.#registerRoutes();
    await this.#app.listen({ port: SERVER_PORT, host: "0.0.0.0" });
  }

  async stop(): Promise<void> {
    this.#relays.close();
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
        .header("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS");
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
      maxHistoryEntries: (userId) => this.#configForUser(userId).maxHistoryEntries,
      maxCaptureFileCount: (userId) => this.#configForUser(userId).maxCaptureFileCount,
    });
    registerDeviceRoutes(this.#app, {
      store: this.#store,
      isDeviceOnline: this.#sockets.isDeviceOnline.bind(this.#sockets),
      disconnectUser: this.#sockets.disconnectUser.bind(this.#sockets),
    });
    registerAuthRoutes(this.#app, {
      auth: this.#auth,
      onPasswordChanged: (userId) => this.#sockets.disconnectUser(userId, "Password changed"),
      serverSettings: (userId) => serverSettings(this.#configForUser(userId)),
    });
    this.#app.get("/health", async () => ({ status: "ok", service: "cliproam-server" }));
    this.#app.get("/ws", { websocket: true }, (socket) => this.#sockets.handleSocket(socket));
    registerAdminRoutes(this.#app, {
      admin: this.#admin,
      tls: this.#tls,
      config: this.config,
      onSettingsChanged: (userId) => {
        const users = userId ? [{ id: userId }] : this.#store.listUsers();
        for (const user of users) {
          this.#sockets.broadcast(user.id, { type: "settings.updated", settings: serverSettings(this.#configForUser(user.id)) });
        }
      },
      store: this.#store,
      broadcast: this.#sockets.broadcast.bind(this.#sockets),
      liveServer: this.#app.server as unknown as { setSecureContext?: (context: TlsOptions) => void },
      disconnectUser: this.#sockets.disconnectUser.bind(this.#sockets),
      isDeviceOnline: this.#sockets.isDeviceOnline.bind(this.#sockets),
    });
    registerAdminUiRoutes(this.#app);
    this.#app.addHook("onClose", async () => {
      this.#store.close();
    });
  }

  #configForUser(userId: string): ServerConfig {
    return this.#store.resolveTransferSettings(userId, this.config);
  }

  #publishFileAvailability(fileId: string): void {
    this.#sockets.broadcastAll({ type: "file.available", fileId });
  }

}

function createApp(tls: TlsOptions | undefined): FastifyInstance {
  // forceCloseConnections: hijacked relay GETs park on a live pipe and are
  // never "idle" — without this, `stop()` would wait on them for the full
  // GracefulShutdownTimeout (or forever) and SIGINT would not shut down.
  return (tls
    ? Fastify({ logger: false, forceCloseConnections: true, https: tls })
    : Fastify({ logger: false, forceCloseConnections: true })) as FastifyInstance;
}

// The client-facing caps, derived from the live config on every call so admin
// settings changes apply without a restart.
function serverSettings(config: ServerConfig): ServerSettings {
  return {
    maxStoredFileMb: config.maxStoredFileMb,
    maxStoredImageMb: config.maxStoredImageMb,
    maxHistoryEntries: config.maxHistoryEntries,
    maxCaptureFileCount: config.maxCaptureFileCount,
  };
}
