// A download demand: a device asked for content the pool does not hold, and
// some other device of the same account may be able to push it up.
type PullRequest = {
  fileId: string;
  entryId: string;
  size: number;
  createdAt: number;
};

// A demand nobody serves should not haunt the pollers forever.
const REQUEST_TTL_MS = 10 * 60_000;
// A runaway client must not be able to grow the ledger without bound.
const MAX_REQUESTS_PER_USER = 50;

// Downloads pull raw bytes over HTTP (`GET /files/:entryId/:fileId`) and the
// download route itself is side-effect free. This service only tracks what is
// missing: a client that wants the relay declares its demand explicitly
// (`POST /files/requests`) — the client retries the download meanwhile — while
// any device of the account long-polls `pending` and serves what it actually
// holds through the upload routes. No socket is involved.
export class FileDownloadService {
  #requests = new Map<string, Map<string, PullRequest>>();
  #pollWaiters = new Map<string, Set<() => void>>();

  // Records a demand and wakes every device polling for this account's work.
  request(userId: string, fileId: string, entryId: string, size: number): void {
    this.#prune();
    const requests = this.#requests.get(userId) ?? new Map<string, PullRequest>();
    if (!requests.has(fileId)) {
      if (requests.size >= MAX_REQUESTS_PER_USER) return;
      requests.set(fileId, { fileId, entryId, size, createdAt: Date.now() });
      this.#requests.set(userId, requests);
    }
    this.#wake(this.#pollWaiters.get(userId));
  }

  // The demand list a serving device long-polls for.
  pending(userId: string): PullRequest[] {
    this.#prune();
    return [...(this.#requests.get(userId)?.values() ?? [])];
  }

  // Resolves as soon as the account gains a new demand, or after `timeoutMs`.
  waitForRequests(userId: string, timeoutMs: number): Promise<void> {
    return new Promise<void>((resolve) => {
      let wake: () => void;
      const timer = setTimeout(() => wake(), timeoutMs);
      timer.unref();
      wake = () => {
        clearTimeout(timer);
        const waiters = this.#pollWaiters.get(userId);
        waiters?.delete(wake);
        if (waiters && waiters.size === 0) this.#pollWaiters.delete(userId);
        resolve();
      };
      const waiters = this.#pollWaiters.get(userId) ?? new Set<() => void>();
      waiters.add(wake);
      this.#pollWaiters.set(userId, waiters);
    });
  }

  // The upload routes just promoted this content: the demand is served, so
  // it drops out of every poller's list.
  notifyAvailable(fileId: string): void {
    for (const [userId, requests] of this.#requests) {
      if (requests.delete(fileId) && requests.size === 0) this.#requests.delete(userId);
    }
  }

  #wake(waiters: Set<() => void> | undefined): void {
    if (!waiters) return;
    for (const wake of [...waiters]) wake();
  }

  #prune(): void {
    const cutoff = Date.now() - REQUEST_TTL_MS;
    for (const [userId, requests] of this.#requests) {
      for (const [fileId, request] of requests) {
        if (request.createdAt < cutoff) requests.delete(fileId);
      }
      if (requests.size === 0) this.#requests.delete(userId);
    }
  }
}
