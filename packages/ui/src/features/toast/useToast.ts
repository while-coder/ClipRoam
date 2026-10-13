import { native } from "../../platform/native";
import { toast } from "@qingfeng346/ui-kit";
import type { ToastPayload, ToastTone } from "../../types";

export function displayToast(payload: ToastPayload): void {
  toast.show(payload.tone, payload.message);
}

export function showToast(message: string, tone: ToastTone = "info"): void {
  const normalized = message.trim();
  if (!normalized) return;
  const payload = {message:normalized,tone};
  void native.showToast(payload).catch(() => displayToast(payload));
}
