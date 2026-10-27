import { FILE_CHUNK_SIZE, type FileRelayRequest } from "@cliproam/protocol";
import type { AccountSession } from "../../accountSession";
import { errorMessageFromBody, type SyncRequester } from "../sync/httpClient";
type RelayUploaderDeps = {
  session: AccountSession;
};
import type { ServeTaskSnapshot } from "./uploadManager";

/** Each relay request starts immediately; independent sessions run concurrently. */
export class RelayUploader {
  #servingSessions = new Set<string>();
  constructor(private readonly http: SyncRequester, private readonly deps: RelayUploaderDeps) {}
  // This device may hold the content a `file.requested` push is asking for.
  // Serving it is a loop of local reads streamed as chunked PUTs into the
  // requester's parked relay pipe. Devices without the bytes stay quiet; the
  // server owns relay timeouts; every new request is handled independently.
  serveRelayRequest(request: FileRelayRequest): Promise<void> {
    return this.deps.session.start(() => this.#serveRelayRequest(request));
  }

  async #serveRelayRequest(request: FileRelayRequest): Promise<void> {
    if (this.#servingSessions.has(request.sessionId)) return;
    this.#servingSessions.add(request.sessionId);
    let task: ServeTaskSnapshot | undefined;
    try {
      const startOffset = request.offset ?? 0;
      const endOffset = request.endOffset ?? request.size;
      if (!Number.isSafeInteger(startOffset) || !Number.isSafeInteger(endOffset)
        || startOffset < 0 || startOffset > endOffset || endOffset > request.size) {
        throw new Error("文件起始位置无效");
      }
      // Probe before claiming: a device without the bytes stays quiet so another
      // holder can serve the request. Empty files still need a local existence check.
      const remaining = endOffset - startOffset;
      const probe = await this.deps.session.files.read(request.fileId, startOffset, remaining > 0 ? 1 : 0);
      if (remaining > 0 && !probe.length) return;
      task = this.deps.session.uploads.record(request);
      this.deps.session.uploads.update(task, { status: "serving" });
      let offset = startOffset;
      for (;;) {
        const length = Math.min(FILE_CHUNK_SIZE, endOffset - offset);
        const bytes = await this.deps.session.files.read(request.fileId, offset, length);
        if (length > 0 && !bytes.length) throw new Error("本机文件内容不可用");
        const last = offset + bytes.byteLength >= endOffset;
        const put = await this.http.fetch(
          "PUT",
          `/files/relay/${request.sessionId}${last ? "?end=1" : ""}`,
          {
            headers: { "Content-Type": "application/octet-stream" },
            body: bytes,
          },
        );
        // 410: the requester hung up or the session expired — nothing to serve.
        if (put.status === 410 || put.status === 409) {
          // 409: another device claimed the session first; 410: requester left.
          this.deps.session.uploads.update(task, {
            status: "skipped",
            error: put.status === 409 ? "其他设备已接管" : "中转会话已结束",
          });
          return;
        }
        if (put.status === 401) throw new Error("登录已失效，请重新登录");
        if (!put.ok) {
          const body = await put.json().catch(() => undefined) as unknown;
          throw new Error(errorMessageFromBody(body, put.status));
        }
        offset += bytes.byteLength;
        this.deps.session.uploads.update(task, { sentBytes: offset - startOffset });
        if (last) {
          this.deps.session.uploads.update(task, { status: "succeeded", sentBytes: remaining });
          return;
        }
      }
    } catch (error) {
      if (task) {
        this.deps.session.uploads.update(task, {
          status: "failed",
          error: error instanceof Error ? error.message : String(error),
        });
      }
    } finally {
      this.#servingSessions.delete(request.sessionId);
    }
  }

}
