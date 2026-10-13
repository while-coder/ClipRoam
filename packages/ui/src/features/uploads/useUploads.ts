import { computed } from "vue";
import { getAccountSession } from "../sync/accountSession";

export const uploadTasks = computed(() => getAccountSession()?.uploads.tasks.value ?? []);
export const activeUploadCount = computed(() => getAccountSession()?.uploads.activeCount.value ?? 0);
