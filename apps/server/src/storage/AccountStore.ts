import { createHash, randomBytes, randomUUID, scrypt, timingSafeEqual } from "node:crypto";
import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { promisify } from "node:util";
import type { AuthResponse } from "@cliproam/protocol";
import { accountsDatabasePath } from "./DataPaths.js";

const scryptAsync = promisify(scrypt);
const passwordKeyLength = 64;
const sessionLifetimeMs = 30 * 24 * 60 * 60 * 1000;

export type AccountUser = { id: string; username: string };
export type LegacyUserRow = AccountUser & {
  password_hash: Uint8Array;
  password_salt: Uint8Array;
  created_at: string;
};
export type LegacySessionRow = { token_hash: string; user_id: string; expires_at: string; created_at: string };

export class UsernameTakenError extends Error {
  constructor() { super("该账号已存在"); }
}

export class InvalidCredentialsError extends Error {
  constructor() { super("账号或密码错误"); }
}

export class AccountStore {
  readonly #database: DatabaseSync;

  constructor(databasePath = accountsDatabasePath) {
    const path = resolve(databasePath);
    mkdirSync(dirname(path), { recursive: true });
    this.#database = new DatabaseSync(path);
    this.#database.exec(`
      PRAGMA journal_mode = WAL;
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
      CREATE TABLE IF NOT EXISTS storage_migrations (
        id TEXT PRIMARY KEY,
        completed_at TEXT NOT NULL
      );
    `);
    const sessionColumns = this.#database.prepare("PRAGMA table_info(sessions)").all() as Array<{ name: string }>;
    if (!sessionColumns.some((column) => column.name === "device_id")) {
      this.#database.exec("ALTER TABLE sessions ADD COLUMN device_id TEXT NOT NULL DEFAULT ''");
      this.#database.exec("DELETE FROM sessions WHERE device_id = ''");
    }
    this.#database.exec("CREATE UNIQUE INDEX IF NOT EXISTS sessions_user_device_id ON sessions(user_id, device_id)");
    this.#removeExpiredSessions(new Date());
  }

  async register(username: string, password: string, deviceId: string): Promise<AuthResponse> {
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

  async login(username: string, password: string, deviceId: string): Promise<AuthResponse> {
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

  hasMigration(id: string): boolean {
    return Boolean(this.#database.prepare("SELECT 1 FROM storage_migrations WHERE id = ?").get(id));
  }

  hasUsers(): boolean {
    return Boolean(this.#database.prepare("SELECT 1 FROM users LIMIT 1").get());
  }

  markMigration(id: string): void {
    this.#database.prepare("INSERT OR IGNORE INTO storage_migrations (id, completed_at) VALUES (?, ?)")
      .run(id, new Date().toISOString());
  }

  removeMigration(id: string): void {
    this.#database.prepare("DELETE FROM storage_migrations WHERE id = ?").run(id);
  }

  importLegacyUser(user: LegacyUserRow): void {
    this.#database.prepare(`
      INSERT OR IGNORE INTO users (id, username, password_hash, password_salt, created_at)
      VALUES (?, ?, ?, ?, ?)
    `).run(user.id, user.username, user.password_hash, user.password_salt, user.created_at);
  }

  importLegacySessions(sessions: LegacySessionRow[]): void {
    for (const session of sessions) {
      this.#database.prepare(`
        INSERT OR REPLACE INTO sessions (token_hash, user_id, device_id, expires_at, created_at)
        VALUES (?, ?, ?, ?, ?)
      `).run(session.token_hash, session.user_id, `legacy:${session.token_hash}`, session.expires_at, session.created_at);
    }
  }

  close(): void { this.#database.close(); }

  #issueSession(user: AccountUser, deviceId: string): AuthResponse {
    const token = randomBytes(32).toString("base64url");
    const createdAt = new Date();
    const expiresAt = new Date(createdAt.getTime() + sessionLifetimeMs);
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
  return await scryptAsync(password, salt, passwordKeyLength) as Buffer;
}

function hashSessionToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}
