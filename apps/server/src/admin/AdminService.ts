import { randomBytes, timingSafeEqual } from "node:crypto";
import { AttemptThrottle } from "../common/AttemptThrottle.js";
import { ADMIN_SESSION_LIFETIME_MS, LOGIN_ATTEMPT_WINDOW_MS, LOGIN_BLOCKED_FOR_MS, LOGIN_MAX_ATTEMPTS } from "../app/ServerConfig.js";

export class AdminService {
  #sessions = new Map<string, number>();
  #throttle = new AttemptThrottle({ maxAttempts: LOGIN_MAX_ATTEMPTS, windowMs: LOGIN_ATTEMPT_WINDOW_MS, blockedForMs: LOGIN_BLOCKED_FOR_MS });
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
    this.#sessions.set(token, now + ADMIN_SESSION_LIFETIME_MS);
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
