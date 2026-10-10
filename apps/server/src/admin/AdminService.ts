import { randomBytes } from "node:crypto";
import { secretsEqual } from "../common/secretsEqual.js";
import { ADMIN_SESSION_LIFETIME_MS } from "../app/ServerConfig.js";

export class AdminService {
  #sessions = new Map<string, number>();
  readonly #password: string;

  constructor(password = process.env.CLIPROAM_ADMIN_PASSWORD ?? "") {
    this.#password = password;
  }

  get isConfigured(): boolean {
    return this.#password.length > 0;
  }

  login(password: unknown): { token: string } | { error: "NOT_CONFIGURED" | "INVALID_CREDENTIALS" } {
    if (!this.isConfigured) return { error: "NOT_CONFIGURED" };
    const now = Date.now();

    if (!secretsEqual(this.#password, password)) {
      return { error: "INVALID_CREDENTIALS" };
    }

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
