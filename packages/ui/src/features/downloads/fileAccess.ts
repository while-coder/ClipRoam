import { FileAccessResponseSchema } from "@cliproam/protocol";
import type { AccountSession } from "../../accountSession";

/** Browser downloads stream from a temporary URL without an application-owned copy. */
export async function downloadWithBrowser(session: AccountSession, entryId: string, fileId: string, name: string): Promise<void> {
  session.signal.throwIfAborted();
  const http = session.http;
  const result = await http.request("POST", `/files/${encodeURIComponent(entryId)}/${fileId}/access`, {
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, download: true }),
  }, FileAccessResponseSchema, "服务器返回的文件访问链接无效");
  session.signal.throwIfAborted();
  if (!result) throw new Error("文件访问链接不可用");
  const link = document.createElement("a");
  link.href = http.url(result.path);
  link.download = name;
  link.rel = "noreferrer";
  link.referrerPolicy = "no-referrer";
  document.body.append(link);
  link.click();
  link.remove();
}
