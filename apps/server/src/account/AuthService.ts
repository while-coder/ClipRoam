import { AuthCredentialsSchema, ChangePasswordSchema } from "@cliproam/protocol";
import { ClipRoamStore, InvalidCredentialsError, UsernameTakenError } from "./ClipRoamStore.js";
import type { AuthenticatedUser } from "./AccountStore.js";

export type HttpResult = { statusCode: number; payload: unknown };

export class AuthService {
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
      const deviceId = parsed.data.device.id;
      const session = await this.store.register(parsed.data.username, parsed.data.password, deviceId);
      this.store.upsertDevice(session.user.id, parsed.data.device);
      return { statusCode: 201, payload: session };
    } catch (error) {
      if (error instanceof UsernameTakenError) {
        return { statusCode: 409, payload: { code: "USERNAME_TAKEN", message: error.message } };
      }
      throw error;
    }
  }

  async login(body: unknown): Promise<HttpResult> {
    const parsed = AuthCredentialsSchema.safeParse(body);
    if (!parsed.success) {
      return { statusCode: 400, payload: { code: "INVALID_CREDENTIALS", message: "账号或密码格式不正确" } };
    }

    try {
      const deviceId = parsed.data.device.id;
      const session = await this.store.login(parsed.data.username, parsed.data.password, deviceId);
      this.store.upsertDevice(session.user.id, parsed.data.device);
      return { statusCode: 200, payload: session };
    } catch (error) {
      if (error instanceof InvalidCredentialsError) {
        return { statusCode: 401, payload: { code: "INVALID_CREDENTIALS", message: error.message } };
      }
      throw error;
    }
  }

  async changePassword(user: AuthenticatedUser | undefined, body: unknown): Promise<HttpResult> {
    const parsed = ChangePasswordSchema.safeParse(body);
    if (!parsed.success) {
      return { statusCode: 400, payload: { code: "INVALID_PASSWORD", message: "新密码至少 6 位，且不能与当前密码相同" } };
    }
    if (!user) {
      return { statusCode: 401, payload: { code: "AUTH_REQUIRED", message: "登录已失效，请重新登录" } };
    }

    try {
      await this.store.changePassword(
        user.id,
        parsed.data.currentPassword,
        parsed.data.newPassword,
      );
      return { statusCode: 204, payload: undefined };
    } catch (error) {
      if (error instanceof InvalidCredentialsError) {
        return { statusCode: 401, payload: { code: "INVALID_CREDENTIALS", message: "当前密码错误" } };
      }
      throw error;
    }
  }

  authenticateSession(token: string): AuthenticatedUser | undefined {
    return this.store.authenticateSession(token);
  }
}
