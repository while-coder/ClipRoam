import { computed, effectScope, ref, shallowRef, type Ref, type WritableComputedRef } from "vue";
import { AccountDatabase } from "../platform/database";
import { HistoryManager } from "../features/history/historyManager";
import { PendingUploadManager } from "../features/pending-upload/pendingUploadManager";
import { UploadManager } from "../features/uploads/uploadManager";
import { DownloadManager } from "../features/downloads/downloadManager";
import { CaptureService } from "../features/pending-upload/captureService";
import { AccountFiles } from "../platform/files";
import { clipboardPlatform } from "../platform/clipboard";
import { profileName } from "../utils";
import { showToast } from "../platform/runtime";
import type { ClientConnection } from "../features/sync/clientSessionManager";
import type { SyncRequester } from "../features/sync/httpClient";
import type { SyncConfig, AccountPreferences, Device, SettingsPage } from "../types";

function createSettingsState() {
  const settingsVisible = ref(false);
  const settingsPage = ref<SettingsPage>("general");
  /** 设备别名草稿；空串表示未设置，展示回退到系统机器名。 */
  const deviceAliasInput = ref("");
  const savedDeviceAlias = ref("");
  /** 系统机器名，作别名输入框的 placeholder。 */
  const systemDeviceName = ref("");
  const savingSettings = ref(false);
  const changingPassword = ref(false);
  const settingsError = ref("");
  const passwordChangeError = ref("");
  const currentPassword = ref("");
  const newPassword = ref("");
  const confirmNewPassword = ref("");
  return { settingsVisible, settingsPage, deviceAliasInput, savedDeviceAlias, systemDeviceName, savingSettings, changingPassword, settingsError, passwordChangeError, currentPassword, newPassword, confirmNewPassword };
}

function createAccountState() {
  return {
    ...createSettingsState(),
    settingsRequestController: undefined as AbortController | undefined,
    connected: ref(false),
    devicesById: ref<Record<string, Device>>({}),
    activeView: ref<"history" | "pending-upload" | "uploads" | "downloads" | "devices">("history"),
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

export const activeView = accountStateRef("activeView");

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
  readonly ready: Promise<void>;
  readonly database: AccountDatabase;
  readonly files: AccountFiles;
  readonly history: HistoryManager;
  readonly pendingUploads: PendingUploadManager;
  readonly uploads: UploadManager;
  readonly downloads: DownloadManager;
  readonly capture: CaptureService;
  client?: ClientConnection;
  #cleanups = new Set<() => void>();
  #timers = new Map<string, number>();

  constructor(readonly config: SyncConfig, public preferences: AccountPreferences) {
    this.database = new AccountDatabase(this.accountKey);
    this.files = new AccountFiles(this.accountKey);
    this.history = new HistoryManager(this);
    this.pendingUploads = new PendingUploadManager(this);
    this.uploads = new UploadManager(this);
    this.downloads = new DownloadManager(this);
    this.capture = new CaptureService(this, () => {
      if (this.signal.aborted) return;
      this.history.invalidate();
      void this.pendingUploads.refresh().catch((error) => {
        if (!this.signal.aborted) showToast(`待上传记录读取失败：${String(error)}`, "error");
      });
    }, (message) => { if (!this.signal.aborted) showToast(message, "error"); });
    this.ready = this.downloads.restore();
    this.own(() => { this.downloads.stop(); this.files.dispose(); });
    this.own(() => {
      void clipboardPlatform.watch(profileName(this.accountKey),false).catch(() => undefined);
    });
  }

  get http(): SyncRequester {
    this.signal.throwIfAborted();
    if (!this.client) throw new Error("同步账号未登录");
    return this.client.http;
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

  delay(milliseconds: number): Promise<void> {
    return this.start((signal) => new Promise<void>((resolve) => {
      const done = () => { signal.removeEventListener("abort", cancel); resolve(); };
      const timer = window.setTimeout(done, milliseconds);
      const cancel = () => { window.clearTimeout(timer); done(); };
      signal.addEventListener("abort", cancel, { once: true });
    }));
  }

  dispose(): void {
    if (this.signal.aborted) return;
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
