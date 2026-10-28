import { anyAbortSignal } from "../../utils";
import {
  AuthResponseSchema,
  type AuthResponse,
  type Device,
} from "@cliproam/protocol";


import { errorMessageFromBody } from "./httpClient";

export type AuthMode = "login" | "register";

export function normalizeServerAddress(value: string): string {
  const candidate = value.trim();
  if (!candidate) throw new Error("请输入服务器地址，例如 https://example.com/cliproam");
  if (!/^https?:\/\/[^/]/i.test(candidate)) {
    throw new Error("服务器地址需以 http:// 或 https:// 开头");
  }

  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    throw new Error("服务器地址格式不正确");
  }
  if (!url.hostname || url.username || url.password || url.search || url.hash) {
    throw new Error("服务器地址仅支持协议、域名或 IP、端口和子路径，请勿包含账号、查询参数或锚点");
  }
  return `${url.origin}${url.pathname.replace(/\/+$/, "")}`;
}

export function getServerUrls(
  address: string,
): { httpUrl: string; webSocketUrl: string } {
  const normalized = normalizeServerAddress(address);
  return {
    httpUrl: normalized,
    webSocketUrl: `${normalized.replace(/^http/, "ws")}/ws`,
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
    throw new Error("无法连接服务器，请检查服务器地址和网络");
  }
  if (!response.ok) throw new Error(errorMessageFromBody(responseBody, response.status));
  return responseBody;
}

export async function authenticateAccount(
  address: string,
  username: string,
  password: string,
  mode: AuthMode,
  device: Device,
  signal?: AbortSignal,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(address);
  // 设备信息随登录上报：登录即注册设备行，不依赖推送通道连得上。
  const body = await postJson(httpUrl, `/auth/${mode}`, { username, password, device }, {}, signal);
  const result = AuthResponseSchema.safeParse(body);
  if (!result.success) throw new Error("服务器返回了不兼容的登录响应");
  return result.data;
}

// 应用启动时用已有 token 验证会话，同时刷新设备信息。
export async function verifyAccountSession(
  config: { serverAddress: string; username: string; sessionToken: string },
  device: Device,
  signal?: AbortSignal,
): Promise<AuthResponse> {
  const { httpUrl } = getServerUrls(config.serverAddress);
  try {
    const body = await postJson(httpUrl, "/auth/verify", { username: config.username, sessionToken: config.sessionToken, device }, {}, signal);
    const result = AuthResponseSchema.safeParse(body);
    if (!result.success) throw new Error("服务器返回了不兼容的登录响应");
    return result.data;
  } catch (error) {
    if (error instanceof Error && error.message.includes("Route POST:/auth/verify not found")) {
      throw new Error("服务器尚未支持登录状态验证，请更新服务器，或输入密码重新登录。");
    }
    throw error;
  }
}

// 设备信息（别名等）变化后主动上报。
export async function pushDeviceInfo(
  address: string,
  sessionToken: string,
  device: Device,
  signal?: AbortSignal,
): Promise<void> {
  const { httpUrl } = getServerUrls(address);
  await postJson(httpUrl, "/devices/current", device, { Authorization: `Bearer ${sessionToken}` }, signal);
}

export async function changeAccountPassword(
  address: string,
  sessionToken: string,
  currentPassword: string,
  newPassword: string,
  signal?: AbortSignal,
): Promise<void> {
  const { httpUrl } = getServerUrls(address);
  await postJson(
    httpUrl,
    "/auth/password",
    { currentPassword, newPassword },
    { Authorization: `Bearer ${sessionToken}` },
    signal,
  );
}
