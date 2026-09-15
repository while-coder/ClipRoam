import {
  AuthResponseSchema,
  type AuthResponse,
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
  device: Device,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(address, protocol);
  // 设备信息随登录上报：登录即注册设备行，不依赖推送通道连得上。
  const body = await postJson(httpUrl, `/auth/${mode}`, { username, password, deviceId: device.id, device });
  const result = AuthResponseSchema.safeParse(body);
  if (!result.success) throw new Error("服务器返回了不兼容的登录响应");
  return result.data;
}

// 设备信息（别名等）变化后主动上报。旧服务器没有该端点会失败，由调用方
// 忽略——重连时的 WS auth 消息仍会携带 device 兜底。
export async function pushDeviceInfo(
  address: string,
  protocol: ServerProtocol,
  sessionToken: string,
  device: Device,
): Promise<void> {
  const { httpUrl } = getServerUrls(address, protocol);
  await postJson(httpUrl, "/devices/current", device, { Authorization: `Bearer ${sessionToken}` });
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
