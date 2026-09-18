import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "../toast/useToast";
import { errorMessage } from "../../utils/error";
import type { LocalClipboardEntry } from "../../types";

/**
 * The durable upload queue's rows: captured offline or waiting to publish.
 * Served by Rust's `list_pending_entries` (every queue row as an entry-shaped
 * view with a temporary `p{seq}` id) — never derived from a whole-history read.
 * Details load only while the pending-sync view is open; refresh bursts
 * elsewhere carry the O(1) `count_pending_entries` instead.
 */
export const pendingEntries = ref<LocalClipboardEntry[]>([]);
export const pendingCount = ref(0);

/** The pending-sync list re-queries Rust on every refresh. */
export async function refreshPendingEntries(): Promise<void> {
  try {
    pendingEntries.value = await invoke<LocalClipboardEntry[]>("list_pending_entries");
    pendingCount.value = pendingEntries.value.length;
  } catch (error) {
    showToast(`待同步记录读取失败：${errorMessage(error)}`, "error");
  }
}

/**
 * Sidebar badge only: an O(1) count, so refresh bursts outside the pending
 * view never haul the queue rows across the IPC boundary.
 */
export async function refreshPendingCount(): Promise<void> {
  try {
    pendingCount.value = await invoke<number>("count_pending_entries");
  } catch {
    // The badge is auxiliary; a failed refresh keeps the previous value.
  }
}
