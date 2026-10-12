import {
  AuthResponseSchema,
  type AuthResponse,
  type Device,
} from "@cliproam/protocol";


import { errorMessageFromBody } from "./syncHttp";
import { anyAbortSignal } from "../../utils/abort";

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

async function postJson(httpUrl: string, path: string, body: unknown, headers: Record<string, string> = {}, signal?: AbortSignal): Promise<unknown> {
  const timeoutSignal = AbortSignal.timeout(30_000);
  const requestSignal = signal ? anyAbortSignal([signal, timeoutSignal]) : timeoutSignal;
  let response: Response;
  let responseBody: unknown;
  try {
    response = await fetch(`${httpUrl}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
      signal: requestSignal,
    });
    responseBody = await response.json().catch((error) => {
      if (requestSignal.aborted) throw error;
      return undefined;
    });
  } catch {
    if (signal?.aborted) throw new Error("请求已取消");
    if (timeoutSignal.aborted) throw new Error("服务器响应超时，请稍后重试");
    throw new Error("无法连接服务器，请检查 IP、端口和网络");
  }
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
  signal?: AbortSignal,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(address, protocol);
  // 设备信息随登录上报：登录即注册设备行，不依赖推送通道连得上。
  const body = await postJson(httpUrl, `/auth/${mode}`, { username, password, device }, {}, signal);
  const result = AuthResponseSchema.safeParse(body);
  if (!result.success) throw new Error("服务器返回了不兼容的登录响应");
  return result.data;
}

// 应用启动时用已有 token 验证会话，同时刷新设备信息。
export async function verifyAccountSession(
  config: { serverAddress: string; serverProtocol: ServerProtocol; sessionToken: string },
  device: Device,
  signal?: AbortSignal,
): Promise<void> {
  const { httpUrl } = getServerUrls(config.serverAddress, config.serverProtocol);
  try {
    await postJson(httpUrl, "/auth/session/verify", device, { Authorization: `Bearer ${config.sessionToken}` }, signal);
  } catch (error) {
    if (error instanceof Error && error.message.includes("Route POST:/auth/session/verify not found")) {
      throw new Error("服务器尚未支持登录状态验证，请更新服务器，或输入密码重新登录。");
    }
    throw error;
  }
}

// 设备信息（别名等）变化后主动上报。
export async function pushDeviceInfo(
  address: string,
  protocol: ServerProtocol,
  sessionToken: string,
  device: Device,
  signal?: AbortSignal,
): Promise<void> {
  const { httpUrl } = getServerUrls(address, protocol);
  await postJson(httpUrl, "/devices/current", device, { Authorization: `Bearer ${sessionToken}` }, signal);
}

export async function changeAccountPassword(
  address: string,
  protocol: ServerProtocol,
  sessionToken: string,
  currentPassword: string,
  newPassword: string,
  signal?: AbortSignal,
): Promise<void> {
  const { httpUrl } = getServerUrls(address, protocol);
  await postJson(
    httpUrl,
    "/auth/password",
    { currentPassword, newPassword },
    { Authorization: `Bearer ${sessionToken}` },
    signal,
  );
}
