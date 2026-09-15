import type { FastifyInstance } from "fastify";
import { DeviceSchema, type DeviceListResponse } from "@cliproam/protocol";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { requireSessionUser } from "./SessionUser.js";
import { parseOr400 } from "./parseRequest.js";

export type DeviceRouteDeps = {
  store: Pick<ClipRoamStore, "listDevices" | "upsertDevice">;
};

export function registerDeviceRoutes(app: FastifyInstance, deps: DeviceRouteDeps): void {
  const { store } = deps;
  app.get("/devices", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    return { devices: store.listDevices(user.id) } satisfies DeviceListResponse;
  });
  // 设备信息上报（登录时也会随 body 带一份）。以会话绑定的 deviceId 为准：
  // body.id 仅为形状校验，落库时覆盖，杜绝与 session 不一致的孤儿设备行。
  app.post("/devices/current", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    const device = parseOr400(reply, DeviceSchema, request.body, "设备信息格式不正确");
    if (!device) return reply;
    store.upsertDevice(user.id, { ...device, id: user.deviceId });
    return reply.code(204).send();
  });
}
