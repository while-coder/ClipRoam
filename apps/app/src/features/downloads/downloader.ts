import { invoke } from "@tauri-apps/api/core";

/** 取消专用哨兵：被取消的批次（含去重合并方连带取消）抛出，调用方据此静默收尾。 */
export class DownloadCancelledError extends Error {}

type DownloadTaskStatus = "queued" | "downloading" | "succeeded" | "failed" | "cancelled";

/** Rust `cliproam://download-changed` 事件推送的任务快照，字段一一对应。 */
export type DownloadTaskSnapshot = {
  id: string;
  /** 每次 downloadFiles() 自增；同 entry 重复批次按此归代，派生进度只看最新批。 */
  batchId: number;
  entryId: string;
  fileId: string;
  saveId?: string;
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

type DownloadRequest = {
  fileId: string;
  size: number;
};

type BatchOutcome = {
  cancelled: boolean;
  failedCount: number;
  total: number;
};

type DownloaderDeps = {
  /** 任务列表变化（Rust 事件推来，含节流后的字节进度）后触发；调用方在此重拉快照。 */
  onTasksChanged?: () => void;
};

export class Downloader {
  #deps: DownloaderDeps;
  #tasks: DownloadTaskSnapshot[] = [];

  constructor(deps: DownloaderDeps) {
    this.#deps = deps;
  }

  /**
   * 一批文件（同一 entry）：任一失败聚合报错，任一取消抛 DownloadCancelledError。
   * 并发与去重在 Rust 侧；webview 中途销毁只影响本次调用，任务照常跑完。
   */
  async downloadFiles(
    entryId: string,
    files: readonly DownloadRequest[],
    options: { saveId?: string; entryLabel?: string } = {},
  ): Promise<void> {
    const outcome = await invoke<BatchOutcome>("download_files", {
      entryId,
      files,
      saveId: options.saveId,
      entryLabel: options.entryLabel,
    });
    if (outcome.cancelled) throw new DownloadCancelledError("已取消");
    if (outcome.failedCount > 0) {
      throw new Error(`有 ${outcome.failedCount} 个文件下载失败（共 ${outcome.total} 个）`);
    }
  }

  cancel(taskId: string): void {
    void invoke("cancel_download", { taskId }).catch(() => undefined);
  }

  /** 取消该 entry 所有非终态任务；返回取消数，0 = 没有下载在跑。 */
  async cancelEntry(entryId: string): Promise<number> {
    return invoke<number>("cancel_entry_downloads", { entryId });
  }

  /** 中止全部活动任务并清空队列（stopSyncClient / 面板「全部取消」共用）。 */
  stopAll(reason?: string): void {
    void invoke("stop_all_downloads", { reason }).catch(() => undefined);
  }

  /** 窗口打开时的初始拉取；之后靠 download-changed 事件跟进。 */
  async refresh(): Promise<void> {
    this.applySnapshot(await invoke<DownloadTaskSnapshot[]>("download_tasks"));
  }

  /** downloading → queued → 终态 排序的任务快照（Rust 已排好序，原样缓存）。 */
  tasksSnapshot(): readonly DownloadTaskSnapshot[] {
    return this.#tasks;
  }

  applySnapshot(payload: DownloadTaskSnapshot[]): void {
    this.#tasks = payload;
    this.#deps.onTasksChanged?.();
  }
}
