import { computed, effectScope, ref, shallowRef, type Ref, type WritableComputedRef } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { createSettingsState } from "../settings/settingsState";
import type { SyncClient } from "./syncClient";
import type { SyncConfig, AccountPreferences, Device, LocalClipboardEntry, UploadProgress } from "../../types";
import type { ServeTaskSnapshot } from "../uploads/relayUploader";
import type { DownloadTaskSnapshot } from "../downloads/useDownloads";

function createAccountState() {
  return {
    ...createSettingsState(),
    settingsRequestController: undefined as AbortController | undefined,
    connected: ref(false),
    devicesById: ref<Record<string, Device>>({}),
    activeView: ref<"history" | "pending-upload" | "uploads" | "downloads">("history"),
    historyRevision: ref(0),
    pendingEntries: ref<LocalClipboardEntry[]>([]),
    pendingCount: ref(0),
    uploadTasks: ref<ServeTaskSnapshot[]>([]),
    downloadTasks: ref<DownloadTaskSnapshot[]>([]),
    uploadProgress: ref<Record<string, UploadProgress>>({}),
    pendingUploadProgress: new Map<string, UploadProgress>(),
    activatingEntryIds: ref(new Set<string>()),
    savingEntryId: ref(""),
    clipboardWriteChain: Promise.resolve() as Promise<unknown>,
    localClipboardRevision: 0,
    remoteActivationRevision: 0,
  };
}
let sessionSequence = 0;
export const activeAccountSession = shallowRef<AccountSession>();
const emptyState = createAccountState();
type AccountState = ReturnType<typeof createAccountState>;

export function getAccountSession(): AccountSession | undefined { return activeAccountSession.value; }

export function replaceAccountSession(session?: AccountSession): void {
  activeAccountSession.value?.dispose();
  activeAccountSession.value = session;
}

/** Compatibility bindings expose the active instance without keeping account state globally. */
export function accountStateRef<K extends keyof AccountState>(key: K): WritableComputedRef<AccountState[K] extends Ref<infer T> ? T : never> {
  type Value = AccountState[K] extends Ref<infer T> ? T : never;
  return computed({
    get: () => ((activeAccountSession.value?.state ?? emptyState)[key] as Ref<Value>).value,
    set: (value: Value) => {
      const session = activeAccountSession.value;
      if (session) (session.state[key] as Ref<Value>).value = value;
    },
  });
}

export function archiveKeyFor(config: SyncConfig | undefined): string {
  const serverAddress = config?.serverAddress.trim().toLowerCase() ?? "";
  const username = config?.username.trim().toLowerCase() ?? "";
  return username ? `account:${serverAddress}:${username}` : "";
}

/** Account-owned state, resources and cooperative task lifetime. */
export class AccountSession {
  #controller = new AbortController();
  readonly signal = this.#controller.signal;

  readonly state = createAccountState();
  readonly scope = effectScope(true);
  readonly id = ++sessionSequence;
  readonly sessionId = crypto.randomUUID();
  readonly ready: Promise<void>;
  #stopNativeTasks = true;
  client?: SyncClient;
  #cleanups = new Set<() => void>();
  #timers = new Map<string, number>();

  constructor(readonly config: SyncConfig, public preferences: AccountPreferences) {
    this.ready = invoke<void>("open_account_session", { sessionId: this.sessionId, config }).then(() => {
      this.own(() => {
        void invoke("close_account_session", { sessionId: this.sessionId, stopDownloads: this.#stopNativeTasks }).catch(() => undefined);
      });
    });
  }

  get accountKey(): string { return archiveKeyFor(this.config); }

  own(cleanup: () => void): void {
    if (this.signal.aborted) cleanup();
    else this.#cleanups.add(cleanup);
  }

  async ownAsync(resource: Promise<() => void>): Promise<void> {
    this.own(await resource);
  }

  acquire<T>(resource: Promise<T>, release: (value: T) => void): Promise<T> {
    return this.start(() => resource.then((value) => {
      this.own(() => release(value));
      return value;
    }));
  }

  guard<Args extends unknown[]>(callback: (...args: Args) => void): (...args: Args) => void {
    return (...args) => { if (!this.signal.aborted) callback(...args); };
  }

  schedule(key: string, callback: () => void, milliseconds: number): void {
    if (this.signal.aborted || this.#timers.has(key)) return;
    this.#timers.set(key, window.setTimeout(() => {
      this.#timers.delete(key);
      if (!this.signal.aborted) callback();
    }, milliseconds));
  }

  cancelScheduled(key: string): void {
    const timer = this.#timers.get(key);
    if (timer !== undefined) window.clearTimeout(timer);
    this.#timers.delete(key);
  }

  interval(callback: () => void, milliseconds: number): void {
    if (this.signal.aborted) return;
    const timer = window.setInterval(this.guard(callback), milliseconds);
    this.own(() => window.clearInterval(timer));
  }

  /** Disposal rejects waiting callers; tasks must use the signal for cancellable I/O. */
  async start<T>(task: (signal: AbortSignal) => Promise<T>): Promise<T> {
    this.signal.throwIfAborted();
    let cancel!: () => void;
    const cancelled = new Promise<never>((_, reject) => {
      cancel = () => reject(this.signal.reason);
      this.signal.addEventListener("abort", cancel, { once: true });
    });
    try {
      const result = await Promise.race([task(this.signal), cancelled]);
      this.signal.throwIfAborted();
      return result;
    } finally {
      this.signal.removeEventListener("abort", cancel);
    }
  }

  /** IPC cannot be revoked, so it always targets this session's archive. */
  invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.start(async () => {
      await this.ready;
      this.signal.throwIfAborted();
      return invoke<T>(command, { ...args, sessionId: this.sessionId });
    });
  }

  delay(milliseconds: number): Promise<void> {
    return this.start((signal) => new Promise<void>((resolve) => {
      const done = () => { signal.removeEventListener("abort", cancel); resolve(); };
      const timer = window.setTimeout(done, milliseconds);
      const cancel = () => { window.clearTimeout(timer); done(); };
      signal.addEventListener("abort", cancel, { once: true });
    }));
  }

  dispose(options: { stopNativeTasks?: boolean } = {}): void {
    if (this.signal.aborted) return;
    this.#stopNativeTasks = options.stopNativeTasks !== false;
    this.#controller.abort();
    for (const timer of this.#timers.values()) window.clearTimeout(timer);
    this.#timers.clear();
    this.scope.stop();
    for (const cleanup of [...this.#cleanups].reverse()) {
      try { cleanup(); } catch { /* Finish releasing the remaining resources. */ }
    }
    this.#cleanups.clear();
    this.client = undefined;
    if (activeAccountSession.value === this) activeAccountSession.value = undefined;
  }
}
