import { renameSync, writeFileSync } from "node:fs";

// 原子写入：先写临时文件再改名，写一半崩溃/断电不会留下截断的文件。
// 临时文件名带 pid，避免多个进程写同一目标时互相踩踏。
export function writeFileAtomic(path: string, contents: Buffer | string, mode?: number): void {
  const temporaryPath = `${path}.${process.pid}.new`;
  writeFileSync(temporaryPath, contents, { mode });
  renameSync(temporaryPath, path);
}
