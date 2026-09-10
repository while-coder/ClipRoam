import { createHash, randomBytes, randomUUID, scrypt, timingSafeEqual } from "node:crypto";
import { promisify } from "node:util";
import type { AuthResponse } from "@cliproam/protocol";
// `AuthResponse` 去掉服务器附带字段：`settings` 由路由层注入。
type AuthSession = Omit<AuthResponse, "settings">;
import type Database from "better-sqlite3";
import { openDatabase } from "../sqlite.js";
import { accountsDatabasePath } from "../DataPaths.js";
import { ACCOUNT_SESSION_LIFETIME_MS, PASSWORD_KEY_LENGTH } from "../app/ServerConfig.js";

const scryptAsync = promisify(scrypt);

export type AccountUser = { id: string; username: string };

export type AdminUserSummary = { id: string; username: string; createdAt: string; activeSessions: number };

export class UsernameTakenError extends Error {
  constructor() { super("该账号已存在"); }
}

export class InvalidCredentialsError extends Error {
  constructor() { super("账号或密码错误"); }
}

export class AccountStore {
  readonly #database: Database.Database;

  constructor() {
    this.#database = openDatabase(accountsDatabasePath);
    this.#database.exec(`
      PRAGMA foreign_keys = ON;

      CREATE TABLE IF NOT EXISTS users (
        id TEXT PRIMARY KEY,
        username TEXT NOT NULL UNIQUE COLLATE NOCASE,
        password_hash BLOB NOT NULL,
        password_salt BLOB NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS sessions (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL,
        device_id TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
      );
      CREATE INDEX IF NOT EXISTS sessions_user_id ON sessions(user_id);
      CREATE INDEX IF NOT EXISTS sessions_expires_at ON sessions(expires_at);
    `);
    this.#database.exec("CREATE UNIQUE INDEX IF NOT EXISTS sessions_user_device_id ON sessions(user_id, device_id)");
    this.#removeExpiredSessions(new Date());
  }

  async register(username: string, password: string, deviceId: string): Promise<AuthSession> {
    const normalizedUsername = username.trim();
    if (this.#database.prepare("SELECT id FROM users WHERE username = ? COLLATE NOCASE").get(normalizedUsername)) {
      throw new UsernameTakenError();
    }
    const id = randomUUID();
    const salt = randomBytes(16);
    const passwordHash = await derivePassword(password, salt);
    try {
      this.#database.prepare(`
        INSERT INTO users (id, username, password_hash, password_salt, created_at)
        VALUES (?, ?, ?, ?, ?)
      `).run(id, normalizedUsername, passwordHash, salt, new Date().toISOString());
    } catch (error) {
      if (String(error).includes("UNIQUE constraint failed")) throw new UsernameTakenError();
      throw error;
    }
    return this.#issueSession({ id, username: normalizedUsername }, deviceId);
  }

  async login(username: string, password: string, deviceId: string): Promise<AuthSession> {
    const row = this.#database.prepare(`
      SELECT id, username, password_hash, password_salt
      FROM users WHERE username = ? COLLATE NOCASE
    `).get(username.trim()) as (AccountUser & { password_hash: Uint8Array; password_salt: Uint8Array }) | undefined;
    if (!row) {
      await derivePassword(password, randomBytes(16));
      throw new InvalidCredentialsError();
    }
    const actualHash = await derivePassword(password, Buffer.from(row.password_salt));
    const expectedHash = Buffer.from(row.password_hash);
    if (actualHash.length !== expectedHash.length || !timingSafeEqual(actualHash, expectedHash)) {
      throw new InvalidCredentialsError();
    }
    return this.#issueSession({ id: row.id, username: row.username }, deviceId);
  }

  async changePassword(userId: string, currentPassword: string, newPassword: string): Promise<void> {
    const row = this.#database.prepare(`
      SELECT id, username, password_hash, password_salt
      FROM users WHERE id = ?
    `).get(userId) as (AccountUser & { password_hash: Uint8Array; password_salt: Uint8Array }) | undefined;
    if (!row) throw new InvalidCredentialsError();

    const actualHash = await derivePassword(currentPassword, Buffer.from(row.password_salt));
    const expectedHash = Buffer.from(row.password_hash);
    if (actualHash.length !== expectedHash.length || !timingSafeEqual(actualHash, expectedHash)) {
      throw new InvalidCredentialsError();
    }

    const salt = randomBytes(16);
    const passwordHash = await derivePassword(newPassword, salt);
    this.#database.prepare("UPDATE users SET password_hash = ?, password_salt = ? WHERE id = ?")
      .run(passwordHash, salt, row.id);
    this.#database.prepare("DELETE FROM sessions WHERE user_id = ?").run(row.id);
  }

  authenticateSession(token: string): AccountUser | undefined {
    if (!token) return undefined;
    return this.#database.prepare(`
      SELECT users.id, users.username
      FROM sessions JOIN users ON users.id = sessions.user_id
      WHERE sessions.token_hash = ? AND sessions.expires_at > ?
    `).get(hashSessionToken(token), new Date().toISOString()) as AccountUser | undefined;
  }

  listUserIds(): string[] {
    return (this.#database.prepare("SELECT id FROM users").all() as Array<{ id: string }>).map(({ id }) => id);
  }

  listUsers(search?: string): AdminUserSummary[] {
    // Active sessions double as the device count: one session row per user +
    // device pair, so opening each per-user database for a real count is not
    // worth the cost on the admin list.
    const keyword = search?.trim().slice(0, 100) ?? "";
    return this.#database.prepare(`
      SELECT users.id, users.username, users.created_at AS createdAt,
        (SELECT COUNT(*) FROM sessions WHERE sessions.user_id = users.id AND sessions.expires_at > @now) AS activeSessions
      FROM users
      WHERE (@search IS NULL OR username LIKE @search ESCAPE '\\')
      ORDER BY users.created_at DESC
    `).all({
      now: new Date().toISOString(),
      search: keyword ? `%${escapeLike(keyword)}%` : null,
    }) as AdminUserSummary[];
  }

  deleteUser(userId: string): boolean {
    return this.#database.prepare("DELETE FROM users WHERE id = ?").run(userId).changes > 0;
  }

  hasUser(userId: string): boolean {
    return !!this.#database.prepare("SELECT 1 FROM users WHERE id = ?").get(userId);
  }

  // Administrator-initiated reset: no current-password check, and every
  // session is dropped so signed-in devices must sign in again.
  async resetPassword(userId: string, newPassword: string): Promise<boolean> {
    if (!this.hasUser(userId)) return false;
    const salt = randomBytes(16);
    const passwordHash = await derivePassword(newPassword, salt);
    this.#database.prepare("UPDATE users SET password_hash = ?, password_salt = ? WHERE id = ?")
      .run(passwordHash, salt, userId);
    this.#database.prepare("DELETE FROM sessions WHERE user_id = ?").run(userId);
    return true;
  }

  deleteSession(userId: string, deviceId: string): void {
    this.#database.prepare("DELETE FROM sessions WHERE user_id = ? AND device_id = ?").run(userId, deviceId);
  }

  close(): void { this.#database.close(); }

  #issueSession(user: AccountUser, deviceId: string): AuthSession {
    const token = randomBytes(32).toString("base64url");
    const createdAt = new Date();
    const expiresAt = new Date(createdAt.getTime() + ACCOUNT_SESSION_LIFETIME_MS);
    this.#removeExpiredSessions(createdAt);
    this.#database.prepare(`
      INSERT INTO sessions (token_hash, user_id, device_id, expires_at, created_at)
      VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(user_id, device_id) DO UPDATE SET
        token_hash = excluded.token_hash,
        expires_at = excluded.expires_at,
        created_at = excluded.created_at
    `).run(hashSessionToken(token), user.id, deviceId, expiresAt.toISOString(), createdAt.toISOString());
    return { sessionToken: token, expiresAt: expiresAt.toISOString(), user };
  }

  #removeExpiredSessions(now: Date): void {
    this.#database.prepare("DELETE FROM sessions WHERE expires_at <= ?").run(now.toISOString());
  }
}

async function derivePassword(password: string, salt: Uint8Array): Promise<Buffer> {
  return await scryptAsync(password, salt, PASSWORD_KEY_LENGTH) as Buffer;
}

function escapeLike(value: string): string {
  return value.replace(/[\\%_]/g, "\\$&");
}

function hashSessionToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}
