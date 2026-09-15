import { AuthCredentialsSchema, ChangePasswordSchema, type Device } from "@cliproam/protocol";
import { LOGIN_ATTEMPT_WINDOW_MS, LOGIN_BLOCKED_FOR_MS, LOGIN_MAX_ATTEMPTS } from "../app/ServerConfig.js";
import { getLogger } from "../app/Logger.js";
import { AttemptThrottle } from "../common/AttemptThrottle.js";
import { ClipRoamStore, InvalidCredentialsError, UsernameTakenError } from "./ClipRoamStore.js";
import type { AuthenticatedUser } from "./AccountStore.js";

export type HttpResult = { statusCode: number; payload: unknown };

const logger = getLogger("AuthService");

export class AuthService {
  // WINDOW_MS 内失败 MAX_ATTEMPTS 次即封锁 BLOCKED_FOR_MS（键为 ip:username）。
  #throttle = new AttemptThrottle({ maxAttempts: LOGIN_MAX_ATTEMPTS, windowMs: LOGIN_ATTEMPT_WINDOW_MS, blockedForMs: LOGIN_BLOCKED_FOR_MS });

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
      const deviceId = parsed.data.device?.id ?? parsed.data.deviceId;
      const session = await this.store.register(parsed.data.username, parsed.data.password, deviceId);
      this.#syncDevice(session.user.id, parsed.data.device);
      return { statusCode: 201, payload: session };
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
    if (this.#throttle.isBlocked(attemptKey, now)) {
      return { statusCode: 429, payload: { code: "TOO_MANY_ATTEMPTS", message: "登录尝试过多，请稍后再试" } };
    }

    try {
      const deviceId = parsed.data.device?.id ?? parsed.data.deviceId;
      const session = await this.store.login(parsed.data.username, parsed.data.password, deviceId);
      this.#syncDevice(session.user.id, parsed.data.device);
      this.#throttle.reset(attemptKey);
      return { statusCode: 200, payload: session };
    } catch (error) {
      if (error instanceof InvalidCredentialsError) {
        this.#throttle.recordFailure(attemptKey, now);
        return { statusCode: 401, payload: { code: "INVALID_CREDENTIALS", message: error.message } };
      }
      throw error;
    }
  }

  // 设备信息随登录上报（AuthCredentialsSchema.device）：注册失败不拖累登录
  // ——session 已落库，WS auth 携带 device 时仍会补齐。
  #syncDevice(userId: string, device: Device | undefined): void {
    if (!device) return;
    try {
      this.store.upsertDevice(userId, device);
    } catch (error) {
      logger.error(`Failed to register device ${device.id} for user ${userId}:`, error);
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
    if (this.#throttle.isBlocked(attemptKey, now)) {
      return { statusCode: 429, payload: { code: "TOO_MANY_ATTEMPTS", message: "密码验证尝试过多，请稍后再试" } };
    }

    try {
      await this.store.changePassword(
        user.id,
        parsed.data.currentPassword,
        parsed.data.newPassword,
      );
      this.#throttle.reset(attemptKey);
      return { statusCode: 204, payload: undefined };
    } catch (error) {
      if (error instanceof InvalidCredentialsError) {
        this.#throttle.recordFailure(attemptKey, now);
        return { statusCode: 401, payload: { code: "INVALID_CREDENTIALS", message: "当前密码错误" } };
      }
      throw error;
    }
  }

  authenticateSession(token: string): AuthenticatedUser | undefined {
    return this.store.authenticateSession(token);
  }
}
