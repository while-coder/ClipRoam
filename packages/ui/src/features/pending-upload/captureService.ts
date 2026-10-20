import { base64ToBytes, nameMatches, safeName, profileName, Sha256 } from "../../utils";
import { isFileNode, type FileInfo } from "@cliproam/protocol";
import type { AccountSession } from "../../accountSession";
import { clipboardPlatform, type ClipboardData, type ClipboardWrite } from "../../platform/clipboard";
import type { LocalSources, StoredEntry } from "../../types";
import type { FileStat } from "../../platform/files";
import { applicationDatabase, type FileHashRecord } from "../../platform/database";
import { isNative } from "../../platform/runtime";
import type { FilePickerMode } from "../../platform/native";

/** All filtering, limits, hashing, queue persistence and retries are portable. */
export class CaptureService {
  #work: Promise<void> = Promise.resolve();
  constructor(private readonly session: AccountSession, private readonly changed: () => void, private readonly error: (message: string) => void) {}

  async start(): Promise<void> {
    if (clipboardPlatform.monitoringAvailable) {
      await this.session.ownAsync(clipboardPlatform.subscribe(({scope, capture}) => {
        if (scope !== profileName(this.session.accountKey) || this.session.signal.aborted) return;
        const preferences = this.session.preferences;
        const enabled = capture.data.kind === "text" ? preferences.watchClipboardText
          : capture.data.kind === "image" ? preferences.watchClipboardImage : preferences.watchClipboardFiles;
        if (!enabled) return;
        void this.capture(capture.data, capture.id).catch((error) => {
          if (!this.session.signal.aborted) this.error(`剪贴板捕获失败：${String(error)}`);
        });
      }));
      await this.updateWatch();
    }
    void this.resume().catch((error) => { if (!this.session.signal.aborted) this.error(`文件处理失败：${String(error)}`); });
  }

  async updateWatch(): Promise<void> {
    const preferences = this.session.preferences;
    const kinds = { text: preferences.watchClipboardText, image: preferences.watchClipboardImage, files: preferences.watchClipboardFiles };
    await clipboardPlatform.watch(profileName(this.session.accountKey), kinds.text || kinds.image || kinds.files, kinds);
  }

  capture(data: ClipboardData, captureId?: string): Promise<void> {
    const work = this.#work.then(() => this.#capture(data, captureId));
    this.#work = work.catch(() => undefined);
    return work;
  }

