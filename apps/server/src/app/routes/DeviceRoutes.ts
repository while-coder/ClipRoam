import type { FastifyInstance } from "fastify";
import { type DeviceListResponse } from "@cliproam/protocol";
import type { ClipRoamStore } from "../../account/ClipRoamStore.js";
import { requireSessionUser } from "./SessionUser.js";

export type DeviceRouteDeps = {
  store: Pick<ClipRoamStore, "listDevices">;
};

export function registerDeviceRoutes(app: FastifyInstance, deps: DeviceRouteDeps): void {
  const { store } = deps;
  app.get("/devices", async (request, reply) => {
    const user = requireSessionUser(request, reply);
    if (!user) return reply;
    return { devices: store.listDevices(user.id) } satisfies DeviceListResponse;
  });
}
