import {
  AuthResponseSchema,
  ServerMessageSchema,
  type AuthResponse,
  type ClientMessage,
  type Device,
} from "@cliproam/protocol";

import { errorMessageFromBody } from "./syncHttp";

export type AuthMode = "login" | "register";
export type ServerProtocol = "http" | "https";

export function normalizeServerAddress(value: string): string {
  const candidate = value.trim();
  if (!candidate) throw new Error("请输入服务器 IP 和端口");
  if (candidate.includes("://") || candidate.includes("/")) {
    throw new Error("只需填写 IP 和端口，例如 192.168.1.20:4810");
  }

  let url: URL;
  try {
    url = new URL(`http://${candidate}`);
  } catch {
    throw new Error("服务器地址格式不正确");
  }
  if (!url.hostname || !url.port) throw new Error("服务器地址必须包含 IP 和端口");
  return url.host;
}

export function getServerUrls(
  address: string,
  protocol: ServerProtocol,
): { httpUrl: string; webSocketUrl: string } {
  const normalized = normalizeServerAddress(address);
  const secure = protocol === "https";
  return {
    httpUrl: `${secure ? "https" : "http"}://${normalized}`,
    webSocketUrl: `${secure ? "wss" : "ws"}://${normalized}/ws`,
  };
}

async function postJson(httpUrl: string, path: string, body: unknown, headers: Record<string, string> = {}): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(`${httpUrl}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error("无法连接服务器，请检查 IP、端口和网络");
  }
  const responseBody = await response.json().catch(() => undefined) as unknown;
  if (!response.ok) throw new Error(errorMessageFromBody(responseBody, response.status));
  return responseBody;
}

export async function authenticateAccount(
  address: string,
  username: string,
  password: string,
  mode: AuthMode,
  protocol: ServerProtocol,
  deviceId: string,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(address, protocol);
  const body = await postJson(httpUrl, `/auth/${mode}`, { username, password, deviceId });
  const result = AuthResponseSchema.safeParse(body);
  if (!result.success) throw new Error("服务器返回了不兼容的登录响应");
  return result.data;
}

export async function changeAccountPassword(
  address: string,
  protocol: ServerProtocol,
  sessionToken: string,
  currentPassword: string,
  newPassword: string,
): Promise<void> {
  const { httpUrl } = getServerUrls(address, protocol);
  await postJson(
    httpUrl,
    "/auth/password",
    { currentPassword, newPassword },
    { Authorization: `Bearer ${sessionToken}` },
  );
}

export async function testSyncConnection(
  url: string,
  token: string,
  device: Device,
  timeoutMs = 6000,
): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const socket = new WebSocket(url);
    let settled = false;

    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timeout);
      socket.close();
      if (error) reject(error);
      else resolve();
    };

    const timeout = window.setTimeout(
      () => finish(new Error("连接超时，请检查服务器地址和网络")),
      timeoutMs,
    );

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ type: "auth", token, device } satisfies ClientMessage));
    });
    socket.addEventListener("message", (event) => {
      try {
        const result = ServerMessageSchema.safeParse(JSON.parse(String(event.data)));
        if (!result.success) {
          finish(new Error("服务器返回了不兼容的响应"));
          return;
        }
        if (result.data.type === "auth.ack") finish();
        else if (result.data.type === "error") finish(new Error(result.data.message));
      } catch {
        finish(new Error("服务器返回了无法解析的数据"));
      }
    });
    socket.addEventListener("error", () => finish(new Error("无法连接服务器，请检查地址和网络")));
    socket.addEventListener("close", () => {
      if (!settled) finish(new Error("服务器在认证完成前断开了连接"));
    });
  });
}
