/** 取消专用哨兵：被取消的批次（含去重合并方连带取消）抛出，调用方据此静默收尾。 */
export class DownloadCancelledError extends Error {}

type DownloadTaskStatus = "queued" | "downloading" | "succeeded" | "failed" | "cancelled";

/** Account-owned download queue snapshot. */
export type DownloadTaskSnapshot = {
  id: string;
  accountKey: string;
  /** 任务创建的批次序号；同一 fileId 的重复任务只统计最新一次。 */
  batchId: number;
  entryId: string;
  fileId: string;
  requestIds: string[];
  /** 面板显示名：entryLabel + 序号，缺省用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: DownloadTaskStatus;
  receivedBytes: number;
  error?: string;
};

export type DownloadRecord = {
  id: string;
  entryId: string;
  label: string;
  size: number;
  fileCount: number;
  status: "queued" | "downloading" | "saving" | "succeeded" | "failed" | "cancelled" | "interrupted";
  createdMs: number;
  finishedMs: number | null;
  receivedBytes: number;
  destination: string;
  error: string | null;
  files: DownloadTaskSnapshot[];
};

export type EntryDownloadToast = {
  id: string;
  entryId: string;
  label: string;
  status: "downloading" | "succeeded" | "failed" | "cancelled";
  total: number;
  totalBytes: number;
  ready: boolean;
  cancelling: boolean;
  error?: string;
  files: Record<string, DownloadTaskSnapshot>;
};

