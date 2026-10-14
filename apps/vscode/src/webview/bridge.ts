declare function acquireVsCodeApi(): { postMessage(message: unknown): void };
const vscode = acquireVsCodeApi();
let nextId = 0;
const pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void; timer: number }>();

window.addEventListener("message", ({ data }) => {
  if (data?.type !== "response" || typeof data.id !== "number") return;
  const request = pending.get(data.id);
  if (!request) return;
  pending.delete(data.id);
  window.clearTimeout(request.timer);
  if (typeof data.error === "string") request.reject(new Error(data.error));
  else request.resolve(data.value);
});

export function request<T>(operation: string, value?: unknown): Promise<T> {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      pending.delete(id);
      reject(new Error("VS Code 操作超时，请重试"));
    }, 30_000);
    pending.set(id, { resolve: (result) => resolve(result as T), reject, timer });
    vscode.postMessage({ type: "request", id, operation, value });
  });
}

export function postMessage(message: unknown): void { vscode.postMessage(message); }
