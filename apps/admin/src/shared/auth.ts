import { ref } from "vue";
import { fetchStatus } from "./api.js";

export const authenticated = ref(false);

// The session cookie is the source of truth, so the first navigation (and any
// navigation after a sign-out) confirms it against the server once; later
// navigations trust the cached result until something changes it.
let checked = false;

export async function ensureAuthenticated(): Promise<boolean> {
  if (checked && authenticated.value) return true;
  try {
    await fetchStatus();
    authenticated.value = true;
  } catch {
    authenticated.value = false;
  } finally {
    checked = true;
  }
  return authenticated.value;
}

export function markAuthenticated(): void {
  authenticated.value = true;
  checked = true;
}

export function markSignedOut(): void {
  authenticated.value = false;
  checked = false;
}
