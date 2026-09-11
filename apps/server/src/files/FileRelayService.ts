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
  // Device id of the sender that won the claim: `file.requested` reaches every
  // online device holding the content, so without this binding a second sender
  // would interleave its chunks into the same pipe and corrupt the stream.
  claimedBy?: string;
  createdAt: number;
  // Last byte the sender fed in; a claimed session with no activity for
  // SESSION_IDLE_MS is a half-open transfer and gets swept like an idle one.
  lastActivity: number;
  // Serializes `push` calls: backpressure waits happen on this chain, so two
  // concurrent PUTs cannot interleave their writes and corrupt the byte order.
  queue: Promise<unknown>;
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

  constructor() {
    // Session lifecycle is driven by the requester's GET and the sender's
    // PUTs, neither of which guarantees a `create` to prune after — so the
    // sweep runs on its own (unref'd, purely advisory) timer as well.
    const timer = setInterval(() => this.#prune(), SESSION_IDLE_MS / 2);
    timer.unref?.();
  }

  // Returns undefined when the user already pins the session cap: a misbehaving
  // client must not be able to hold unbounded streams. The route answers 429.
  create(userId: string, entryId: string, fileId: string, size: number, stream: PassThrough): RelaySession | undefined {
    this.#prune();
    let held = 0;
    for (const session of this.#sessions.values()) {
      if (session.userId === userId) held += 1;
    }
    if (held >= MAX_SESSIONS_PER_USER) return undefined;
    const session: RelaySession = {
      id: randomUUID(),
      userId,
      entryId,
      fileId,
      size,
      stream,
      claimed: false,
      createdAt: Date.now(),
      lastActivity: Date.now(),
      queue: Promise.resolve(),
    };
    this.#sessions.set(session.id, session);
    return session;
  }

  get(sessionId: string): RelaySession | undefined {
    return this.#sessions.get(sessionId);
  }

  // Only one sender may feed a session: the first PUT claims it for that
  // sender's device, later chunks from the same device pass, and a different
  // device (two devices hold the same content) is rejected so the two byte
  // streams cannot interleave.
  admit(sessionId: string, senderDeviceId: string): boolean {
    const session = this.#sessions.get(sessionId);
    if (!session || session.stream.destroyed) return false;
    if (session.claimed) return session.claimedBy === senderDeviceId;
    session.claimed = true;
    session.claimedBy = senderDeviceId;
    return true;
  }

  // Write one chunk into the requester's pipe, respecting its backpressure.
  // Chunks join the session's write queue, so concurrent PUTs keep byte order
  // even while one is parked on a `drain`. Resolves false when the stream is
  // gone (requester disconnected): the sender reads that as "stop sending".
  async push(sessionId: string, chunk: Buffer): Promise<boolean> {
    const session = this.#sessions.get(sessionId);
    if (!session || session.stream.destroyed) return false;
    const run = session.queue.then(() => this.#write(session, chunk));
    session.queue = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  async #write(session: RelaySession, chunk: Buffer): Promise<boolean> {
    const stream = session.stream;
    if (stream.destroyed) return false;
    session.lastActivity = Date.now();
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
      const idleSince = session.claimed ? session.lastActivity : session.createdAt;
      if (now - idleSince > SESSION_IDLE_MS) {
        this.#sessions.delete(id);
        if (!session.stream.destroyed) session.stream.destroy();
      }
    }
  }
}
