import { computed } from "vue";
import { getAccountSession } from "../accountSession";

export const transferTabs = computed(() => {
  const session = getAccountSession();
  return [
    { view: "pending-upload" as const, label: "待同步", count: session?.pendingUploads.count.value ?? 0,
      failed: !!session?.pendingUploads.failedCount.value },
    { view: "uploads" as const, label: "上传", count: session?.uploads.activeCount.value ?? 0,
      failed: session?.uploads.tasks.value.some((task) => task.status === "failed") ?? false },
    { view: "downloads" as const, label: "下载", count: session?.downloads.activeCount.value ?? 0,
      failed: session?.downloads.downloadRecords.value.some((record) => record.status === "failed" || record.status === "interrupted") ?? false },
  ];
});
export const hasTransferFailure = computed(() => transferTabs.value.some((tab) => tab.failed));
