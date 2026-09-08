import { randomBytes, timingSafeEqual } from "node:crypto";

const sessionLifetimeMs = 8 * 60 * 60 * 1_000;
const maxAttempts = 5;
const attemptWindowMs = 5 * 60 * 1_000;
const blockedForMs = 60 * 1_000;

type LoginAttempt = { failures: number; windowStarted: number; blockedUntil: number };

export class AdminService {
  #sessions = new Map<string, number>();
  #attempts = new Map<string, LoginAttempt>();
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
    const attempt = this.#attempts.get(ip);
    if (attempt && attempt.blockedUntil > now) return { error: "TOO_MANY_ATTEMPTS" };

    if (typeof password !== "string" || !sameSecret(this.password, password)) {
      this.#recordFailure(ip, now);
      return { error: "INVALID_CREDENTIALS" };
    }

    this.#attempts.delete(ip);
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

  #recordFailure(ip: string, now: number): void {
    const previous = this.#attempts.get(ip);
    const current = !previous || now - previous.windowStarted > attemptWindowMs
      ? { failures: 0, windowStarted: now, blockedUntil: 0 }
      : previous;
    current.failures += 1;
    if (current.failures >= maxAttempts) current.blockedUntil = now + blockedForMs;
    this.#attempts.set(ip, current);
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
