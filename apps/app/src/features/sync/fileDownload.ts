import { invoke } from "@tauri-apps/api/core";

/**
 * 公共文件下载管线：一次 HTTP GET 拉流，字节经 Rust 命令写入内容寻址缓存
 * （或另存会话的 staging 目录）。不依赖 WebSocket——服务器池缺内容时 GET
 * 会挂在 relay 管道上，由持有内容的设备经其 WebSocket 收到推送后回填，
 * 请求方无感知。任何窗口（主窗口的 SyncClient、paste 窗口的直接下载）
 * 都可凭凭据直接使用。
 */

/** 下载凭据：只需服务器地址与令牌，无需持有同步连接。 */
export type FileDownloadCredentials = { httpUrl: string; token: string };

/** The file-shape fields a download transfer needs. */
export type FileDownloadReference = { fileId: string; size: number };

const DOWNLOAD_TIMEOUT_MS = 5 * 60_000;
const DOWNLOAD_RETRY_MS = 3_000;

async function httpFetch(
  credentials: FileDownloadCredentials,
  path: string,
  init: RequestInit,
): Promise<Response> {
  try {
    return await fetch(`${credentials.httpUrl}${path}`, {
      ...init,
      headers: {
        Authorization: `Bearer ${credentials.token}`,
        ...init.headers as Record<string, string>,
      },
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === "TimeoutError") {
      throw new Error("同步服务响应超时");
    }
    throw new Error("同步连接已断开");
  }
}

// `String.fromCharCode` blows the call stack past ~64K arguments, so the bytes
// go in bounded slices.
function bytesToBase64(bytes: Uint8Array): string {
  const sliceSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += sliceSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + sliceSize));
  }
  return btoa(binary);
}

async function drainIntoTransfer(
  transferId: string,
  response: Response,
  offset: number,
): Promise<number> {
  const reader = response.body?.getReader();
  if (!reader) throw new Error("服务器未返回文件内容");
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    await invoke("append_file_download", { transferId, data: bytesToBase64(value) });
    offset += value.byteLength;
  }
  return offset;
}

// One pull attempt: the single download GET either streams stored bytes or
// parks on the relay pipe until a holder fills it. Returns the offset the
// transfer has advanced to; completion is the caller's check.
async function pullOnce(
  credentials: FileDownloadCredentials,
  transferId: string,
  entryId: string,
  file: FileDownloadReference,
  offset: number,
  signal: AbortSignal | undefined,
): Promise<number> {
  const response = await httpFetch(
    credentials,
    `/files/${entryId}/${file.fileId}`,
    { signal },
  );
  if (response.status === 401) throw new Error("登录已失效，请重新登录");
  // Error bodies are JSON; the success body is the file itself, so it must
  // stay unread until the streaming loop below.
  if (!response.ok) {
    const body = await response.json().catch(() => undefined) as unknown;
    const message = typeof body === "object" && body && "message" in body
      ? String((body as { message: unknown }).message)
      : `服务器返回错误 ${response.status}`;
    throw new Error(message);
  }
  return drainIntoTransfer(transferId, response, offset);
}

/**
 * 下载一个内容到本地（Rust 侧落盘并校验 digest）。中途断流会退避重试：
 * GET 无 offset 参数，重试时整个 transfer 从零重启。
 */
export async function downloadStoredFile(
  credentials: FileDownloadCredentials,
  entryId: string,
  file: FileDownloadReference,
  options: { saveId?: string; signal?: AbortSignal } = {},
): Promise<void> {
  const deadline = Date.now() + DOWNLOAD_TIMEOUT_MS;
  let transferId = crypto.randomUUID();
  await invoke("begin_file_download", {
    transferId,
    fileId: file.fileId,
    expectedSize: file.size,
    saveId: options.saveId,
  });
  let offset = 0;
  try {
    for (;;) {
      if (offset >= file.size) {
        await invoke("finish_file_download", { transferId });
        return;
      }
      if (Date.now() >= deadline) throw new Error("文件下载超时，没有设备能够提供该文件");
      try {
        offset = await pullOnce(
          credentials,
          transferId,
          entryId,
          file,
          offset,
          options.signal,
        );
      } catch (error) {
        if (options.signal?.aborted) throw error;
        // Broken pipe, expired session, network hiccup: back off and retry.
        await new Promise((resolve) => setTimeout(resolve, DOWNLOAD_RETRY_MS));
        // The retry GET streams from byte 0 again (no offset parameter), so
        // the transfer must restart too — appending the fresh stream onto the
        // already-received prefix would overflow the declared size and fail on
        // every subsequent attempt.
        await invoke("cancel_file_download", {
          transferId,
          reason: error instanceof Error ? error.message : String(error),
        }).catch(() => undefined);
        transferId = crypto.randomUUID();
        await invoke("begin_file_download", {
          transferId,
          fileId: file.fileId,
          expectedSize: file.size,
          saveId: options.saveId,
        });
        offset = 0;
      }
    }
  } catch (error) {
    await invoke("cancel_file_download", {
      transferId,
      reason: error instanceof Error ? error.message : String(error),
    }).catch(() => undefined);
    throw error;
  }
}
