import { errorMessage } from "../../utils/error";

/** Structural stand-in for the protocol's zod schemas, keeping zod out of call sites' imports. */
type Schema<T> = { safeParse: (value: unknown) => { success: true; data: T } | { success: false } };

export function errorMessageFromBody(body: unknown, status: number): string {
  return typeof body === "object" && body && "message" in body
    ? String((body as { message: unknown }).message)
    : `服务器返回错误 ${status}`;
}

// The network-level failure every retry loop matches on: fetch-level errors
// (unreachable server, lost connection) map to this one message. A timeout
// keeps its own separate one — the server answered slowly, which is not a
// disconnect and must not masquerade as one or retry loops would spin.
export const TRANSIENT_NETWORK_ERROR_MESSAGE = "同步连接已断开";

export function isTransientNetworkError(error: unknown): boolean {
  return errorMessage(error) === TRANSIENT_NETWORK_ERROR_MESSAGE;
}

export type SyncRequester = {
  // Raw fetch with the Bearer header attached and network failures mapped to
  // the transient error message above.
  fetch: (method: string, path: string, init: RequestInit) => Promise<Response>;
  // Shared pipeline for the JSON endpoints: one fetch, one 401 check, one
  // error-body extraction and one schema validation each. With `tolerate404`
  // a missing resource resolves to undefined instead of failing.
  request: <T>(
    method: string,
    path: string,
    init: RequestInit,
    schema: Schema<T> | null,
    incompatible: string,
    tolerate404?: boolean,
  ) => Promise<T | undefined>;
};

// One requester per client session: everything rides the login session's
// Bearer token over plain HTTP.
export function createSyncRequester(httpUrl: string, token: string): SyncRequester {
  const httpFetch = async (method: string, path: string, init: RequestInit): Promise<Response> => {
    try {
      return await fetch(`${httpUrl}${path}`, {
        ...init,
        method,
        headers: {
          Authorization: `Bearer ${token}`,
          ...(init.headers as Record<string, string>),
        },
      });
    } catch (error) {
      // A timeout means the server is reachable but too slow — the retry
      // loops treat only the transient message as retryable, so a timeout
      // surfaces as its own error instead of a disconnect.
      if (error instanceof DOMException && error.name === "TimeoutError") {
        throw new Error("同步服务响应超时");
      }
      throw new Error(TRANSIENT_NETWORK_ERROR_MESSAGE);
    }
  };

  const request = async <T>(
    method: string,
    path: string,
    init: RequestInit,
    schema: Schema<T> | null,
    incompatible: string,
    tolerate404 = false,
  ): Promise<T | undefined> => {
    const response = await httpFetch(method, path, init);
    if (response.status === 401) throw new Error("登录已失效，请重新登录");
    if (tolerate404 && response.status === 404) return undefined;
    const body = await response.json().catch(() => undefined) as unknown;
    if (!response.ok) throw new Error(errorMessageFromBody(body, response.status));
    if (!schema) return undefined;
    const parsed = schema.safeParse(body);
    if (!parsed.success) throw new Error(incompatible);
    return parsed.data;
  };

  return { fetch: httpFetch, request };
}
