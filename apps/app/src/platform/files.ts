import { entryContents, isFileNode, type FileInfo } from "@cliproam/protocol";
import { AccountDatabase } from "../core/database";
import { profileName, Sha256 } from "../core/hash";
import { safeName, type LocalSource, type StoredEntry } from "../core/content";
import { isNative } from "./runtime";
import { native, type DirectoryEntry } from "./native";
import type { ClipboardData } from "./clipboard";

export type FileStat = { path: string; name: string; directory: boolean; symlink: boolean; size: number; modifiedAt?: number };
export type SaveTarget = { kind: "path" | "documentTree" | "browser"; value: string }
  | { kind: "browserDirectory"; value: string; directory: FileSystemDirectoryHandle };
export interface ContentWriter {
  append(bytes: Uint8Array): Promise<void>;
  commit(): Promise<void>;
  abort(): Promise<void>;
}

let paths: Promise<{ appDir: string }> | undefined;
export const appPaths = () => paths ??= native.getPaths();
const contentId = (id: string) => { if (!/^[a-f0-9]{64}$/.test(id)) throw new Error("内容标识不合法"); return id; };

/** Platform byte access only. Scheduling, hashing and records stay in TS. */
export class AccountFiles {
  #urls = new Map<string, string>();
  readonly database: AccountDatabase;
  #root: Promise<string> | undefined;
  constructor(readonly account: string) { this.database = new AccountDatabase(account); }
  root(): Promise<string> { return this.#root ??= appPaths().then((paths) => `${paths.appDir}/histories/${profileName(this.account)}/files`); }

  async stats(paths: string[]): Promise<(FileStat | null)[]> {
    return native.statFiles(paths);
  }
  async list(path: string): Promise<FileStat[]> { return native.listFiles(path); }
  async readImage(path: string): Promise<ClipboardData> { return native.readImage(path); }
  async selectSaveTarget(single: boolean, fileName: string): Promise<SaveTarget | null> {
    return native.selectSaveTarget(single, fileName);
  }
  async readPath(path: string, offset: number, length: number): Promise<Uint8Array> {
    if (length <= 8 * 1024 * 1024) return native.readFile(path, offset, length);
    const bytes = new Uint8Array(length);
    let received = 0;
    while (received < length) {
      const chunk = await this.readPath(path, offset + received, Math.min(8 * 1024 * 1024, length - received));
      if (!chunk.length) break;
      bytes.set(chunk, received); received += chunk.length;
    }
    return bytes.subarray(0, received);
  }

  async resolve(fileId: string): Promise<string | undefined> {
    return (await this.resolveMany([fileId])).get(fileId);
  }

  async resolveMany(fileIds: string[]): Promise<Map<string,string>> {
    const ids = [...new Set(fileIds)].map(contentId), result = new Map<string,string>();
    if (!ids.length) return result;
    if (!isNative) {
      const blobs = await this.database.getMany<Blob>("blobs",ids);
      for (let i = 0; i < ids.length; i++) {
        const blob = blobs[i], id = ids[i]!;
        if (!blob) continue;
        let url = this.#urls.get(id);
        if (!url) { url = URL.createObjectURL(blob); this.#urls.set(id,url); }
        result.set(id,url);
      }
      return result;
    }
    const root = await this.root();
    const sources = await this.database.getMany<LocalSource[]>("sources",ids);
    const candidates: { id: string; path: string; source?: LocalSource }[] = [];
    for (let i = 0; i < ids.length; i++) {
      const id = ids[i]!;
      candidates.push({id,path:`${root}/upload/images/${id}`},{id,path:`${root}/download/${id}`});
      for (const source of sources[i] ?? []) candidates.push({id,path:source.source,source});
    }
    const stats = await this.stats(candidates.map((value) => value.path));
    for (let i = 0; i < candidates.length; i++) {
      const {id,path,source} = candidates[i]!, stat = stats[i];
      if (!result.has(id) && stat && !stat.directory && !stat.symlink && (!source ||
        (stat.size === source.size && (source.modifiedAt == null || source.modifiedAt === stat.modifiedAt)))) result.set(id,path);
    }
    return result;
  }

  async read(fileId: string, offset: number, length: number): Promise<Uint8Array> {
    if (!isNative) {
      const blob = await this.database.get<Blob>("blobs", contentId(fileId));
      if (!blob) throw new Error("本机文件内容不可用");
      return new Uint8Array(await blob.slice(offset, offset + length).arrayBuffer());
    }
    const path = await this.resolve(fileId);
    if (!path) throw new Error("本机文件内容不可用");
    return this.readPath(path, offset, length);
  }

  async rememberSource(source: LocalSource): Promise<void> {
    if (!source.fileId) return;
    const existing = await this.database.get<LocalSource[]>("sources", source.fileId) ?? [];
    await this.database.put("sources", source.fileId, [...existing.filter((value) => value.source !== source.source), source]);
  }

  async writer(fileId: string, expectedSize: number, captured = false): Promise<ContentWriter> {
    contentId(fileId);
    let size = 0;
    const hash = new Sha256();
    const chunks: string[] = [];
    const partial = `partial:${crypto.randomUUID()}`;
    const final = isNative ? `${await this.root()}/${captured ? "upload/images" : "download"}/${fileId}` : "";
    const temp = `${final}.${crypto.randomUUID()}.part`;
    if (isNative) await native.writeFile(temp, new Uint8Array(), false);
    return {
      append: async (bytes) => {
        size += bytes.length;
        if (size > expectedSize) throw new Error("下载内容超过声明大小");
        hash.update(bytes);
        if (isNative) await native.writeFile(temp, bytes, true);
        else { const id = `${partial}:${chunks.length}`; await this.database.put("blobs",id,new Blob([bytes])); chunks.push(id); }
      },
      commit: async () => {
        if (size !== expectedSize || hash.digest() !== fileId) throw new Error("文件大小或内容校验失败");
        if (isNative) await native.moveFile(temp, final);
        else {
          const blobs = await this.database.getMany<Blob>("blobs",chunks);
          if (blobs.some((blob) => !blob)) throw new Error("下载缓存不完整");
          await this.database.batch([
            { name:"blobs",id:fileId,value:new Blob(blobs as Blob[]) },
            { name:"meta",id:`blob:${fileId}`,value:{createdMs:Date.now(),captured} },
            ...chunks.map((id) => ({name:"blobs" as const,id})),
          ]);
        }
      },
      abort: async () => {
        if (isNative) await native.removeFile(temp);
        else await this.database.batch(chunks.map((id) => ({name:"blobs",id})));
      },
    };
  }

  async putBlob(bytes: Uint8Array): Promise<string> {
    const id = new Sha256().update(bytes).digest();
    const writer = await this.writer(id, bytes.length, true);
    try { await writer.append(bytes); await writer.commit(); } catch (error) { await writer.abort(); throw error; }
    return id;
  }

  async materialize(entry: StoredEntry, destination?: string, signal?: AbortSignal, createdRoots: string[] = []): Promise<string[]> {
    if (!isNative) throw new Error("浏览器请使用文件下载入口");
    if (!destination && entry.sources?.roots.length) {
      const stats = await this.stats(entry.sources.files.map((source) => source.source));
      const roots = await this.stats(entry.sources.roots);
      if (roots.every(Boolean) && entry.sources.files.every((source, i) => {
        const stat = stats[i];
        return stat && !stat.symlink && stat.size === source.size && (source.modifiedAt == null || source.modifiedAt === stat.modifiedAt);
      })) return entry.sources.roots;
    }
    const root = destination ?? `${await this.root()}/views/${safeName(entry.id)}`;
    if (!destination && (await this.stats([root]))[0]) await native.removeFile(root);
    const paths: string[] = [];
    const walk = async (info: FileInfo, parent: string): Promise<void> => {
      const used = new Set<string>();
      for (const [name, node] of Object.entries(info)) {
        signal?.throwIfAborted();
        if (safeName(name) !== name || used.has(name.toLowerCase())) throw new Error(`文件名称不合法或重复：${name}`);
        used.add(name.toLowerCase());
        let path: string;
        if (isFileNode(node)) {
          const source = await this.resolve(node.f);
          if (!source) throw new Error(`文件内容不可用：${name}`);
          signal?.throwIfAborted();
          path = await native.copyFile(source, parent, name);
          if (parent === root) { paths.push(path); createdRoots.push(path); }
        } else {
          path = await native.createDirectory(parent, name);
          if (parent === root) { paths.push(path); createdRoots.push(path); }
          await walk(node, path);
        }
        signal?.throwIfAborted();
      }
    };
    await walk(entry.fileInfo ?? {}, root);
    return paths;
  }

  async export(entry: StoredEntry, target: SaveTarget, signal: AbortSignal, ensureFile?: (fileId: string) => Promise<void>): Promise<number> {
    if (target.kind === "browserDirectory") {
      const database = this.database;
      const stream = async (fileId: string): Promise<ReadableStream<Uint8Array>> => {
        signal.throwIfAborted();
        await ensureFile?.(fileId);
        signal.throwIfAborted();
        const blob = await database.get<Blob>("blobs", fileId);
        if (!blob) throw new Error("本机文件内容不可用");
        return blob.stream();
      };
      async function* walk(info: FileInfo, parent: string[]): AsyncGenerator<DirectoryEntry> {
        const used = new Set<string>();
        for (const [name, node] of Object.entries(info)) {
          signal.throwIfAborted();
          if (safeName(name) !== name || used.has(name.toLowerCase())) throw new Error(`文件名称不合法或重复：${name}`);
          used.add(name.toLowerCase());
          const path = [...parent, name];
          if (isFileNode(node)) yield {path, stream: await stream(node.f)};
          else { yield {path}; yield* walk(node, path); }
        }
      }
      async function* contents(): AsyncGenerator<DirectoryEntry> {
        if (entry.kind === "image") yield {path:[`${safeName(entry.content)}.webp`],stream:await stream(entry.imageInfo!.fileId)};
        else yield* walk(entry.fileInfo ?? {}, []);
      }
      return native.exportDirectory(target, contents(), signal);
    }
    if (!isNative) {
      let fileId: string, name: string;
      if (entry.kind === "image") {
        fileId = entry.imageInfo!.fileId;
        name = `${safeName(entry.content)}.webp`;
      } else {
        const files = Object.entries(entry.fileInfo ?? {});
        if (files.length !== 1 || !isFileNode(files[0]![1])) throw new Error("当前浏览器不支持目录保存，无法导出文件夹或多文件");
        name = files[0]![0];
        if (safeName(name) !== name) throw new Error("文件名称不合法");
        fileId = files[0]![1].f;
      }
      const blob = await this.database.get<Blob>("blobs", fileId);
      if (!blob) throw new Error("本机文件内容不可用");
      signal.throwIfAborted();
      const url = URL.createObjectURL(blob), link = document.createElement("a");
      link.href = url; link.download = name;
      document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url),60000);
      return 1;
    }
    const contents = entryContents(entry);
    const singles = Object.values(entry.fileInfo ?? {});
    const single = entry.kind === "image" || (singles.length === 1 && isFileNode(singles[0]!));
    const destination = target.value;
    const createdRoots: string[] = [];
    try {
      signal.throwIfAborted();
      if (single && target.kind === "path") {
        const source = await this.resolve(contents[0]!.fileId);
        if (!source) throw new Error("文件内容不可用");
        signal.throwIfAborted();
        await native.copyFile(source, destination);
      } else if (entry.kind === "image") {
        const source = await this.resolve(contents[0]!.fileId);
        if (!source) throw new Error("图片内容不可用");
        signal.throwIfAborted();
        createdRoots.push(await native.copyFile(source, destination, `${safeName(entry.content)}.webp`));
      } else await this.materialize(entry, destination, signal, createdRoots);
      signal.throwIfAborted();
      return entry.fileInfo ? countLeaves(entry.fileInfo) : 1;
    } catch (error) {
      if (target.kind === "documentTree") {
        for (const path of createdRoots.reverse()) await native.removeFile(path).catch(() => undefined);
      }
      throw error;
    }
  }

  dispose(): void { for (const url of this.#urls.values()) URL.revokeObjectURL(url); this.#urls.clear(); }
}

function countLeaves(info: FileInfo): number {
  return Object.values(info).reduce((sum, node) => sum + (isFileNode(node) ? 1 : countLeaves(node)), 0);
}
