import type { WebSocket } from "@fastify/websocket";
import type { Device, ServerMessage } from "@cliproam/protocol";

export type ClientConnection = {
  socket: WebSocket;
  device: Device;
  userId: string;
};

export type ConnectionTarget = Pick<ClientConnection, "socket">;
export type SendMessage = (client: ConnectionTarget, message: ServerMessage) => void;
