type ServeTaskStatus = "pending" | "serving" | "succeeded" | "skipped" | "failed";

export type ServeTaskSnapshot = {
  id: string;
  entryId: string;
  fileId: string;
  /** 展示名：entry.content；解析不到时用 fileId 前 8 位。 */
  label: string;
  size: number;
  status: ServeTaskStatus;
  sentBytes: number;
  /** 终态补充说明（跳过原因 / 失败原因）。 */
  error?: string;
  startedAt: number;
};

