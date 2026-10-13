/** Native modules are loaded only inside the Tauri adapter, never by the core. */
export const isNative = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function nativeCall<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isNative) throw new Error("此操作需要桌面或手机客户端");
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

export async function nativeListen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  if (!isNative) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<T>(event, ({ payload }) => handler(payload));
}

export async function previewSource(path: string): Promise<string> {
  if (!isNative || /^(blob:|data:|https?:)/.test(path)) return path;
  return (await import("@tauri-apps/api/core")).convertFileSrc(path);
}
