/** Combine cancellation signals on WebViews that lack AbortSignal.any. */
export function anyAbortSignal(signals: AbortSignal[]): AbortSignal {
  if (typeof AbortSignal.any === "function") return AbortSignal.any(signals);

  const controller = new AbortController();
  const alreadyAborted = signals.find((signal) => signal.aborted);
  if (alreadyAborted) {
    controller.abort(alreadyAborted.reason);
    return controller.signal;
  }

  const listeners = new Map<AbortSignal, () => void>();
  for (const signal of new Set(signals)) {
    const onAbort = () => {
      controller.abort(signal.reason);
      for (const [source, listener] of listeners) {
        source.removeEventListener("abort", listener);
      }
      listeners.clear();
    };
    listeners.set(signal, onAbort);
    signal.addEventListener("abort", onAbort, { once: true });
  }
  return controller.signal;
}
