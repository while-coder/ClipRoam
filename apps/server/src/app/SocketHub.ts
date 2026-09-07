import type { WebSocket } from "@fastify/websocket";
import {
  ClientMessageSchema,
  type ClientMessage,
  type Device,
  type ServerMessage,
} from "@cliproam/protocol";
import { getLogger } from "./Logger.js";
import type { ClientConnection, ConnectionTarget } from "./Connection.js";

const logger = getLogger("SocketHub");

// Everything the hub needs from the rest of the server, expressed as plain
// functions so the hub stays ignorant of stores and sessions.
export type SocketHubDeps = {
  authenticateSession: (token: string) => { id: string } | undefined;
  registerDevice: (userId: string, device: Device) => void;
};

// Authenticated sockets only ever send auth and ping: every other
// request/response exchange moved to HTTP routes. What remains here is the
// handshake and the server-push fan-out the routes call into.
export class SocketHub {
  readonly #clients = new Set<ClientConnection>();

  constructor(private readonly deps: SocketHubDeps) {}

  handleSocket(socket: WebSocket): void {
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

  // Publishes, activates and deletes are HTTP routes now. Broadcasts go to
  // every online device of the account: the initiator's own handlers are
  // idempotent (upsert / acknowledge), except activation, where a delayed
  // self-echo would overwrite a newer local clipboard — so the acting device
  // is excluded there, by device id rather than by connection object.
  broadcast(userId: string, message: ServerMessage, exceptDeviceId?: string): void {
    for (const client of this.#clients) {
      if (client.device.id !== exceptDeviceId && client.userId === userId) this.#send(client, message);
    }
  }

  // Not scoped to an account on purpose — used for content-pool events that
  // any signed-in device may care about (see `file.available`).
  broadcastAll(message: ServerMessage): void {
    for (const client of this.#clients) this.#send(client, message);
  }

  disconnectUser(userId: string, reason: string): void {
    for (const client of this.#clients) {
      if (client.userId === userId) client.socket.close(1008, reason);
    }
  }

  #authenticateClient(socket: WebSocket, token: string, device: Device): ClientConnection | undefined {
    const user = this.deps.authenticateSession(token);
    if (!user) {
      logger.warn(`Rejected WebSocket authentication for device ${device.id}`);
      this.#send({ socket }, { type: "error", code: "AUTH_FAILED", message: "登录已失效，请重新登录" });
      socket.close(1008, "Authentication failed");
      return undefined;
    }
    const client: ClientConnection = { socket, userId: user.id, device };
    this.#clients.add(client);
    this.deps.registerDevice(user.id, device);
    logger.info(`Device authenticated: user=${user.id} device=${device.id}`);
    // A bare confirmation; the client pulls the manifest and device list over
    // HTTP (`GET /entries/manifest`) once it sees this.
    this.#send(client, { type: "auth.ack" });
    this.broadcast(user.id, { type: "device.presence", device, online: true });
    return client;
  }

  async #handleMessage(client: ClientConnection, message: ClientMessage): Promise<void> {
    switch (message.type) {
      case "ping":
        this.#send(client, { type: "pong" });
        return;
    }
  }

  #handleClientClose(client: ClientConnection): void {
    this.#clients.delete(client);
    logger.info(`Device disconnected: user=${client.userId} device=${client.device.id}`);
    this.broadcast(client.userId, { type: "device.presence", device: client.device, online: false });
  }

  #send(client: ConnectionTarget, message: ServerMessage): void {
    if (client.socket.readyState === 1) client.socket.send(JSON.stringify(message));
  }
}
