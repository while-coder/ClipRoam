import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Device } from "@cliproam/protocol";

/**
 * 跨窗口设备名广播：主窗口是唯一 sync 持有者，设备表由 sync 回调维护；
 * paste 快捷粘贴窗口不持有 sync 客户端，设备名靠主窗口广播补充。
 * （文件下载不走桥：paste 窗口凭配置里的凭据直接走公共 HTTP 下载管线。）
 */

/** paste → main 的设备列表请求（fire-and-forget，主窗口经广播回）。 */
export const SYNC_BRIDGE_DEVICES_REQUEST_EVENT = "cliproam://sync-bridge-devices-request";
/** main → paste 的设备表广播（主动推送兼请求回执）。 */
export const SYNC_BRIDGE_DEVICES_EVENT = "cliproam://sync-bridge-devices";

type DevicesRequest = { kind: "devices" };

/** paste 窗口：向主窗口要一次设备名（fire-and-forget，失败静默）。 */
export function requestProxyDevices(): void {
  void emitTo("main", SYNC_BRIDGE_DEVICES_REQUEST_EVENT, {
    kind: "devices",
  } satisfies DevicesRequest).catch(() => undefined);
}

/** paste 窗口：监听设备广播；返回 unlisten 交给 unlisteners。 */
export async function startPasteBridge(handlers: {
  onDevices: (devices: Device[]) => void;
}): Promise<UnlistenFn> {
  return listen<{ devices: Device[] }>(SYNC_BRIDGE_DEVICES_EVENT, ({ payload }) => {
    handlers.onDevices(payload.devices);
  });
}

/** 主窗口：应答 paste 的设备请求，并借机把当前设备表推过去。 */
export async function startSyncBridgeService(deps: {
  getDevices: () => Device[];
}): Promise<UnlistenFn> {
  return listen<DevicesRequest>(SYNC_BRIDGE_DEVICES_REQUEST_EVENT, () => {
    void emitTo("paste", SYNC_BRIDGE_DEVICES_EVENT, {
      devices: deps.getDevices(),
    }).catch(() => undefined);
  });
}
