import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { PassThrough } from "node:stream";

// A relay session lives exactly as long as the requester's held GET. If no
// holder claims it within this window the stream is torn down and the
// requester simply retries.
const SESSION_IDLE_MS = 60_000;

// One session per waiting download, but a misbehaving client must not be able
// to pin unbounded streams to the server.
const MAX_SESSIONS_PER_USER = 20;

export type RelaySession = {
  id: string;
  userId: string;
  entryId: string;
  fileId: string;
  size: number;
  stream: PassThrough;
  claimed: boolean;
  createdAt: number;
};

/**
 * Online forwarding for content the server does not store: the requester holds
 * a GET whose response body is a live pipe, the holder streams the bytes into
 * `PUT /files/relay/:sessionId`, and nothing ever touches the disk. Byte flow
 * is chunked PUTs (natural ordering, backpressure via request/response), so
 * the only state here is the session row itself.
 */
export class FileRelayService {
  readonly #sessions = new Map<string, RelaySession>();

  create(userId: string, entryId: string, fileId: string, size: number, stream: PassThrough): RelaySession {
    this.#prune();
    const session: RelaySession = {
      id: randomUUID(),
      userId,
      entryId,
      fileId,
      size,
      stream,
      claimed: false,
      createdAt: Date.now(),
    };
    this.#sessions.set(session.id, session);
    return session;
  }

  get(sessionId: string): RelaySession | undefined {
    return this.#sessions.get(sessionId);
  }

  // Only one sender may feed a session; a second claim (e.g. two devices hold
  // the same content) gets a false and keeps its bytes.
  claim(sessionId: string): boolean {
    const session = this.#sessions.get(sessionId);
    if (!session || session.claimed || session.stream.destroyed) return false;
    session.claimed = true;
    return true;
  }

  // Write one chunk into the requester's pipe, respecting its backpressure.
  // Resolves false when the stream is gone (requester disconnected): the
  // sender reads that as "stop sending".
  async push(sessionId: string, chunk: Buffer): Promise<boolean> {
    const session = this.#sessions.get(sessionId);
    if (!session || session.stream.destroyed) return false;
    const stream = session.stream;
    if (!stream.write(chunk)) {
      await Promise.race([
        once(stream, "drain"),
        once(stream, "close"),
        once(stream, "error"),
      ]);
    }
    return !stream.destroyed;
  }

  // Sender finished: let the requester's body end cleanly.
  end(sessionId: string): void {
    const session = this.#sessions.get(sessionId);
    if (!session) return;
    this.#sessions.delete(sessionId);
    session.stream.end();
  }

  // Requester hung up (or the idle timer fired): tear the pipe down so a
  // still-sending sender sees a failure on its next PUT.
  abandon(sessionId: string): void {
    const session = this.#sessions.get(sessionId);
    if (!session) return;
    this.#sessions.delete(sessionId);
    if (!session.stream.destroyed) session.stream.destroy();
  }

  #prune(): void {
    const now = Date.now();
    for (const [id, session] of this.#sessions) {
      if (!session.claimed && now - session.createdAt > SESSION_IDLE_MS) {
        this.#sessions.delete(id);
        if (!session.stream.destroyed) session.stream.destroy();
      }
    }
  }
}