  async #capture(data: ClipboardData, captureId?: string): Promise<void> {
    this.session.signal.throwIfAborted();
    if (captureId && await this.session.database.get("captures", captureId)) return;
    this.session.state.localClipboardRevision++;
    const entry: StoredEntry = { id: "", version: 0, kind: data.kind, content: "",
      sourceDeviceId: "", createdAt: new Date().toISOString() };
    if (data.kind === "text") {
      if (!data.text.trim()) return;
      Object.assign(entry, { content: data.text, html: data.html, rtf: data.rtf });
    } else if (data.kind === "image") {
      const bytes = base64ToBytes(data.data);
      const fileId = await this.session.files.putBlob(bytes);
      entry.content = `截图（${data.width} × ${data.height}）`;
      entry.imageInfo = { fileId, size: bytes.length, thumbnail: data.thumbnail };
    } else {
      const collected = await this.#collect(data.paths);
      entry.fileInfo = collected.info; entry.sources = collected.sources;
      entry.content = Object.keys(collected.info).join("、");
    }
    const id = await this.session.pendingUploads.enqueue(entry, captureId, data.kind === "files");
    this.changed();
    if (data.kind === "files") await this.#resolve(id);
  }

  async #collect(paths: string[]): Promise<{ info: FileInfo; sources: LocalSources }> {
    const info: FileInfo = Object.create(null);
    const sources: LocalSources = { roots: [], files: [] };
    const preferences = this.session.preferences;
    const collect = async (stat: FileStat, parent: FileInfo, name: string, relative: string): Promise<void> => {
      this.session.signal.throwIfAborted();
      if (stat.symlink || nameMatches(stat.name, preferences.excludePatterns)) return;
      if (stat.directory) {
        const children: FileInfo = Object.create(null); parent[name] = children;
        for (const child of (await this.session.files.list(stat.path)).sort((a,b) => a.name.localeCompare(b.name))) {
          const childName = safeName(child.name);
          if (Object.prototype.hasOwnProperty.call(children, childName)) throw new Error("过滤后的文件名称重复");
          await collect(child, children, childName, `${relative}/${childName}`);
        }
      } else {
        if (sources.files.length >= preferences.maxCaptureFileCount) throw new Error(`单次复制文件数量超过上限 ${preferences.maxCaptureFileCount}`);
        parent[name] = { f: "", s: stat.size };
        sources.files.push({ path: relative, source: stat.path, size: stat.size, modifiedAt: stat.modifiedAt });
      }
    };
    const stats = await this.session.files.stats(paths);
    for (const stat of stats) {
      if (!stat || stat.symlink || nameMatches(stat.name, preferences.excludePatterns)) continue;
      const base = safeName(stat.name); let name = base, n = 2;
      while (Object.prototype.hasOwnProperty.call(info, name)) { const dot = base.lastIndexOf("."); name = dot > 0 ? `${base.slice(0,dot)} (${n++})${base.slice(dot)}` : `${base} (${n++})`; }
      sources.roots.push(stat.path); await collect(stat, info, name, name);
    }
    if (!Object.keys(info).length) throw new Error("没有可用的文件");
    return { info, sources };
  }

  async #resolve(id: number): Promise<void> {
    const row = await this.session.pendingUploads.get(id);
    if (!row?.resolving) return;
    try {
      for (const source of row.entry.sources?.files ?? []) {
        this.session.signal.throwIfAborted();
        const [before] = await this.session.files.stats([source.source]);
        if (!before || before.directory || before.symlink || before.size !== source.size || (source.modifiedAt != null && before.modifiedAt !== source.modifiedAt)) throw new Error(`源文件已修改或不可用：${source.path}`);
        const cached = await applicationDatabase.get<FileHashRecord>("file_hash", source.source);
        let fileId = cached && source.modifiedAt != null && cached.size === source.size && cached.modifiedAt === source.modifiedAt ? cached.hash : undefined;
        if (!fileId) {
          const hash = new Sha256();
          for (let offset = 0; offset < source.size;) {
            this.session.signal.throwIfAborted();
            const bytes = await this.session.files.readPath(source.source, offset, Math.min(128 * 1024, source.size - offset));
            if (!bytes.length) throw new Error("文件读取不完整");
            hash.update(bytes); offset += bytes.length;
          }
          fileId = hash.digest();
          const [after] = await this.session.files.stats([source.source]);
          if (!after || after.size !== before.size || after.modifiedAt !== before.modifiedAt) throw new Error(`读取期间源文件发生变化：${source.path}`);
          await applicationDatabase.put("file_hash", source.source, { hash: fileId, size: source.size, modifiedAt: source.modifiedAt } satisfies FileHashRecord);
        }
        source.fileId = fileId; await this.session.files.rememberSource(source);
        const segments = source.path.split("/"); let parent = row.entry.fileInfo!;
        for (const part of segments.slice(0,-1)) { const node = parent[part]!; if (isFileNode(node)) throw new Error("文件树结构不合法"); parent = node; }
        const node = parent[segments[segments.length - 1]!]!;
        if (!isFileNode(node)) throw new Error("文件树结构不合法");
        node.f = fileId;
      }
      row.resolving = false;
      await this.session.pendingUploads.updateResolved(row);
    } catch (error) {
      if (!this.session.signal.aborted) await this.session.pendingUploads.updateResolved(row, true);
      throw error;
    } finally { this.changed(); }
  }

  async resume(): Promise<void> {
    for (const row of await this.session.pendingUploads.rows()) if (row.resolving && row.status === "wait") {
      const work = this.#work.then(() => this.#resolve(row.id));
      this.#work = work.catch(() => undefined);
      try { await work; } catch (error) { if (!this.session.signal.aborted) this.error(`文件处理失败：${String(error)}`); }
    }
  }

  async readCurrentText(): Promise<boolean> {
    const data = await clipboardPlatform.readText();
    if (!data) return false; await this.capture(data); return true;
  }

  async pickFiles(mode: FilePickerMode): Promise<boolean> {
    const picked = await clipboardPlatform.pickFiles(mode,profileName(this.session.accountKey));
    if (picked.files?.length) { await this.captureFiles(picked.files); return true; }
    if (picked.paths?.length) { await this.capture({kind:"files",paths:picked.paths}); return true; }
    return false;
  }

  /** WebKit's native file input and browsers both return File objects. */
  async captureFiles(files: File[], captureId?: string): Promise<void> {
    if (captureId && await this.session.database.get("captures", captureId)) return;
    const info: FileInfo = Object.create(null);
    let count = 0;
    for (const file of files) {
      this.session.signal.throwIfAborted();
      const names = (file.webkitRelativePath || file.name).split("/").map(safeName);
      if (names.some((name) => nameMatches(name, this.session.preferences.excludePatterns))) continue;
      if (++count > this.session.preferences.maxCaptureFileCount) throw new Error("单次复制文件数量超过上限");
      let parent = info;
      for (const name of names.slice(0,-1)) {
        let node = parent[name];
        if (!node) { node = Object.create(null) as FileInfo; parent[name] = node; }
        if (isFileNode(node)) throw new Error("文件目录名称冲突"); parent = node;
      }
      let name = names[names.length - 1]!;
      const base = name; let n = 2;
      while (Object.prototype.hasOwnProperty.call(parent,name)) name = `${base} (${n++})`;
      const hash = new Sha256();
      for (let offset = 0; offset < file.size; offset += 128 * 1024) {
        this.session.signal.throwIfAborted(); hash.update(new Uint8Array(await file.slice(offset,offset + 128 * 1024).arrayBuffer()));
      }
      const fileId = hash.digest();
      if (isNative) {
        const writer = await this.session.files.writer(fileId, file.size, true);
        try {
          for (let offset = 0; offset < file.size; offset += 128 * 1024) {
            this.session.signal.throwIfAborted();
            await writer.append(new Uint8Array(await file.slice(offset,offset + 128 * 1024).arrayBuffer()));
          }
          this.session.signal.throwIfAborted();
          await writer.commit();
        } catch (error) {
          await writer.abort().catch(() => undefined);
          throw error;
        }
      } else await this.session.database.put("blobs", fileId, file);
      parent[name] = { f: fileId, s: file.size };
    }
    if (!count) throw new Error("没有可用的文件");
    this.session.state.localClipboardRevision++;
    await this.session.pendingUploads.enqueue({ id: "", version: 0, kind: "files", content: Object.keys(info).join("、"),
      fileInfo: info, sourceDeviceId: "", createdAt: new Date().toISOString() }, captureId);
    this.changed();
  }

  async writeEntry(entryId: string, synthesize = false): Promise<void> {
    const entry = await this.session.history.get(entryId);
    this.session.signal.throwIfAborted();
    let payload: ClipboardWrite;
    if (entry.kind === "text") payload = {kind:"text",text:entry.content,html:entry.html,rtf:entry.rtf};
    else if (entry.kind === "files") payload = {kind:"files",paths:await this.session.files.materialize(entry,undefined,this.session.signal)};
    else payload = {kind:"image",bytes:await this.session.files.read(entry.imageInfo!.fileId,0,entry.imageInfo!.size)};
    this.session.signal.throwIfAborted();
    await clipboardPlatform.write(payload,synthesize);
  }
}
