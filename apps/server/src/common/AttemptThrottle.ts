// Shared login throttling: after `maxAttempts` failures inside `windowMs` the
// key is blocked for `blockedForMs`. AuthService (per ip:username) and
// AdminService (per ip) both use the identical sliding-window policy.
export type AttemptThrottleOptions = {
  maxAttempts: number;
  windowMs: number;
  blockedForMs: number;
};

type Attempt = { failures: number; windowStarted: number; blockedUntil: number };

export class AttemptThrottle {
  readonly #attempts = new Map<string, Attempt>();
  readonly #options: AttemptThrottleOptions;

  constructor(options: AttemptThrottleOptions) {
    this.#options = options;
  }

  isBlocked(key: string, now: number): boolean {
    const attempt = this.#attempts.get(key);
    return attempt !== undefined && attempt.blockedUntil > now;
  }

  recordFailure(key: string, now: number): void {
    const { maxAttempts, windowMs, blockedForMs } = this.#options;
    const previous = this.#attempts.get(key);
    const current = !previous || now - previous.windowStarted > windowMs
      ? { failures: 0, windowStarted: now, blockedUntil: 0 }
      : previous;
    current.failures += 1;
    if (current.failures >= maxAttempts) current.blockedUntil = now + blockedForMs;
    this.#attempts.set(key, current);
  }

  reset(key: string): void {
    this.#attempts.delete(key);
  }
}
