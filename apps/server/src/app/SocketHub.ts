import type { WebSocket } from "@fastify/websocket";
import {
  ClientMessageSchema,
  type Device,
  type ServerMessage,
} from "@cliproam/protocol";
import { getLogger } from "./Logger.js";

type ClientConnection = {
  socket: WebSocket;
  device: Device;
  userId: string;
  token: string;
};

type ConnectionTarget = Pick<ClientConnection, "socket">;

const logger = getLogger("SocketHub");

// Everything the hub needs from the rest of the server, expressed as plain
// functions so the hub stays ignorant of stores and sessions. Device info
// rides the HTTP login; `authenticateSession` already carries the device id,
// so the hub resolves device info from the store.
export type SocketHubDeps = {
  authenticateSession: (token: string) => { id: string; deviceId: string } | undefined;
  getDevice: (userId: string, deviceId: string) => Device | undefined;
  touchDevice: (userId: string, deviceId: string) => void;
};

// Authenticated sockets only ever send auth and ping: every other
// request/response exchange moved to HTTP routes. What remains here is the
// handshake and the server-push fan-out the routes call into.
export class SocketHub {
  readonly #clients = new Set<ClientConnection>();

  constructor(private readonly deps: SocketHubDeps) {}

  handleSocket(socket: WebSocket): void {
    let client: ClientConnection | undefined;
    const authTimer = setTimeout(() => socket.terminate(), 15_000);
    authTimer.unref();

    socket.on("message", (data: Buffer) => {
      try {
        const parsed = ClientMessageSchema.safeParse(JSON.parse(data.toString()));
        if (!parsed.success) {
          // 详细原因只进日志，客户端拿通用文案——Zod 原文对用户是天书。
          logger.warn(`Rejected invalid WebSocket message: ${parsed.error.issues[0]?.message ?? "unknown validation error"}`);
          this.#send({ socket }, {
            type: "error",
            code: "INVALID_MESSAGE",
            message: "消息格式不正确，请升级客户端后重试",
          });
          return;
        }
        if (parsed.data.type === "auth") {
          if (client) {
            this.#send(client, { type: "error", code: "ALREADY_AUTHENTICATED", message: "Connection is already authenticated." });
            return;
          }
          client = this.#authenticateClient(socket, parsed.data.token);
          if (client) clearTimeout(authTimer);
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
        if (!this.#isAuthenticated(client)) return;
        this.#send(client, { type: "pong" });
      } catch (error) {
        logger.warn("Rejected invalid WebSocket JSON:", error);
        this.#send({ socket }, { type: "error", code: "INVALID_JSON", message: "Messages must be valid JSON." });
      }
    });
    socket.on("close", () => {
      clearTimeout(authTimer);
      if (client) this.#handleClientClose(client);
    });
    // A socket-level error (TLS failure, connection reset, fatal frame error)
    // with no listener would surface as an uncaughtException — which the
    // entrypoint treats as fatal and shuts the whole server down for. Log it,
    // drop the connection, keep serving everyone else.
    socket.on("error", (error: Error) => {
      logger.warn(`WebSocket error${client ? ` from device ${client.device.id}` : ""}:`, error);
      socket.terminate();
    });
  }

  // Publishes, activates and deletes are HTTP routes now. Broadcasts go to
  // every online device of the account: the initiator's own handlers are
  // idempotent (upsert / acknowledge), except activation, where a delayed
  // self-echo would overwrite a newer local clipboard — so the acting device
  // is excluded there, by device id rather than by connection object.
  broadcast(userId: string, message: ServerMessage, exceptDeviceId?: string): void {
    for (const client of this.#clients) {
      if (client.device.id !== exceptDeviceId && client.userId === userId && this.#isAuthenticated(client)) this.#send(client, message);
    }
  }

  disconnectUser(userId: string, reason: string, deviceId?: string): void {
    for (const client of this.#clients) {
      if (client.userId === userId && (!deviceId || client.device.id === deviceId)) client.socket.close(1008, reason);
    }
  }

  broadcastAll(message: ServerMessage): void {
    for (const client of this.#clients) {
      if (this.#isAuthenticated(client)) this.#send(client, message);
    }
  }

  #authenticateClient(socket: WebSocket, token: string): ClientConnection | undefined {
    const user = this.deps.authenticateSession(token);
    if (!user) {
      logger.warn("Rejected WebSocket authentication");
      this.#send({ socket }, { type: "error", code: "AUTH_FAILED", message: "登录已失效，请重新登录" });
      socket.close(1008, "Authentication failed");
      return undefined;
    }
    const device = this.deps.getDevice(user.id, user.deviceId);
    if (!device) {
      logger.warn(`No device info available for user ${user.id} device ${user.deviceId}`);
      this.#send({ socket }, { type: "error", code: "AUTH_FAILED", message: "登录已失效，请重新登录" });
      socket.close(1008, "Authentication failed");
      return undefined;
    }
    const client: ClientConnection = { socket, userId: user.id, device, token };
    this.#clients.add(client);
    this.deps.touchDevice(user.id, device.id);
    logger.info(`Device authenticated: user=${user.id} device=${device.id}`);
    // A bare confirmation; the client pulls the manifest and device list over
    // HTTP (`GET /entries/manifest`) once it sees this.
    this.#send(client, { type: "auth.ack" });
    this.broadcast(user.id, { type: "device.presence", device, online: true });
    return client;
  }

  #handleClientClose(client: ClientConnection): void {
    this.#clients.delete(client);
    logger.info(`Device disconnected: user=${client.userId} device=${client.device.id}`);
    this.broadcast(client.userId, { type: "device.presence", device: client.device, online: false });
  }

  #send(client: ConnectionTarget, message: ServerMessage): void {
    if (client.socket.readyState !== 1) return;
    if (client.socket.bufferedAmount > 32 * 1024 * 1024) {
      client.socket.terminate();
      return;
    }
    client.socket.send(JSON.stringify(message));
  }

  #isAuthenticated(client: ClientConnection): boolean {
    const user = this.deps.authenticateSession(client.token);
    if (user?.id === client.userId && user.deviceId === client.device.id) return true;
    client.socket.close(1008, "Session expired or revoked");
    return false;
  }
}
