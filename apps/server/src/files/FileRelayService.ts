import { randomUUID } from "node:crypto";
import { PassThrough } from "node:stream";

// A relay session lives exactly as long as the requester's held GET. If no
// holder claims it within the unclaimed window the stream is torn down and
// the requester simply retries; once claimed, the transfer is only swept
// when the sender goes silent for the idle window.
const UNCLAIMED_TIMEOUT_MS = 15_000;
const CLAIMED_IDLE_MS = 60_000;

export type RelaySession = {
  id: string;
  userId: string;
  stream: PassThrough;
  // Device id of the sender that won the claim: `file.requested` reaches every
  // online device holding the content, so without this binding a second sender
  // would interleave its chunks into the same pipe and corrupt the stream.
  claimedBy?: string;
  createdAt: number;
  // Last byte the sender fed in; a claimed session with no activity for
  // CLAIMED_IDLE_MS is a half-open transfer and gets swept like an idle one.
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
  readonly #timer: NodeJS.Timeout;

  constructor() {
    // Session lifecycle is driven by the requester's GET and the sender's
    // PUTs, neither of which guarantees a `create` to prune after — so the
    // sweep runs on its own (unref'd, purely advisory) timer as well. The
    // interval is well under the unclaimed window so expiry stays close to
    // the intended timeout.
    this.#timer = setInterval(() => this.#prune(), UNCLAIMED_TIMEOUT_MS / 3);
    this.#timer.unref();
  }

  close(): void {
    clearInterval(this.#timer);
    for (const id of this.#sessions.keys()) this.abandon(id);
  }

  // Independent downloads start immediately; idle sessions are reclaimed by the timer.
  create(userId: string, stream: PassThrough): RelaySession {
    this.#prune();
    const session: RelaySession = {
      id: randomUUID(),
      userId,
      stream,
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
    if (session.claimedBy !== undefined) return session.claimedBy === senderDeviceId;
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
      await new Promise<void>((resolve, reject) => {
        const cleanup = () => {
          stream.off("drain", finished);
          stream.off("close", finished);
          stream.off("error", failed);
        };
        const finished = () => { cleanup(); resolve(); };
        const failed = (error: Error) => { cleanup(); reject(error); };
        stream.once("drain", finished);
        stream.once("close", finished);
        stream.once("error", failed);
      });
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
      const expired = session.claimedBy !== undefined
        ? now - session.lastActivity > CLAIMED_IDLE_MS
        : now - session.createdAt > UNCLAIMED_TIMEOUT_MS;
      if (expired) {
        this.#sessions.delete(id);
        if (!session.stream.destroyed) session.stream.destroy();
      }
    }
  }
}
