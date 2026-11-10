import { watch } from "vue";
import { appName, isChinese } from "@cliproam/ui/branding";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { availableMonitors, PhysicalPosition } from "@tauri-apps/api/window";
import { activeAccountSession, type AccountSession } from "@cliproam/ui/account-session";
import { stickyDesktop, showStickyNotes } from "@cliproam/ui/sticky-desktop";
import { copyEntry, saveEntry } from "@cliproam/ui/entry-actions";
import { native } from "@cliproam/ui/native";
import { showNotification } from "@cliproam/ui/platform";
import { errorMessage } from "@cliproam/ui/errors";
import type { ClipboardEntry } from "@cliproam/protocol";
import { STICKY_ACTION, STICKY_CONTENT, STICKY_RESULT, STICKY_EDIT, type StickyAction, type StickyLayout, type StickyPayload } from "./messages";

type Note = { window: WebviewWindow; payload: StickyPayload; ready: boolean; positioned: boolean; loadingImage?: Promise<void> };

/** Only the main window owns sessions, downloads and clipboard writes. */
export async function initializeStickyDesktop(): Promise<() => void> {
  // A main-webview reload can leave native note windows whose old JS owner no
  // longer exists. Rebuild them from the account's full server list.
  for (const window of await WebviewWindow.getAll()) {
    if (window.label.startsWith("sticky-")) await window.destroy();
  }
  const notes = new Map<string, Note>();
  let session: AccountSession | undefined;
  let refreshing: Promise<void> | undefined;
  let refreshAgain = false;
  let disposed = false;
  const report = (message: string, error: unknown) => showNotification({ message: `${message}：${errorMessage(error)}`, tone: "error" });

  async function send(note: Note): Promise<void> {
    if (note.ready && note.positioned) await emitTo(note.window.label, STICKY_CONTENT, note.payload);
  }

  async function loadImage(note: Note, owner: AccountSession): Promise<void> {
    const image = note.payload.entry.imageInfo;
    if (!image || note.loadingImage || note.payload.imageSource) return note.loadingImage;
    note.loadingImage = (async () => {
      try {
        await owner.downloads.ensureLocalFile(note.payload.entry.id, image.fileId);
        const path = await owner.files.resolveCached(image.fileId);
        owner.signal.throwIfAborted();
        if (!path) throw new Error("图片缓存不可用");
        note.payload.imageSource = await native.previewSource(path);
        note.payload.imageError = undefined;
      } catch (error) {
        if (owner.signal.aborted) return;
        note.payload.imageError = `原图加载失败：${errorMessage(error)}`;
      }
      if (!owner.signal.aborted && session === owner) await send(note);
    })().finally(() => { note.loadingImage = undefined; });
    return note.loadingImage;
  }

  async function createNote(entry: ClipboardEntry, owner: AccountSession, index: number): Promise<void> {
    const label = `sticky-${owner.id}-${entry.id}`;
    const layout = await owner.database.get<StickyLayout>("settings", `sticky-layout:${entry.id}`);
    const monitors = await availableMonitors();
    if (owner.signal.aborted || session !== owner) return;
    // Positions are physical pixels; content determines the window dimensions.
    // Recenter if a saved monitor disappeared so the title bar remains reachable.
    const visible = layout && monitors.some((monitor) => {
      const { position, size } = monitor.workArea;
      return layout.x >= position.x && layout.y >= position.y
        && layout.x + 100 * monitor.scaleFactor <= position.x + size.width
        && layout.y + 40 * monitor.scaleFactor <= position.y + size.height;
    });
    const noteWindow = new WebviewWindow(label, {
      url: "index.html", title: `${appName.value} ${isChinese.value ? "便签" : "Sticky Note"}`, width: 320, height: 160,
      minWidth: 200, minHeight: 64, decorations: false, resizable: false,
      maximizable: false, minimizable: false,
      skipTaskbar: true, alwaysOnTop: layout?.alwaysOnTop ?? false,
      alwaysOnBottom: !(layout?.alwaysOnTop ?? false), focusable: false,
      visible: false, focus: false, center: true, acceptFirstMouse: true,
      transparent: true, backgroundColor: "#00000000", shadow: false,
    });
    const note: Note = { window: noteWindow, payload: { entry }, ready: false, positioned: false };
    notes.set(label, note);
    try {
      await new Promise<void>((resolve, reject) => {
        const timeout = window.setTimeout(() => reject(new Error("便签窗口创建超时")), 20_000);
        void noteWindow.once("tauri://created", () => { window.clearTimeout(timeout); resolve(); });
        void noteWindow.once("tauri://error", (event) => { window.clearTimeout(timeout); reject(new Error(String(event.payload))); });
      });
      if (owner.signal.aborted || session !== owner || disposed) { await noteWindow.destroy(); return; }
      if (visible) await noteWindow.setPosition(new PhysicalPosition(layout.x, layout.y));
      else if (monitors[0]) {
        const monitor = monitors[0];
        const offset = (index % 8) * 28 * monitor.scaleFactor;
        await noteWindow.setPosition(new PhysicalPosition(monitor.workArea.position.x + 32 + offset, monitor.workArea.position.y + 48 + offset));
      }
      note.positioned = true;
      await send(note);
    } catch (error) {
      notes.delete(label);
      await noteWindow.destroy().catch(() => {});
      if (!owner.signal.aborted) throw error;
    }
  }

  async function reconcile(owner: AccountSession): Promise<void> {
    const entries = await owner.history.listSticky();
    if (owner.signal.aborted || session !== owner || disposed) return;
    const ids = new Set(entries.map((entry) => entry.id));
    for (const [label, note] of notes) if (!ids.has(note.payload.entry.id)) {
      notes.delete(label);
      await note.window.destroy();
    }
    for (const [index, entry] of entries.entries()) {
      if (owner.signal.aborted || session !== owner || disposed) return;
      const note = notes.get(`sticky-${owner.id}-${entry.id}`);
      if (note) { note.payload.entry = entry; await send(note); }
      else await createNote(entry, owner, index);
    }
  }

  function refresh(): Promise<void> {
    if (refreshing) { refreshAgain = true; return refreshing; }
    refreshing = (async () => {
      do {
        refreshAgain = false;
        const owner = session;
        if (owner?.client && !owner.signal.aborted) {
          try { await reconcile(owner); }
          catch (error) { if (!owner.signal.aborted) throw error; }
        }
      } while (refreshAgain && !disposed);
    })().finally(() => { refreshing = undefined; });
    return refreshing;
  }

  const unlisten = await listen<StickyAction>(STICKY_ACTION, ({ payload }) => {
    const owner = session;
    const note = notes.get(payload.label);
    if (!owner || owner.signal.aborted || !note) return;
    void (async () => {
      if (payload.action === "layout") {
        const layout = payload.layout;
        if (layout && Object.values(layout).every((value) => typeof value === "boolean" || (typeof value === "number" && Number.isFinite(value)))) {
          await owner.database.put("settings", `sticky-layout:${note.payload.entry.id}`, layout);
        }
        return;
      }
      if (payload.action === "ready") {
        note.ready = true;
        await send(note);
        if (note.payload.entry.kind === "image") await loadImage(note, owner);
        return;
      }
      if (payload.action === "image") {
        note.payload.imageSource = undefined;
        note.payload.imageError = undefined;
        await send(note);
        await loadImage(note, owner);
      }
      else if (payload.action === "unpin") {
        await owner.history.setSticky([note.payload.entry.id], false);
        if (!owner.signal.aborted) showNotification({ message: "已取消便签", tone: "success" });
        await refresh();
      } else if (payload.action === "copy" || payload.action === "save") {
        const entry = await owner.history.getEntry(note.payload.entry.id);
        owner.signal.throwIfAborted();
        if (payload.action === "copy") await copyEntry(entry);
        else await saveEntry(entry);
      }
      if (!owner.signal.aborted) await emitTo(payload.label, STICKY_RESULT, "");
    })().catch((error) => {
      if (!owner.signal.aborted) {
        report("便签操作失败", error);
        void emitTo(payload.label, STICKY_RESULT, errorMessage(error)).catch(() => {});
      }
    });
  }, { target: "main" });

  const stop = watch(activeAccountSession, (owner, _previous, onCleanup) => {
    session = owner;
    for (const note of notes.values()) void note.window.destroy().catch(() => {});
    notes.clear();
    if (!owner) return;
    const changed = () => owner.schedule("sticky-refresh", () => {
      void refresh().catch((error) => { if (!owner.signal.aborted) report("便签同步失败", error); });
    }, 120);
    owner.history.addEvent(changed);
    const stopEntries = watch(owner.history.stickyEntries, changed);
    onCleanup(() => { owner.history.delEvent(changed); stopEntries(); });
    changed();
  }, { immediate: true, flush: "sync" });

  stickyDesktop.value = async (entry) => {
    const owner = session;
    if (!owner?.client) return;
    try {
      await owner.history.setSticky([entry.id], !entry.stickyId);
      if (!owner.signal.aborted) showNotification({ message: entry.stickyId ? "已取消便签" : "已添加便签", tone: "success" });
      await refresh();
    } catch (error) { if (!owner.signal.aborted) report("修改便签失败", error); }
  };
  async function editExistingNotes(): Promise<void> {
    await invoke("native_edit_sticky_notes");
  }
  showStickyNotes.value = async () => {
    // Unlock existing windows before any network request, including offline.
    try { await editExistingNotes(); }
    catch (error) { report("恢复便签操作失败", error); }
    let refreshed = true;
    try {
      await session?.history.listSticky(true);
      await refresh();
    }
    catch (error) { refreshed = false; report("便签同步失败", error); }
    try {
      const owner = session;
      if (!owner) return;
      await editExistingNotes();
      if (!notes.size && refreshed) showNotification({ message: "还没有便签，在剪贴板历史上右键选择“添加便签”", tone: "info" });
    } catch (error) { report("显示便签失败", error); }
  };
  const unlistenEdit = await listen(STICKY_EDIT, () => { void showStickyNotes.value?.(); }, { target: "main" });

  return () => {
    disposed = true;
    session = undefined;
    stop(); unlisten(); unlistenEdit();
    stickyDesktop.value = undefined;
    showStickyNotes.value = undefined;
    for (const note of notes.values()) void note.window.destroy().catch(() => {});
    notes.clear();
  };
}
