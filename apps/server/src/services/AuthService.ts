import { AuthCredentialsSchema, ChangePasswordSchema } from "@cliproam/protocol";
import { ClipRoamStore, InvalidCredentialsError, UsernameTakenError } from "../storage/ClipRoamStore.js";

type LoginAttempt = { failures: number; windowStarted: number; blockedUntil: number };
export type HttpResult = { statusCode: number; payload: unknown };

export class AuthService {
  #loginAttempts = new Map<string, LoginAttempt>();

  constructor(private readonly store: ClipRoamStore) {}

  async register(body: unknown): Promise<HttpResult> {
    const parsed = AuthCredentialsSchema.safeParse(body);
    if (!parsed.success) {
      return {
        statusCode: 400,
        payload: { code: "INVALID_CREDENTIALS", message: "账号需为 3-32 位字母、数字或 _.-，密码至少 6 位" },
      };
    }
    try {
      return { statusCode: 201, payload: await this.store.register(parsed.data.username, parsed.data.password, parsed.data.deviceId) };
    } catch (error) {
      if (error instanceof UsernameTakenError) {
        return { statusCode: 409, payload: { code: "USERNAME_TAKEN", message: error.message } };
      }
      throw error;
    }
  }

  async login(ip: string, body: unknown): Promise<HttpResult> {
    const parsed = AuthCredentialsSchema.safeParse(body);
    if (!parsed.success) {
      return { statusCode: 400, payload: { code: "INVALID_CREDENTIALS", message: "账号或密码格式不正确" } };
    }

    const attemptKey = `${ip}:${parsed.data.username.toLocaleLowerCase()}`;
    const now = Date.now();
    const attempt = this.#loginAttempts.get(attemptKey);
    if (attempt && attempt.blockedUntil > now) {
      return { statusCode: 429, payload: { code: "TOO_MANY_ATTEMPTS", message: "登录尝试过多，请稍后再试" } };
    }

    try {
      const session = await this.store.login(parsed.data.username, parsed.data.password, parsed.data.deviceId);
      this.#loginAttempts.delete(attemptKey);
      return { statusCode: 200, payload: session };
    } catch (error) {
      if (error instanceof InvalidCredentialsError) {
        this.#recordLoginFailure(attemptKey, now);
        return { statusCode: 401, payload: { code: "INVALID_CREDENTIALS", message: error.message } };
      }
      throw error;
    }
  }

  async changePassword(ip: string, token: string | undefined, body: unknown): Promise<HttpResult> {
    const parsed = ChangePasswordSchema.safeParse(body);
    if (!parsed.success) {
      return { statusCode: 400, payload: { code: "INVALID_PASSWORD", message: "新密码至少 6 位，且不能与当前密码相同" } };
    }
    const user = token ? this.store.authenticateSession(token) : undefined;
    if (!user) {
      return { statusCode: 401, payload: { code: "AUTH_REQUIRED", message: "登录已失效，请重新登录" } };
    }

    const attemptKey = `${ip}:${user.username.toLocaleLowerCase()}`;
    const now = Date.now();
    const attempt = this.#loginAttempts.get(attemptKey);
    if (attempt && attempt.blockedUntil > now) {
      return { statusCode: 429, payload: { code: "TOO_MANY_ATTEMPTS", message: "密码验证尝试过多，请稍后再试" } };
    }

    try {
      await this.store.changePassword(
        user.id,
        parsed.data.currentPassword,
        parsed.data.newPassword,
      );
      this.#loginAttempts.delete(attemptKey);
      return { statusCode: 204, payload: undefined };
    } catch (error) {
      if (error instanceof InvalidCredentialsError) {
        this.#recordLoginFailure(attemptKey, now);
        return { statusCode: 401, payload: { code: "INVALID_CREDENTIALS", message: "当前密码错误" } };
      }
      throw error;
    }
  }

  authenticateSession(token: string): { id: string; username: string } | undefined {
    return this.store.authenticateSession(token);
  }

  #recordLoginFailure(key: string, now: number): void {
    const previous = this.#loginAttempts.get(key);
    const current = !previous || now - previous.windowStarted > 5 * 60_000
      ? { failures: 0, windowStarted: now, blockedUntil: 0 }
      : previous;
    current.failures += 1;
    if (current.failures >= 5) current.blockedUntil = now + 60_000;
    this.#loginAttempts.set(key, current);
  }
}
