import { ref } from "vue";
import { fetchStatus } from "./api.js";

export const authenticated = ref(false);

// Recheck on protected navigation: the cookie can expire or the server restart.
export async function ensureAuthenticated(): Promise<boolean> {
  try {
    await fetchStatus();
    authenticated.value = true;
  } catch {
    authenticated.value = false;
  }
  return authenticated.value;
}

export function markAuthenticated(): void {
  authenticated.value = true;
}

export function markSignedOut(): void {
  authenticated.value = false;
}
