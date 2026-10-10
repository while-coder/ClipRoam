import { computed, ref } from "vue";
import type { ServeTaskSnapshot } from "./relayUploader";

/** 「上传」页的中继应答任务快照：file.requested 且本机应答时记入（内存台账，随客户端重建清空）。 */
export const uploadTasks = ref<ServeTaskSnapshot[]>([]);
/** 侧边栏「上传」入口的角标：待发送 + 发送中的任务数。 */
export const activeUploadCount = computed(() =>
  uploadTasks.value.filter((task) => task.status === "pending" || task.status === "serving").length);
