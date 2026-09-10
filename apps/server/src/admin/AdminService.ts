import { randomBytes, timingSafeEqual } from "node:crypto";
import { AttemptThrottle } from "../common/AttemptThrottle.js";
import { SERVER_DEFAULTS } from "../app/ServerConfig.js";

const sessionLifetimeMs = SERVER_DEFAULTS.adminSessionLifetimeMs;
const maxAttempts = 5;
const attemptWindowMs = 5 * 60 * 1_000;
const blockedForMs = 60 * 1_000;

export class AdminService {
  #sessions = new Map<string, number>();
  #throttle = new AttemptThrottle({ maxAttempts, windowMs: attemptWindowMs, blockedForMs });
  readonly #password: string;

  constructor(password = process.env.CLIPROAM_ADMIN_PASSWORD ?? "") {
    this.#password = password;
  }

  get password(): string {
    return this.#password;
  }

  get isConfigured(): boolean {
    return this.#password.length > 0;
  }

  login(ip: string, password: unknown): { token: string } | { error: "NOT_CONFIGURED" | "INVALID_CREDENTIALS" | "TOO_MANY_ATTEMPTS" } {
    if (!this.isConfigured) return { error: "NOT_CONFIGURED" };
    const now = Date.now();
    if (this.#throttle.isBlocked(ip, now)) return { error: "TOO_MANY_ATTEMPTS" };

    if (typeof password !== "string" || !sameSecret(this.password, password)) {
      this.#throttle.recordFailure(ip, now);
      return { error: "INVALID_CREDENTIALS" };
    }

    this.#throttle.reset(ip);
    this.#removeExpiredSessions(now);
    const token = randomBytes(32).toString("base64url");
    this.#sessions.set(token, now + sessionLifetimeMs);
    return { token };
  }

  authenticate(token: string | undefined): boolean {
    if (!token) return false;
    const expiresAt = this.#sessions.get(token);
    if (!expiresAt || expiresAt <= Date.now()) {
      this.#sessions.delete(token);
      return false;
    }
    return true;
  }

  logout(token: string | undefined): void {
    if (token) this.#sessions.delete(token);
  }

  #removeExpiredSessions(now: number): void {
    for (const [token, expiresAt] of this.#sessions) {
      if (expiresAt <= now) this.#sessions.delete(token);
    }
  }
}

function sameSecret(expected: string, actual: string): boolean {
  const expectedBuffer = Buffer.from(expected);
  const actualBuffer = Buffer.from(actual);
  return expectedBuffer.length === actualBuffer.length && timingSafeEqual(expectedBuffer, actualBuffer);
}
