import { FileAccessResponseSchema } from "@cliproam/protocol";
import type { AccountSession } from "../../accountSession";

export async function getFileUrl(session: AccountSession, entryId: string, fileId: string, name: string, download = false): Promise<string> {
  const http = session.http;
  const result = await http.request("POST", `/files/${encodeURIComponent(entryId)}/${fileId}/access`, {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, download }),
  }, FileAccessResponseSchema, "服务器返回的文件访问链接无效");
  session.signal.throwIfAborted();
  if (!result) throw new Error("文件访问链接不可用");
  return http.url(result.path);
}

/** Hand the response stream to the browser's download manager, without an IndexedDB copy. */
export async function downloadWithBrowser(session: AccountSession, entryId: string, fileId: string, name: string): Promise<void> {
  session.signal.throwIfAborted();
  const cached = await session.files.resolve(fileId);
  const url = cached ?? await getFileUrl(session, entryId, fileId, name, true);
  session.signal.throwIfAborted();
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.rel = "noreferrer";
  link.referrerPolicy = "no-referrer";
  document.body.append(link);
  link.click();
  link.remove();
}
