export type TlsStatus = { enabled: boolean; source: "managed" | "none" };
export type TransferSettings = { maxStoredFileMb: number; resumableUploadTtlHours: number };
export type StatusResponse = { tls: TlsStatus; transfer: TransferSettings };
export type AdminUser = { id: string; username: string; createdAt: string; activeSessions: number };
export type AdminDevice = { id: string; name: string; platform: string; osVersion: string; lastSeenAt: string };

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/admin-api/${path}`, {
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", ...init?.headers },
    ...init,
  });
  const body = await response.json().catch(() => ({})) as { message?: string } & T;
  if (!response.ok) throw new Error(body.message ?? `请求失败（${response.status}）`);
  return body;
}

export async function fetchStatus(): Promise<StatusResponse> {
  return request<StatusResponse>("status");
}

export async function login(password: string): Promise<void> {
  await request("login", { method: "POST", body: JSON.stringify({ password }) });
}

export async function logout(): Promise<void> {
  await request("logout", { method: "POST" }).catch(() => undefined);
}

export async function fetchUsers(search = ""): Promise<AdminUser[]> {
  const keyword = search.trim();
  const query = keyword ? `?search=${encodeURIComponent(keyword)}` : "";
  const result = await request<{ users: AdminUser[] }>(`users${query}`);
  return result.users;
}

export async function deleteUser(userId: string): Promise<void> {
  await request(`users/${userId}`, { method: "DELETE" });
}

export async function resetUserPassword(userId: string, password: string): Promise<void> {
  await request(`users/${userId}/password`, { method: "PUT", body: JSON.stringify({ password }) });
}

export async function fetchUserDevices(userId: string): Promise<AdminDevice[]> {
  const result = await request<{ devices: AdminDevice[] }>(`users/${userId}/devices`);
  return result.devices;
}

export async function deleteUserDevice(userId: string, deviceId: string): Promise<void> {
  await request(`users/${userId}/devices/${encodeURIComponent(deviceId)}`, { method: "DELETE" });
}

export async function updateTransferSettings(settings: TransferSettings): Promise<TransferSettings> {
  const result = await request<{ transfer: TransferSettings }>("transfer-settings", {
    method: "PUT",
    body: JSON.stringify(settings),
  });
  return result.transfer;
}

export async function replaceTls(cert: string, key: string): Promise<{ tls: TlsStatus; restartRequired: boolean }> {
  return request<{ tls: TlsStatus; restartRequired: boolean }>("tls", {
    method: "PUT",
    body: JSON.stringify({ cert, key }),
  });
}

export async function removeTls(): Promise<{ tls: TlsStatus; restartRequired: boolean }> {
  return request<{ tls: TlsStatus; restartRequired: boolean }>("tls", { method: "DELETE" });
}
