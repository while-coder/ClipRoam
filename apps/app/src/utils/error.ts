/** The message of an unknown thrown value, for toasts and error strings. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
